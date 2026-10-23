"""Tests for `cox_vendor.jev_fixtures`: no network and no key — `fetch` is
a stub that answers like System One, the key is a fake set through
monkeypatch, and FIXTURE_DIR points at a tmp copy of `requests.json`."""

import datetime
import json
import re
import shutil

import pytest

from cox_vendor import cli
from cox_vendor import jev_fixtures as jf

# Captured at import, before the autouse fixture points FIXTURE_DIR at tmp_path.
REAL_FIXTURE_DIR = jf.FIXTURE_DIR
KEY = "ts-live-0123456789abcdef"
DAY = datetime.date(2026, 10, 21)


def answer(qid: str) -> dict:
    if qid.endswith(".severity"):
        return {"type": "score", "score": 1.0, "confidence": 0.9,
                "legend": {"0": "only reads"}, "probabilities": {"0": 0.1}}
    return {"type": "noul", "noul": 0.1}


class Typesafe:
    """Answers every question asked; records what it was sent."""

    def __init__(self, extra=None):
        self.sent = []
        self.extra = extra or {}

    def __call__(self, url, data, head):
        self.sent.append((url, data, head))
        body = json.loads(data)
        return json.dumps({"model": "jev-1.13.0",
                           "answers": {q: answer(q) for q in body["questions"]},
                           "usage": {"input_tokens": 400, "output_tokens": 0},
                           **self.extra}).encode()


@pytest.fixture(autouse=True)
def isolated_dir(tmp_path, monkeypatch):
    target = tmp_path / "jev"
    target.mkdir()
    shutil.copy(REAL_FIXTURE_DIR / jf.REQUESTS, target / jf.REQUESTS)
    monkeypatch.setattr(jf, "FIXTURE_DIR", target)
    monkeypatch.delenv(jf.KEY_ENV, raising=False)
    return target


@pytest.fixture
def key(monkeypatch):
    monkeypatch.setenv(jf.KEY_ENV, KEY)


def recorded(directory):
    return sorted(p.name for p in directory.iterdir() if p.name != jf.REQUESTS)


def test_no_key_is_a_clear_exit_with_nothing_sent(isolated_dir, capsys):
    server = Typesafe()
    with pytest.raises(jf.MissingKey, match=jf.KEY_ENV):
        jf.run(fetch=server)
    assert server.sent == []
    assert recorded(isolated_dir) == []
    assert cli.main(["jev-fixtures"]) == 1
    assert jf.KEY_ENV in capsys.readouterr().err


def test_each_body_is_sent_verbatim_with_the_bearer_key(key):
    server = Typesafe()
    jf.run(fetch=server, today=DAY)
    requests = jf.load_requests()
    assert len(server.sent) == len(requests) >= 4
    for (url, data, head), body in zip(server.sent, requests.values()):
        assert url == "https://api.typesafe.ai/v1/systemone"
        assert json.loads(data) == body
        assert head == {"authorization": f"Bearer {KEY}", "content-type": "application/json"}


def test_a_recording_holds_body_reply_model_and_date(isolated_dir, key):
    assert jf.run(fetch=Typesafe(), today=DAY)
    names = recorded(isolated_dir)
    assert "risk-git-push.json" in names
    doc = json.loads((isolated_dir / "risk-git-push.json").read_text())
    assert doc["model"] == "jev-1.13.0"
    assert doc["recorded"] == "2026-10-21"
    assert doc["request"] == jf.load_requests()["risk-git-push"]
    assert set(doc["response"]["answers"]) == set(doc["request"]["questions"])


def test_secrets_and_the_key_are_redacted_before_writing(isolated_dir, key):
    echo = {"echo": f"Authorization: Bearer {KEY}", "note": f"key {KEY} and sk-abcdefghijk"}
    jf.run(fetch=Typesafe(extra=echo), today=DAY)
    for name in recorded(isolated_dir):
        text = (isolated_dir / name).read_text()
        assert KEY not in text and "sk-abcdefghijk" not in text
        doc = json.loads(text)
        assert doc["response"]["echo"] == "Authorization: «redacted»"
        assert doc["response"]["note"] == "key «redacted» and «redacted»"


