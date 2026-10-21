"""`cox_evals.harness`: task loading, the TOML it writes for cox, and how a
`cox run` outcome becomes a pass/fail row with cost and tokens."""

import os
import subprocess
import tomllib

import pytest

from cox_evals import harness

PAYLOAD = {
    "session": "s",
    "result": "done",
    "usage": {
        "input_tokens": 4,
        "output_tokens": 80,
        "cache_read_tokens": 16464,
        "cache_write_tokens": 220,
    },
    "cost_usd": 0.0047,
    "turns": 2,
    "exit_code": 0,
}


def task(**overrides):
    return {"name": "t", "prompt": "do it", "setup": "true", "check": "true", **overrides}


def run(fake_cox, t, **kwargs):
    return harness.run_task(
        t, cox_bin=fake_cox.path, dry_run=False, provider="anthropic",
        model="claude-sonnet-5", **kwargs,
    )


def test_load_tasks_reads_every_task_file():
    tasks = harness.load_tasks([])
    assert len(tasks) == len(list(harness.TASKS.glob("*.yaml")))
    assert all({"name", "prompt", "check"} <= set(t) for _, t in tasks)


def test_only_matches_the_name_with_or_without_its_number():
    by_name = harness.load_tasks(["create-file"])
    by_stem = harness.load_tasks(["01-create-file"])
    assert [p.stem for p, _ in by_name] == ["01-create-file"]
    assert by_name == by_stem


def test_scenario_toml_turns_a_call_into_a_tool_round_and_a_final_answer():
    turns = [{"text": "creating", "calls": [{"tool": "write", "input": {"path": "a", "content": "x"}}],
              "final": "done"}]
    parsed = tomllib.loads(harness.scenario_toml(turns))
    assert parsed == {"turn": [
        {"text": "creating", "tool_calls": [{"name": "write", "input": {"path": "a", "content": "x"}}]},
        {"text": "done"},
    ]}


def test_scenario_toml_keeps_quotes_and_newlines_verbatim():
    text = 'say "hi"\nthen \\ stop'
    parsed = tomllib.loads(harness.scenario_toml([{"text": text}]))
    assert parsed == {"turn": [{"text": text}]}


def test_every_task_dry_run_is_valid_toml():
    for _, t in harness.load_tasks([]):
        tomllib.loads(harness.scenario_toml(t.get("dry_run", {}).get("turns")))


def sh(script, cwd):
    return subprocess.run(["sh", "-c", script], cwd=cwd, capture_output=True, text=True)


def test_write_only_dry_runs_turn_their_failing_check_green(tmp_path):
    # The scripted answer is the task's reference solution: the check must
    # fail on the setup alone, or the task measures nothing, and pass once
    # the answer's files are written.
    for path, t in harness.load_tasks([]):
        calls = [c for turn in t.get("dry_run", {}).get("turns", [])
                 for c in turn.get("calls") or []]
        if not calls or any(c["tool"] != "write" for c in calls):
            continue
        work = tmp_path / path.stem
        work.mkdir()
        assert sh(t["setup"], work).returncode == 0, path.stem
        assert sh(t["check"], work).returncode != 0, path.stem
        for call in calls:
            target = work / call["input"]["path"]
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(call["input"]["content"])
        checked = sh(t["check"], work)
        assert checked.returncode == 0, (path.stem, checked.stdout, checked.stderr)


def test_verify_preset_writes_the_addendum_and_a_post_tool_use_hook(tmp_path):
    harness.write_verify_preset(tmp_path)
    assert (tmp_path / "AGENTS.md").read_text() == harness.VERIFY_ADDENDUM
    hooks = tomllib.loads((tmp_path / "config.toml").read_text())["hooks"]["PostToolUse"]
    assert hooks == [{
        "matcher": "edit|apply_patch|write",
        "command": str(harness.HOOKS / "verify.sh"),
        "timeout_s": 130,
    }]
    assert (harness.HOOKS / "verify.sh").exists()


def test_a_passing_run_records_cost_turns_and_tokens(fake_cox):
    fake_cox.respond(PAYLOAD, write="hello.txt")
    res = run(fake_cox, task(check='test "$(cat hello.txt)" = hi'))
    assert res["pass"] and res["note"] == ""
    assert res["cost_usd"] == 0.0047 and res["turns"] == 2
    assert res["tokens"] == PAYLOAD["usage"]


