"""Vendors model prices and provider `models` lists from models.dev
(T30.20): the `[[model]]` rows of `crates/cox-provider/prices.toml` and the
`[providers.*].models` arrays of `crates/cox-protocol/default.toml`. This
module is the only thing that writes those rows — no hand-copying numbers
out of models.dev into either file.

Source: ``GET https://models.dev/api.json`` — the full registry models.dev's
own site renders (models.dev; README/repo: https://github.com/sst/models.dev).
No API key, no pagination: one JSON object keyed by provider id (`anthropic`,
`openai`, `deepseek`, `openrouter`, `moonshotai`, `zai`, 200+ others), each
value carrying `models`, itself keyed by model id. A model entry carries
`limit.context` (context window) and `cost.*` — USD per MTok: `input`,
`output`, `cache_read`, `cache_write` (absent means the vendor bills no
surcharge there, i.e. 0 — matches how `usage.rs::PriceTable::cost` prices a
`[[model]]` row; some rows also carry `reasoning`, `tiers` and
`context_over_200k` for tiered/context-overage pricing, which this module
ignores in favour of the base rate, per docs/design/providers.md) and
`reasoning_options`: a list of shape markers, typically `{"type": "effort",
"values": [...]}"` and/or `{"type": "toggle"}`; some rows instead carry
`{"type": "budget_tokens", ...}` or an empty/absent list. `name` is the
model's display name (`Claude Sonnet 5`), written as `display_name` next to
the id in `default.toml` (A111). `modalities.input` lists what the model
takes (`["text", "image", "pdf"]`); whether it holds `"image"` is written as
`images = true/false` (T40.10). Checked
2026-09-25: `curl -s https://models.dev/api.json | python3 -c
"import json,sys; d=json.load(sys.stdin); print(len(d))"` returned 223
providers.

Source of truth for *which* model ids to price or list is the two files
themselves (plan.md T30.20) — this module never adds an id, and drops only a
`prices.toml` row that no `[providers.*].models` array lists any more
(T33.40.15). An id
cox already lists that models.dev does not have, or whose provider section
has no models.dev counterpart (`local` — no vendor row there),
or whose `reasoning_options` shape this module does not recognise, is
reported on stdout and left byte-for-byte unchanged. Anthropic's four native
rows are cross-checked against models.dev's `anthropic` provider but never
overwritten from it: `prices.toml`'s header says those rows are verified
against Anthropic's own pricing page, which outranks a mirror; a value that
differs is reported for a human to re-verify by hand.

To vendor a newer snapshot: `uv run --project scripts/vendor cox-vendor
models` (or `--check` to preview a diff without writing). `cox-vendor
model-names` writes only the `display_name`s and leaves every price,
context window and effort as it is — for taking new names without also
shipping upstream price or effort drift nobody has reviewed yet. See
docs/design/providers.md for the `reasoning_options` -> cox `Effort`
mapping and `crates/cox-provider/src/usage.rs` for the `[[model]]` row shape
this writes: `PriceTable::cost` uses `cache_read`/`cache_write` as absolute
USD/MTok rates, never as a multiplier of `input` — this module owns
`prices.toml`'s header comment saying so (and the paragraph below it) and
rewrites both whenever a row actually changes.

Stability across days: a row's `verified_on`/`source_url` and the header's
run date change only when that row's numbers actually differ from what's
on disk — re-running on a later day against unchanged upstream data is a
byte-for-byte no-op, so `--check` reports no diff.
"""

from __future__ import annotations

import difflib
import json
import urllib.request
from datetime import date
from pathlib import Path
from typing import Any

import tomlkit
from tomlkit.items import SingleKey

REPO_ROOT = Path(__file__).resolve().parents[4]
PRICES_FILE = REPO_ROOT / "crates" / "cox-provider" / "prices.toml"
DEFAULT_TOML_FILE = REPO_ROOT / "crates" / "cox-protocol" / "default.toml"

API_URL = "https://models.dev/api.json"

# cox `[providers.<name>]` section -> models.dev provider id. The two don't
# always match (`moonshot` -> `moonshotai`, `z-ai` -> `zai`, `gemini` ->
# `google`: models.dev's `google` provider names env `GEMINI_API_KEY`,
# checked 2026-09-28). A section
# absent here (`local`: no vendor row on models.dev — local inference) is
# never looked up;
# its `models` array is left completely untouched.
PROVIDER_TO_MODELS_DEV = {
    "anthropic": "anthropic",
    "openai": "openai",
    "deepseek": "deepseek",
    "openrouter": "openrouter",
    "moonshot": "moonshotai",
    "z-ai": "zai",
    "gemini": "google",
}

