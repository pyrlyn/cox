"""Opt-in eval harness (T12.1): one fresh tempdir per task, `setup`, a
headless `cox run`, then `check`. Cost comes from the JSON output.

    just eval --dry-run            # scripted provider, no network, no key
    just eval                      # real provider (needs a key in env)
    just eval --only create-file   # one task
    just eval --provider openai --model gpt-4o-mini
    just eval --preset verify      # T30.3: verify-before-done + test hook
    just eval --preset jev-route --plugin-dir <built plugins/jev package>

`--preset verify` (T30.3) adds a harness system addendum telling the model
to run the task's tests and show the output before reporting done, and
wires `evals/hooks/verify.sh` as a `PostToolUse` hook on `edit`/
`apply_patch`/`write` (hooks stay enabled instead of the default
`--no-hooks`). The addendum goes into the per-task `COX_HOME/AGENTS.md`
and the hook into `COX_HOME/config.toml` — both already fresh per task.

`--preset jev-route` (E2, T33.40.10) runs every task twice: `baseline` on
the static tier pick and `jev` with the Jev plugin answering `route`
(J§5.2). Cost comes from the ledger (`cox stats --session --csv`), since
the plugin's own calls are ledger rows the JSON output does not sum; the
run stops once the ledger total reaches `--max-usd`. `--dry-run` runs the
baseline arm only, so it needs no key and spends nothing.

Exit code is 0 only when every selected task passes.
"""

import argparse
import csv
import io
import json
import math
import os
import shutil
import statistics
import subprocess
import tempfile
import time
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

import tomli_w
import yaml

# The uv project root (`evals/`); tasks and hooks live beside the package,
# which uv installs editable, so `__file__` stays inside the checkout.
EVALS = Path(__file__).resolve().parents[2]
TASKS = EVALS / "tasks"
HOOKS = EVALS / "hooks"

# T30.3: harness system addendum for `--preset verify`, loaded through the
# AGENTS.md/CLAUDE.md hierarchy (cox_ext::instructions candidates() puts
# `<COX_HOME>/AGENTS.md` first, ahead of every project file).
VERIFY_ADDENDUM = (
    "Before reporting done, run the task's tests and show the output.\n"
)

# E2's falsifier (T33.40.10): below this median saving per task, `route`
# is not worth its risk and stays off by default.
MIN_MEDIAN_SAVING = 0.15


def load_tasks(names):
    paths = sorted(TASKS.glob("*.yaml"))
    if names:
        wanted = set(names)
        paths = [p for p in paths if p.stem in wanted or p.stem.split("-", 1)[-1] in wanted]
    return [(p, yaml.safe_load(p.read_text())) for p in paths]


def find_cox_bin(explicit):
    if explicit:
        return explicit
    if os.environ.get("COX_BIN"):
        return os.environ["COX_BIN"]
    found = shutil.which("cox")
    if found:
        return found
    try:
        meta = subprocess.run(
            ["cargo", "metadata", "--no-deps", "--format-version", "1"],
            capture_output=True, text=True, check=True, cwd=EVALS.parent,
        )
        candidate = (
            Path(json.loads(meta.stdout)["target_directory"]) / "debug" / "cox"
        )
        if candidate.exists():
            return str(candidate)
    except Exception:
        pass
    raise SystemExit("no cox binary: build first (`cargo build -p cox`) or pass --cox-bin")


def scenario_toml(turns):
    """Embedded dry-run turns to a Scripted provider scenario."""
    out = []
    for turn in turns or []:
        calls = turn.get("calls") or []
        head = {"text": turn.get("text", "")}
        if calls:
            head["tool_calls"] = [
                {"name": call["tool"], "input": call.get("input", {})} for call in calls
            ]
        out.append(head)
        if calls:
            out.append({"text": turn.get("final", "")})
    return tomli_w.dumps({"turn": out})


