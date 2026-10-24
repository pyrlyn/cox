# Vector store benchmark: qdrant-edge vs SQLite HNSW (cox-vecdb)

Experimental, local-only branch. Compares the in-house SQLite-backed HNSW
(`crates/cox-vecdb`) with `qdrant-edge` 0.9 (local fork with forced
`parking_lot/deadlock_detection` removed, so it links next to `nucleo`'s
`send_guard`) and an exact SQLite brute-force baseline.

## Setup

- Data: 100,000 base vectors + 10,000 insert vectors, dim 384, cosine,
  1,000 queries. Half uniform-on-sphere (hard case), half clustered around
  256 centres (realistic embedding-like case). Ground truth by exact search.
- Machine: Ivan's Mac (Apple Silicon), release build, page cache warm.
- Each phase runs in a fresh process under `/usr/bin/time -l` (peak RSS).
- Recall@10 grading: < 0.85 FAIL, >= 0.90 prototype, >= 0.95 production.

## Build, disk, cold start, incremental insert

| config | build s | disk MiB | build RSS MiB | query RSS MiB | open ms (median) | first result ms (median) | first result ms, 3 runs | insert 10k vec/s | insert+persist vec/s | recall@10 after insert (ef) |
|---|---|---|---|---|---|---|---|---|---|
| vecdb-m16 | 14.2 | 158 | 500 | 398 | 46 | 47 | 41/44/47/48 | 6392 | 3626 | 0.7772 FAIL (ef 512) |
| vecdb-m32 | 41.2 | 168 | 428 | 406 | 46 | 48 | 41/43/48/67 | 1340 | 1110 | 0.8818 <0.90 (ef 512) |
| vecdb-m32-i8 | 19.4 | 58 | 393 | 183 | 27 | 28 | 23/24/28/30 | 2695 | 2304 | 0.8771 <0.90 (ef 512) |
| qdrant-m16 | 27.4 | 318 | 932 | 304 | 3166 | 3167 | 192/280/3167/4491 | 49405 | 20846 | 0.8291 FAIL (ef 512) |
| qdrant-m32 | 87.5 | 323 | 949 | 333 | 161 | 162 | 83/116/162/239 | 115572 | 51304 | 0.9284 ok (ef 512) |
| sqlite-brute | 144.8 | 198 | 303 | 6437 | 1 | 298 | 298 | - | - | - |

## Recall / latency vs ef (1 thread)

