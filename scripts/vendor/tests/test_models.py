"""Tests for `cox_vendor.models`: no network — every test stubs the fetch
with a recorded models.dev subset (`fixtures/models_dev_subset.json`), and
PRICES_FILE/DEFAULT_TOML_FILE are pointed at copies of the small fixtures
under `fixtures/` instead of the real vendored files."""

import json
import shutil
from datetime import date
from pathlib import Path

import pytest
import tomlkit

from cox_vendor import models, registry

FIXTURES = Path(__file__).parent / "fixtures"
REGISTRY_BODY = (FIXTURES / "models_dev_subset.json").read_bytes()
TODAY = date.today().isoformat()


@pytest.fixture
def isolated_files(tmp_path, monkeypatch):
    prices = tmp_path / "prices.toml"
    default = tmp_path / "default.toml"
    shutil.copy(FIXTURES / "prices.toml", prices)
    shutil.copy(FIXTURES / "default.toml", default)
    monkeypatch.setattr(models, "PRICES_FILE", prices)
    monkeypatch.setattr(models, "DEFAULT_TOML_FILE", default)
    return prices, default


def fetching(body: bytes = REGISTRY_BODY):
    return lambda: body


def test_validate_accepts_the_registry_shape():
    models.validate(REGISTRY_BODY)  # no raise


def test_validate_rejects_non_json():
    with pytest.raises(ValueError):
        models.validate(b"not json")


def test_validate_rejects_a_registry_with_no_anthropic_provider():
    with pytest.raises(ValueError):
        models.validate(json.dumps({"openai": {}}).encode())


class TestCoxEffortFor:
    def test_effort_values_map_low_medium_high_xhigh_max(self):
        assert models.cox_effort_for(
            [{"type": "effort", "values": ["low", "medium", "high", "xhigh", "max"]}]
        ) == ["low", "medium", "high", "xhigh"]

    def test_medium_maps_to_medium_not_high(self):
        assert models.cox_effort_for([{"type": "effort", "values": ["medium"]}]) == ["medium"]

    def test_effort_without_low_or_xhigh_omits_them(self):
        assert models.cox_effort_for([{"type": "effort", "values": ["high", "max"]}]) == ["high", "xhigh"]

    def test_none_value_is_not_a_cox_effort(self):
        assert models.cox_effort_for([{"type": "effort", "values": ["none", "low"]}]) == ["low"]

    def test_toggle_only_maps_to_all_four(self):
        assert models.cox_effort_for([{"type": "toggle"}]) == ["low", "medium", "high", "xhigh"]

    def test_toggle_alongside_effort_uses_the_effort_values(self):
        assert models.cox_effort_for([{"type": "toggle"}, {"type": "effort", "values": ["low"]}]) == ["low"]

    @pytest.mark.parametrize("shape", [None, [], [{"type": "budget_tokens", "min": 1024}]])
    def test_unrecognised_shapes_return_none(self, shape):
        assert models.cox_effort_for(shape) is None


def test_run_writes_both_files(isolated_files):
    prices, default = isolated_files
    changed = models.run(fetch=fetching())
    assert changed
    assert prices.read_text() != (FIXTURES / "prices.toml").read_text()
    assert default.read_text() != (FIXTURES / "default.toml").read_text()


def test_check_reports_a_diff_and_writes_nothing(isolated_files):
    prices, default = isolated_files
    before_prices, before_default = prices.read_text(), default.read_text()
    changed = models.run(check=True, fetch=fetching())
    assert changed
    assert prices.read_text() == before_prices
    assert default.read_text() == before_default


def test_check_after_a_real_run_reports_no_diff(isolated_files):
    models.run(fetch=fetching())
    assert models.run(check=True, fetch=fetching()) is False


def test_rerun_is_idempotent(isolated_files):
    models.run(fetch=fetching())
    prices, default = isolated_files
    after_first = (prices.read_text(), default.read_text())
    models.run(fetch=fetching())
    assert (prices.read_text(), default.read_text()) == after_first


