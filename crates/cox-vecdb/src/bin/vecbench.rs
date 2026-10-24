//! Benchmark driver for `exp/vector-bench`. Every subcommand prints one JSON
//! object per line; `scripts/vector_bench.py` orchestrates the runs.
//!
//! vecbench gen <dir> <n> <dim> <queries> <extra>
//! vecbench build <engine> <dir> <db> <m> <ef_construction> <quant> <threads>
//! vecbench query <engine> <dir> <db> <ef_list> <thread_list> <curve_ef> [gt]
//! vecbench insert <engine> <dir> <db> <ef> <threads>
//! engine: vecdb | qdrant | sqlite

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use serde_json::json;

const K: usize = 10;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn unit(&mut self) -> f64 {
        ((self.next() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }
    fn gauss(&mut self) -> f32 {
        ((-2.0 * self.unit().ln()).sqrt() * (std::f64::consts::TAU * self.unit()).cos()) as f32
    }
}

fn norm(v: &mut [f32]) {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    v.iter_mut().for_each(|x| *x /= n);
}

/// Half uniform-on-sphere vectors, half clustered around 256 centres.
fn gen_set(rng: &mut Rng, centres: &[Vec<f32>], n: usize, dim: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(n * dim);
    for i in 0..n {
        let mut v: Vec<f32> = if i % 2 == 0 {
            (0..dim).map(|_| rng.gauss()).collect()
        } else {
            let c = &centres[(rng.next() % centres.len() as u64) as usize];
            c.iter().map(|x| x + 0.35 * rng.gauss() / (dim as f32).sqrt() * 4.0).collect()
        };
        norm(&mut v);
        out.extend_from_slice(&v);
    }
    out
}

fn write_f32(p: &Path, v: &[f32]) {
    std::fs::write(p, v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<_>>()).unwrap();
}
fn read_f32(p: &Path) -> Vec<f32> {
    std::fs::read(p).unwrap().chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect()
}
fn read_u32(p: &Path) -> Vec<u32> {
    std::fs::read(p).unwrap().chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect()
}

fn threads() -> usize {
    std::thread::available_parallelism().map_or(8, |n| n.get())
}

/// Exact top-K by dot product (vectors are normalized), parallel over queries.
fn ground_truth(base: &[f32], queries: &[f32], dim: usize) -> Vec<u32> {
    let nq = queries.len() / dim;
    let mut out = vec![0u32; nq * K];
    let t = threads();
    let per = nq.div_ceil(t);
    std::thread::scope(|s| {
        for (ci, chunk) in out.chunks_mut(per * K).enumerate() {
            s.spawn(move || {
                for (j, slot) in chunk.chunks_mut(K).enumerate() {
                    let q = &queries[(ci * per + j) * dim..][..dim];
                    let mut best: Vec<(f32, u32)> = Vec::with_capacity(K + 1);
                    for (i, v) in base.chunks_exact(dim).enumerate() {
                        let d: f32 = q.iter().zip(v).map(|(a, b)| a * b).sum();
                        if best.len() < K || d > best[K - 1].0 {
                            let pos = best.partition_point(|x| x.0 >= d);
                            best.insert(pos, (d, i as u32));
                            best.truncate(K);
                        }
                    }
                    for (o, b) in slot.iter_mut().zip(&best) {
                        *o = b.1;
                    }
                }
            });
        }
    });
    out
}

trait Engine: Sync {
    fn insert_batch(&self, first_id: u64, vecs: &[f32], dim: usize, threads: usize);
    fn search(&self, q: &[f32], k: usize, ef: usize) -> Vec<u64>;
    fn persist(&self);
}

struct VecDbEngine(cox_vecdb::VecDb);
impl Engine for VecDbEngine {
    fn insert_batch(&self, first: u64, vecs: &[f32], dim: usize, threads: usize) {
        let n = vecs.len() / dim;
        let next = AtomicUsize::new(0);
        std::thread::scope(|s| {
            for _ in 0..threads {
                s.spawn(|| loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= n {
                        break;
                    }
                    self.0.insert(first + i as u64, &vecs[i * dim..][..dim]).unwrap();
                });
            }
        });
    }
    fn search(&self, q: &[f32], k: usize, ef: usize) -> Vec<u64> {
        self.0.set_ef_search(ef);
        self.0.search(q, k).into_iter().map(|h| h.0).collect()
    }
    fn persist(&self) {
        self.0.save().unwrap();
    }
}

