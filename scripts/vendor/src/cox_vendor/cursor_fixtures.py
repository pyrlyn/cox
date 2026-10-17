"""Vendors the Cursor CLI fixtures (T35.7): the documented `stream-json`
and ACP messages the test-only `fake_agent` binary replays in
`crates/cox/tests/it/external_agents_cursor.rs`. This module is the only thing
that writes `crates/cox/tests/fixtures/cursor/*.json` — no hand-pasted
lines, and never a live Cursor call: every message is lifted verbatim from
a documentation page (research.md §4.3.8 cites the same pages).

Sources, each page's Markdown rendering (both sites serve `<page>.md`):

- stream-json: the "Example sequence" NDJSON block of
  https://cursor.com/docs/cli/reference/output-format, kept as the exact
  documented lines so the e2e can prove they reach cox unchanged.
- ACP: Cursor documents only the flow and the permission option ids
  (https://cursor.com/docs/cli/acp) and defers the message shapes to the
  Agent Client Protocol spec, so the agent-side messages come from the
  spec's examples (initialization, session-setup, prompt-turn, tool-calls
  pages); the permission request's option ids are checked against Cursor's
  list.

To re-vendor: `uv run --project scripts/vendor cox-vendor cursor-fixtures`
(`--check` reports a diff without writing).
"""

from __future__ import annotations

import json
import re
import urllib.request
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[4]
FIXTURE_DIR = REPO_ROOT / "crates" / "cox" / "tests" / "fixtures" / "cursor"

OUTPUT_FORMAT_URL = "https://cursor.com/docs/cli/reference/output-format.md"
CURSOR_ACP_URL = "https://cursor.com/docs/cli/acp.md"
ACP_SPEC = "https://agentclientprotocol.com/protocol/"
ACP_INIT_URL = ACP_SPEC + "initialization.md"
ACP_SESSION_URL = ACP_SPEC + "session-setup.md"
ACP_PROMPT_URL = ACP_SPEC + "prompt-turn.md"
ACP_TOOLS_URL = ACP_SPEC + "tool-calls.md"

# The event types output-format.md documents; anything else in its example
# means the page changed and the mapper (T35.4) needs a look first.
STREAM_JSON_TYPES = {"system", "user", "assistant", "tool_call", "result"}

_FENCE = re.compile(r"```json[^\n]*\n(.*?)```", re.S)


def download(url: str) -> str:
    with urllib.request.urlopen(url, timeout=30) as resp:  # noqa: S310 - fixed https URLs
        return resp.read().decode()


def json_blocks(markdown: str) -> list[dict]:
    """Every object a ```json fence holds: one pretty-printed object, or one
    per line of an NDJSON block."""
    out = []
    for block in _FENCE.findall(markdown):
        try:
            out.append(json.loads(block))
        except json.JSONDecodeError:
            out.extend(json.loads(line) for line in block.splitlines() if line.strip())
    return [o for o in out if isinstance(o, dict)]


def section(markdown: str, heading: str) -> str:
    """The text from `heading` to the next heading of any level."""
    start = markdown.find(f"\n{heading}\n")
    if start == -1:
        raise ValueError(f"no {heading!r} section")
    rest = markdown[start + len(heading) + 2 :]
    end = re.search(r"\n#{1,6} ", rest)
    return rest[: end.start()] if end else rest


def stream_json_lines(markdown: str) -> list[str]:
    """The documented example run, line for line."""
    fences = _FENCE.findall(section(markdown, "### Example sequence"))
    if not fences:
        raise ValueError("the example sequence has no json block")
    lines = [line for line in fences[0].splitlines() if line.strip()]
    events = [json.loads(line) for line in lines]
    for event in events:
        if event.get("type") not in STREAM_JSON_TYPES:
            raise ValueError(f"undocumented stream-json type: {event.get('type')!r}")
    if events[0].get("subtype") != "init":
        raise ValueError("the example does not start with system/init")
    if events[-1].get("type") != "result" or events[-1].get("is_error") is not False:
        raise ValueError("the example does not end with a successful result")
    return lines


def cursor_permission_ids(markdown: str) -> set[str]:
    """The option ids Cursor's ACP page says a client returns."""
    ids = set(re.findall(r"^- `([a-z-]+)`", section(markdown, "### Permissions"), re.M))
    if not ids:
        raise ValueError("Cursor's ACP page lists no permission options")
    return ids


def first(blocks: list[dict], what: str, pred) -> dict:
    for block in blocks:
        if pred(block):
            return block
    raise ValueError(f"no documented {what}")


def acp_fixture(pages: dict[str, str]) -> dict:
    init = json_blocks(pages[ACP_INIT_URL])
    session = json_blocks(pages[ACP_SESSION_URL])
    prompt = json_blocks(pages[ACP_PROMPT_URL])
    tools = json_blocks(pages[ACP_TOOLS_URL])
    result = lambda key: lambda b: key in b.get("result", {})  # noqa: E731
    update = lambda b: b.get("params", {}).get("update", {}).get("sessionUpdate")  # noqa: E731
    permission = first(tools, "permission request", lambda b: b.get("method") == "session/request_permission")
    offered = {o["optionId"] for o in permission["params"]["options"]}
    unknown = offered - cursor_permission_ids(pages[CURSOR_ACP_URL])
    if unknown:
        raise ValueError(f"option ids Cursor does not document: {sorted(unknown)}")
    return {
        "mode": "acp",
        "sources": [CURSOR_ACP_URL, ACP_INIT_URL, ACP_SESSION_URL, ACP_PROMPT_URL, ACP_TOOLS_URL],
        # The agent's answer to each client request, by method.
        "responses": {
            "initialize": first(init, "initialize response", result("agentCapabilities"))["result"],
            "session/new": first(session, "session/new response", result("sessionId"))["result"],
            "session/prompt": first(prompt, "prompt response", result("stopReason"))["result"],
        },
        # Sent in order while `session/prompt` runs; a request waits for its answer.
        "prompt_turn": [
            first(prompt, "agent message chunk", lambda b: update(b) == "agent_message_chunk"),
            permission,
        ],
    }


def build(fetch) -> dict[str, str]:
    """File name -> the bytes it should hold."""
    urls = [OUTPUT_FORMAT_URL, CURSOR_ACP_URL, ACP_INIT_URL, ACP_SESSION_URL, ACP_PROMPT_URL, ACP_TOOLS_URL]
    pages = {url: fetch(url) for url in urls}
    stream = {
        "mode": "stream-json",
        "sources": [OUTPUT_FORMAT_URL],
        "lines": stream_json_lines(pages[OUTPUT_FORMAT_URL]),
    }
    dump = lambda doc: json.dumps(doc, indent=2, ensure_ascii=False) + "\n"  # noqa: E731
    return {"stream_json.json": dump(stream), "acp.json": dump(acp_fixture(pages))}


def run(*, check: bool = False, fetch=None) -> bool:
    """Fetch the pages, rebuild both fixtures and — unless `check` — write
    the ones whose bytes differ. Returns whether any differs."""
    changed = False
    for name, text in build(fetch or download).items():
        path = FIXTURE_DIR / name
        if path.exists() and path.read_text() == text:
            continue
        changed = True
        if not check:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
    return changed