# cox `Effort` values, in the fixed order rendered into `efforts = [...]`.
EFFORT_ORDER = ("low", "medium", "high", "xhigh")

_CACHE_LINE_PREFIX = "# Cache pricing:"
_ANTHROPIC_LINE_PREFIX = "# Verified on"
_HEADER_MARKER = "# Type-2 provider rows verified on"


def download(url: str = API_URL) -> bytes:
    # models.dev 403s the default urllib User-Agent; any identifiable one works.
    req = urllib.request.Request(url, headers={"User-Agent": "cox-dev (https://github.com/pyrlyn/cox)"})
    with urllib.request.urlopen(req, timeout=30) as resp:  # noqa: S310 - fixed https URL
        return resp.read()


def validate(body: bytes) -> dict[str, Any]:
    """Raise ValueError on anything that isn't the models.dev registry shape."""
    try:
        parsed = json.loads(body)
    except json.JSONDecodeError as exc:
        raise ValueError(f"registry is not valid JSON: {exc}") from exc
    if not isinstance(parsed, dict) or not parsed:
        raise ValueError("registry has no top-level provider map")
    if "anthropic" not in parsed:
        raise ValueError('registry has no "anthropic" provider')
    return parsed


def cox_effort_for(reasoning_options: Any) -> list[str] | None:
    """Map a models.dev `reasoning_options` list to cox `Effort` names
    (docs/design/providers.md): an `effort` entry's `values` map
    low->low, medium->medium, high->high, xhigh/max->xhigh (cox has
    `Effort::Medium` since T30.26, so `medium` no longer folds into
    `high`); a `toggle`-only list (no `effort` entry) maps to all four. `none` (no-reasoning) is not a
    cox effort and is dropped. Anything else — missing, empty, or another
    shape such as `budget_tokens` — is not recognised: returns None so the
    caller reports it and keeps what's on disk.
    """
    if not isinstance(reasoning_options, list) or not reasoning_options:
        return None
    by_type = {e.get("type"): e for e in reasoning_options if isinstance(e, dict)}
    effort = by_type.get("effort")
    if effort is not None:
        values = set(effort.get("values") or [])
        mapped: set[str] = set()
        if "low" in values:
            mapped.add("low")
        if "medium" in values:
            mapped.add("medium")
        if "high" in values:
            mapped.add("high")
        if "xhigh" in values or "max" in values:
            mapped.add("xhigh")
        return [e for e in EFFORT_ORDER if e in mapped] or None
    if "toggle" in by_type:
        return list(EFFORT_ORDER)
    return None


def _id_to_provider_sections(default_doc: Any) -> dict[str, list[str]]:
    """`model id -> [cox provider section names that list it]`, read from
    every `[providers.*].models` array. Used to find which models.dev
    provider prices a given `prices.toml` row."""
    index: dict[str, list[str]] = {}
    for section, table in default_doc.get("providers", {}).items():
        for entry in table.get("models", []):
            index.setdefault(entry["id"], []).append(section)
    return index


def _resolve(mid: str, sections: list[str], registry: dict) -> tuple[str | None, dict | None]:
    """The (cox section, models.dev model entry) that prices `mid`, or
    (None, None) if no listed section has a models.dev counterpart or lists
    this id."""
    for section in sections:
        dev_id = PROVIDER_TO_MODELS_DEV.get(section)
        if dev_id is None:
            continue
        provider = registry.get(dev_id)
        if provider is None:
            continue
        row = provider.get("models", {}).get(mid)
        if row is not None:
            return section, row
    return None, None


