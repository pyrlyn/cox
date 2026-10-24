"""Tests for `cox_vendor.cursor_live_fixtures`: no network and no Cursor
CLI — the recorder is a stub, the ACP agent a scripted pipe, and
FIXTURE_DIR points at tmp_path instead of the real fixtures."""

import json
import subprocess
import sys

import pytest

from cox_vendor import cli
from cox_vendor import cursor_fixtures as cf
from cox_vendor import cursor_live_fixtures as live

KEY = "key_live_0123456789abcdef"
SESSION = "c6b62c6f-7ead-4fd6-9922-e952131177ff"
REPO = "/private/var/folders/xy/T/cox-cursor-live-1/repo"
OPT_IN = {live.OPT_IN: "1", live.KEY_ENV: KEY}

STREAM = [
    json.dumps({"type": "system", "subtype": "init", "cwd": REPO, "session_id": SESSION,
                "model": "m", "permissionMode": "default"}),
    json.dumps({"type": "assistant", "session_id": SESSION,
                "message": {"role": "assistant", "content": [{"type": "text", "text": "# Scratch"}]}}),
    json.dumps({"type": "result", "subtype": "success", "is_error": False, "result": "# Scratch",
                "session_id": SESSION, "request_id": "req-42"}),
]


def recorded(stream=STREAM, echo_key=False) -> dict:
    chunk = {"jsonrpc": "2.0", "method": "session/update", "params": {
        "sessionId": "sess_live_1", "update": {"sessionUpdate": "agent_message_chunk",
                                              "content": {"type": "text", "text": KEY if echo_key else "# Scratch"}}}}
    return {
        "version": "2026.10.01-abc",
        "stream": list(stream),
        "acp": {
            "responses": {"initialize": {"protocolVersion": 1}, "session/new": {"sessionId": "sess_live_1"},
                          "session/prompt": {"stopReason": "end_turn"}},
            "prompt_turn": [chunk],
        },
        "paths": {REPO: "/scratch/repo"},
    }


class Calls:
    """A recorder stub that remembers whether it ran."""

    def __init__(self, result=None):
        self.keys = []
        self.result = result or recorded()

    def __call__(self, key):
        self.keys.append(key)
        return self.result


@pytest.fixture(autouse=True)
def isolated_dir(tmp_path, monkeypatch):
    monkeypatch.setattr(cf, "FIXTURE_DIR", tmp_path / "cursor")
    return tmp_path / "cursor"


@pytest.mark.parametrize("flag", [None, "0", "true", "yes"])
def test_without_the_opt_in_no_key_is_read_and_nothing_runs(flag, isolated_dir):
    env = {live.KEY_ENV: KEY} | ({live.OPT_IN: flag} if flag else {})
    stub = Calls()
    with pytest.raises(ValueError, match=live.OPT_IN):
        live.run(fetch=stub, env=env)
    assert stub.keys == []
    assert not isolated_dir.exists()


def test_the_default_environment_refuses_without_the_opt_in(monkeypatch):
    monkeypatch.delenv(live.OPT_IN, raising=False)
    monkeypatch.setenv(live.KEY_ENV, KEY)
    with pytest.raises(ValueError, match=live.OPT_IN):
        live.run(fetch=Calls())


def test_the_opt_in_without_a_key_is_a_clear_exit(monkeypatch, capsys):
    monkeypatch.setenv(live.OPT_IN, "1")
    monkeypatch.delenv(live.KEY_ENV, raising=False)
    assert cli.main(["cursor-live-fixtures"]) == 1
    assert live.KEY_ENV in capsys.readouterr().err


def test_a_recorded_run_is_written_in_the_shape_the_e2e_replays(isolated_dir):
    stub = Calls()
    assert live.run(fetch=stub, env=OPT_IN)
    assert stub.keys == [KEY]
    stream = json.loads((isolated_dir / "live_stream_json.json").read_text())
    assert stream["mode"] == "stream-json"
    assert [json.loads(line)["type"] for line in stream["lines"]] == ["system", "assistant", "result"]
    assert "2026.10.01-abc" in stream["sources"][0]
    acp = json.loads((isolated_dir / "live_acp.json").read_text())
    assert acp["mode"] == "acp"
    assert set(acp["responses"]) == {"initialize", "session/new", "session/prompt"}
    assert acp["prompt_turn"][0]["params"]["update"]["content"]["text"] == "# Scratch"