| config | ef | recall@10 | grade | uniform | clustered | QPS 1t | p50 ms | p95 ms | p99 ms |
|---|---|---|---|---|---|---|---|---|---|
| vecdb-m16 | 64 | 0.5932 | FAIL | 0.1882 | 0.9982 | 1346 | 0.615 | 1.140 | 3.533 |
| vecdb-m16 | 128 | 0.6458 | FAIL | 0.2932 | 0.9984 | 628 | 1.078 | 2.071 | 18.585 |
| vecdb-m16 | 256 | 0.7131 | FAIL | 0.4278 | 0.9984 | 484 | 2.052 | 2.966 | 4.939 |
| vecdb-m16 | 512 | 0.7896 | FAIL | 0.5808 | 0.9984 | 227 | 4.217 | 5.405 | 10.208 |
| vecdb-m16 | 1024 | 0.8711 | <0.90 | 0.7438 | 0.9984 | 110 | 8.316 | 11.367 | 25.915 |
| vecdb-m16 | 2048 | 0.9435 | ok | 0.8886 | 0.9984 | 46 | 20.428 | 33.005 | 49.473 |
| vecdb-m32 | 64 | 0.6426 | FAIL | 0.2866 | 0.9986 | 811 | 1.172 | 1.869 | 2.841 |
| vecdb-m32 | 128 | 0.7133 | FAIL | 0.4276 | 0.9990 | 421 | 2.380 | 3.684 | 4.403 |
| vecdb-m32 | 256 | 0.7994 | FAIL | 0.5998 | 0.9990 | 240 | 4.322 | 5.372 | 5.964 |
| vecdb-m32 | 512 | 0.8916 | <0.90 | 0.7842 | 0.9990 | 118 | 8.524 | 10.338 | 11.924 |
| vecdb-m32 | 1024 | 0.9625 | prod | 0.9260 | 0.9990 | 65 | 15.136 | 17.700 | 19.902 |
| vecdb-m32 | 2048 | 0.9924 | prod | 0.9858 | 0.9990 | 41 | 23.669 | 27.762 | 33.190 |
| vecdb-m32-i8 | 64 | 0.6394 | FAIL | 0.2926 | 0.9862 | 1014 | 0.925 | 1.556 | 2.015 |
| vecdb-m32-i8 | 128 | 0.7069 | FAIL | 0.4274 | 0.9864 | 668 | 1.473 | 2.175 | 2.626 |
| vecdb-m32-i8 | 256 | 0.7930 | FAIL | 0.5996 | 0.9864 | 322 | 2.962 | 4.392 | 5.819 |
| vecdb-m32-i8 | 512 | 0.8844 | <0.90 | 0.7824 | 0.9864 | 184 | 5.345 | 6.910 | 8.233 |
| vecdb-m32-i8 | 1024 | 0.9505 | prod | 0.9146 | 0.9864 | 96 | 10.003 | 13.751 | 17.387 |
| vecdb-m32-i8 | 2048 | 0.9797 | prod | 0.9730 | 0.9864 | 57 | 17.149 | 23.476 | 30.851 |
| qdrant-m16 | 64 | 0.6069 | FAIL | 0.2160 | 0.9978 | 1009 | 0.839 | 1.902 | 3.614 |
| qdrant-m16 | 128 | 0.6582 | FAIL | 0.3166 | 0.9998 | 673 | 1.246 | 3.080 | 5.812 |
| qdrant-m16 | 256 | 0.7312 | FAIL | 0.4624 | 1.0000 | 312 | 2.565 | 7.240 | 13.688 |
| qdrant-m16 | 512 | 0.8109 | FAIL | 0.6218 | 1.0000 | 166 | 5.441 | 10.359 | 15.113 |
| qdrant-m16 | 1024 | 0.8964 | <0.90 | 0.7928 | 1.0000 | 113 | 7.933 | 13.032 | 19.278 |
| qdrant-m16 | 2048 | 0.9582 | prod | 0.9164 | 1.0000 | 68 | 13.773 | 21.869 | 28.355 |
| qdrant-m32 | 64 | 0.6550 | FAIL | 0.3116 | 0.9984 | 875 | 1.073 | 1.558 | 1.766 |
| qdrant-m32 | 128 | 0.7336 | FAIL | 0.4672 | 1.0000 | 475 | 2.097 | 2.910 | 3.801 |
| qdrant-m32 | 256 | 0.8281 | FAIL | 0.6562 | 1.0000 | 212 | 4.136 | 9.033 | 15.363 |
| qdrant-m32 | 512 | 0.9196 | ok | 0.8392 | 1.0000 | 84 | 9.825 | 25.041 | 37.953 |
| qdrant-m32 | 1024 | 0.9769 | prod | 0.9538 | 1.0000 | 64 | 15.094 | 20.525 | 24.930 |
| qdrant-m32 | 2048 | 0.9975 | prod | 0.9950 | 1.0000 | 40 | 23.700 | 32.602 | 38.011 |
| sqlite-brute | 10 | 1.0000 | prod | 1.0000 | 1.0000 | 3 | 324.448 | 479.097 | 615.833 |

## Concurrency curve (ef 512)