def _rewrite_header(text: str, *, today: str) -> str:
    """Rewrite the two header paragraphs this module owns: the one-line
    cache-pricing note (fixed wording — `cache_read`/`cache_write` are
    absolute USD/MTok rates, confirmed against `usage.rs::PriceTable::cost`,
    never a multiplier) and the "Type-2 provider rows..." paragraph (source,
    regenerate command, today's date). Everything above/between them (title,
    USD/MTok note, the Anthropic vendor-page line) is hand-maintained and
    left alone. Called only when a row actually changed, so the header's
    run date changes only then too."""
    lines = text.splitlines(keepends=True)

    cache_i = next((i for i, line in enumerate(lines) if line.startswith(_CACHE_LINE_PREFIX)), None)
    if cache_i is not None:
        end = cache_i + 1
        while (
            end < len(lines)
            and lines[end].startswith("#")
            and not lines[end].startswith(_ANTHROPIC_LINE_PREFIX)
            and not lines[end].startswith(_HEADER_MARKER)
        ):
            end += 1
        lines[cache_i:end] = [
            "# Cache pricing: cache_write happens to be 1.25x input_price for\n",
            "# Anthropic's own SKUs; cache_read is an absolute USD/MTok rate, not a\n",
            "# multiplier — see usage.rs PriceTable::cost.\n",
        ]

    start = next((i for i, line in enumerate(lines) if line.startswith(_HEADER_MARKER)), None)
    if start is None:
        return "".join(lines)
    end = start + 1
    while end < len(lines) and lines[end].startswith("#"):
        end += 1
    replacement = [
        f"# Type-2 provider rows regenerated on {today} from {API_URL} by\n",
        "# `uv run --project scripts/vendor cox-vendor models` (scripts/vendor/src/\n",
        "# cox_vendor/models.py) — re-run that command to refresh instead of\n",
        "# hand-editing. ids match default.toml [providers.*].models exactly so\n",
        "# every catalogued model prices its ledger row. OpenAI-compatible vendors\n",
        "# bill no cache-write surcharge unless models.dev's `cost.cache_write` says\n",
        "# otherwise; tiered context-overage pricing (gpt-5.5/5.6-sol, grok) is\n",
        "# recorded at the base tier.\n",
    ]
    lines[start:end] = replacement
    return "".join(lines)


def build_prices_toml(
    text: str, registry: dict, *, id_to_sections: dict[str, list[str]], today: str, report: list[str]
) -> str:
    doc = tomlkit.parse(text)
    rows = doc.get("model")
    if not rows:
        raise ValueError("prices.toml has no [[model]] rows")

    # A row no `[providers.*].models` array lists prices a model cox no longer
    # ships (T33.40.15: `jev-latest` moved to the jev plugin's own `[[models]]`
    # table), so it goes instead of lingering as a "kept unchanged" exception.
    for index in reversed([i for i, row in enumerate(rows) if row["id"] not in id_to_sections]):
        report.append(f"prices.toml: `{rows[index]['id']}` is listed by no [providers.*] section, dropped")
        del rows[index]

    for row in rows:
        mid = row["id"]
        section, dev_row = _resolve(mid, id_to_sections.get(mid, []), registry)
        if dev_row is None:
            report.append(f"prices.toml: `{mid}` not found on models.dev, kept unchanged")
            continue
        cost = dev_row.get("cost") or {}
        new = (
            float(cost.get("input", 0.0)),
            float(cost.get("output", 0.0)),
            float(cost.get("cache_write", 0.0)),
            float(cost.get("cache_read", 0.0)),
        )
        old = (float(row["input"]), float(row["output"]), float(row["cache_write"]), float(row["cache_read"]))
        differs = any(abs(a - b) > 1e-9 for a, b in zip(old, new))
        if section == "anthropic":
            if differs:
                report.append(
                    f"prices.toml: `{mid}` differs from models.dev "
                    f"(file input/output/cache_write/cache_read = {old}; "
                    f"models.dev = {new}) — kept the vendor-page numbers, needs "
                    "human re-verification against the pricing page"
                )
            continue  # native Anthropic rows stay vendor-page-sourced, always
        if not differs:
            continue  # already matches models.dev: don't touch verified_on/source_url
        row["input"], row["output"], row["cache_write"], row["cache_read"] = new
        row["verified_on"] = today
        row["source_url"] = "https://models.dev"

    dumped = tomlkit.dumps(doc)
    if dumped == text:
        return text  # no row changed: the header's run date stays put too
    return _rewrite_header(dumped, today=today)


def _set_display_name(entry: Any, dev_row: dict) -> None:
    """Write models.dev's `name` as the entry's `display_name`, right after
    its `id` and in the file's compact `key="value"` style. A row with no
    usable name keeps whatever it has: the reader falls back to the id."""
    name = dev_row.get("name")
    if not isinstance(name, str) or not name.strip():
        return
    _set_compact(entry, "display_name", name.strip(), after="id")


def _set_images(entry: Any, dev_row: dict) -> None:
    """Write `images` from whether models.dev's `modalities.input` lists
    `"image"` (T40.9 reads it on a Chat section). A row with no such list
    keeps whatever it has: unset means "not declared", never a guess."""
    modalities = dev_row.get("modalities")
    inputs = modalities.get("input") if isinstance(modalities, dict) else None
    if not isinstance(inputs, list):
        return
    _set_compact(entry, "images", "image" in inputs, after=list(entry.keys())[-1])