mod qd {
    use qdrant_edge::*;
    pub struct Q(pub EdgeShard);
    pub fn open(path: &std::path::Path, dim: usize, m: usize, efc: usize) -> Q {
        if path.exists() {
            return Q(EdgeShard::load(path, None).unwrap());
        }
        std::fs::create_dir_all(path).unwrap();
        let cfg = EdgeConfigBuilder::new()
            .vector(DEFAULT_VECTOR_NAME, EdgeVectorParamsBuilder::new(dim, Distance::Cosine).build())
            .hnsw_config(HnswIndexConfig { m, ef_construct: efc, ..Default::default() })
            .build();
        Q(EdgeShard::new(path, cfg).unwrap())
    }
    impl super::Engine for Q {
        fn insert_batch(&self, first: u64, vecs: &[f32], dim: usize, _threads: usize) {
            for (bi, batch) in vecs.chunks(dim * 1000).enumerate() {
                let pts: Vec<_> = batch
                    .chunks_exact(dim)
                    .enumerate()
                    .map(|(j, v)| PointStruct::new(first + (bi * 1000 + j) as u64, v.to_vec(), serde_json::json!({})).into())
                    .collect();
                self.0
                    .update(UpdateOperation::PointOperation(PointOperations::UpsertPoints(
                        PointInsertOperations::PointsList(pts),
                    )))
                    .unwrap();
            }
        }
        fn search(&self, q: &[f32], k: usize, ef: usize) -> Vec<u64> {
            let req = SearchRequestBuilder::new(
                QueryEnum::Nearest(NamedQuery { query: q.to_vec().into(), using: None }),
                k,
            )
            .params(SearchParams { hnsw_ef: Some(ef), ..Default::default() })
            .build();
            self.0
                .search(req)
                .unwrap()
                .into_iter()
                .filter_map(|p| match p.id {
                    PointId::NumId(n) => Some(n),
                    _ => None,
                })
                .collect()
        }
        fn persist(&self) {
            self.0.flush().unwrap();
        }
    }
}

struct SqliteEngine(cox_vector_memory::SqliteVectorStore);
impl Engine for SqliteEngine {
    fn insert_batch(&self, first: u64, vecs: &[f32], dim: usize, _threads: usize) {
        use cox_vector_memory::{Entry, Kind, VectorStore};
        for (i, v) in vecs.chunks_exact(dim).enumerate() {
            let e = Entry { project: "bench".into(), kind: Kind::Fact, text: (first + i as u64).to_string() };
            self.0.upsert(&e, v.to_vec()).unwrap();
        }
    }
    fn search(&self, q: &[f32], k: usize, _ef: usize) -> Vec<u64> {
        use cox_vector_memory::VectorStore;
        self.0.search(q.to_vec(), k, -2.0).unwrap().into_iter().filter_map(|h| h.entry.text.parse().ok()).collect()
    }
    fn persist(&self) {}
}

fn open(engine: &str, db: &Path, dim: usize, m: usize, efc: usize, quant: &str) -> Box<dyn Engine> {
    match engine {
        "vecdb" => {
            let q = if quant == "i8" { cox_vecdb::Quant::I8 } else { cox_vecdb::Quant::F32 };
            let p = cox_vecdb::Params { m, ef_construction: efc, ef_search: 64, quant: q };
            Box::new(VecDbEngine(cox_vecdb::VecDb::open(db.to_str().unwrap(), dim, p).unwrap()))
        }
        "qdrant" => Box::new(qd::open(db, dim, m, efc)),
        "sqlite" => Box::new(SqliteEngine(cox_vector_memory::SqliteVectorStore::open(db, "bench").unwrap())),
        e => panic!("unknown engine {e}"),
    }
}

fn disk(p: &Path) -> u64 {
    let md = std::fs::symlink_metadata(p).map(|m| m.is_dir()).unwrap_or(false);
    if !md {
        let base = std::fs::metadata(p).map_or(0, |m| m.len());
        // SQLite side files.
        let side: u64 = ["-wal", "-shm"]
            .iter()
            .map(|s| std::fs::metadata(format!("{}{s}", p.display())).map_or(0, |m| m.len()))
            .sum();
        return base + side;
    }
    std::fs::read_dir(p).unwrap().flatten().map(|e| disk(&e.path())).sum()
}

