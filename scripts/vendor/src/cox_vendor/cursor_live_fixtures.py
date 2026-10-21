"""Records the Cursor CLI fixtures from one real run (T35.10): the lines of
`agent -p --output-format stream-json` and the agent's side of one
`agent acp` session, both against a scratch git repository, in the shape
`cursor_fixtures` (T35.7) builds from the docs. The `fake_agent` e2e
(`crates/cox/tests/it/external_agents_cursor.rs`) replays them through the
same mapper and ACP client, which is how a CLI release that drifted from
the documented shapes shows up. They go to their own `live_*.json` files
because each vendored file has one writer (A48): the documented fixtures
stay reproducible by `cursor-fixtures --check`.

Never in CI, never by default: it refuses unless `COX_LIVE_CURSOR=1` is
set, and it calls Cursor with the creator's own key. The key comes from
`CURSOR_API_KEY` in the environment alone — no keychain, and the CLI runs
with a scratch `HOME`, so no `agent login` or desktop session is used
(A54's official-paths rule). It is redacted from both files with the
session and request ids and the scratch paths; a key that survives
redaction means nothing is written.

    COX_LIVE_CURSOR=1 CURSOR_API_KEY=... uv run --project scripts/vendor \\
        cox-vendor cursor-live-fixtures
"""

from __future__ import annotations

import datetime
import json
import os
import queue
import shutil
import subprocess
import tempfile
import threading
from pathlib import Path

from . import cursor_fixtures as cf

OPT_IN = "COX_LIVE_CURSOR"
KEY_ENV = "CURSOR_API_KEY"
# The plugin's own invocations (`plugins/cursor/plugin.toml`); the host
# appends the prompt to the stream-json one, as here.
STREAM_ARGS = ["-p", "--output-format", "stream-json"]
PROMPT = "Read README.md and reply with its first line only."
README = "# Scratch\n\nA scratch repository for cox's live Cursor check (T35.10).\n"
TIMEOUT = 300  # seconds per CLI run or ACP reply


def run(*, check: bool = False, fetch=None, env=None) -> bool:
    """Record a live run (`fetch(key)`, defaulting to `record`), redact it
    and — unless `check` — write the files whose bytes differ."""
    env = os.environ if env is None else env
    if env.get(OPT_IN) != "1":
        raise ValueError(f"a live Cursor run is opt-in: set {OPT_IN}=1 and {KEY_ENV}")
    key = env.get(KEY_ENV, "").strip()
    if not key:
        raise ValueError(f"{KEY_ENV} is not set; this run takes the key from the environment only")
    return cf.write(build((fetch or record)(key), key), check=check)


def build(recorded: dict, key: str) -> dict[str, str]:
    """File name -> redacted bytes, from `record`'s output."""
    lines = recorded["stream"]
    cf.check_stream(lines)
    acp = recorded["acp"]
    secrets = {key: "<redacted>", str(Path.home()): "/home/user", **recorded["paths"]}
    for event in map(json.loads, lines):
        for field in ("session_id", "request_id"):
            if isinstance(event.get(field), str):
                secrets[event[field]] = f"<{field}>"
    session = acp["responses"]["session/new"].get("sessionId")
    if isinstance(session, str):
        secrets[session] = "<sessionId>"
    source = f"live run of agent {recorded['version']}, {datetime.date.today().isoformat()}"
    files = {
        "live_stream_json.json": cf.dump({"mode": "stream-json", "sources": [source], "lines": lines}),
        "live_acp.json": cf.dump({"mode": "acp", "sources": [source], **acp}),
    }
    # Longest first, so a path is never half-replaced by a prefix of it.
    order = sorted((s for s in secrets if s), key=len, reverse=True)
    for name, text in files.items():
        for secret in order:
            text = text.replace(secret, secrets[secret])
        if key in text:
            raise ValueError("the key survived redaction; nothing written")
        files[name] = text
    return files