def test_a_nonzero_cox_exit_fails_but_keeps_the_cost(fake_cox):
    # Exit 2 is a denied tool call: the file may be right, the run still fails.
    fake_cox.respond({**PAYLOAD, "exit_code": 2}, exit_code=2, write="hello.txt")
    res = run(fake_cox, task())
    assert not res["pass"]
    assert res["note"] == "cox exit 2"
    assert res["cost_usd"] == 0.0047


def test_a_failed_check_reports_its_output(fake_cox):
    fake_cox.respond(PAYLOAD)
    res = run(fake_cox, task(check="echo wrong content >&2; false"))
    assert not res["pass"]
    assert res["note"] == "check failed: wrong content"


def test_a_failed_setup_never_runs_cox(fake_cox):
    res = run(fake_cox, task(setup="false"))
    assert not res["pass"] and res["note"] == "setup failed"
    assert res["cost_usd"] == 0.0


def test_unparseable_output_is_a_zero_cost_row_not_a_crash(fake_cox, monkeypatch):
    monkeypatch.setenv("FAKE_COX_PAYLOAD", "not json")
    res = run(fake_cox, task())
    assert res["cost_usd"] == 0.0 and res["tokens"] == {}


def test_default_runs_are_hermetic_and_pass_provider_and_model(fake_cox):
    fake_cox.respond(PAYLOAD)
    run(fake_cox, task())
    argv = fake_cox.argv()
    assert {"--no-hooks", "--no-mcp"} <= set(argv)
    assert argv[argv.index("--provider") + 1] == "anthropic"
    assert argv[argv.index("--tier") + 1] == "code=claude-sonnet-5"
    assert argv[argv.index("--approve") + 1] == "never"


def test_verify_preset_turns_hooks_back_on(fake_cox):
    fake_cox.respond(PAYLOAD)
    run(fake_cox, task(), preset="verify")
    argv = fake_cox.argv()
    assert "--no-hooks" not in argv and "--no-mcp" in argv


def test_main_sums_cost_and_tokens_and_exits_nonzero_on_a_failure(fake_cox, capsys):
    fake_cox.respond({**PAYLOAD, "exit_code": 2}, exit_code=2)
    code = harness.main(["--cox-bin", fake_cox.path, "--only", "create-file", "find-word"])
    out = capsys.readouterr().out
    assert code == 1
    assert "0/2 passed  total cost $0.0094" in out
    assert "tokens in/out/cache-read/cache-write 8/160/32928/440" in out


def test_token_line_orders_the_four_ledger_counts():
    assert harness.token_line(PAYLOAD["usage"]) == "4/80/16464/220"
    assert harness.token_line({}) == "0/0/0/0"


def test_cox_bin_prefers_the_flag_then_the_env(monkeypatch):
    monkeypatch.setenv("COX_BIN", "/from/env")
    assert harness.find_cox_bin("/from/flag") == "/from/flag"
    assert harness.find_cox_bin(None) == "/from/env"


def test_dry_run_passes_end_to_end_against_the_real_binary(real_cox):
    t = dict(harness.load_tasks(["create-file"])[0][1])
    res = harness.run_task(t, cox_bin=real_cox, dry_run=True, provider=None, model=None)
    assert res["pass"], res["note"]
    assert res["tokens"]["input_tokens"] > 0


# --- E2: `--preset jev-route` (T33.40.10) ---

CSV_HEAD = "turn,job,tier,model,effort,input,output,cache_read,cache_write,cost_usd,latency_ms\n"
BASE_LEDGER = CSV_HEAD + (
    "1,main,code,claude-sonnet-5,high,10,50,1000,200,0.0400,900\n"
    "2,main,code,claude-sonnet-5,high,10,50,1200,0,0.0600,800\n"
)
JEV_LEDGER = CSV_HEAD + (
    "0,plugin:jev,cheap,jev-1.13.0,low,1500,0,0,0,0.0001,120\n"
    "1,main,cheap,claude-haiku-4-5,low,10,50,0,900,0.0300,500\n"
    "2,main,code,claude-sonnet-5,high,10,50,0,300,0.0199,700\n"
)


@pytest.fixture
def keys(monkeypatch):
    monkeypatch.setenv("ANTHROPIC_API_KEY", "sk-test")
    monkeypatch.setenv("TYPESAFE_API_KEY", "ts-test")


@pytest.fixture
def plugin_dir(tmp_path):
    pkg = tmp_path / "jev"
    pkg.mkdir()
    (pkg / "plugin.toml").write_text('id = "jev"\n')
    return pkg