def test_default_toml_regenerates_context_window_and_efforts(isolated_files):
    prices, default = isolated_files
    models.run(fetch=fetching())
    doc = tomlkit.parse(default.read_text())

    anthropic = {m["id"]: m for m in doc["providers"]["anthropic"]["models"]}
    # claude-sonnet-5: effort values low/medium/high/xhigh/max -> low/medium/high/xhigh.
    assert list(anthropic["claude-sonnet-5"]["efforts"]) == ["low", "medium", "high", "xhigh"]
    assert anthropic["claude-sonnet-5"]["context_window"] == 1000000
    # claude-haiku-4-5: budget_tokens-only reasoning_options is not recognised,
    # so efforts stay untouched, but context_window still comes from `limit`.
    assert list(anthropic["claude-haiku-4-5"]["efforts"]) == ["low"]
    assert anthropic["claude-haiku-4-5"]["context_window"] == 200000

    openai = {m["id"]: m for m in doc["providers"]["openai"]["models"]}
    # none/low/medium/high -> low/medium/high (`none` is not a cox effort).
    assert list(openai["gpt-5.1"]["efforts"]) == ["low", "medium", "high"]
    assert openai["gpt-5.1"]["context_window"] == 400000

    moonshot = {m["id"]: m for m in doc["providers"]["moonshot"]["models"]}
    # toggle-only -> all four.
    assert list(moonshot["kimi-k2.6"]["efforts"]) == ["low", "medium", "high", "xhigh"]
    # kimi-k2.7-code isn't in the fixture registry: left byte-for-byte alone.
    assert list(moonshot["kimi-k2.7-code"]["efforts"]) == ["low", "high", "xhigh"]
    assert moonshot["kimi-k2.7-code"]["context_window"] == 262144

    zai = {m["id"]: m for m in doc["providers"]["z-ai"]["models"]}
    # effort values high/max -> high/xhigh (low dropped).
    assert list(zai["glm-5.2"]["efforts"]) == ["high", "xhigh"]

    openrouter = {m["id"]: m for m in doc["providers"]["openrouter"]["models"]}
    # reasoning_options is null on models.dev: not recognised, kept unchanged.
    assert list(openrouter["qwen/qwen3-coder-plus"]["efforts"]) == ["low", "high", "xhigh"]

    # deepseek section maps to a models.dev provider absent from this fixture
    # registry: its models array is completely untouched.
    deepseek = {m["id"]: m for m in doc["providers"]["deepseek"]["models"]}
    assert deepseek["deepseek-v4-pro"]["context_window"] == 1000000
    assert list(deepseek["deepseek-v4-pro"]["efforts"]) == ["high", "xhigh"]

    # local has no models.dev counterpart at all: untouched.
    local = {m["id"]: m for m in doc["providers"]["local"]["models"]}
    assert local["qwen3-coder"]["context_window"] == 32768


def test_default_toml_takes_the_models_dev_name_as_display_name_after_the_id(isolated_files):
    prices, default = isolated_files
    models.run(fetch=fetching())
    text = default.read_text()
    assert '{id="claude-sonnet-5", display_name="Claude Sonnet 5", context_window=1000000' in text
    doc = tomlkit.parse(text)
    anthropic = {m["id"]: m for m in doc["providers"]["anthropic"]["models"]}
    assert anthropic["claude-haiku-4-5"]["display_name"] == "Claude Haiku 4.5"
    # No name on models.dev (glm-5.2), a blank one (qwen/qwen3-coder-plus) or
    # no models.dev row at all (kimi-k2.7-code): no display_name, the reader
    # falls back to the id.
    zai = {m["id"]: m for m in doc["providers"]["z-ai"]["models"]}
    assert "display_name" not in zai["glm-5.2"]
    openrouter = {m["id"]: m for m in doc["providers"]["openrouter"]["models"]}
    assert "display_name" not in openrouter["qwen/qwen3-coder-plus"]
    moonshot = {m["id"]: m for m in doc["providers"]["moonshot"]["models"]}
    assert "display_name" not in moonshot["kimi-k2.7-code"]
    assert moonshot["kimi-k2.6"]["display_name"] == "Kimi K2.6"