def _set_compact(entry: Any, key: str, value: Any, *, after: str) -> None:
    """Set `key` in an inline-table entry, inserting a new key right after
    `after` in the file's compact `key=value` style."""
    if key in entry:
        entry[key] = value
        return
    item = tomlkit.item(value)
    item.trivia.indent = " "
    # tomlkit has no public insert-at-position for an inline table; its
    # container's `_insert_after` is what `Table` uses internally (pinned in
    # uv.lock), and appending instead renders `,display_name = "..."`.
    entry.value._insert_after(after, SingleKey(key, sep="="), item)


def build_default_toml(text: str, registry: dict, *, report: list[str], names_only: bool = False) -> str:
    doc = tomlkit.parse(text)
    providers = doc.get("providers", {})
    for section, table in providers.items():
        dev_id = PROVIDER_TO_MODELS_DEV.get(section)
        if dev_id is None:
            continue  # no models.dev counterpart (local): untouched
        dev_provider = registry.get(dev_id)
        if dev_provider is None:
            report.append(
                f"default.toml: models.dev has no provider `{dev_id}` for "
                f"[providers.{section}], models array kept unchanged"
            )
            continue
        dev_models = dev_provider.get("models", {})
        models_array = table.get("models")
        if models_array is None:
            continue
        for entry in models_array:
            mid = entry["id"]
            dev_row = dev_models.get(mid)
            if dev_row is None:
                report.append(
                    f"default.toml: [providers.{section}] `{mid}` not found on "
                    f"models.dev (`{dev_id}`), kept unchanged"
                )
                continue
            _set_display_name(entry, dev_row)
            if names_only:
                continue
            limit = dev_row.get("limit") or {}
            new_ctx = limit.get("context")
            if isinstance(new_ctx, int):
                entry["context_window"] = new_ctx
            efforts = cox_effort_for(dev_row.get("reasoning_options"))
            if efforts is None:
                report.append(
                    f"default.toml: [providers.{section}] `{mid}` reasoning_options "
                    "shape not recognised, efforts kept unchanged"
                )
            else:
                entry["efforts"] = efforts
            _set_images(entry, dev_row)
    return tomlkit.dumps(doc)


def _unified_diff(before: str, after: str, path: str) -> str:
    return "".join(
        difflib.unified_diff(
            before.splitlines(keepends=True), after.splitlines(keepends=True), fromfile=path, tofile=path
        )
    )


def run(*, check: bool = False, fetch=None, today: str | None = None, names_only: bool = False) -> bool:
    """Fetch the models.dev registry, validate it, and — unless `check` —
    rewrite `prices.toml` and `default.toml`. Prints one report line per
    unknown id / ambiguous effort shape / Anthropic discrepancy, and (in
    `check` mode) a unified diff for each file that would change. Returns
    whether either file's bytes differ from what's on disk. `names_only`
    writes the `display_name`s alone: `prices.toml` is not touched and no
    other `default.toml` field moves.

    `today` defaults to the real date; tests inject a fixed value to prove
    a row's `verified_on` and the header's run date only move when a row's
    numbers actually change — re-running later against unchanged upstream
    data is a no-op regardless of what day it is."""
    body = (fetch or download)()
    registry = validate(body)
    today = today or date.today().isoformat()
    report: list[str] = []

    default_text = DEFAULT_TOML_FILE.read_text()
    id_to_sections = _id_to_provider_sections(tomlkit.parse(default_text))

    prices_text = PRICES_FILE.read_text()
    new_prices_text = (
        prices_text
        if names_only
        else build_prices_toml(prices_text, registry, id_to_sections=id_to_sections, today=today, report=report)
    )
    new_default_text = build_default_toml(default_text, registry, report=report, names_only=names_only)

    for line in report:
        print(line)

    prices_changed = new_prices_text != prices_text
    default_changed = new_default_text != default_text
    changed = prices_changed or default_changed

    if check:
        if prices_changed:
            print(_unified_diff(prices_text, new_prices_text, str(PRICES_FILE)))
        if default_changed:
            print(_unified_diff(default_text, new_default_text, str(DEFAULT_TOML_FILE)))
        return changed

    if prices_changed:
        PRICES_FILE.write_text(new_prices_text)
    if default_changed:
        DEFAULT_TOML_FILE.write_text(new_default_text)
    return changed


def run_names(*, check: bool = False, fetch=None) -> bool:
    """`cox-vendor model-names`: `run` with `names_only`."""
    return run(check=check, fetch=fetch, names_only=True)