def record(key: str) -> dict:
    """One real run of each mode in a fresh scratch repository."""
    agent = shutil.which("agent")
    if agent is None:
        raise ValueError("no `agent` on PATH: install the Cursor CLI (https://cursor.com/docs/cli/installation)")
    with tempfile.TemporaryDirectory(prefix="cox-cursor-live-") as tmp:
        repo, home = Path(tmp).resolve() / "repo", Path(tmp).resolve() / "home"
        home.mkdir()
        repo.mkdir()
        (repo / "README.md").write_text(README)
        subprocess.run(["git", "init", "-q"], cwd=repo, check=True, timeout=60)
        env = {"PATH": os.environ.get("PATH", ""), "HOME": str(home), KEY_ENV: key}
        cli = lambda *args: subprocess.run(  # noqa: E731
            [agent, *args], cwd=repo, env=env, capture_output=True, text=True, timeout=TIMEOUT
        )
        version = cli("--version").stdout.strip()
        out = cli(*STREAM_ARGS, PROMPT)
        if out.returncode != 0:
            raise ValueError(f"agent -p exited {out.returncode}: {out.stderr[-500:].replace(key, '<redacted>')}")
        proc = subprocess.Popen(
            [agent, "acp"], cwd=repo, env=env, text=True,
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
        )
        try:
            acp = acp_session(Pipe(proc), str(repo))
        finally:
            proc.kill()
            proc.wait()
        return {
            "version": version,
            "stream": [line for line in out.stdout.splitlines() if line.strip()],
            "acp": acp,
            "paths": {str(repo): "/scratch/repo", str(home): "/scratch/home"},
        }


class Pipe:
    """Newline-delimited JSON-RPC over a child's stdio, with a read deadline
    so a CLI that goes quiet ends the run instead of hanging it."""

    def __init__(self, proc):
        self._proc = proc
        self._lines: queue.Queue = queue.Queue()
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self):
        for line in self._proc.stdout:
            self._lines.put(line)
        self._lines.put(None)

    def send(self, msg: dict) -> None:
        self._proc.stdin.write(json.dumps(msg) + "\n")
        self._proc.stdin.flush()

    def recv(self) -> dict:
        try:
            line = self._lines.get(timeout=TIMEOUT)
        except queue.Empty:
            raise ValueError("agent acp sent nothing in time") from None
        if line is None:
            raise ValueError("agent acp closed its stdout")
        return json.loads(line)


def acp_session(pipe, cwd: str) -> dict:
    """The client side of one prompt turn: the agent's result for each of
    the three requests cox's client sends, and everything the agent sends
    while the prompt runs, in order — the fixture's `responses` and
    `prompt_turn`."""
    responses: dict = {}
    turn: list = []
    capabilities = {"fs": {"readTextFile": False, "writeTextFile": False}, "terminal": False}
    requests = [
        ("initialize", lambda: {"protocolVersion": 1, "clientCapabilities": capabilities}),
        ("session/new", lambda: {"cwd": cwd, "mcpServers": []}),
        ("session/prompt", lambda: {
            "sessionId": responses["session/new"]["sessionId"],
            "prompt": [{"type": "text", "text": PROMPT}],
        }),
    ]
    for id_, (method, params) in enumerate(requests):
        pipe.send({"jsonrpc": "2.0", "id": id_, "method": method, "params": params()})
        while True:
            msg = pipe.recv()
            if "method" not in msg and msg.get("id") == id_:
                if "error" in msg:
                    # json.dumps escapes control and non-ASCII characters, so
                    # the CLI's text cannot drive the terminal.
                    raise ValueError(f"{method} failed: {json.dumps(msg['error'])}")
                responses[method] = msg["result"]
                break
            if method == "session/prompt" and "method" in msg:
                turn.append(msg)
            if "method" in msg and "id" in msg:
                pipe.send(answer(msg))
    return {"responses": responses, "prompt_turn": turn}


def answer(request: dict) -> dict:
    """Refuse every permission request (the recording must not let the agent
    act) the way cox's fail-closed `Ask` does, and every other method, since
    the client offers neither files nor a terminal."""
    reply = {"jsonrpc": "2.0", "id": request["id"]}
    if request["method"] != "session/request_permission":
        return reply | {"error": {"code": -32601, "message": f"{request['method']} is not offered"}}
    options = request.get("params", {}).get("options", [])
    reject = next((o.get("optionId") for o in options if str(o.get("kind", "")).startswith("reject")), None)
    outcome = {"outcome": "selected", "optionId": reject} if reject else {"outcome": "cancelled"}
    return reply | {"result": {"outcome": outcome}}
