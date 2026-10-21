"""Registry of vendored files `cox-vendor` can produce (plan.md A48). Each
entry is a `run(*, check: bool, fetch=None) -> bool` callable: True if the
vendored bytes differ from what's on disk, nothing written when `check` is
set.
"""

from . import anthropic_spec, cursor_fixtures, cursor_live_fixtures, models, whisper_models

COMMANDS = {
    "anthropic-spec": anthropic_spec.run,
    "cursor-fixtures": cursor_fixtures.run,
    "cursor-live-fixtures": cursor_live_fixtures.run,
    "models": models.run,
    "model-names": models.run_names,
    "whisper-models": whisper_models.run,
}