def test_jev_arm_routes_through_the_plugin_and_installs_it(fake_cox, tmp_path, plugin_dir):
    home = tmp_path / "home"
    home.mkdir()
    ctx = {"cox_bin": fake_cox.path, "env": {**os.environ, "COX_HOME": str(home)},
           "plugin_dir": plugin_dir}
    assert harness.prepare_jev_route(home, "jev", ctx) is None
    assert tomllib.loads((home / "config.toml").read_text()) == {
        "plugins": {"decide": {"route": "jev"}}}
    assert fake_cox.plugin_calls() == [f"plugin install {plugin_dir} --yes"]


def test_baseline_arm_keeps_the_static_pick(fake_cox, tmp_path, plugin_dir):
    ctx = {"cox_bin": fake_cox.path, "env": dict(os.environ), "plugin_dir": plugin_dir}
    assert harness.prepare_jev_route(tmp_path, "baseline", ctx) is None
    assert not (tmp_path / "config.toml").exists()
    assert fake_cox.plugin_calls() == []


def test_a_failed_plugin_install_fails_the_run_before_cox_runs(fake_cox, monkeypatch, plugin_dir):
    monkeypatch.setenv("FAKE_COX_PLUGIN_EXIT", "1")
    res = run(fake_cox, task(), preset="jev-route", arm="jev", plugin_dir=plugin_dir)
    assert not res["pass"] and res["note"].startswith("plugin install failed")
    assert not fake_cox._args_file.exists()


def test_jev_route_runs_pass_the_budget_and_never_read_the_keychain(fake_cox, monkeypatch):
    seen = {}
    real_run = harness.subprocess.run

    def spy(cmd, **kwargs):
        if cmd[1:2] == ["run"]:
            seen.update(kwargs["env"])
        return real_run(cmd, **kwargs)

    monkeypatch.setattr(harness.subprocess, "run", spy)
    fake_cox.respond(PAYLOAD)
    run(fake_cox, task(), preset="jev-route", arm="baseline", budget=0.25)
    argv = fake_cox.argv()
    assert argv[argv.index("--budget") + 1] == "0.25"
    assert "--no-hooks" in argv
    assert seen["COX_KEYRING"] == "off"


def test_the_ledger_is_the_cost_and_splits_it_by_tier_and_job(fake_cox):
    fake_cox.respond(PAYLOAD)
    fake_cox.ledger(BASE_LEDGER, JEV_LEDGER)
    res = run(fake_cox, task(), preset="jev-route", arm="baseline")
    assert res["cost_usd"] == pytest.approx(0.10)
    assert res["ledger"]["by_tier_job"] == {"code/main": pytest.approx(0.10)}
    assert res["ledger"]["main_calls"] == {"code": 2}
    assert (res["ledger"]["cache_read"], res["ledger"]["cache_write"]) == (2200, 200)


def test_plugin_rows_count_toward_cost_and_give_the_latency(fake_cox):
    fake_cox.ledger(JEV_LEDGER)
    led = harness.read_ledger(fake_cox.path, dict(os.environ), "s")
    assert led["cost_usd"] == pytest.approx(0.05)
    assert led["by_tier_job"]["cheap/plugin:jev"] == pytest.approx(0.0001)
    assert led["main_calls"] == {"cheap": 1, "code": 1}
    assert led["plugin_latency_ms"] == [120]
    assert led["models"] == ["jev-1.13.0", "claude-haiku-4-5", "claude-sonnet-5"]


def test_no_session_or_no_csv_is_an_empty_ledger(fake_cox):
    assert harness.read_ledger(fake_cox.path, dict(os.environ), None)["cost_usd"] == 0.0
    fake_cox.ledger("No usage records found for this session\n")
    assert harness.read_ledger(fake_cox.path, dict(os.environ), "s")["by_tier_job"] == {}


def test_jev_route_runs_every_task_in_both_arms_and_prints_the_paired_table(
        fake_cox, keys, plugin_dir, capsys):
    fake_cox.respond(PAYLOAD, write="hello.txt")
    fake_cox.ledger(BASE_LEDGER, JEV_LEDGER)
    code = harness.main(["--cox-bin", fake_cox.path, "--preset", "jev-route",
                         "--plugin-dir", str(plugin_dir), "--only", "create-file"])
    out = capsys.readouterr().out
    assert code == 0
    assert "create-file [baseline]" in out and "create-file [jev]" in out
    assert "| create-file | PASS $0.1000 | PASS $0.0500 | 50% | 1/2 |" in out
    assert "median saving: 50%" in out
    assert "falsifier: not triggered" in out