def write_verify_preset(home):
    """T30.3: drop the addendum + hook config into a fresh COX_HOME."""
    (home / "AGENTS.md").write_text(VERIFY_ADDENDUM)
    hook = {
        "matcher": "edit|apply_patch|write",
        "command": str(HOOKS / "verify.sh"),
        # Above the script's own 120s cap, so the cap (not this timeout)
        # is what fires on a hanging test command.
        "timeout_s": 130,
    }
    (home / "config.toml").write_text(tomli_w.dumps({"hooks": {"PostToolUse": [hook]}}))


def prepare_jev_route(home, arm, ctx):
    """E2: the `jev` arm installs the plugin into this run's COX_HOME and
    hands it `route`; the baseline keeps the static pick. Returns an error
    note, or None."""
    if arm != "jev":
        return None
    (home / "config.toml").write_text(tomli_w.dumps({"plugins": {"decide": {"route": "jev"}}}))
    install = subprocess.run(
        [ctx["cox_bin"], "plugin", "install", str(ctx["plugin_dir"]), "--yes"],
        env=ctx["env"], capture_output=True, text=True,
    )
    if install.returncode != 0:
        return f"plugin install failed: {install.stderr.strip() or install.stdout.strip()}"
    return None


@dataclass(frozen=True)
class Preset:
    """One `--preset`: the arms each task runs under, in table order, and
    what an arm puts into its fresh COX_HOME before `cox run`."""
    arms: tuple
    prepare: Callable  # (home, arm, ctx) -> error note or None
    hooks: bool = False
    # Env vars a live run needs. They are read from the env only: the runs
    # get `COX_KEYRING=off`, so a missing key fails instead of prompting.
    keys: tuple = ()
    plugin: bool = False  # needs `--plugin-dir` for any arm but the first
    budget_usd: float | None = None  # per-run `--budget` default
    max_usd: float | None = None  # ledger total that stops the run
    ledger: bool = False  # read cost, tiers and latency from the ledger


PRESETS = {
    "verify": Preset(("verify",), lambda home, arm, ctx: write_verify_preset(home), hooks=True),
    # The cap is the creator's (decision 9 in `docs/design/plugins.md` §14):
    # $3 in total, and each run needs the creator's go-ahead.
    "jev-route": Preset(("baseline", "jev"), prepare_jev_route,
                        keys=("ANTHROPIC_API_KEY", "TYPESAFE_API_KEY"), plugin=True,
                        budget_usd=0.25, max_usd=3.00, ledger=True),
}


def read_ledger(cox_bin, env, session):
    """One run's usage rows from `cox stats --session ID --csv`, summed into
    the E2 metrics: $ by (tier, job), main calls per tier, cache tokens and
    every `plugin:*` call's latency."""
    out = {"cost_usd": 0.0, "by_tier_job": {}, "main_calls": {}, "cache_read": 0,
           "cache_write": 0, "plugin_latency_ms": [], "models": []}
    if not session:
        return out
    proc = subprocess.run([cox_bin, "stats", "--session", session, "--csv"],
                          env=env, capture_output=True, text=True)
    if proc.returncode != 0 or not proc.stdout.startswith("turn,"):
        return out
    for row in csv.DictReader(io.StringIO(proc.stdout)):
        try:
            cost = float(row["cost_usd"])
            key = f'{row["tier"]}/{row["job"]}'
            out["cost_usd"] += cost
            out["by_tier_job"][key] = out["by_tier_job"].get(key, 0.0) + cost
            out["cache_read"] += int(row["cache_read"])
            out["cache_write"] += int(row["cache_write"])
            if row["job"] == "main":
                out["main_calls"][row["tier"]] = out["main_calls"].get(row["tier"], 0) + 1
            elif row["job"].startswith("plugin:"):
                out["plugin_latency_ms"].append(int(row["latency_ms"]))
            if row["model"] not in out["models"]:
                out["models"].append(row["model"])
        except (KeyError, ValueError, TypeError):
            continue
    return out


