"""Records live Jev fixtures (T33.40.17): TypeSafe's real System One
replies to the bodies the Jev plugin sends, so the hand-built replies in
`crates/cox/tests/it/plugins_jev.rs` can be checked against a real
release. This module is the only thing that writes
`crates/cox/tests/fixtures/jev/<point>-<name>.json`.

The bodies are not built here: `requests.json` beside them is written by
the plugin's own `decide` (`plugins/jev/tests/fixtures.rs`), so the
question set has one owner. A recorded file holds the body sent, the
reply, the reply's model version and the date, every string redacted with
the rollout scrubber's shapes plus the key itself.

Runs only with the creator's `TYPESAFE_API_KEY` in the environment (it
never reads the keychain) and spends real money, so it never runs in CI:

    TYPESAFE_API_KEY=… uv run --project scripts/vendor cox-vendor jev-fixtures
    uv run --project scripts/vendor cox-vendor jev-fixtures --check

`--check` needs no key and sends nothing: it reports whether a recorded
file is missing or was recorded for a body the plugin no longer sends.
"""

from __future__ import annotations

import datetime
import json
import os
import re
import urllib.request
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[4]
FIXTURE_DIR = REPO_ROOT / "crates" / "cox" / "tests" / "fixtures" / "jev"
REQUESTS = "requests.json"

# `plugin.toml`'s `[[provider]] base_url` and `api_key_env`.
ENDPOINT = "https://api.typesafe.ai/v1/systemone"
KEY_ENV = "TYPESAFE_API_KEY"

# `cox_sanitize::redact` (the rollout scrubber): its marker and the five
# secret shapes `scrub` replaces. `test_jev_fixtures` checks both against
# redact.rs, so a new shape there fails here instead of leaking.
REDACTED = "«redacted»"
_SECRET = re.compile(
    r"Bearer \S*"
    r"|sk-[A-Za-z0-9_-]{8,}"
    r"|AKIA[A-Za-z0-9]{16,}"
    r"|ghp_[A-Za-z0-9]{8,}"
    r"|-----BEGIN (?:.*?\n-----END [^\n]*|.*\Z)",
    re.S,
)

ANSWER_TYPES = {"choice", "score", "noul"}


class MissingKey(ValueError):
    """No key in the environment: nothing is sent."""


def scrub(text: str, key: str = "") -> str:
    if key:
        text = text.replace(key, REDACTED)
    return _SECRET.sub(REDACTED, text)


def redact(value, key: str = ""):
    """Every string in a JSON value, keys included, scrubbed in place of
    the serialized text, so a redaction can never break the JSON."""
    if isinstance(value, str):
        return scrub(value, key)
    if isinstance(value, list):
        return [redact(v, key) for v in value]
    if isinstance(value, dict):
        return {scrub(k, key): redact(v, key) for k, v in value.items()}
    return value


def body_bytes(body: dict) -> bytes:
    return json.dumps(body, ensure_ascii=False, separators=(",", ":")).encode()


def headers(key: str) -> dict[str, str]:
    # The same two headers the host and `provider::stream` send.
    return {"authorization": f"Bearer {key}", "content-type": "application/json"}


def post(url: str, data: bytes, head: dict[str, str]) -> bytes:
    req = urllib.request.Request(url, data=data, headers=head, method="POST")
    with urllib.request.urlopen(req, timeout=60) as resp:  # noqa: S310 - fixed https URL
        return resp.read()


def load_requests() -> dict[str, dict]:
    """`<point>-<name>` -> the body the plugin sends for that state."""
    doc = json.loads((FIXTURE_DIR / REQUESTS).read_text())
    out = {}
    for point, states in doc.items():
        for name, body in states.items():
            if not isinstance(body, dict) or not isinstance(body.get("questions"), dict):
                raise ValueError(f"{REQUESTS}: {point}/{name} is not a System One body")
            out[f"{point}-{name}"] = body
    if not out:
        raise ValueError(f"{REQUESTS} holds no bodies")
    return out


def checked_reply(name: str, body: dict, raw: bytes) -> dict:
    """The parsed reply, refused unless it answers exactly the questions
    asked with a documented answer type (J2)."""
    try:
        reply = json.loads(raw)
    except json.JSONDecodeError as exc:
        raise ValueError(f"{name}: the reply is not JSON") from exc
    answers = reply.get("answers") if isinstance(reply, dict) else None
    if not isinstance(answers, dict) or set(answers) != set(body["questions"]):
        raise ValueError(f"{name}: the reply does not answer exactly the questions asked")
    for qid, answer in answers.items():
        if not isinstance(answer, dict) or answer.get("type") not in ANSWER_TYPES:
            raise ValueError(f"{name}: answer {qid!r} has no documented type")
    return reply


def dump(doc: dict) -> str:
    return json.dumps(doc, indent=2, ensure_ascii=False) + "\n"


def stale(requests: dict[str, dict]) -> list[str]:
    """Names with no recorded file, or one recorded for another body."""
    out = []
    for name, body in requests.items():
        path = FIXTURE_DIR / f"{name}.json"
        if not path.exists() or json.loads(path.read_text()).get("request") != body:
            out.append(name)
    return out


def run(*, check: bool = False, fetch=None, today: datetime.date | None = None) -> bool:
    """Send every body, check and redact every reply, then — only when all
    of them passed — write one file per state. With `check`, send nothing
    and report whether any recorded file is missing or stale."""
    requests = load_requests()
    if check:
        return bool(stale(requests))
    key = os.environ.get(KEY_ENV, "")
    if not key:
        raise MissingKey(
            f"{KEY_ENV} is not set; recording sends paid requests with the creator's "
            "own key, which this script reads from the environment only"
        )
    fetch = fetch or post
    date = (today or datetime.datetime.now(datetime.UTC).date()).isoformat()
    files = {}
    for name, body in requests.items():
        reply = checked_reply(name, body, fetch(ENDPOINT, body_bytes(body), headers(key)))
        files[f"{name}.json"] = dump(redact({
            "endpoint": ENDPOINT,
            "model": reply.get("model", body.get("model")),
            "recorded": date,
            "request": body,
            "response": reply,
        }, key))
    changed = False
    for fname, text in files.items():
        path = FIXTURE_DIR / fname
        if path.exists() and path.read_text() == text:
            continue
        changed = True
        path.write_text(text)
    return changed