def test_a_renamed_model_updates_its_display_name_in_place(isolated_files):
    prices, default = isolated_files
    models.run(fetch=fetching())
    renamed = json.loads(REGISTRY_BODY)
    renamed["anthropic"]["models"]["claude-sonnet-5"]["name"] = "Claude Sonnet 5 (new)"
    assert models.run(fetch=fetching(json.dumps(renamed).encode()))
    text = default.read_text()
    assert '{id="claude-sonnet-5", display_name="Claude Sonnet 5 (new)", context_window=' in text
    assert text.count("display_name=") == 4


def test_names_only_writes_the_names_and_nothing_else(isolated_files):
    prices, default = isolated_files
    before_prices = prices.read_text()
    assert models.run(fetch=fetching(), names_only=True)
    assert prices.read_text() == before_prices
    doc = tomlkit.parse(default.read_text())
    anthropic = {m["id"]: m for m in doc["providers"]["anthropic"]["models"]}
    sonnet, haiku = anthropic["claude-sonnet-5"], anthropic["claude-haiku-4-5"]
    assert sonnet["display_name"] == "Claude Sonnet 5"
    # The fixture's stale efforts and context window stay as they were.
    assert list(sonnet["efforts"]) == ["low", "high"]
    assert haiku["context_window"] == 111111
    # A second names-only run has nothing left to write.
    assert models.run(check=True, fetch=fetching(), names_only=True) is False


def test_model_names_is_a_registered_command():
    assert registry.COMMANDS["model-names"] is models.run_names


def test_default_toml_comments_and_unrelated_tables_are_preserved(isolated_files):
    prices, default = isolated_files
    models.run(fetch=fetching())
    text = default.read_text()
    assert "# id, context window, efforts per model (effort values from models.dev)" in text
    assert "# local inference, never on models.dev" in text
    assert '[permissions]\nmode = "default"\ndeny = ["Read(~/.ssh/**)", "Read(~/.aws/**)"]' in text


def test_prices_toml_regenerates_rows_from_models_dev(isolated_files):
    prices, default = isolated_files
    models.run(fetch=fetching())
    doc = tomlkit.parse(prices.read_text())
    rows = {r["id"]: r for r in doc["model"]}

    # gpt-5.1's fixture cache_read (0.999) is deliberately wrong: corrected
    # to models.dev's 0.125, and re-stamped as freshly verified.
    gpt = rows["gpt-5.1"]
    assert gpt["input"] == 1.25 and gpt["cache_read"] == 0.125
    assert gpt["verified_on"] == TODAY
    assert gpt["source_url"] == "https://models.dev"

    # kimi-k2.6, glm-5.2 and qwen/qwen3-coder-plus already match models.dev
    # exactly: their verified_on/source_url are left completely untouched.
    for mid in ("kimi-k2.6", "glm-5.2", "qwen/qwen3-coder-plus"):
        assert rows[mid]["verified_on"] == "2026-09-04", mid
        assert rows[mid]["source_url"] == "https://models.dev", mid


def test_unknown_id_in_prices_toml_is_reported_and_kept(isolated_files, capsys):
    prices, default = isolated_files
    models.run(fetch=fetching())
    out = capsys.readouterr().out
    assert "`kimi-k2.7-code` not found on models.dev, kept unchanged" in out
    assert "`deepseek-v4-pro` not found on models.dev, kept unchanged" in out
    assert "`qwen3-coder` not found on models.dev, kept unchanged" in out
    doc = tomlkit.parse(prices.read_text())
    rows = {r["id"]: r for r in doc["model"]}
    assert rows["kimi-k2.7-code"]["verified_on"] == "2026-09-04"
    assert rows["deepseek-v4-pro"]["input"] == 0.435
    assert rows["qwen3-coder"]["source_url"] == "local inference, no billing"