def test_the_total_cap_stops_the_run_from_the_ledger(fake_cox, keys, plugin_dir, capsys):
    fake_cox.respond(PAYLOAD)
    fake_cox.ledger(BASE_LEDGER, JEV_LEDGER)
    code = harness.main(["--cox-bin", fake_cox.path, "--preset", "jev-route",
                         "--plugin-dir", str(plugin_dir), "--max-usd", "0.12",
                         "--only", "create-file", "find-word"])
    out = capsys.readouterr().out
    assert code == 1
    # $0.10 + $0.05 crosses $0.12, so find-word never starts.
    assert "find-word" not in out
    assert "stopped: spent $0.1500 of the $0.12 cap" in out


def test_a_live_jev_route_run_needs_both_keys_and_the_plugin(fake_cox, monkeypatch, plugin_dir):
    monkeypatch.delenv("TYPESAFE_API_KEY", raising=False)
    monkeypatch.setenv("ANTHROPIC_API_KEY", "sk-test")
    with pytest.raises(SystemExit, match="needs TYPESAFE_API_KEY"):
        harness.main(["--cox-bin", fake_cox.path, "--preset", "jev-route",
                      "--plugin-dir", str(plugin_dir)])
    monkeypatch.setenv("TYPESAFE_API_KEY", "ts-test")
    with pytest.raises(SystemExit, match="needs --plugin-dir"):
        harness.main(["--cox-bin", fake_cox.path, "--preset", "jev-route"])


def test_a_jev_route_dry_run_runs_the_baseline_only_with_no_key(fake_cox, monkeypatch, capsys):
    monkeypatch.delenv("ANTHROPIC_API_KEY", raising=False)
    monkeypatch.delenv("TYPESAFE_API_KEY", raising=False)
    fake_cox.respond(PAYLOAD, write="hello.txt")
    code = harness.main(["--cox-bin", fake_cox.path, "--preset", "jev-route", "--dry-run",
                         "--only", "create-file"])
    out = capsys.readouterr().out
    assert code == 0
    assert "[jev]" not in out and "median saving" not in out
    assert fake_cox.plugin_calls() == []


def row(name, arm, ok, cost, ledger=None):
    return {"name": name, "arm": arm, "pass": ok, "cost_usd": cost, "ledger": ledger}


def test_a_task_that_fails_only_with_the_plugin_triggers_the_falsifier():
    report = harness.paired_report([
        row("a", "baseline", True, 0.10), row("a", "jev", False, 0.05),
        row("b", "baseline", True, 0.10), row("b", "jev", True, 0.05),
    ], ("baseline", "jev"))
    assert "median saving: 50%" in report
    assert "falsifier: fails only with jev: a" in report


def test_a_median_saving_under_15_percent_triggers_the_falsifier():
    report = harness.paired_report([
        row("a", "baseline", True, 0.10), row("a", "jev", True, 0.095),
        row("b", "baseline", True, 0.10), row("b", "jev", True, 0.12),
        row("c", "baseline", False, 0.10), row("c", "jev", True, 0.05),
    ], ("baseline", "jev"))
    assert "| b | PASS $0.1000 | PASS $0.1200 | -20% | 0/0 |" in report
    assert "median saving: 5%" in report
    assert "falsifier: median saving below 15%" in report


def test_the_arm_totals_sum_tiers_jobs_downgrades_and_plugin_latency():
    jev = {"by_tier_job": {"cheap/main": 0.03, "cheap/plugin:jev": 0.0001},
           "main_calls": {"cheap": 1, "code": 1}, "cache_read": 5, "cache_write": 7,
           "plugin_latency_ms": [100, 300, 200], "models": ["jev-1.13.0"]}
    report = harness.paired_report(
        [row("a", "baseline", True, 0.1), row("a", "jev", True, 0.03, jev)], ("baseline", "jev"))
    assert ("jev: 1/1 passed  $0.0300  by tier/job cheap/main $0.0300, cheap/plugin:jev $0.0001"
            "  downgrades 1/2  cache read/write 5/7  plugin p95 300ms  models jev-1.13.0") in report
    assert "baseline: 1/1 passed  $0.1000  by tier/job -  downgrades 0/0" in report


def test_p95_is_nearest_rank():
    assert harness.p95([]) is None
    assert harness.p95([5]) == 5
    assert harness.p95(list(range(1, 101))) == 95
    assert harness.p95(list(range(1, 21))) == 19
