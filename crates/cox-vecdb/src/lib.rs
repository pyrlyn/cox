// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Experimental SQLite-backed HNSW vector store (branch `exp/vector-bench`).
//!
//! * Graph: HNSW (Malkov & Yashunin) with the neighbour-selection heuristic;
//!   `m`, `ef_construction` and `ef_search` are tunable.
//! * Vectors: cosine on normalized vectors, stored as contiguous `f32` or as
//!   `i8` with one `f32` scale per vector.
//! * Concurrency: nodes live in a lock-free append-only arena (doubling
//!   buckets), every adjacency list has its own `RwLock`, so searches never take
//!   a global lock and inserts only lock the lists they rewrite.
//! * Persistence: nodes are serialized into 1024-node BLOB pages in one SQLite
//!   table; only pages touched since the last `save` are rewritten. Loading uses
//!   SQLite's mmap I/O and parses pages straight into the arena.

mod sqlite;

use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{BTreeSet, BinaryHeap};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering as AO};

use parking_lot::{Mutex, RwLock};

#[derive(Debug)]
pub struct Error(pub String);
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quant {
    F32,
    I8,
}

#[derive(Clone, Copy, Debug)]
pub struct Params {
    pub m: usize,
    pub ef_construction: usize,
    pub ef_search: usize,
    pub quant: Quant,
}

impl Default for Params {
    fn default() -> Self {
        Self { m: 16, ef_construction: 200, ef_search: 64, quant: Quant::F32 }
    }
}

enum Vector {
    F32(Box<[f32]>),
    I8(Box<[i8]>, f32),
}

struct Node {
    id: u64,
    vec: Vector,
    links: Box<[RwLock<Vec<u32>>]>,
}

const PAGE: usize = 1024;
const BASE: usize = 1024;
const BUCKETS: usize = 40;

/// Append-only arena with doubling buckets: `get` is lock-free.
struct Arena {
    buckets: [OnceLock<Box<[OnceLock<Node>]>>; BUCKETS],
    len: AtomicUsize,
}

fn locate(i: usize) -> (usize, usize) {
    let x = i + BASE;
    let b = (usize::BITS - 1 - x.leading_zeros()) as usize - BASE.trailing_zeros() as usize;
    (b, x - (BASE << b))
}

impl Arena {
    fn new() -> Self {
        Self { buckets: std::array::from_fn(|_| OnceLock::new()), len: AtomicUsize::new(0) }
    }
    fn push(&self, n: Node) -> u32 {
        let i = self.len.fetch_add(1, AO::AcqRel);
        let (b, o) = locate(i);
        let bucket = self.buckets[b].get_or_init(|| (0..BASE << b).map(|_| OnceLock::new()).collect());
        if bucket[o].set(n).is_err() {
            unreachable!("slot reserved twice");
        }
        i as u32
    }
    fn get(&self, i: u32) -> Option<&Node> {
        let (b, o) = locate(i as usize);
        self.buckets[b].get()?[o].get()
    }
    fn len(&self) -> usize {
        self.len.load(AO::Acquire)
    }
}