def test_a_price_row_no_section_lists_is_dropped_and_reported(isolated_files, capsys):
    # `jev-latest` is in the fixture prices.toml but in no default.toml
    # section: its price now comes from the jev plugin's own `[[models]]`.
    prices, default = isolated_files
    assert "jev-latest" in prices.read_text()
    models.run(fetch=fetching())
    out = capsys.readouterr().out
    assert "`jev-latest` is listed by no [providers.*] section, dropped" in out
    ids = [r["id"] for r in tomlkit.parse(prices.read_text())["model"]]
    assert "jev-latest" not in ids
    # Rows a section does list, with or without a models.dev counterpart, stay.
    assert "qwen3-coder" in ids and "kimi-k2.7-code" in ids


def test_dropping_an_orphan_row_is_stable_on_a_rerun(isolated_files):
    # The fixture is needed for its isolated copies, not its paths.
    models.run(fetch=fetching())
    assert models.run(check=True, fetch=fetching()) is False


def test_anthropic_discrepancy_is_reported_and_the_row_is_never_overwritten(isolated_files, capsys):
    prices, default = isolated_files
    models.run(fetch=fetching())
    out = capsys.readouterr().out
    assert "`claude-sonnet-5` differs from models.dev" in out
    doc = tomlkit.parse(prices.read_text())
    rows = {r["id"]: r for r in doc["model"]}
    # The fixture's claude-sonnet-5 input (999.0) is deliberately wrong; the
    # script must report it, never silently overwrite a vendor-page row.
    assert rows["claude-sonnet-5"]["input"] == 999.0
    assert rows["claude-sonnet-5"]["source_url"] == "https://platform.claude.com/docs/pricing"
    # A matching Anthropic price row (claude-haiku-4-5) is never flagged as a
    # price discrepancy (it's separately reported for its unrelated,
    # unrecognised default.toml effort shape -- a different message).
    assert "`claude-haiku-4-5` differs from models.dev" not in out


def test_prices_toml_header_points_at_the_new_source_and_date(isolated_files):
    prices, default = isolated_files
    models.run(fetch=fetching())
    text = prices.read_text()
    assert f"regenerated on {TODAY} from {models.API_URL}" in text
    assert "cox-vendor models" in text
    # The Anthropic vendor-page line above it is untouched.
    assert "# Verified on 2026-09-02 from https://platform.claude.com/docs/pricing" in text
    # The cache-pricing line is corrected: cache_read is an absolute rate,
    # never a multiplier (usage.rs::PriceTable::cost) -- confirmed by reading
    # that file before writing this.
    assert "cache_read is an absolute USD/MTok rate, not a\n# multiplier" in text
    assert "cache_read is multiplier" not in text


def test_rerun_on_a_later_day_with_unchanged_data_is_a_byte_for_byte_no_op(isolated_files):
    prices, default = isolated_files
    changed = models.run(fetch=fetching(), today="2020-01-01")
    assert changed
    after_first = (prices.read_text(), default.read_text())

    # Same upstream data, a much later injected date: nothing to re-verify,
    # so nothing should move -- not a row's verified_on, not the header.
    changed_check = models.run(check=True, fetch=fetching(), today="2099-12-31")
    assert changed_check is False
    assert (prices.read_text(), default.read_text()) == after_first

    changed_again = models.run(fetch=fetching(), today="2099-12-31")
    assert changed_again is False
    assert (prices.read_text(), default.read_text()) == after_first

    doc = tomlkit.parse(prices.read_text())
    gpt = {r["id"]: r for r in doc["model"]}["gpt-5.1"]
    assert gpt["verified_on"] == "2020-01-01"
    assert "regenerated on 2020-01-01" in prices.read_text()
    assert "2099-12-31" not in prices.read_text()


