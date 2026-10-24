// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Memoization of read-only MCP tool results (docs/design/agent-context-caching.md
//! §1). Separate from `client` so the keying rule, which must never serve one
//! request's answer to another, is tested without a running server. One
//! [`Memo`] belongs to one connected server in one session; nothing is shared
//! across sessions or persisted.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use cox_protocol::types::ToolOutput;
use serde_json::Value;

/// Bounded, TTL'd cache of successful read-only results.
pub struct Memo {
    ttl: Duration,
    cap: usize,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    map: BTreeMap<String, (Instant, ToolOutput)>,
    order: VecDeque<String>,
}

/// The cache key: tool name and the canonical arguments, length-prefixed so
/// no tool/argument split can collide with another.
pub fn key(tool: &str, args: &Value) -> String {
    let canon = canonical(args).to_string();
    format!("{}:{tool}\u{0}{canon}", tool.len())
}

/// `args` with object keys sorted at every depth. Values, their JSON types
/// and array order are left alone: each of those can change a result.
fn canonical(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let sorted: BTreeMap<&String, Value> =
                m.iter().map(|(k, v)| (k, canonical(v))).collect();
            Value::Object(sorted.into_iter().map(|(k, v)| (k.clone(), v)).collect())
        }
        Value::Array(a) => Value::Array(a.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

impl Memo {
    /// `ttl_secs == 0` or `cap == 0` disables the memo.
    pub fn new(ttl_secs: u64, cap: usize) -> Self {
        Self {
            ttl: Duration::from_secs(ttl_secs),
            cap,
            inner: Mutex::new(Inner::default()),
        }
    }

    pub fn enabled(&self) -> bool {
        !self.ttl.is_zero() && self.cap > 0
    }

    /// A stored result for exactly `key`, if it has not expired.
    pub fn get(&self, key: &str) -> Option<ToolOutput> {
        self.get_at(key, Instant::now())
    }

    fn get_at(&self, key: &str, now: Instant) -> Option<ToolOutput> {
        if !self.enabled() {
            return None;
        }
        let mut inner = self.inner.lock().ok()?;
        match inner.map.get(key) {
            Some((at, out)) if now.duration_since(*at) < self.ttl => Some(out.clone()),
            Some(_) => {
                inner.map.remove(key);
                inner.order.retain(|k| k != key);
                None
            }
            None => None,
        }
    }

    /// Stores `out` unless it is an error; evicts the oldest entry past `cap`.
    pub fn put(&self, key: String, out: &ToolOutput) {
        self.put_at(key, out, Instant::now());
    }

    fn put_at(&self, key: String, out: &ToolOutput, now: Instant) {
        if !self.enabled() || out.is_error {
            return;
        }
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        if inner.map.insert(key.clone(), (now, out.clone())).is_some() {
            inner.order.retain(|k| *k != key);
        }
        inner.order.push_back(key);
        while inner.order.len() > self.cap {
            if let Some(old) = inner.order.pop_front() {
                inner.map.remove(&old);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn out(text: &str) -> ToolOutput {
        ToolOutput {
            text: text.into(),
            is_error: false,
            diff: None,
            structured: None,
        }
    }

    /// Fake server: counts calls and answers with the key it was asked for,
    /// so a foreign hit shows up as the wrong text.
    struct Server {
        memo: Memo,
        calls: usize,
    }

    impl Server {
        fn new() -> Self {
            Self {
                memo: Memo::new(60, 16),
                calls: 0,
            }
        }
        fn call(&mut self, tool: &str, args: &Value) -> String {
            let k = key(tool, args);
            if let Some(hit) = self.memo.get(&k) {
                return hit.text;
            }
            self.calls += 1;
            let fresh = out(&format!("{tool} {args}"));
            self.memo.put(k, &fresh);
            fresh.text
        }
    }

    #[test]
    fn identical_request_is_served_from_memo_once_fetched() {
        let mut s = Server::new();
        let a = s.call("search", &json!({"q": "slint"}));
        let b = s.call("search", &json!({"q": "slint"}));
        assert_eq!(a, b);
        assert_eq!(s.calls, 1);
    }

    #[test]
    fn key_order_alone_does_not_miss() {
        let mut s = Server::new();
        s.call("search", &json!({"q": "x", "n": 2, "o": {"b": 1, "a": 2}}));
        s.call("search", &json!({"o": {"a": 2, "b": 1}, "n": 2, "q": "x"}));
        assert_eq!(s.calls, 1);
    }

    #[test]
    fn different_tool_with_same_args_never_shares_a_result() {
        let mut s = Server::new();
        let a = s.call("search", &json!({"q": "x"}));
        let b = s.call("fetch", &json!({"q": "x"}));
        assert_ne!(a, b);
        assert!(b.starts_with("fetch "), "foreign hit: {b}");
        assert_eq!(s.calls, 2);
    }

    #[test]
    fn any_argument_difference_is_a_fresh_call() {
        let variants = [
            json!({"q": "x"}),
            json!({"q": "y"}),
            json!({"q": "x", "page": 2}),
            json!({"q": 1}),
            json!({"q": "1"}),
            json!({"q": ["a", "b"]}),
            json!({"q": ["b", "a"]}),
            json!({"q": {"user": "ivan"}}),
            json!({"q": {"user": "other"}}),
            json!({"q": null}),
            json!({}),
            json!(null),
        ];
        let mut s = Server::new();
        for v in &variants {
            let got = s.call("search", v);
            assert_eq!(got, format!("search {v}"), "wrong result for {v}");
        }
        assert_eq!(s.calls, variants.len());
    }

    #[test]
    fn tool_and_args_boundary_cannot_collide() {
        assert_ne!(key("a", &json!("b")), key("a\"", &json!("")));
        assert_ne!(key("ab", &json!({})), key("a", &json!({"b": {}})));
    }

    #[test]
    fn separate_memos_never_see_each_others_entries() {
        let one = Memo::new(60, 16);
        let other = Memo::new(60, 16);
        one.put(key("search", &json!({"q": "x"})), &out("server one"));
        assert!(other.get(&key("search", &json!({"q": "x"}))).is_none());
    }

    #[test]
    fn errors_are_never_cached() {
        let m = Memo::new(60, 16);
        let mut e = out("boom");
        e.is_error = true;
        m.put("k".into(), &e);
        assert!(m.get("k").is_none());
    }

    #[test]
    fn expired_entry_is_a_fresh_call() {
        let m = Memo::new(10, 16);
        let t0 = Instant::now();
        m.put_at("k".into(), &out("old"), t0);
        assert!(m.get_at("k", t0 + Duration::from_secs(9)).is_some());
        assert!(m.get_at("k", t0 + Duration::from_secs(10)).is_none());
    }

    #[test]
    fn cap_evicts_the_oldest_entry() {
        let m = Memo::new(60, 2);
        m.put("a".into(), &out("a"));
        m.put("b".into(), &out("b"));
        m.put("c".into(), &out("c"));
        assert!(m.get("a").is_none());
        assert_eq!(m.get("c").map(|o| o.text), Some("c".into()));
    }

    #[test]
    fn disabled_memo_stores_nothing() {
        let m = Memo::new(0, 16);
        m.put("k".into(), &out("x"));
        assert!(m.get("k").is_none());
    }
}
