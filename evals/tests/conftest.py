"""Shared fixtures: a fake `cox` for harness logic, the real one for e2e.

The fake is a shell script that prints whatever `cox run` JSON payload a
test sets and records its argv, so accounting, flags and exit handling are
tested with no binary, no network and no key. `cox stats` prints a
test-set ledger CSV (the `jev` arm's own when its COX_HOME routes through
the plugin), and `cox plugin` is logged and exits with a test-set code.
"""

import json
import os
import stat
from pathlib import Path

import pytest

from cox_evals import harness

FAKE_COX = """#!/bin/sh
case "$1" in
  stats)
    if grep -q route "$COX_HOME/config.toml" 2>/dev/null; then
      printf '%s' "$FAKE_COX_LEDGER_JEV"
    else
      printf '%s' "$FAKE_COX_LEDGER"
    fi
    exit 0 ;;
  plugin)
    printf '%s\\n' "$*" >> "$FAKE_COX_ARGS.plugin"
    exit "${FAKE_COX_PLUGIN_EXIT:-0}" ;;
esac
printf '%s\\n' "$@" > "$FAKE_COX_ARGS"
[ -n "$FAKE_COX_WRITE" ] && printf 'hi' > "$FAKE_COX_WRITE"
printf '%s\\n' "$FAKE_COX_PAYLOAD"
exit "${FAKE_COX_EXIT:-0}"
"""


class FakeCox:
    def __init__(self, path, args_file, monkeypatch):
        self.path = str(path)
        self._args_file = args_file
        self._mp = monkeypatch
        self.respond({})

    def respond(self, payload, exit_code=0, write=None):
        """What the next `cox run` prints, exits with, and writes to disk."""
        self._mp.setenv("FAKE_COX_PAYLOAD", json.dumps(payload))
        self._mp.setenv("FAKE_COX_EXIT", str(exit_code))
        if write:
            self._mp.setenv("FAKE_COX_WRITE", write)
        else:
            self._mp.delenv("FAKE_COX_WRITE", raising=False)

    def ledger(self, base, jev=None):
        """The `cox stats --csv` rows each arm's run leaves in its ledger."""
        self._mp.setenv("FAKE_COX_LEDGER", base)
        self._mp.setenv("FAKE_COX_LEDGER_JEV", base if jev is None else jev)

    def plugin_calls(self):
        log = Path(f"{self._args_file}.plugin")
        return log.read_text().splitlines() if log.exists() else []

    def argv(self):
        return self._args_file.read_text().splitlines()


@pytest.fixture
def fake_cox(tmp_path, monkeypatch):
    path = tmp_path / "cox"
    path.write_text(FAKE_COX)
    path.chmod(path.stat().st_mode | stat.S_IEXEC)
    args_file = tmp_path / "argv"
    monkeypatch.setenv("FAKE_COX_ARGS", str(args_file))
    return FakeCox(path, args_file, monkeypatch)


@pytest.fixture
def real_cox():
    """The built binary (`COX_BIN`, `cox` on PATH, or the cargo target)."""
    try:
        path = harness.find_cox_bin(None)
    except SystemExit:
        pytest.skip("no cox binary: `cargo build -p cox` first")
    if not os.access(path, os.X_OK):
        pytest.skip(f"{path} is not executable")
    return path
