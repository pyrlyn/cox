# cox-vendor

The one entry point for a file no package manager fetches: a vendored API
spec, a price or model table, any JSON/YAML data pulled from outside the
repo (`plan.md` A48). No hand `curl`, no pasted rows — every such file is
produced by a saved, tested Python script here and re-run to update it.

## Commands

```bash
uv run --project scripts/vendor cox-vendor anthropic-spec           # fetch, validate, write
uv run --project scripts/vendor cox-vendor anthropic-spec --check   # report a diff, write nothing, exit 1 if stale

uv run --project scripts/vendor cox-vendor models           # regenerate prices.toml + default.toml from models.dev
uv run --project scripts/vendor cox-vendor models --check   # report a diff, write nothing, exit 1 if stale
uv run --project scripts/vendor cox-vendor model-names      # only the models' display names, nothing else

uv run --project scripts/vendor cox-vendor whisper-models   # the pinned ggml table cox voice may download

TYPESAFE_API_KEY=… uv run --project scripts/vendor cox-vendor jev-fixtures   # live Jev replies; paid, creator only
uv run --project scripts/vendor cox-vendor jev-fixtures --check             # no key, no request: are recordings current?
```

`whisper-models` (T54.1) writes `crates/cox-voice/data/whisper-models.json`
from the Hugging Face API for `ggerganov/whisper.cpp`: one row per offered
model with a URL pinned to the repository commit, the size and the SHA-256
(the LFS oid). A file with no LFS oid or size is refused. Its `fetch` takes
the URL, because it reads two documents.

`jev-fixtures` (T33.40.17) records TypeSafe's live System One replies
into `crates/cox/tests/fixtures/jev/<point>-<name>.json`. It sends the
bodies in `requests.json` beside them, which the Jev plugin's own
`decide` writes (`plugins/jev/tests/fixtures.rs`; re-bless with
`cargo --config 'env.COX_BLESS="1"' test --manifest-path plugins/Cargo.toml
-p cox-plugin-jev --test fixtures` after a question-set change), so the
question set is never copied here. Each file holds the body, the reply,
the reply's model version and the date, with every string redacted by
the rollout scrubber's shapes plus the key itself; a reply that does not
answer exactly the questions asked writes nothing. It costs money and
needs the creator's `TYPESAFE_API_KEY` in the environment (never the
keychain; without it the command exits 1 and sends nothing), so it never
runs in CI. `--check` needs no key and sends nothing: it reports a
recording that is missing or was made for a body the plugin no longer
sends.

`models` (T30.20) regenerates the `[[model]]` rows of
`crates/cox-provider/prices.toml` and the `[providers.*].models` arrays of
`crates/cox-protocol/default.toml` from `https://models.dev/api.json`. It
only ever updates ids the two files already list — an id models.dev
doesn't have, or whose provider section has no models.dev counterpart
(`local`), is reported on stdout and left unchanged. The one row it drops is
a `prices.toml` row that no `[providers.*].models` array lists any more (a
model that moved to a plugin's own `[[models]]` table, T33.40.15). Anthropic's four native rows are cross-checked against models.dev
but never overwritten from it (the vendor pricing page outranks a mirror);
a difference is reported for a human to re-verify by hand. Each listed
model also takes models.dev's `name` as its `display_name` (A111);
`model-names` writes those alone, so new names can land without also
shipping price or effort drift nobody has reviewed.

Or through the task runner: `just vendor anthropic-spec`, `just vendor anthropic-spec --check`, `just vendor models --check`.

## Adding a vendored file

1. Add a module `src/cox_vendor/<name>.py` with:
   - the source URL or API as a module-level constant, so bumping it later
     is a one-line diff;
   - a `run(*, check: bool = False, fetch=None) -> bool` that calls
     `fetch()` (defaulting to its own `download()`) to get the bytes,
     validates them (raise `ValueError` and write nothing on anything
     unexpected), writes the target file(s) unless `check`, and returns
     whether the bytes changed.
2. Register it in `src/cox_vendor/registry.py`'s `COMMANDS` dict, keyed by
   the name used on the command line.
3. Add tests under `tests/` with no network: stub `fetch`, and cover a
   first write, a same-bytes no-op re-run, `--check` on both a diff and no
   diff, and rejection of a malformed payload with nothing written.

## Tests

```bash
just vendor-test
```