def run_task(task, *, cox_bin, dry_run, provider, model, preset=None, arm=None,
             budget=None, plugin_dir=None):
    spec = PRESETS.get(preset)
    arm = arm or (spec.arms[0] if spec else None)
    work = Path(tempfile.mkdtemp(prefix="cox-eval-"))
    home = Path(tempfile.mkdtemp(prefix="cox-eval-home-"))
    scenario_file = None
    env = dict(os.environ, COX_HOME=str(home), HOME=str(home))
    if spec and spec.keys:
        # E2's keys come from the creator's env, never the keychain (A49).
        env["COX_KEYRING"] = "off"
    started = time.time()
    if spec:
        error = spec.prepare(home, arm, {"cox_bin": cox_bin, "env": env, "plugin_dir": plugin_dir})
        if error:
            return result(task, False, 0.0, 0, time.time() - started, error, arm=arm)
    if dry_run:
        scenario_file = work / "scenario.toml"
        scenario_file.write_text(scenario_toml(task.get("dry_run", {}).get("turns")))
        env["COX_PROVIDER"] = "scripted"
        env["COX_SCENARIO"] = str(scenario_file)
    setup = subprocess.run(
        ["sh", "-c", task.get("setup") or "true"],
        cwd=work, capture_output=True, text=True,
    )
    if setup.returncode != 0:
        return result(task, False, 0.0, 0, time.time() - started, "setup failed", arm=arm)
    cmd = [
        cox_bin, "run", "-p", task["prompt"],
        "--output-format", "json", "--max-turns", "40",
        "--approve", "never", "--permission-mode", "auto",
        # Hermetic evals: ambient MCP servers would add seconds of startup
        # noise (and nondeterminism) to every task. Hooks stay off too,
        # except under `--preset verify`, which wires its own PostToolUse
        # hook through this task's fresh COX_HOME/config.toml.
        "--no-mcp",
    ]
    if not (spec and spec.hooks):
        cmd.append("--no-hooks")
    if provider:
        cmd += ["--provider", provider]
    if model:
        cmd += ["--tier", f"code={model}"]
    if budget is not None:
        cmd += ["--budget", str(budget)]
    out_file = work / "cox-out.json"
    try:
        proc = subprocess.run(
            cmd, cwd=work, env=env, capture_output=True, text=True,
            timeout=task.get("timeout_s", 300),
        )
    except subprocess.TimeoutExpired:
        # The ledger cannot be read without the session id the killed run
        # never printed, so the cap charges the whole per-run budget.
        return result(task, False, budget or 0.0, 0, time.time() - started, "timeout", arm=arm)
    out_file.write_text(proc.stdout)
    cost, turns, tokens, session = 0.0, 0, {}, None
    try:
        payload = json.loads(proc.stdout or "{}")
        cost = float(payload.get("cost_usd", 0.0))
        turns = int(payload.get("turns", 0))
        tokens = {k: int(v) for k, v in (payload.get("usage") or {}).items()}
        session = payload.get("session")
    except (ValueError, TypeError, AttributeError):
        pass
    ledger = None
    if spec and spec.ledger:
        ledger = read_ledger(cox_bin, env, session)
        cost = ledger["cost_usd"]
    if proc.returncode != 0:
        return result(task, False, cost, turns, time.time() - started,
                      f"cox exit {proc.returncode}", tokens, arm=arm, ledger=ledger)
    check_env = dict(os.environ, COX_OUT=str(out_file))
    check = subprocess.run(["sh", "-c", task.get("check") or "true"],
                           cwd=work, env=check_env, capture_output=True, text=True)
    ok = check.returncode == 0
    return result(task, ok, cost, turns, time.time() - started,
                  "" if ok else f"check failed: {check.stderr.strip() or check.stdout.strip()}",
                  tokens, arm=arm, ledger=ledger)


