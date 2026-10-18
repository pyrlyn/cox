# Design: provider registry (two types, opencode-shaped data)

## Problem

Adding a provider today costs a code change: `ProviderId` has three
variants, `Router::pick` matches three names, `provider_for` builds three
clients, and prices live in a 4-row table. Each new API (DeepSeek,
OpenRouter, Kimi, GLM, …) would repeat that trail, although all of them
speak the same OpenAI Chat Completions wire format cox already implements.
Measurable question: can a user add DeepSeek or OpenRouter with config
lines only, zero new Rust, and still get a priced ledger row?

## The field

**OpenCode + models.dev (evidence 2026-09-04, `~/.cache/opencode/models.json`,
213 providers).** One registry row per provider: `api` base URL, `env` key
name, `npm` adapter (`@ai-sdk/anthropic`, `@ai-sdk/openai`,
`@ai-sdk/openai-compatible`), and a `models` map each carrying
`limit.context/output`, `cost.*` per MTok, `tool_call`, and
`reasoning_options.effort` values. A custom provider is the same shape
hand-written: nuclear option is `npm: @ai-sdk/openai-compatible` +
`options.baseURL` + a `models` map (verified in the user's own
`opencode.json`: `headroom`, `teamorouter`). Native code exists only per
*adapter*, never per vendor: DeepSeek/OpenRouter/MoonShot are all
`openai-compatible`.

**Aider / Pi.** Same split: a short list of hand-rolled clients plus a
generic OpenAI-compatible endpoint taking base URL + key + model name.

## cox

Two types, split by wire protocol, not by vendor:

- **Type 1 — native (`Provider` impl).** Only when the wire format is new:
  `AnthropicProvider` (Messages), `OpenAiResponsesProvider` (Responses),
  `OpenAiChatProvider` (Chat Completions). A new vendor on an existing
  protocol adds no code — writing a `DeepseekProvider` struct over the
  Chat URL would duplicate `OpenAiChatProvider` line for line.
- **Type 2 — compatible (`[providers.<name>]` table).** Pure data, the
  opencode custom-provider shape: `base_url`, `api_key_env`, `api = "chat"`,
  a default `model`, a fallback `context_window`, and a `models` list where
  each entry carries the model id, its context window and the efforts it
  understands (models.dev `reasoning_options.effort` mapped to cox
  `Effort`: `low→low`, `medium→medium`, `high→high`, `xhigh/max→xhigh`;
  `toggle`-only models accept all four; `medium` folded into `high` until
  T30.26). Costs stay in `prices.toml` — the one file the
  ledger reads — extended with the same ids from models.dev. Runtime use of
  the list is real, not decorative: per-model context resolution feeds
  `Caps::max_context`, which drives the compaction trigger.

Custom providers report `ProviderId::Local` — the ledger's
"OpenAI-compatible family" id (precedent: `Scripted`/`Replay` already do),
so no storage migration; the model string disambiguates the row. Unknown
tier names still fail closed (`RouteError::UnknownProvider`); a typo'd
`[providers.*]` table can only become a routable name by being referenced
from `[tiers.*]`.

Seed: `deepseek` (3 models), `openrouter` (curated coding subset),
`moonshot` (Kimi K2.x), `z-ai` (GLM-5.x) — all `tool_call=true` in
models.dev, all Chat-compatible. Candidates deliberately deferred (same
shape, one table each when asked): `groq`, `togetherai`, `fireworks-ai`,
`nvidia`, `x-ai`, `cerebras`, `minimax`.

## Falsifier

Add a fifth compatible provider (e.g. `groq`) using only config lines; if
any `.rs` file must change, the registry leaked. Conversely, if a vendor
ships a wire format none of the three clients parses (fixtures fail to
produce `ToolUseStart`), that vendor graduates to Type 1 — the split is
decided by fixture, not by brand.

## Target shape: one model of providers, models, prices and effort (T30.17)

**What the registry left split** (R§4.3.3). The registry unified *which* providers exist. It did not unify *how* each one is built or where model facts live:

- Only two of five families have retry and timeout knobs.
- Three key paths exist, and one of them ignores its own `api_key_env`.
- There are three sources for context and capability: `ProviderModel`, the `Caps.max_context` literals, and `ADAPTIVE_THINKING_PREFIXES`.
- Every wire maps effort ad hoc.

Routing (`Router::pick`) and costing (`Priced`) are already single and stay that way.

**Target.**

1. **One transport descriptor per section.** Every `[providers.*]` table, native or compatible, flattens the same `Transport { base_url, api_key_env, timeout_s, max_retries }`. Section-specific knobs stay beside it, for example Anthropic's `cache_ttl` and the `api` shape. Every constructor takes a `&Transport`. `backend_for` becomes one lookup from the `api` shape to a constructor, not one arm per family.

   Implemented (T30.22): config side. `cox_protocol::config::Transport` is the one type; every section (`AnthropicProviderConfig`, `OpenAiProviderConfig`, `LocalProviderConfig`, `CompatibleProviderConfig`) exposes it via a `transport()` accessor rather than `#[serde(flatten)]` — serde refuses to combine `flatten` with `deny_unknown_fields` on the same struct, and keeping the four fields flat on each section (`base_url = …` stays a section-level TOML key) preserves the typo check. `openai`, `local` and every compatible section gained `timeout_s`/`max_retries` (matching `retry::Policy::default()`'s `max_retries: 4` and Anthropic's `timeout_s: 120` convention — neither is wired into a client yet, so behaviour is unchanged); `local` also gained `api_key_env` (default empty = no key; LM Studio's `LM_API_TOKEN` will use it, T30.15).

   Implemented (T30.23): constructors. `AnthropicProvider::new`, `OpenAiChatProvider::new` and `OpenAiResponsesProvider::new` all take `&Transport` (plus their own section-specific knobs — Anthropic's `ttl`/`fallbacks`) instead of five-odd positional scalars; `client_with_timeout` in `cox-provider/src/http.rs` is the one place that builds a `reqwest::Client` with a bounded connect timeout and `transport.timeout_s` as the read/idle timeout, reused by Anthropic (which used to duplicate it), Chat and Responses (which used to build an untimed `reqwest::Client::new()`); every backend's retry policy reads `transport.max_retries` instead of `Policy::default()`. `backend_for` in `crates/cox/src/session.rs` is one lookup: Anthropic keeps its own arm (its extra knobs), and `openai`/`local`/every compatible section go through one `openai_shaped(owner, &Transport, models, context_window, api)` that resolves the key once and picks the Chat or Responses constructor by `api`. `local`'s `timeout_s` default is 600, not the 120 every remote section keeps — slow on-device prefill can otherwise cut a large prompt off before the first byte.
2. **One key resolver.** `http::resolve_key(section)` reads the section's `api_key_env`, then the keyring `cox/<section>`, for every section. A local server needs no key: a missing key there is "no auth header", not an error. That is what LM Studio needs (T30.15).

   Implemented (T30.21): `http::resolve_key(api_key_env, section)` in `cox-provider/src/http.rs` is the one resolver every section goes through. Anthropic, which always needs a key, propagates its `Err` as `ProviderError::Auth`; `openai`, `local` and every compatible section build with an `Option<String>` key and call `.ok()`, so a missing key there is "no `Authorization` header", not a startup failure.

   Implemented (T30.28): `resolve_key`'s keyring fallback is the real platform store (macOS Keychain, Secret Service, Windows Credential Manager), so no test may call it. Tests inject the lookup instead of reaching it: `resolve_key_with(env_var, section, lookup)` (`pub(crate)`, the Anthropic tests use it directly); `doctor`'s `check_api_keys_with(config, resolve)`; and `crates/cox/src/session.rs`'s `provider_for_with`/`backend_for_with(config, resolve)`, which thread the same injected resolver through `openai_shaped` and through `AnthropicProvider::with_key` (the already-resolved-key constructor) instead of ever calling `AnthropicProvider::new`, which resolves internally. `provider_for`/`backend_for` pass the real `cox_provider::http::resolve_key`. A source-scan test (`crates/cox/tests/it/no_real_keychain_in_tests.rs`) fails the build if a `#[cfg(test)]` module, or any file under a `tests/` directory, still calls `resolve_key(`, `platform_keyring` or `keyring::Entry` (A49); every e2e test that spawns the real binary sets `COX_PROVIDER=scripted`/`replay`, which short-circuits provider construction before it would ever resolve a key.

   Implemented (T30.29): a switch as well as the seams. `COX_KEYRING=off` (also `0` or `false`; `cox_protocol::config::keyring_enabled`) makes `platform_keyring` return nothing and the MCP OAuth store (`cox_mcp::auth`) read nothing and refuse writes, so the binary never shows a keychain prompt. The repository's `.cargo/config.toml` sets it for everything cargo runs (`cargo run`, `cargo test`, `cargo nextest`), and a test (`tests_run_with_the_keyring_switched_off`) fails if that stops applying. The config env layer ignores the name. An installed `cox` never sees the switch unless the user sets it, and a value already in the shell wins over cargo's, so `COX_KEYRING=on cargo run` still reaches the keyring on purpose.
3. **One model catalog** in a new pure crate `cox-models` (see `crates.md`).
   - Each row: `id → context_window, max_output, efforts, capabilities (tools, adaptive_thinking, reasoning_effort_param, images), price`.
   - Built-in rows are embedded, the way `prices.toml` is today, and written only by T30.20's script from models.dev (A48). `[providers.<name>].models` entries and a user `prices.toml` override them by id.
   - `Caps` is derived from the catalog. The literals `200_000`, `128_000` and `400_000` and the prefix table are deleted.
   - A local server's loaded context (T30.16) is one more override source.

   Implemented (T30.24): `cox-models` (`crates/cox-models`) holds `Catalog::builtin()`/`Catalog::load(config, user_prices_toml)`, merging `ModelRow { id, context_window, max_output, efforts, capabilities, price }` in the order above — `Capabilities` and `max_output` are `Option`/absent everywhere today because no data source emits them yet (T30.25/T30.26 are the first readers). `Price`/`PriceError`/`PriceTable` moved here too, unchanged; `cox-provider::usage` keeps only `load_price_table` (the one place that still reads a price file from disk, since this crate does no I/O), `ledger_row` and `Priced`, which prices calls through the same `PriceTable` as before (no cost-number change). `crates/cox/tests/deps.rs`: `cox-models` depends only on `cox-protocol`; `cox-provider` may additionally depend on `cox-models`; `cox-core` may too (not yet used). `Caps`/adaptive-thinking derivation from `ModelRow` and the doctor sync row are still T30.25/T30.27.

   Implemented (T30.25): the three `Caps.max_context` literals are gone. `AnthropicProvider` gains a `max_context: u32` field — `capabilities()` carries no per-request model (plan.md §1.2), so the value is resolved once, at construction, by the caller. `backend_for_with` (`crates/cox/src/session.rs`) builds one `cox_models::Catalog::load(config, None)` per session and looks up `catalog.get(&config.tiers.code.model)`/`catalog.get(&t.model)` for the Anthropic arm, falling back to the pre-T30.25 literal (200 000/128 000) when the catalog has no row for that model — the same shape `openai_shaped`'s `context_window` parameter already used, now also covering the native `openai` section's former `400_000` fallback. `Catalog::load` always carries the embedded built-in rows (unlike reading `providers.*.models` off a bare `Config` directly, which is empty until something overlays it), so a built-in model's real window (e.g. `claude-sonnet-5`'s 1M) surfaces without any user config, and a `[[providers.anthropic.models]]` entry overriding a model's `context_window` now actually reaches `Caps` instead of being ignored. `ADAPTIVE_THINKING_PREFIXES` moved from `cox-provider::anthropic::request` to `cox_models::supports_adaptive_thinking` (`crates/cox-models/src/catalog.rs`) verbatim — same list, same prefix match — rather than becoming a `Capabilities.adaptive_thinking` row: that field stays `None` on every row (the vendor pipeline still doesn't emit it from models.dev's `reasoning_options`), and a row-keyed lookup would stop matching a model id that names no catalog row at all (a preview/custom variant the prefix table has always matched by name alone). `request.rs`'s `build_body` now calls `cox_models::supports_adaptive_thinking(&req.model.0)` at request-build time, so it still needs no `Config`/`Catalog` and stays snapshot-tested with no key and no socket. models.dev's `reasoning_options` shape (`budget_tokens` vs `effort`/`toggle`, already distinguished by `scripts/vendor/src/cox_vendor/models.py::cox_effort_for`) is a plausible future source for a real `Capabilities.adaptive_thinking`, but wiring it needs verification against live registry data and Python test coverage — out of scope for this Rust-only card.
4. **One effort map.** The mapping `effort_for(api, Effort, &caps) -> Option<WireEffort>` lives in `cox-models`, next to the catalog.
   - Anthropic: `output_config.effort` plus adaptive thinking when the catalog says so.
   - Responses: `reasoning.effort`.
   - Chat: the `reasoning_effort` field when the row declares it. Whether the OpenAI Chat API and LM Studio accept it is checked against their API references in that card.

   `clamp_effort` keeps enforcing the model's supported levels, now from the catalog. `Effort` gains `Medium` so models.dev's four levels map without loss.

   Implemented (T30.26): `cox_models::effort_for(Api, Effort, &Capabilities) -> Option<WireEffort { effort, adaptive_thinking }>` (`crates/cox-models/src/effort.rs`) is the one mapping; each wire only converts the returned `Effort` into its own generated enum.

   | `Api` | Sends | Condition |
   |---|---|---|
   | `Anthropic` | `output_config.effort`; `thinking: adaptive` when the tier asks for thinking | always; adaptive thinking when `caps.adaptive_thinking == Some(true)`, which `request.rs` fills from `supports_adaptive_thinking(model)` (the T30.25 prefix rule) |
   | `Responses` | `reasoning.effort` | always |
   | `Chat` | top-level `reasoning_effort` | only when `caps.reasoning_effort_param == Some(true)` |

   A Chat row declares the field with `reasoning_effort = true` on its `[providers.<name>].models` entry (`ProviderModel.reasoning_effort`, read through `Capabilities::declared_by` by both the catalog merge and `OpenAiChatProvider`). No built-in row sets it, so every Chat request is byte-identical to before: OpenAI's Chat API documents `reasoning_effort`, but LM Studio's compatible endpoint does not list it (R§4.3.3), so the field is opt-in per model, not per wire. `Effort::Medium` sits between `Low` and `High` and goes out as `medium` on all three wires that send an effort; `/effort medium` accepts it. `cox-vendor models` now maps models.dev's `medium` to `medium` (no longer `high`); the vendored `default.toml` rows still list the old three-level sets until the script is re-run, and until then `clamp_effort` lowers a `medium` request on such a row to `low` (never up). `clamp_effort` still reads the section's `models` list (the same entries the catalog overlays), not `Catalog`.

   Image input (T37.6) follows the same rule: `Provider::accepts_images(model)` is `true` on the Anthropic Messages and OpenAI Responses wires, and on the Chat wire only for a `models` entry with `images = true` (`ProviderModel.images`, read through `Capabilities::declared_by`). `cox-core` sends an attached png/jpeg/gif/webp as `Content::Image` only where it is accepted, sends any other UTF-8 file as a tagged text block, and holds back the rest with a warning notice.
5. **One sync check.** `cox doctor` reports a routable model with no catalog price, instead of relying only on the unit test.

   Implemented (T30.27): `Config::configured_model_ids` (`crates/cox-protocol/src/config.rs`) is the one enumeration of "every model reachable without touching a price file" — each `[tiers.*].model`, `providers.local.model` (and the legacy `providers.typesafe.model` tombstone when set), every native and compatible section's `models` list, and every `[providers.<custom>]` section's own default `model` — extracted from `cox_models::price`'s `usage_prices_cover_every_configured_model` test so `cox doctor`'s new `catalog prices` row (`crates/cox/src/doctor.rs`) and that test can never disagree about what "configured" means. The row builds a `cox_models::Catalog::load(config, None)` and warns, naming every unpriced id and pointing at `uv run --project scripts/vendor cox-vendor models`, when one has no `ModelRow.price`; on `default.toml` the row is `ok`, since `usage_prices_cover_every_configured_model` already guarantees every built-in id is priced.

**Kept, with reasons.**

- `ModelId` stays a plain string: gateway ids pass through verbatim, and nothing needs to parse `vendor/model`.
- `--provider`/`--model` keep retargeting only the `code` tier; `--tier` covers the others.
- Custom providers keep `ProviderId::Local`, so storage needs no migration.
- `cache_write_tokens = 0` on Chat and Responses is correct, because those APIs bill no cache writes.

**Migration order** (each step green; cards T30.21–T30.27 in `plan.md`, U*n* = T30.*(20+n)*):

| Card | Change | Files |
|---|---|---|
| U1 | One key resolver for every section | `http.rs`, `anthropic/mod.rs`, `session.rs` |
| U2 | `Transport` flattened into every section; schema regenerated | `config.rs`, schema, `config_load.rs` |
| U3 | Constructors take `&Transport`; Chat and Responses get configured retries | `session.rs`, `chat.rs`, `responses.rs` |
| U4 | `cox-models` crate: catalog types, embedded rows, merge with config and `prices.toml` | new crate, `usage.rs` |
| U5 | `Caps` and adaptive thinking from the catalog; delete the literals and the prefix table | `anthropic/*`, `session.rs` |
| U6 | `effort_for` in one place; `Effort::Medium` | `cox-models`, `request.rs`, `responses.rs` |
| U7 | `cox doctor` catalog/price sync row | `doctor.rs` |

T30.15 (LM Studio chat) waits for U1–U3 (T30.21–T30.23), so it lands as one more `Transport` section and not a sixth divergent one. T30.16 (loaded context) waits for U4–U5 (T30.24–T30.25), so the server's context feeds the catalog.

Implemented (T30.15): `[providers.lmstudio]` (`LmStudioProviderConfig`) is that sixth native section — its own `Transport`, no `models` list (the running server serves one model, normally pinned through `tiers.code.model`/`--tier code=<model>` rather than a section default). `backend_for_with`'s new `lmstudio` arm builds it through the *same* `AnthropicProvider` the `anthropic` arm uses, not a new client: LM Studio's native `/api/v1/chat` takes no custom tool schemas, and cox's OpenAI Chat path drops tool calls, so `/v1/messages` stays the one working chat transport (R§4.3.2). `AnthropicProvider::with_key` now takes `api_key: Option<String>`, and `headers()` sends `x-api-key` only when one is resolved — what LM Studio needs when "Require Authentication" is off; a resolved key (`LM_API_TOKEN`, or the keyring entry `cox/lmstudio` — never the Anthropic one, since this resolves under its own section name) goes out the same header. `Router::pick` and `AnthropicProvider::id()` both report `ProviderId::Anthropic` for this section, the same "wire family, not vendor" bucketing `ProviderId::Local` already uses for compatible sections, so the ledger row and the route label agree. `context_window = 0` (the default) means "ask the server"; until T30.16 wires that up, a `backend_for_with` lookup falls back to the model catalog, then a 32 768 floor.

Implemented (T30.16): `context_window = 0` now asks the server. `cox-provider::lmstudio` holds hand-written serde types for the subset of LM Studio's native `GET /api/v1/models` and `POST /api/v1/models/load` cox reads (no Rust SDK, no spec; unknown fields ignored; shape checked against `fixtures/lmstudio/models.json`, a live capture) and a small client whose `prepare` reads the model's entry and, with `[providers.lmstudio] load = true`, loads an unloaded model with `context_window` as `context_length`, then reads the allocated context back. `session::open` (in `crates/cox`, not the core) runs it before building the provider; `Catalog::overlay_served` makes the loaded context and `trained_for_tool_use` the last catalog layer, so the `lmstudio` arm's lookup order is: configured `context_window` → served loaded context → catalog row → 32 768. A model not trained for tool use, or not loaded, raises one `Notice(Warn)` each through `Session::notice`; an unreachable server fails session open as a transport error. `cox doctor` adds an LM Studio row (reachable, loaded, loaded vs max context, tool use) when it is the code tier's provider. `cox acp` still builds its provider synchronously inside the ACP runtime and keeps the catalog fallback.