#[derive(Clone, Copy, PartialEq)]
struct D(f32, u32);
impl Eq for D {}
impl PartialOrd for D {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for D {
    fn cmp(&self, o: &Self) -> Ordering {
        self.0.total_cmp(&o.0).then(self.1.cmp(&o.1))
    }
}

thread_local! {
    static VISITED: RefCell<(Vec<u32>, u32)> = const { RefCell::new((Vec::new(), 0)) };
}

#[inline]
fn dot(a: &[f32], b: &[f32]) -> f32 {
    let mut acc = [0f32; 8];
    let (ca, ra) = a.as_chunks::<8>();
    let (cb, rb) = b.as_chunks::<8>();
    for (x, y) in ca.iter().zip(cb) {
        for k in 0..8 {
            acc[k] += x[k] * y[k];
        }
    }
    let mut s: f32 = acc.iter().sum();
    for (x, y) in ra.iter().zip(rb) {
        s += x * y;
    }
    s
}

#[inline]
fn dot_i8(q: &[f32], v: &[i8]) -> f32 {
    let mut acc = [0f32; 8];
    let (cq, rq) = q.as_chunks::<8>();
    let (cv, rv) = v.as_chunks::<8>();
    for (x, y) in cq.iter().zip(cv) {
        for k in 0..8 {
            acc[k] += x[k] * y[k] as f32;
        }
    }
    let mut s: f32 = acc.iter().sum();
    for (x, y) in rq.iter().zip(rv) {
        s += x * *y as f32;
    }
    s
}

fn normalize(v: &mut [f32]) {
    let n = dot(v, v).sqrt();
    if n > 0.0 {
        v.iter_mut().for_each(|x| *x /= n);
    }
}

impl Vector {
    fn encode(v: &[f32], q: Quant) -> Self {
        match q {
            Quant::F32 => Vector::F32(v.into()),
            Quant::I8 => {
                let max = v.iter().fold(0f32, |m, x| m.max(x.abs())).max(f32::MIN_POSITIVE);
                let s = max / 127.0;
                Vector::I8(v.iter().map(|x| (x / s).round() as i8).collect(), s)
            }
        }
    }
    fn decode(&self) -> Vec<f32> {
        match self {
            Vector::F32(v) => v.to_vec(),
            Vector::I8(v, s) => v.iter().map(|x| *x as f32 * s).collect(),
        }
    }
    /// Cosine distance (`1 - dot`) to a normalized `f32` query.
    #[inline]
    fn dist(&self, q: &[f32]) -> f32 {
        1.0 - match self {
            Vector::F32(v) => dot(q, v),
            Vector::I8(v, s) => dot_i8(q, v) * s,
        }
    }
}

pub struct VecDb {
    dim: usize,
    params: Params,
    ef_search: AtomicUsize,
    arena: Arena,
    /// (entry node, its level); `u64::MAX` when empty.
    entry: RwLock<u64>,
    dirty: Mutex<BTreeSet<u32>>,
    rng: AtomicU64,
    db: Mutex<sqlite::Db>,
}

fn pack(node: u32, level: usize) -> u64 {
    (u64::from(node) << 8) | level as u64
}
fn unpack(e: u64) -> (u32, usize) {
    ((e >> 8) as u32, (e & 0xff) as usize)
}

impl VecDb {
    /// Opens (or creates) the store at `path`. Existing files keep their stored
    /// `m`/`ef_construction`/quantization; `params.ef_search` always applies.
    pub fn open(path: &str, dim: usize, params: Params) -> Result<Self> {
        let db = sqlite::Db::open(path)?;
        db.exec(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA mmap_size=17179869184;
             PRAGMA page_size=16384;
             CREATE TABLE IF NOT EXISTS vdb_meta (k INTEGER PRIMARY KEY, v INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS vdb_page (no INTEGER PRIMARY KEY, data BLOB NOT NULL);",
        )?;
        let mut me = Self {
            dim,
            params,
            ef_search: AtomicUsize::new(params.ef_search),
            arena: Arena::new(),
            entry: RwLock::new(u64::MAX),
            dirty: Mutex::new(BTreeSet::new()),
            rng: AtomicU64::new(0x9E37_79B9_7F4A_7C15),
            db: Mutex::new(db),
        };
        me.load()?;
        Ok(me)
    }

    pub fn len(&self) -> usize {
        self.arena.len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn set_ef_search(&self, ef: usize) {
        self.ef_search.store(ef, AO::Relaxed);
    }

    fn load(&mut self) -> Result<()> {
        let db = self.db.lock();
        let mut meta = [0i64; 6];
        let mut found = false;
        {
            let mut st = db.prepare("SELECT k, v FROM vdb_meta")?;
            while st.step()? {
                found = true;
                if let Some(m) = meta.get_mut(st.col_i64(0) as usize) {
                    *m = st.col_i64(1);
                }
            }
        }
        if !found {
            drop(db);
            return Ok(());
        }
        if meta[0] as usize != self.dim {
            return Err(Error(format!("stored dim {} != {}", meta[0], self.dim)));
        }
        self.params.m = meta[1] as usize;
        self.params.ef_construction = meta[2] as usize;
        self.params.quant = if meta[3] == 1 { Quant::I8 } else { Quant::F32 };
        *self.entry.get_mut() = meta[4] as u64;
        let mut st = db.prepare("SELECT data FROM vdb_page ORDER BY no")?;
        while st.step()? {
            let mut b = st.col_blob(0);
            while !b.is_empty() {
                let n = self.read_node(&mut b)?;
                self.arena.push(n);
            }
        }
        Ok(())
    }

    fn read_node(&self, b: &mut &[u8]) -> Result<Node> {
        fn take<'a>(b: &mut &'a [u8], n: usize) -> Result<&'a [u8]> {
            if b.len() < n {
                return Err(Error("truncated page".into()));
            }
            let (h, t) = b.split_at(n);
            *b = t;
            Ok(h)
        }
        let u32_at = |s: &[u8]| u32::from_le_bytes(s.try_into().unwrap_or_default());
        let id = u64::from_le_bytes(take(b, 8)?.try_into().unwrap_or_default());
        let levels = take(b, 1)?[0] as usize + 1;
        let vec = match self.params.quant {
            Quant::F32 => Vector::F32(take(b, self.dim * 4)?.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap_or_default())).collect()),
            Quant::I8 => {
                let s = f32::from_le_bytes(take(b, 4)?.try_into().unwrap_or_default());
                Vector::I8(take(b, self.dim)?.iter().map(|x| *x as i8).collect(), s)
            }
        };
        let mut links = Vec::with_capacity(levels);
        for _ in 0..levels {
            let n = u16::from_le_bytes(take(b, 2)?.try_into().unwrap_or_default()) as usize;
            links.push(RwLock::new(take(b, n * 4)?.chunks_exact(4).map(u32_at).collect()));
        }
        Ok(Node { id, vec, links: links.into() })
    }

    fn write_node(n: &Node, out: &mut Vec<u8>) {
        out.extend_from_slice(&n.id.to_le_bytes());
        out.push((n.links.len() - 1) as u8);
        match &n.vec {
            Vector::F32(v) => v.iter().for_each(|x| out.extend_from_slice(&x.to_le_bytes())),
            Vector::I8(v, s) => {
                out.extend_from_slice(&s.to_le_bytes());
                out.extend(v.iter().map(|x| *x as u8));
            }
        }
        for l in n.links.iter() {
            let l = l.read();
            out.extend_from_slice(&(l.len() as u16).to_le_bytes());
            l.iter().for_each(|x| out.extend_from_slice(&x.to_le_bytes()));
        }
    }

    /// Writes every page touched since the last save in one transaction.
    /// Returns the number of pages written.
    pub fn save(&self) -> Result<usize> {
        let pages: Vec<u32> = std::mem::take(&mut *self.dirty.lock()).into_iter().collect();
        let db = self.db.lock();
        db.exec("BEGIN IMMEDIATE")?;
        {
            let mut st = db.prepare("INSERT OR REPLACE INTO vdb_page (no, data) VALUES (?, ?)")?;
            let mut buf = Vec::new();
            for &p in &pages {
                buf.clear();
                let start = p as usize * PAGE;
                for i in start..(start + PAGE).min(self.len()) {
                    if let Some(n) = self.arena.get(i as u32) {
                        Self::write_node(n, &mut buf);
                    }
                }
                st.bind_i64(1, i64::from(p))?;
                st.bind_blob(2, &buf)?;
                st.step()?;
                st.reset();
            }
            let mut st = db.prepare("INSERT OR REPLACE INTO vdb_meta (k, v) VALUES (?, ?)")?;
            let meta = [
                self.dim as i64,
                self.params.m as i64,
                self.params.ef_construction as i64,
                i64::from(self.params.quant == Quant::I8),
                *self.entry.read() as i64,
                self.len() as i64,
            ];
            for (k, v) in meta.into_iter().enumerate() {
                st.bind_i64(1, k as i64)?;
                st.bind_i64(2, v)?;
                st.step()?;
                st.reset();
            }
        }
        db.exec("COMMIT; PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(pages.len())
    }

    fn node(&self, i: u32) -> &Node {
        self.arena.get(i).expect("linked node is published")
    }

    fn random_level(&self) -> usize {
        // xorshift64*; good enough for level assignment.
        let mut x = self.rng.fetch_add(0x9E37_79B9_7F4A_7C15, AO::Relaxed);
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        let u = ((x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 + 1.0) / (1u64 << 53) as f64;
        let ml = 1.0 / (self.params.m as f64).ln();
        ((-u.ln() * ml) as usize).min(15)
    }

    fn search_layer(&self, q: &[f32], eps: &[D], ef: usize, level: usize) -> Vec<D> {
        VISITED.with(|v| {
            let (marks, gen_) = &mut *v.borrow_mut();
            let n = self.len();
            if marks.len() < n {
                marks.resize(n + n / 4 + 1024, 0);
            }
            *gen_ = gen_.wrapping_add(1);
            if *gen_ == 0 {
                marks.iter_mut().for_each(|m| *m = 0);
                *gen_ = 1;
            }
            let g = *gen_;
            let mut cand: BinaryHeap<std::cmp::Reverse<D>> = BinaryHeap::new();
            let mut res: BinaryHeap<D> = BinaryHeap::new();
            for &e in eps {
                marks[e.1 as usize] = g;
                cand.push(std::cmp::Reverse(e));
                res.push(e);
            }
            let mut buf = Vec::with_capacity(64);
            while let Some(std::cmp::Reverse(c)) = cand.pop() {
                if res.len() >= ef && c.0 > res.peek().map_or(f32::MAX, |w| w.0) {
                    break;
                }
                buf.clear();
                if let Some(l) = self.node(c.1).links.get(level) {
                    buf.extend_from_slice(&l.read());
                }
                for &nb in &buf {
                    let Some(m) = marks.get_mut(nb as usize) else { continue };
                    if *m == g {
                        continue;
                    }
                    *m = g;
                    let d = self.node(nb).vec.dist(q);
                    if res.len() < ef || d < res.peek().map_or(f32::MAX, |w| w.0) {
                        cand.push(std::cmp::Reverse(D(d, nb)));
                        res.push(D(d, nb));
                        if res.len() > ef {
                            res.pop();
                        }
                    }
                }
            }
            res.into_sorted_vec()
        })
    }

    /// HNSW heuristic: keep a candidate only if it is closer to the base than
    /// to any already selected neighbour.
    fn select(&self, cands: &[D], m: usize) -> Vec<u32> {
        let mut out: Vec<(u32, Vec<f32>)> = Vec::with_capacity(m);
        for c in cands {
            if out.len() >= m {
                break;
            }
            let cv = self.node(c.1).vec.decode();
            if out.iter().all(|(o, _)| self.node(*o).vec.dist(&cv) > c.0) {
                out.push((c.1, cv));
            }
        }
        out.into_iter().map(|(i, _)| i).collect()
    }

    fn greedy(&self, q: &[f32], ep: u32, from: usize, to: usize) -> D {
        let mut best = D(self.node(ep).vec.dist(q), ep);
        for l in (to..=from).rev() {
            best = self.search_layer(q, &[best], 1, l)[0];
        }
        best
    }

    /// Inserts one vector; safe to call from many threads at once.
    pub fn insert(&self, id: u64, v: &[f32]) -> Result<()> {
        if v.len() != self.dim {
            return Err(Error(format!("dim {} != {}", v.len(), self.dim)));
        }
        let mut q = v.to_vec();
        normalize(&mut q);
        let level = self.random_level();
        let m0 = self.params.m * 2;
        let links = (0..=level).map(|l| RwLock::new(Vec::with_capacity(if l == 0 { m0 } else { self.params.m }))).collect();
        let me = self.arena.push(Node { id, vec: Vector::encode(&q, self.params.quant), links });
        self.mark(me);
        let entry = {
            let e = *self.entry.read();
            if e == u64::MAX {
                let mut w = self.entry.write();
                if *w == u64::MAX {
                    *w = pack(me, level);
                    return Ok(());
                }
                *w
            } else {
                e
            }
        };
        let (ep, ep_level) = unpack(entry);
        let mut eps = vec![if ep_level > level { self.greedy(&q, ep, ep_level, level + 1) } else { D(self.node(ep).vec.dist(&q), ep) }];
        for l in (0..=level.min(ep_level)).rev() {
            let w = self.search_layer(&q, &eps, self.params.ef_construction, l);
            let mmax = if l == 0 { m0 } else { self.params.m };
            let nbrs = self.select(&w, self.params.m);
            *self.node(me).links[l].write() = nbrs.clone();
            for nb in nbrs {
                self.link(nb, me, l, mmax);
            }
            eps = w;
        }
        if level > ep_level {
            let mut w = self.entry.write();
            if unpack(*w).1 < level {
                *w = pack(me, level);
            }
        }
        Ok(())
    }

    fn link(&self, from: u32, to: u32, l: usize, mmax: usize) {
        let n = self.node(from);
        let mut list = n.links[l].write();
        if list.contains(&to) {
            return;
        }
        list.push(to);
        if list.len() > mmax {
            let base = n.vec.decode();
            let mut c: Vec<D> = list.iter().map(|&i| D(self.node(i).vec.dist(&base), i)).collect();
            c.sort();
            *list = self.select(&c, mmax);
        }
        drop(list);
        self.mark(from);
    }

    fn mark(&self, i: u32) {
        self.dirty.lock().insert(i / PAGE as u32);
    }

    /// Top-`k` by cosine similarity: `(id, score)` with higher score first.
    pub fn search(&self, v: &[f32], k: usize) -> Vec<(u64, f32)> {
        let e = *self.entry.read();
        if e == u64::MAX {
            return Vec::new();
        }
        let mut q = v.to_vec();
        normalize(&mut q);
        let (ep, top) = unpack(e);
        let best = if top == 0 { D(self.node(ep).vec.dist(&q), ep) } else { self.greedy(&q, ep, top, 1) };
        let ef = self.ef_search.load(AO::Relaxed).max(k);
        let mut r = self.search_layer(&q, &[best], ef, 0);
        r.truncate(k);
        r.into_iter().map(|d| (self.node(d.1).id, 1.0 - d.0)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vecs(n: usize, dim: usize) -> Vec<Vec<f32>> {
        let mut s = 1u64;
        (0..n)
            .map(|_| {
                (0..dim)
                    .map(|_| {
                        s ^= s << 13;
                        s ^= s >> 7;
                        s ^= s << 17;
                        (s % 2000) as f32 / 1000.0 - 1.0
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn finds_itself_and_persists() {
        let dir = std::env::temp_dir().join(format!("vecdb-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.db");
        let data = vecs(2000, 32);
        for quant in [Quant::F32, Quant::I8] {
            let _ = std::fs::remove_file(&path);
            let db = VecDb::open(path.to_str().unwrap(), 32, Params { quant, ..Params::default() }).unwrap();
            std::thread::scope(|s| {
                for chunk in data.chunks(500).enumerate() {
                    let db = &db;
                    s.spawn(move || {
                        for (j, v) in chunk.1.iter().enumerate() {
                            db.insert((chunk.0 * 500 + j) as u64, v).unwrap();
                        }
                    });
                }
            });
            db.save().unwrap();
            drop(db);
            let db = VecDb::open(path.to_str().unwrap(), 32, Params::default()).unwrap();
            assert_eq!(db.len(), 2000);
            let hits = (0..200).filter(|&i| db.search(&data[i], 1)[0].0 == i as u64).count();
            assert!(hits >= 190, "{quant:?}: {hits}/200");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
