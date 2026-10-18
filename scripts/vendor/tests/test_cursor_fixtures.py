"""Tests for `cox_vendor.cursor_fixtures`: no network — every page is a
small Markdown stand-in built here, served by a `fetch` stub, and
FIXTURE_DIR points at tmp_path instead of the real fixtures."""

import json

import pytest

from cox_vendor import cursor_fixtures as cf

# Captured at import, before the autouse fixture points FIXTURE_DIR at tmp_path.
REAL_FIXTURE_DIR = cf.FIXTURE_DIR

STREAM = [
    '{"type":"system","subtype":"init","model":"m","permissionMode":"default"}',
    '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"hi"}]}}',
    '{"type":"result","subtype":"success","is_error":false,"result":"hi"}',
]


def fence(*objs) -> str:
    return "\n".join(f"```json\n{json.dumps(o, indent=2)}\n```\n" for o in objs)


def pages(stream=STREAM, option_ids=("allow-once", "reject-once")) -> dict[str, str]:
    chunk = {"jsonrpc": "2.0", "method": "session/update", "params": {
        "sessionId": "s", "update": {"sessionUpdate": "agent_message_chunk",
                                     "content": {"type": "text", "text": "hello"}}}}
    options = [{"optionId": o, "name": o, "kind": o.replace("-", "_")} for o in option_ids]
    permission = {"jsonrpc": "2.0", "id": 5, "method": "session/request_permission",
                  "params": {"sessionId": "s", "toolCall": {"toolCallId": "c"}, "options": options}}
    return {
        cf.OUTPUT_FORMAT_URL: "# Output format\n\n### Example sequence\n\n```json\n"
        + "\n".join(stream) + "\n```\n\n## Text format\n",
        cf.CURSOR_ACP_URL: "# ACP\n\n### Permissions\n\n- `allow-once`\n- `allow-always`\n"
        "- `reject-once`\n\n## MCP servers\n",
        cf.ACP_INIT_URL: fence({"id": 0, "method": "initialize", "params": {}},
                               {"id": 0, "result": {"protocolVersion": 1, "agentCapabilities": {}}}),
        cf.ACP_SESSION_URL: fence({"id": 1, "result": {"sessionId": "s"}}),
        cf.ACP_PROMPT_URL: fence(chunk, {"id": 2, "result": {"stopReason": "end_turn"}}),
        cf.ACP_TOOLS_URL: fence(permission),
    }


def fetching(served):
    return lambda url: served[url]


@pytest.fixture(autouse=True)
def isolated_dir(tmp_path, monkeypatch):
    monkeypatch.setattr(cf, "FIXTURE_DIR", tmp_path / "cursor")
    return tmp_path / "cursor"


def test_stream_json_fixture_keeps_the_documented_lines_verbatim(isolated_dir):
    assert cf.run(fetch=fetching(pages()))
    doc = json.loads((isolated_dir / "stream_json.json").read_text())
    assert doc["mode"] == "stream-json"
    assert doc["lines"] == STREAM


def test_acp_fixture_holds_the_documented_messages_in_turn_order(isolated_dir):
    cf.run(fetch=fetching(pages()))
    doc = json.loads((isolated_dir / "acp.json").read_text())
    assert set(doc["responses"]) == {"initialize", "session/new", "session/prompt"}
    assert doc["responses"]["session/new"] == {"sessionId": "s"}
    assert doc["responses"]["session/prompt"] == {"stopReason": "end_turn"}
    methods = [m["method"] for m in doc["prompt_turn"]]
    assert methods == ["session/update", "session/request_permission"]


def test_a_permission_option_cursor_does_not_document_is_rejected(isolated_dir):
    with pytest.raises(ValueError, match="allow-forever"):
        cf.run(fetch=fetching(pages(option_ids=("allow-forever", "reject-once"))))
    assert not isolated_dir.exists()


def test_an_example_without_a_final_result_is_rejected(isolated_dir):
    with pytest.raises(ValueError, match="result"):
        cf.run(fetch=fetching(pages(stream=STREAM[:2])))
    assert not isolated_dir.exists()


def test_an_undocumented_event_type_is_rejected():
    stream = [STREAM[0], '{"type":"thinking"}', STREAM[2]]
    with pytest.raises(ValueError, match="thinking"):
        cf.run(fetch=fetching(pages(stream=stream)))


def test_rerun_is_a_no_op_and_check_writes_nothing(isolated_dir):
    assert cf.run(check=True, fetch=fetching(pages()))
    assert not isolated_dir.exists()
    cf.run(fetch=fetching(pages()))
    assert cf.run(fetch=fetching(pages())) is False
    assert cf.run(check=True, fetch=fetching(pages())) is False


def test_default_target_is_what_the_e2e_reads():
    crate = cf.REPO_ROOT / "crates" / "cox"
    assert REAL_FIXTURE_DIR == crate / "tests" / "fixtures" / "cursor"
    e2e = (crate / "tests" / "it" / "external_agents_cursor.rs").read_text()
    for name in ("stream_json.json", "acp.json"):
        assert (REAL_FIXTURE_DIR / name).is_file()
        assert f'"{name}"' in e2e