def test_scrub_matches_the_rollout_scrubbers_table():
    pem = "-----BEGIN RSA PRIVATE KEY-----\nMIIEow\n-----END RSA PRIVATE KEY-----"
    cases = [
        ("sk-abc12345678", "«redacted»"),
        ("Bearer tokensecret", "«redacted»"),
        ("AKIA0123456789ABCDEF", "«redacted»"),
        ("ghp_0123456789abcdef0123", "«redacted»"),
        (pem, "«redacted»"),
        (pem + "\nafter", "«redacted»\nafter"),
        ("-----BEGIN KEY-----\nno tail", "«redacted»"),
        ("key=sk-abcdefghijk Authorization: Bearer tokensecret",
         "key=«redacted» Authorization: «redacted»"),
        ("café sk-abcdefghijk 日本語", "café «redacted» 日本語"),
        ("sk-shrt", "sk-shrt"),
        ("AKIA0123", "AKIA0123"),
        ("plain text 1234", "plain text 1234"),
        ("x-api-key: sk-ant-api03-R2D2_c3po-XyZ0123456789-abcDEF_ghiAA end",
         "x-api-key: «redacted» end"),
    ]
    for text, want in cases:
        assert jf.scrub(text) == want, text


def test_scrub_knows_every_shape_redact_rs_does():
    source = (jf.REPO_ROOT / "crates" / "cox-sanitize" / "src" / "redact.rs").read_text()
    assert f'pub const REDACTED: &str = "{jf.REDACTED}";' in source
    markers = re.search(r"\[(\"sk-\".*?)\]\s*\.iter\(\)", source, re.S)
    assert markers, "the marker list in scrub() moved"
    assert re.findall(r'"([^"]+)"', markers.group(1)) == [
        "sk-", "Bearer ", "AKIA", "ghp_", "-----BEGIN "]


@pytest.mark.parametrize("reply", [
    b"not json",
    b'{"answers": {}}',
    b'{"answers": {"0.severity": {"type": "score"}}}',
])
def test_a_reply_that_does_not_answer_the_questions_writes_nothing(isolated_dir, key, reply):
    with pytest.raises(ValueError):
        jf.run(fetch=lambda *_: reply, today=DAY)
    assert recorded(isolated_dir) == []


def test_an_undocumented_answer_type_is_refused(isolated_dir, key):
    def odd(url, data, head):
        qs = json.loads(data)["questions"]
        return json.dumps({"answers": {q: {"type": "guess"} for q in qs}}).encode()

    with pytest.raises(ValueError, match="no documented type"):
        jf.run(fetch=odd, today=DAY)
    assert recorded(isolated_dir) == []


def test_check_needs_no_key_and_finds_missing_and_stale_recordings(isolated_dir, monkeypatch):
    assert jf.run(check=True) is True
    monkeypatch.setenv(jf.KEY_ENV, KEY)
    jf.run(fetch=Typesafe(), today=DAY)
    monkeypatch.delenv(jf.KEY_ENV)
    assert jf.run(check=True) is False
    path = isolated_dir / "risk-git-push.json"
    doc = json.loads(path.read_text())
    doc["request"]["questions"].pop("0.severity")
    path.write_text(jf.dump(doc))
    assert jf.stale(jf.load_requests()) == ["risk-git-push"]


def test_rerun_on_the_same_day_is_a_no_op(key):
    assert jf.run(fetch=Typesafe(), today=DAY) is True
    assert jf.run(fetch=Typesafe(), today=DAY) is False


def test_default_target_is_what_the_plugin_test_writes():
    crate = jf.REPO_ROOT / "crates" / "cox"
    assert REAL_FIXTURE_DIR == crate / "tests" / "fixtures" / "jev"
    plugin = (jf.REPO_ROOT / "plugins" / "jev" / "tests" / "fixtures.rs").read_text()
    assert "crates/cox/tests/fixtures/jev" in plugin and '"requests.json"' in plugin
    assert jf.load_requests()