def test_a_malformed_registry_is_rejected_and_nothing_is_written(isolated_files):
    prices, default = isolated_files
    before_prices, before_default = prices.read_text(), default.read_text()
    with pytest.raises(ValueError):
        models.run(fetch=fetching(b"not json"))
    assert prices.read_text() == before_prices
    assert default.read_text() == before_default


def test_a_google_row_fills_the_gemini_section_and_its_price():
    # `[providers.gemini]` is priced from models.dev's `google` provider (P39).
    # The numbers below are test values, not Google's prices.
    registry = {
        "google": {
            "models": {
                "gemini-3.8-flash": {
                    "name": "Gemini 3.8 Flash",
                    "limit": {"context": 1048576},
                    "cost": {"input": 0.5, "output": 3.0, "cache_read": 0.05},
                    "reasoning_options": [{"type": "effort", "values": ["minimal", "low", "medium", "high"]}],
                }
            }
        }
    }
    default_text = (
        "[providers.gemini]\n"
        'api = "chat"\n'
        'models = [{id="gemini-3.8-flash", context_window=0, efforts=[], reasoning_effort=true}]\n'
    )
    prices_text = (
        "[[model]]\n"
        'id = "gemini-3.8-flash"\n'
        "input = 0.0\n"
        "output = 0.0\n"
        "cache_write = 0.0\n"
        "cache_read = 0.0\n"
        'verified_on = "1970-01-01"\n'
        'source_url = "https://models.dev"\n'
    )
    report: list[str] = []

    doc = tomlkit.parse(models.build_default_toml(default_text, registry, report=report))
    row = doc["providers"]["gemini"]["models"][0]
    assert row["display_name"] == "Gemini 3.8 Flash"
    assert row["context_window"] == 1048576
    # `minimal` is not a cox effort and is dropped.
    assert list(row["efforts"]) == ["low", "medium", "high"]
    assert row.unwrap()["reasoning_effort"] is True

    id_to_sections = {"gemini-3.8-flash": ["gemini"]}
    priced = tomlkit.parse(
        models.build_prices_toml(prices_text, registry, id_to_sections=id_to_sections, today=TODAY, report=report)
    )
    price = priced["model"][0]
    assert (price["input"], price["output"], price["cache_write"], price["cache_read"]) == (0.5, 3.0, 0.0, 0.05)
    assert price["verified_on"] == TODAY
    assert report == []


def test_images_follows_models_dev_input_modalities():
    # T40.10: `images` is whether `modalities.input` lists "image"; a row
    # models.dev gives no modalities for keeps what it has (unset stays unset).
    registry = {
        "deepseek": {
            "models": {
                "deepseek-v4-pro": {"modalities": {"input": ["text"], "output": ["text"]}},
                "deepseek-v4-flash": {"modalities": {"input": ["text", "image"], "output": ["text"]}},
                "deepseek-v4-lite": {"name": "DeepSeek V4 Lite"},
            }
        }
    }
    default_text = (
        "[providers.deepseek]\n"
        'models = [{id="deepseek-v4-pro", efforts=["high"]}, '
        '{id="deepseek-v4-flash", efforts=["high"], images=false}, '
        '{id="deepseek-v4-lite", efforts=["high"]}]   # comment\n'
    )
    report: list[str] = []
    text = models.build_default_toml(default_text, registry, report=report)
    assert '{id="deepseek-v4-pro", efforts=["high"], images=false}' in text
    assert '{id="deepseek-v4-flash", efforts=["high"], images=true}' in text
    rows = {m["id"]: m for m in tomlkit.parse(text)["providers"]["deepseek"]["models"]}
    assert "images" not in rows["deepseek-v4-lite"]
    # A second pass has nothing left to change.
    assert models.build_default_toml(text, registry, report=report) == text