def result(task, ok, cost, turns, seconds, note, tokens=None, *, arm=None, ledger=None):
    return {"name": task["name"], "pass": ok, "cost_usd": cost,
            "turns": turns, "seconds": round(seconds, 1), "note": note,
            "tokens": tokens or {}, "arm": arm, "ledger": ledger}


TOKEN_KEYS = ("input_tokens", "output_tokens", "cache_read_tokens", "cache_write_tokens")


def token_line(tokens):
    """`in/out/cache-read/cache-write`, the four counts the ledger keeps."""
    return "/".join(str(tokens.get(k, 0)) for k in TOKEN_KEYS)


def p95(values):
    """Nearest-rank 95th percentile, or None for no values."""
    if not values:
        return None
    ordered = sorted(values)
    return ordered[math.ceil(0.95 * len(ordered)) - 1]


def downgrades(ledger):
    """(main calls on `cheap`, all main calls) of one run."""
    calls = (ledger or {}).get("main_calls") or {}
    return calls.get("cheap", 0), sum(calls.values())


def paired_report(results, arms):
    """The E2 table: one row per task with each arm's outcome and cost and
    the second arm's saving, then the per-arm totals and the falsifier."""
    by = {(r["name"], r["arm"]): r for r in results}
    names = list(dict.fromkeys(r["name"] for r in results))
    base, other = arms[0], arms[-1]
    out = [f"| task | {' | '.join(arms)} | saving | {other} downgrades |",
           "|---" * (len(arms) + 3) + "|"]
    savings, regressions = [], []
    for name in names:
        cells = []
        for arm in arms:
            r = by.get((name, arm))
            cells.append("—" if r is None else
                         f'{"PASS" if r["pass"] else "FAIL"} ${r["cost_usd"]:.4f}')
        b, o = by.get((name, base)), by.get((name, other))
        saving = "—"
        if b and o and b["cost_usd"] > 0:
            savings.append(1 - o["cost_usd"] / b["cost_usd"])
            saving = f"{savings[-1]:.0%}"
        if b and o and b["pass"] and not o["pass"]:
            regressions.append(name)
        cheap, total = downgrades(o and o["ledger"])
        out.append(f"| {name} | {' | '.join(cells)} | {saving} | {cheap}/{total} |")
    out.append("")
    for arm in arms:
        rows = [r for r in results if r["arm"] == arm]
        ledgers = [r["ledger"] or {} for r in rows]
        by_tier_job = {}
        for led in ledgers:
            for key, cost in (led.get("by_tier_job") or {}).items():
                by_tier_job[key] = by_tier_job.get(key, 0.0) + cost
        cheap = sum(downgrades(led)[0] for led in ledgers)
        main = sum(downgrades(led)[1] for led in ledgers)
        latency = p95([ms for led in ledgers for ms in led.get("plugin_latency_ms") or []])
        models = sorted({m for led in ledgers for m in led.get("models") or []})
        out.append(
            f'{arm}: {sum(r["pass"] for r in rows)}/{len(rows)} passed'
            f'  ${sum(r["cost_usd"] for r in rows):.4f}'
            f'  by tier/job {", ".join(f"{k} ${v:.4f}" for k, v in sorted(by_tier_job.items())) or "-"}'
            f'  downgrades {cheap}/{main}'
            f'  cache read/write {sum(led.get("cache_read", 0) for led in ledgers)}'
            f'/{sum(led.get("cache_write", 0) for led in ledgers)}'
            f'  plugin p95 {"-" if latency is None else f"{latency}ms"}'
            f'  models {", ".join(models) or "-"}')
    median = statistics.median(savings) if savings else None
    out.append(f'median saving: {"-" if median is None else f"{median:.0%}"}')
    reasons = [f"fails only with {other}: {', '.join(regressions)}"] if regressions else []
    if median is None or median < MIN_MEDIAN_SAVING:
        reasons.append(f"median saving below {MIN_MEDIAN_SAVING:.0%}")
    out.append(f"falsifier: {'; '.join(reasons) if reasons else 'not triggered'}")
    return "\n".join(out)


