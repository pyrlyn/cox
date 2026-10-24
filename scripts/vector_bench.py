#!/usr/bin/env python3
"""Orchestrates the exp/vector-bench runs and writes results.jsonl.

Usage: vector_bench.py <vecbench-binary> <workdir>
Every engine run is a fresh process wrapped in /usr/bin/time -l (peak RSS).
"""
import json, os, re, shutil, subprocess, sys

B, W = sys.argv[1], sys.argv[2]
DATA = os.path.join(W, "data")
OUT = os.path.join(W, "results.jsonl")
EFS = "64,128,256,512,1024,2048"
THREADS = "1,2,4,8,16,32,64"
CONFIGS = [  # name, engine, m, ef_construction, quant, curve_ef
    ("vecdb-m16", "vecdb", 16, 200, "f32", 512),
    ("vecdb-m32", "vecdb", 32, 400, "f32", 512),
    ("vecdb-m32-i8", "vecdb", 32, 400, "i8", 512),
    ("qdrant-m16", "qdrant", 16, 200, "f32", 512),
    ("qdrant-m32", "qdrant", 32, 400, "f32", 512),
]


def free_gb():
    st = os.statvfs(os.path.expanduser("~"))
    return st.f_bavail * st.f_frsize / 2**30


def run(tag, args):
    if free_gb() < 2:
        sys.exit("LOW DISK, stopping")
    p = subprocess.run(["/usr/bin/time", "-l", B, *args], capture_output=True, text=True)
    if p.returncode:
        print(p.stdout, p.stderr, file=sys.stderr)
        raise SystemExit(f"{tag}: exit {p.returncode}")
    rss = int(re.search(r"(\d+)\s+maximum resident set size", p.stderr).group(1))
    rows = [json.loads(l) for l in p.stdout.splitlines() if l.startswith("{")]
    with open(OUT, "a") as f:
        for r in rows:
            r["config"], r["phase_peak_rss_bytes"] = tag, rss
            f.write(json.dumps(r) + "\n")
    print(tag, args[0], f"rss={rss/2**20:.0f}MiB", json.dumps(rows[-1])[:200], flush=True)
    return rows


def rm(p):
    if os.path.isdir(p):
        shutil.rmtree(p)
    for s in ("", "-wal", "-shm"):
        if os.path.isfile(p + s):
            os.remove(p + s)


def main():
    only = set(sys.argv[3:])
    for name, eng, m, efc, quant, cef in CONFIGS:
        if only and name not in only:
            continue
        db = os.path.join(W, name + (".db" if eng == "vecdb" else ""))
        rm(db)
        run(name, ["build", eng, DATA, db, str(m), str(efc), quant, "16"])
        run(name, ["query", eng, DATA, db, EFS, THREADS, str(cef)])
        for i in range(3):  # fresh process, warm page cache
            run(name + f"#cold{i}", ["query", eng, DATA, db, "64", "1", "64"])
        cp = db + ".ins"
        rm(cp)
        (shutil.copytree if os.path.isdir(db) else shutil.copy)(db, cp)
        run(name, ["insert", eng, DATA, cp, str(cef), "16"])
        rm(cp)
        rm(db)
    if not only or "sqlite-brute" in only:
        db = os.path.join(W, "brute.db")
        rm(db)
        run("sqlite-brute", ["build", "sqlite", DATA, db, "0", "0", "f32", "1"])
        run("sqlite-brute", ["query", "sqlite", DATA, db, "10", "1,4", "10"])
        rm(db)


main()