def test_ids_and_scratch_paths_are_redacted(isolated_dir):
    live.run(fetch=Calls(), env=OPT_IN)
    text = "".join(p.read_text() for p in isolated_dir.iterdir())
    for gone in (SESSION, "req-42", "sess_live_1", REPO):
        assert gone not in text
    first = json.loads(json.loads((isolated_dir / "live_stream_json.json").read_text())["lines"][0])
    assert first["cwd"] == "/scratch/repo"
    assert first["session_id"] == "<session_id>"


def test_the_key_is_redacted_wherever_the_cli_echoes_it(isolated_dir):
    live.run(fetch=Calls(recorded(echo_key=True)), env=OPT_IN)
    acp = (isolated_dir / "live_acp.json").read_text()
    assert KEY not in acp
    assert "<redacted>" in acp


def test_an_undocumented_event_type_writes_nothing(isolated_dir):
    stream = [STREAM[0], json.dumps({"type": "thinking", "subtype": "delta"}), STREAM[2]]
    with pytest.raises(ValueError, match="thinking"):
        live.run(fetch=Calls(recorded(stream=stream)), env=OPT_IN)
    assert not isolated_dir.exists()


def test_check_records_but_writes_nothing(isolated_dir):
    assert live.run(check=True, fetch=Calls(), env=OPT_IN)
    assert not isolated_dir.exists()


class ScriptedAgent:
    """An ACP agent as a pipe: `script[method]` is what it sends, in order,
    on receiving that request; `{"reply": True}` stands for its response."""

    def __init__(self, script):
        self.script, self.sent, self.inbox = script, [], []

    def send(self, msg):
        self.sent.append(msg)
        for out in self.script.get(msg.get("method"), []):
            if out == "reply":
                out = {"jsonrpc": "2.0", "id": msg["id"], "result": {"method": msg["method"]}}
                if msg["method"] == "session/new":
                    out["result"] = {"sessionId": "sess_live_1"}
            self.inbox.append(out)

    def recv(self):
        return self.inbox.pop(0)


def test_acp_session_records_the_prompt_turn_and_refuses_permission():
    update = {"jsonrpc": "2.0", "method": "session/update", "params": {"update": {"sessionUpdate": "plan"}}}
    ask = {"jsonrpc": "2.0", "id": 7, "method": "session/request_permission", "params": {"options": [
        {"optionId": "allow-once", "kind": "allow_once"}, {"optionId": "reject-once", "kind": "reject_once"}]}}
    read = {"jsonrpc": "2.0", "id": 8, "method": "fs/read_text_file", "params": {}}
    agent = ScriptedAgent({
        "initialize": ["reply"],
        # Sent after its reply, so it is read while the prompt runs: the fake
        # can only replay it then.
        "session/new": ["reply", update],
        "session/prompt": [update, ask, read, "reply"],
    })
    got = live.acp_session(agent, "/scratch/repo")
    assert got["responses"]["session/new"] == {"sessionId": "sess_live_1"}
    assert [m.get("method") for m in got["prompt_turn"]] == ["session/update"] * 2 + [
        "session/request_permission", "fs/read_text_file"]
    answers = {m["id"]: m for m in agent.sent if "method" not in m}
    assert answers[7]["result"]["outcome"] == {"outcome": "selected", "optionId": "reject-once"}
    assert answers[8]["error"]["code"] == -32601
    prompt = next(m for m in agent.sent if m.get("method") == "session/prompt")
    assert prompt["params"]["sessionId"] == "sess_live_1"


def test_an_acp_error_ends_the_run():
    agent = ScriptedAgent({"initialize": [{"jsonrpc": "2.0", "id": 0, "error": {"message": "auth\x1b[2J"}}]})
    with pytest.raises(ValueError, match="initialize failed") as err:
        live.acp_session(agent, "/scratch/repo")
    assert "\x1b" not in str(err.value)


def test_a_permission_request_without_a_reject_option_is_cancelled():
    ask = {"id": 1, "method": "session/request_permission", "params": {"options": [{"optionId": "a", "kind": "allow_once"}]}}
    assert live.answer(ask)["result"]["outcome"] == {"outcome": "cancelled"}


def test_pipe_reports_a_cli_that_closes_its_stdout():
    proc = subprocess.Popen([sys.executable, "-c", "print('{\"id\": 0}')"], text=True,
                            stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    pipe = live.Pipe(proc)
    assert pipe.recv() == {"id": 0}
    with pytest.raises(ValueError, match="closed"):
        pipe.recv()
    proc.wait()


def test_live_files_sit_beside_the_documented_ones_the_e2e_reads():
    e2e = (cf.REPO_ROOT / "crates" / "cox" / "tests" / "it" / "external_agents_cursor.rs").read_text()
    for name in ("live_stream_json.json", "live_acp.json"):
        assert f'"{name}"' in e2e