def main(argv=None):
    parser = argparse.ArgumentParser(description="cox eval harness")
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--only", nargs="*", default=[])
    parser.add_argument("--cox-bin", default=None)
    parser.add_argument("--provider", default=None)
    parser.add_argument("--model", default=None)
    parser.add_argument(
        "--preset", choices=sorted(PRESETS), default=None,
        help="verify: add the run-tests-before-done addendum and the "
             "PostToolUse test-runner hook (T30.3); jev-route: every task "
             "on the static pick and with Jev answering `route` (T33.40.10)",
    )
    parser.add_argument("--budget", type=float, default=None, help="per-run `cox run --budget` (USD)")
    parser.add_argument("--max-usd", type=float, default=None,
                        help="stop once the runs' total cost reaches this (USD)")
    parser.add_argument("--plugin-dir", type=Path, default=None,
                        help="the built plugin package a preset installs")
    args = parser.parse_args(argv)
    spec = PRESETS.get(args.preset)
    arms = spec.arms if spec else (None,)
    budget = args.budget if args.budget is not None else spec and spec.budget_usd
    max_usd = args.max_usd if args.max_usd is not None else spec and spec.max_usd
    if spec and args.dry_run:
        # A dry run spends nothing and needs no key, so it never reaches a
        # live plugin: the arms after the first need one (E1/E2's rule).
        arms = arms[:1]
    elif spec:
        missing = [k for k in spec.keys if not os.environ.get(k)]
        if missing:
            raise SystemExit(f"--preset {args.preset} needs {', '.join(missing)} in the env")
        if spec.plugin and not (args.plugin_dir and (args.plugin_dir / "plugin.toml").is_file()):
            raise SystemExit(f"--preset {args.preset} needs --plugin-dir with a plugin.toml")
    cox_bin = find_cox_bin(args.cox_bin)
    tasks = load_tasks(args.only)
    if not tasks:
        raise SystemExit("no tasks selected")
    print(f"cox: {cox_bin}  tasks: {len(tasks)}"
          f"  mode: {'dry-run (scripted)' if args.dry_run else 'live'}"
          f"{f'  preset: {args.preset}' if args.preset else ''}")
    results = []
    spent = 0.0
    for path, task in tasks:
        for arm in arms:
            if max_usd is not None and spent >= max_usd:
                break
            res = run_task(task, cox_bin=cox_bin, dry_run=args.dry_run,
                           provider=args.provider, model=args.model,
                           preset=args.preset, arm=arm, budget=budget,
                           plugin_dir=args.plugin_dir)
            results.append(res)
            spent += res["cost_usd"]
            flag = "PASS" if res["pass"] else "FAIL"
            label = f'{res["name"]}{f" [{arm}]" if len(arms) > 1 else ""}'
            print(f'{label:20} {flag:4}  ${res["cost_usd"]:.4f}'
                  f'  turns={res["turns"]}  tok={token_line(res["tokens"])}'
                  f'  {res["seconds"]}s  {res["note"]}')
    stopped = len(results) < len(tasks) * len(arms)
    if stopped:
        print(f"stopped: spent ${spent:.4f} of the ${max_usd:.2f} cap")
    passed = sum(1 for r in results if r["pass"])
    total_tokens = {k: sum(r["tokens"].get(k, 0) for r in results) for k in TOKEN_KEYS}
    print(f"{passed}/{len(results)} passed  total cost ${spent:.4f}"
          f"  tokens in/out/cache-read/cache-write {token_line(total_tokens)}")
    if len(arms) > 1:
        print(paired_report(results, arms))
    return 0 if passed == len(results) and not stopped else 1


if __name__ == "__main__":
    raise SystemExit(main())