fn recall(res: &[Vec<u64>], gt: &[u32]) -> f64 {
    let hit: usize = res
        .iter()
        .enumerate()
        .map(|(i, r)| r.iter().filter(|id| gt[i * K..][..K].contains(&(**id as u32))).count())
        .sum();
    hit as f64 / (res.len() * K) as f64
}

/// Recall over the uniform (even) or clustered (odd) half of the queries.
fn split(res: &[Vec<u64>], gt: &[u32], parity: usize) -> f64 {
    let (r, g): (Vec<_>, Vec<_>) = res
        .iter()
        .enumerate()
        .filter(|(i, _)| i % 2 == parity)
        .map(|(i, r)| (r.clone(), gt[i * K..][..K].to_vec()))
        .unzip();
    recall(&r, &g.concat())
}

fn pct(lat: &mut [f64], p: f64) -> f64 {
    lat.sort_by(f64::total_cmp);
    lat[((lat.len() as f64 * p).ceil() as usize).clamp(1, lat.len()) - 1]
}

/// Runs `total` queries over `t` threads; returns (qps, per-query ms latencies, results).
fn run(e: &dyn Engine, qs: &[f32], dim: usize, ef: usize, t: usize, total: usize) -> (f64, Vec<f64>, Vec<Vec<u64>>) {
    let nq = qs.len() / dim;
    let next = AtomicUsize::new(0);
    let start = Instant::now();
    let parts: Vec<Vec<(usize, f64, Vec<u64>)>> = std::thread::scope(|s| {
        let hs: Vec<_> = (0..t)
            .map(|_| {
                s.spawn(|| {
                    let mut out = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= total {
                            break;
                        }
                        let q = &qs[(i % nq) * dim..][..dim];
                        let t0 = Instant::now();
                        let r = e.search(q, K, ef);
                        out.push((i, t0.elapsed().as_secs_f64() * 1e3, r));
                    }
                    out
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let wall = start.elapsed().as_secs_f64();
    let mut all: Vec<_> = parts.into_iter().flatten().collect();
    all.sort_by_key(|x| x.0);
    let lat = all.iter().map(|x| x.1).collect();
    let res = all.into_iter().take(nq).map(|x| x.2).collect();
    (total as f64 / wall, lat, res)
}

fn lat_json(lat: &mut [f64]) -> serde_json::Value {
    json!({"p50_ms": pct(lat, 0.50), "p95_ms": pct(lat, 0.95), "p99_ms": pct(lat, 0.99)})
}

fn main() {
    let t_proc = Instant::now();
    let a: Vec<String> = std::env::args().collect();
    let dir = |i: usize| Path::new(&a[i]).to_path_buf();
    let dim_of = |d: &Path| std::fs::read_to_string(d.join("dim")).unwrap().trim().parse::<usize>().unwrap();
    let list = |s: &str| s.split(',').map(|x| x.parse::<usize>().unwrap()).collect::<Vec<_>>();
    match a[1].as_str() {
        "gen" => {
            let d = dir(2);
            let (n, dim, nq, ne) = (a[3].parse().unwrap(), a[4].parse().unwrap(), a[5].parse().unwrap(), a[6].parse().unwrap());
            std::fs::create_dir_all(&d).unwrap();
            let mut rng = Rng(0x1234_5678_9abc_def1);
            let centres: Vec<Vec<f32>> = (0..256)
                .map(|_| {
                    let mut c: Vec<f32> = (0..dim).map(|_| rng.gauss()).collect();
                    norm(&mut c);
                    c
                })
                .collect();
            let base = gen_set(&mut rng, &centres, n, dim);
            let extra = gen_set(&mut rng, &centres, ne, dim);
            let queries = gen_set(&mut rng, &centres, nq, dim);
            let t0 = Instant::now();
            let gt = ground_truth(&base, &queries, dim);
            let mut full = base.clone();
            full.extend_from_slice(&extra);
            let gt_full = ground_truth(&full, &queries, dim);
            std::fs::write(d.join("dim"), dim.to_string()).unwrap();
            write_f32(&d.join("base.f32"), &base);
            write_f32(&d.join("extra.f32"), &extra);
            write_f32(&d.join("queries.f32"), &queries);
            let w = |p: &str, v: &[u32]| std::fs::write(d.join(p), v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<_>>()).unwrap();
            w("gt_base.u32", &gt);
            w("gt_full.u32", &gt_full);
            println!("{}", json!({"cmd": "gen", "n": n, "dim": dim, "queries": nq, "extra": ne, "gt_s": t0.elapsed().as_secs_f64()}));
        }
        "build" => {
            let (d, db) = (dir(3), dir(4));
            let dim = dim_of(&d);
            let base = read_f32(&d.join("base.f32"));
            let (m, efc, threads): (usize, usize, usize) = (a[5].parse().unwrap(), a[6].parse().unwrap(), a[8].parse().unwrap());
            let e = open(&a[2], &db, dim, m, efc, &a[7]);
            let t0 = Instant::now();
            e.insert_batch(0, &base, dim, threads);
            let ins = t0.elapsed().as_secs_f64();
            let t1 = Instant::now();
            if a[2] == "qdrant" {
                // Build the HNSW index with a blocking optimizer run.
                drop(e);
                let q = qd::open(&db, dim, m, efc);
                q.0.optimize().unwrap();
                q.0.flush().unwrap();
            } else {
                e.persist();
            }
            let index_s = t1.elapsed().as_secs_f64();
            println!("{}", json!({"cmd": "build", "engine": a[2], "insert_s": ins, "index_or_save_s": index_s,
                "build_total_s": t0.elapsed().as_secs_f64(), "disk_bytes": disk(&db)}));
        }
        "query" => {
            let (d, db) = (dir(3), dir(4));
            let dim = dim_of(&d);
            let qs = read_f32(&d.join("queries.f32"));
            let gt = read_u32(&d.join(a.get(8).map_or("gt_base.u32", |s| s.as_str())));
            let efs = list(&a[5]);
            let t_open = Instant::now();
            let e = open(&a[2], &db, dim, 16, 200, "f32");
            let opened = t_open.elapsed().as_secs_f64() * 1e3;
            let _ = e.search(&qs[..dim], K, efs[0]);
            let first = t_open.elapsed().as_secs_f64() * 1e3;
            println!("{}", json!({"cmd": "cold", "engine": a[2], "open_ms": opened, "first_result_ms": first,
                "process_to_first_ms": t_proc.elapsed().as_secs_f64() * 1e3}));
            let nq = qs.len() / dim;
            for &ef in &efs {
                let (qps, mut lat, res) = run(&*e, &qs, dim, ef, 1, nq);
                println!("{}", json!({"cmd": "ef", "engine": a[2], "ef": ef, "recall10": recall(&res, &gt), "recall10_uniform": split(&res, &gt, 0), "recall10_clustered": split(&res, &gt, 1), "qps_1t": qps, "lat": lat_json(&mut lat)}));
            }
            let curve_ef: usize = a[7].parse().unwrap();
            for t in list(&a[6]) {
                let (qps, mut lat, res) = run(&*e, &qs, dim, curve_ef, t, nq.max(t * 100) * if a[2] == "sqlite" { 1 } else { 3 });
                println!("{}", json!({"cmd": "curve", "engine": a[2], "ef": curve_ef, "threads": t, "qps": qps, "recall10": recall(&res, &gt), "lat": lat_json(&mut lat)}));
            }
        }
        "insert" => {
            let (d, db) = (dir(3), dir(4));
            let dim = dim_of(&d);
            let n0 = read_f32(&d.join("base.f32")).len() / dim;
            let extra = read_f32(&d.join("extra.f32"));
            let qs = read_f32(&d.join("queries.f32"));
            let gt = read_u32(&d.join("gt_full.u32"));
            let (ef, threads): (usize, usize) = (a[5].parse().unwrap(), a[6].parse().unwrap());
            let e = open(&a[2], &db, dim, 16, 200, "f32");
            let t0 = Instant::now();
            e.insert_batch(n0 as u64, &extra, dim, threads);
            let ins = t0.elapsed().as_secs_f64();
            e.persist();
            let total = t0.elapsed().as_secs_f64();
            let (_, _, res) = run(&*e, &qs, dim, ef, 1, qs.len() / dim);
            let n = extra.len() / dim;
            println!("{}", json!({"cmd": "insert", "engine": a[2], "n": n, "insert_s": ins, "with_persist_s": total,
                "vps": n as f64 / ins, "vps_with_persist": n as f64 / total, "ef": ef, "recall10_after": recall(&res, &gt), "recall10_after_uniform": split(&res, &gt, 0), "recall10_after_clustered": split(&res, &gt, 1), "disk_bytes": disk(&db)}));
        }
        c => panic!("unknown command {c}"),
    }
}