| config | ef | threads | QPS | p50 ms | p95 ms | p99 ms |
|---|---|---|---|---|---|---|
| vecdb-m16 | 512 | 1 | 189 | 4.751 | 8.212 | 13.992 |
| vecdb-m16 | 512 | 2 | 459 | 4.173 | 5.933 | 9.954 |
| vecdb-m16 | 512 | 4 | 872 | 4.147 | 7.545 | 11.961 |
| vecdb-m16 | 512 | 8 | 1677 | 3.894 | 9.731 | 15.278 |
| vecdb-m16 | 512 | 16 | 1925 | 4.072 | 26.969 | 63.727 |
| vecdb-m16 | 512 | 32 | 1865 | 4.433 | 72.633 | 127.906 |
| vecdb-m16 | 512 | 64 | 2417 | 4.165 | 123.907 | 232.449 |
| vecdb-m32 | 512 | 1 | 130 | 7.433 | 9.198 | 11.897 |
| vecdb-m32 | 512 | 2 | 257 | 7.873 | 9.177 | 10.205 |
| vecdb-m32 | 512 | 4 | 539 | 7.507 | 8.723 | 9.842 |
| vecdb-m32 | 512 | 8 | 998 | 7.454 | 11.643 | 17.400 |
| vecdb-m32 | 512 | 16 | 1527 | 7.994 | 21.510 | 31.482 |
| vecdb-m32 | 512 | 32 | 1601 | 9.315 | 63.658 | 105.431 |
| vecdb-m32 | 512 | 64 | 1655 | 10.511 | 151.557 | 237.333 |
| vecdb-m32-i8 | 512 | 1 | 177 | 5.340 | 8.332 | 11.552 |
| vecdb-m32-i8 | 512 | 2 | 406 | 4.886 | 5.812 | 6.601 |
| vecdb-m32-i8 | 512 | 4 | 882 | 4.328 | 6.385 | 9.446 |
| vecdb-m32-i8 | 512 | 8 | 1829 | 3.739 | 7.513 | 13.381 |
| vecdb-m32-i8 | 512 | 16 | 2964 | 3.494 | 14.651 | 29.056 |
| vecdb-m32-i8 | 512 | 32 | 3658 | 3.437 | 33.344 | 72.592 |
| vecdb-m32-i8 | 512 | 64 | 3299 | 3.779 | 94.046 | 179.307 |
| qdrant-m16 | 512 | 1 | 239 | 4.099 | 5.520 | 7.301 |
| qdrant-m16 | 512 | 2 | 395 | 4.762 | 7.273 | 11.110 |
| qdrant-m16 | 512 | 4 | 652 | 5.034 | 11.691 | 21.233 |
| qdrant-m16 | 512 | 8 | 1359 | 4.863 | 11.506 | 21.813 |
| qdrant-m16 | 512 | 16 | 2411 | 4.936 | 14.827 | 35.780 |
| qdrant-m16 | 512 | 32 | 2499 | 5.457 | 50.996 | 87.309 |
| qdrant-m16 | 512 | 64 | 2341 | 21.107 | 78.523 | 123.634 |
| qdrant-m32 | 512 | 1 | 115 | 8.353 | 12.553 | 16.558 |
| qdrant-m32 | 512 | 2 | 237 | 7.924 | 12.317 | 18.842 |
| qdrant-m32 | 512 | 4 | 493 | 7.465 | 13.040 | 20.955 |
| qdrant-m32 | 512 | 8 | 909 | 8.523 | 11.468 | 13.260 |
| qdrant-m32 | 512 | 16 | 1240 | 9.371 | 31.473 | 67.730 |
| qdrant-m32 | 512 | 32 | 1319 | 10.831 | 83.416 | 124.789 |
| qdrant-m32 | 512 | 64 | 1179 | 45.049 | 136.402 | 199.678 |
| sqlite-brute | 10 | 1 | 3 | 310.821 | 555.553 | 674.002 |
| sqlite-brute | 10 | 4 | 4 | 882.331 | 1486.520 | 1932.315 |

## Findings

- Clustered data: every HNSW config reaches recall >= 0.986 already at ef 64
  (~1 ms p50). Uniform data is the hard case and drives the ef needed.
- Production recall (>= 0.95 overall) needs M=32 and ef 1024:
  vecdb-m32 0.963 (p50 15 ms), vecdb-m32-i8 0.951 (p50 10 ms),
  qdrant-m32 0.977 (p50 15 ms). Same-ef quality is close; qdrant is slightly
  higher, vecdb-i8 is faster.
- vecdb builds 2x faster (41 s vs 88 s for M=32), uses half the disk
  (168 vs 323 MiB; i8: 58 MiB) and half the build RSS.
- Cold start: vecdb ~45 ms, i8 ~28 ms; qdrant 80-240 ms, qdrant-m16 had
  3-4.5 s outliers.
- Incremental insert: qdrant is 40-80x faster (it appends to an unindexed
  segment; recall after insert still holds). vecdb M=32 inserts ~1.1k vec/s
  with persist — fine for memory writes, slow for bulk re-index.
- Concurrency: both scale ~linearly up to 8 threads; vecdb-m32-i8 peaks at
  ~3.7k QPS (32 t), qdrant-m32 ~1.3k QPS.
- Brute force over SQLite blobs: exact but ~3 QPS and ~6.4 GiB RSS — only
  as a ground-truth tool.

## How to run

```sh
export CARGO_TARGET_DIR=~/Grok/target/cox
lockf ~/Grok/target/cox.lock cargo build --release -p cox-vecdb --bin vecbench
B=$CARGO_TARGET_DIR/release/vecbench W=~/Grok/scratch/vecbench
$B gen $W/data 100000 384 1000 10000   # dataset + ground truth
python3 scripts/vector_bench.py $B $W [config ...]   # appends results.jsonl
python3 scripts/vector_bench_report.py $W/results.jsonl
```

Enable/disable: the bench is a separate crate; nothing in cox depends on it.
Removing `crates/cox-vecdb` and the `[patch.crates-io]` block disables it.

## Limitations

- `[patch.crates-io]` points at an absolute local fork path; the branch is not
  buildable elsewhere until the fork is published or upstream fixes the
  feature conflict.
- One synthetic dataset, one machine, one run per point; no real embeddings.
- sqlite-vec `vec0` (cox#488) was not measured in this run.
- Brute-force concurrency curve only covers 1 and 4 threads (too slow).
- Disk guard: the runner stops below 2 GB free.
