#!/usr/bin/env python3
"""Renders results.jsonl from vector_bench.py as Markdown tables."""
import json, sys
from collections import defaultdict

rows = [json.loads(l) for l in open(sys.argv[1])]
by = defaultdict(list)
for r in rows:
    by[r["config"].split("#")[0]].append(r)


def grade(r):
    return "FAIL" if r < 0.85 else ("prod" if r >= 0.95 else ("ok" if r >= 0.90 else "<0.90"))


mib = lambda b: f"{b / 2**20:.0f}"
print("| config | build s | disk MiB | build RSS MiB | query RSS MiB | open ms (median) | first result ms (median) | first result ms, 3 runs | insert 10k vec/s | insert+persist vec/s | recall@10 after insert (ef) |")
print("|---|---|---|---|---|---|---|---|---|---|")
for c, rs in by.items():
    b = next(r for r in rs if r["cmd"] == "build")
    colds = sorted((r for r in rs if r["cmd"] == "cold"), key=lambda r: r["first_result_ms"])
    cold = colds[len(colds) // 2] if colds else next(r for r in rs if r["cmd"] == "cold")
    spread = "/".join(f"{r['first_result_ms']:.0f}" for r in colds)
    q = next(r for r in rs if r["cmd"] == "curve")
    ins = next((r for r in rs if r["cmd"] == "insert"), None)
    it = (f"{ins['vps']:.0f} | {ins['vps_with_persist']:.0f} | {ins['recall10_after']:.4f} {grade(ins['recall10_after'])} (ef {ins['ef']})" if ins else "- | - | -")
    print(f"| {c} | {b['build_total_s']:.1f} | {mib(b['disk_bytes'])} | {mib(b['phase_peak_rss_bytes'])} | {mib(q['phase_peak_rss_bytes'])} | {cold['open_ms']:.0f} | {cold['first_result_ms']:.0f} | {spread or '-'} | {it} |")
print()
print("| config | ef | recall@10 | grade | uniform | clustered | QPS 1t | p50 ms | p95 ms | p99 ms |")
print("|---|---|---|---|---|---|---|---|---|---|")
for c, rs in by.items():
    for r in (r for r in rs if r["cmd"] == "ef"):
        l = r["lat"]
        print(f"| {c} | {r['ef']} | {r['recall10']:.4f} | {grade(r['recall10'])} | {r['recall10_uniform']:.4f} | {r['recall10_clustered']:.4f} | {r['qps_1t']:.0f} | {l['p50_ms']:.3f} | {l['p95_ms']:.3f} | {l['p99_ms']:.3f} |")
print()
print("| config | ef | threads | QPS | p50 ms | p95 ms | p99 ms |")
print("|---|---|---|---|---|---|---|")
for c, rs in by.items():
    cur = [r for r in rs if r["cmd"] == "curve" and r["config"] == c]
    for r in cur:
        l = r["lat"]
        print(f"| {c} | {r['ef']} | {r['threads']} | {r['qps']:.0f} | {l['p50_ms']:.3f} | {l['p95_ms']:.3f} | {l['p99_ms']:.3f} |")
