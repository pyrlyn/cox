# Writing a cox plugin in Rust

A cox plugin is a WebAssembly module plus a `plugin.toml` manifest. cox loads it with extism, gives it only the capabilities the user approved, and calls the exports it finds. This guide covers the Rust side: the `cox-plugin-sdk` crate in `plugins/sdk`. The design, including why each rule exists, is `docs/design/plugins.md` (PL); the ABI is its §4.

This page is for writing a plugin. To install and use one that already ships in this repository, see its own guide, e.g. `docs/plugins/cursor.md` for the Cursor external agent.

The API has [shared](#shared), [terminal-only](#terminal-only), and [desktop-only](#desktop-only) parts. The [field index](#api-field-index) lists every manifest and ABI field under its part.

## Setup

The Rust toolchain and its `wasm32-unknown-unknown` target come from the repository's `mise.toml`. `mise install` adds the target if it is missing.

```toml
# Cargo.toml
[package]
name = "turn-counter"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["cdylib"]

[dependencies]
# Not on crates.io yet: a path inside this repository, or
# { git = "https://github.com/pyrlyn/cox" } from outside it.
cox-plugin-sdk = { path = "../../sdk" }
```

```bash
mise exec -- cargo build --release --target wasm32-unknown-unknown
# → target/wasm32-unknown-unknown/release/turn_counter.wasm
```

Copy the `.wasm` next to `plugin.toml` under the name its `wasm` key gives. The manifest's schema is `docs/plugin.schema.json`; PL§2 shows every key.

## Exports

A plugin is a set of handler functions and one `register!` call. `init` is required and comes first. Every other export is optional and exists only if you list it, because cox probes exports by name and treats a missing one as "not provided".

```rust
use cox_plugin_sdk::*;

fn init(_: InitIn) -> Result<InitOut, SdkError> {
    Ok(InitOut {
        commands: vec![CommandDecl { name: "reset".into(), description: "Reset the counter".into() }],
        subscribe: vec!["turn_started".into()],
        ..InitOut::default()
    })
}

fn on_event(batch: EventBatch) -> Result<Effects, SdkError> {
    let turns = kv_get("turns")?.and_then(|v| v.as_u64()).unwrap_or(0);
    kv_put("turns", &Value::from(turns + batch.events.len() as u64))?;
    Ok(Effects { redraw: true, ..Effects::default() })
}

fn command(_: CommandIn) -> Result<CommandOut, SdkError> {
    kv_delete("turns")?;
    Ok(CommandOut::Nothing)
}

cox_plugin_sdk::register!(init => init, on_event => on_event, command => command);
```

The complete version is `plugins/examples/rust`, the reference plugin PL§13 asks every language to implement: it adds a failure-counting hook and a status segment, and keeps its counters in memory as well as in kv, because `cox_render` may not call `kv_get`.

A handler returns `Result<output, E>` for any error type that implements `Display`. An `Err` fails that one call: cox shows the message as a warning and carries on (extensions fail open).

| `register!` key | Export | Input → output | Called |
| --- | --- | --- | --- |
| `init` | `cox_init` | `InitIn` → `InitOut` | once per session; `InitIn.config` is your `[plugins.<id>]` table from cox's config (`{}` when absent), which you validate yourself; contributions you were not granted are dropped with a notice |
| `on_event` | `cox_on_event` | `EventBatch` → `Effects` | for the events in `InitOut.subscribe` (PL§5) |
| `hook` | `cox_hook` | `HookCall` → `HookOutcome` as `Value` | for the hooks in `capabilities.hooks` (PL§6) |
| `decide` | `cox_decide` | `Question` → `Advice` | at the decision points in `capabilities.decide` (PL§6b) |
| `decide_call` | `cox_decide` | `Question` → `DecideOut` | instead of `decide`, when the answer needs a model call on your own `[[provider]]` section (T33.40.1) |
| `decide_resume` | `cox_decide_resume` | `DecideResume` → `Option<Advice>` | after that call, with its events |
| `tool_subject`, `tool_risk`, `tool_call` | `cox_tool_*` | `ToolCallIn` → `String`, `Risk` / `ToolOutput` as `Value` | for the tools in `InitOut.tools` (PL§7) |
| `provider_stream` | `cox_provider_stream` | `ProviderCall` → `Vec<ProviderEvent>` as `Vec<Value>` | for an `api = "plugin"` provider (PL§7a) |
| `command` | `cox_command` | `CommandIn` → `CommandOut` | `/<id>:<name>` on every surface (PL§8) |
| `key` | `cox_key` | `CommandIn` → `CommandOut` | terminal leader keys only (PL§15.3) |
| `render` | `cox_render` | `RenderIn` → `Widget` | for a status segment, panel or overlay (PL§8) |
| `render_item` | `cox_render_item` | `RenderItemIn` → `Option<Widget>` | for the targets in `ui.render` (PL§8) |
| `canvas` | `cox_canvas` | `CanvasIn` → `CanvasOut` | desktop only, while the canvas pane shows: when it opens, on `Effects.redraw`, after each of your own tools finishes, and when the pane's size changes enough to matter; within `min(limits.call_ms, 2000 ms)` |
| `canvas_select` | `cox_canvas_select` | `CanvasSelect` → `Option<CanvasAttach>` | on a click in one of your frames; within `min(limits.call_ms, 500 ms)` |
| `shutdown` | `cox_shutdown` | `()` → `()` | at session end, best effort |

Payloads that carry a cox protocol type (`Event`, `HookOutcome`, `ToolOutput`, `ProviderEvent`, `Request`) are `serde_json::Value`, because the SDK does not depend on the host crates. Their JSON shape is in `docs/protocol.jsonschema`; everything else is in `docs/plugin-abi.schema.json`.

## Host functions

Each wrapper returns `SdkResult<T>`. `SdkError::Host(AbiError)` means cox refused or failed the call: the capability is not granted, the call is not allowed from the current export, the permission engine or budget said no, a cap was hit, or the deadline passed. Refusals are values, not traps, so a plugin can degrade instead of failing its whole call.

| Function | Capability | Not allowed from | Returns |
| --- | --- | --- | --- |
| `log(level, text)` | — | — | `()`; goes to cox's log, rate-limited |
| `notify(level, text)` | — | `render` | `()`; a transcript notice, level at most `Warn`, sanitized |
| `kv_get(key)`, `kv_put(key, value)`, `kv_delete(key)` | `kv` | `render` | `Option<Value>` / `()`; within the store quota (64 KiB per value, 1 MiB per plugin, keys up to 256 bytes) |
| `context()` | `context` | — | the event-folded session snapshot: session id, cwd, tier and model, usage totals, the last 50 items with their text, the todo list, compactions; secret-shaped text redacted |
| `invoke_tool(name, input)` | `invoke` lists `name` | everything but `on_event`, `command`, `key`, `tool_call` | a `ToolOutput`; the call passes hooks, the permission engine and the sandbox like a model's call |
| `model_call(&ModelCall)` | `model` | `render` | `Vec<ProviderEvent>`; at or below the granted tier, budget-gated, recorded in the cost ledger |
| `http(&HttpReq)` | `net` lists the host | `render` | `HttpResp`; a `[[provider]]` host only from `provider_stream`, with cox adding the auth header |
| `output(line)`, `cancelled()` | — | everything but `tool_call` | `()` / `bool`: tool progress and cancellation |
| `redraw()` | any `ui` | — | `()`; marks your slots dirty |

`invoke_tool` is refused inside `hook`, `decide` and `provider_stream` because the agent loop is waiting on those calls, so a tool run from inside them would deadlock.

## Engine features

cox runs every plugin on one engine configuration; a module that needs a WebAssembly proposal outside it fails to load with a parse error.

| Feature | State | Why |
| --- | --- | --- |
| tail calls, function references, GC | on | extism 1.30.0 always enables them |
| exception handling (`try_table`, `throw`, tags) | on | extism's `wasmtime-exceptions` feature, turned on for the workspace (plan.md A61): Kotlin/Wasm output uses it unconditionally and does not parse without it (research.md R§4.3.5 P42–P43) |
| WASI preview 1 | per plugin | linked only for a manifest with `wasi = true` (a `wasip1` guest such as a TinyGo one) or a `[capabilities.fs]` grant; no preopens without the grant (T33.43, T33.14.2) |

The exception switch is workspace-wide: it is one flag on the engine, not a per-plugin capability, so any plugin may use exceptions. It adds guest-visible instructions only; the host still gives each plugin its own engine and store, the same memory cap and the same per-call deadline. RUSTSEC-2026-0222 names exception-related embedder APIs (`ExnRefPre::new`, `Tag::new`), but it needs the host to move objects between engines, which neither extism nor cox does, so enabling the proposal does not make it reachable from a guest (`deny.toml`). The exception-handling support in wasmtime 43 itself has not been reviewed beyond that advisory; revisit it with the T33.43 bump.

## The wire, for other languages

The SDK is a thin layer over one rule, which a guest in any language can follow with its own extism PDK:

- An export reads its input as one JSON value (an empty input means `null`) and sets one JSON value as its output. A failure sets the extism error text and returns 1.
- A host function lives in the import module `cox:host/v1` and has the signature `(u64) -> u64`: it takes the offset of an extism memory block holding one JSON value and returns the offset of a block it allocated. The argument is the function's only argument, an object keyed by its argument names when it has several (`{"level": "warn", "text": "…"}`, `{"key": "turns", "value": 3}`, `{"name": "read", "input": {…}}`), or `null` when it has none. The reply is `{"Ok": value}` or `{"Err": AbiError}`.

## Go

`plugins/sdk-go` (module `github.com/pyrlyn/cox/plugins/sdk-go`) is the same API over `github.com/extism/go-pdk` v1.1.3. Go and TinyGo come from `plugins/mise.toml`. A Go plugin is a TinyGo `wasip1` module, so its `plugin.toml` sets `wasi = true`:

```bash
tinygo build -o plugin.wasm -target wasip1 -buildmode=c-shared .
```

Each export is a `//go:wasmexport` function whose whole body is `cox.Export(handler)`, where the handler maps the export's input type to its output type and an `error`. `cox_init` is required; cox probes the rest by name. The host functions are `cox.Log`, `Notify`, `KVGet`, `KVPut`, `KVDelete`, `Context`, `InvokeTool`, `ModelCall`, `HTTP`, `Output`, `Cancelled`, `Redraw` and `DesktopNotify`, with the capabilities and contexts in the table above. A refusal is a `*cox.HostError` whose `Kind` is the `AbiError` tag. `types.go` has the common payloads; a `json.RawMessage` carries any other, with its shape in `docs/plugin-abi.schema.json`.

```go
//go:wasmexport cox_init
func coxInit() int32 {
	return cox.Export(func(cox.InitIn) (cox.InitOut, error) {
		return cox.InitOut{Subscribe: []string{"turn_started"}}, nil
	})
}
```

TinyGo is not gc: it evaluates `return n, json.Unmarshal(raw, &n)` with `n` read before the call, so take the error into a variable first. Run a package's tests under `tinygo test` as well as `go test`.

The complete version is `plugins/examples/go`; `cox plugin new <name> --lang go` scaffolds a package with a `justfile` and a smoke test. `just plugin-examples go` builds the example and runs its end-to-end tests.

## Kotlin

`plugins/sdk-kotlin` is cox's own PDK over extism's raw `extism:host/env` imports and `cox:host/v1`, because no maintained Kotlin extism PDK exists. It is not published: a plugin takes it through a Gradle composite build (`includeBuild`) of a cox checkout's `plugins/sdk-kotlin`. Java, Gradle and Kotlin come from `plugins/mise.toml`. A Kotlin plugin is a Kotlin/Wasm `wasmWasi` module, and Kotlin's stdlib imports WASI's `random_get`, so its `plugin.toml` sets `wasi = true`.

Each export is a `@WasmExport` function whose whole body is `Cox.export { input -> … }`: the handler takes the export's input as a `JsonElement` and returns its output as one, with the shapes in `docs/plugin-abi.schema.json`. `cox_init` is required; cox probes the rest by name. The host functions are `Cox.log`, `notify`, `kvGet`, `kvPut`, `kvDelete`, `context`, `invokeTool`, `modelCall`, `http`, `output`, `cancelled` and `redraw`. A refusal throws `HostException`, whose `error` is the `AbiError`.

```kotlin
@WasmExport
fun cox_init(): Int = Cox.export {
    buildJsonObject { putJsonArray("subscribe") { add("turn_started") } }
}
```

The complete version is `plugins/examples/kotlin`; `gradle pluginPackage` stages `plugin.toml` and the module in `build/plugin`, which is what `cox plugin install` takes, because it copies a whole directory. `cox plugin new <name> --lang kotlin` scaffolds a package with a `justfile` whose `smoke` recipe installs it into a scratch `COX_HOME`. `just plugin-examples kotlin` builds the example and runs its end-to-end tests.

## Shared

The manifest's package identity, resource limits, providers, models, agents, MCP servers, language servers, debug adapters, and shared capabilities apply wherever the plugin loads. `surfaces` optionally narrows loading to `terminal`, `desktop`, `headless`, or `acp`; when absent, it permits all four. `InitIn.surface` tells the guest which one opened the session. `cox:host/v1`, the exports above except `cox_key`, commands, renderers, and the `status.left`, `status.right`, `panel`, and `overlay` slots use this part. Headless and ACP sessions have no UI, so they omit UI grants. `[[skills]]` entries (`name`, `dir`) ship Agent Skills: `dir` is a package folder named for the skill that holds a `SKILL.md` in the `.cox/skills` format. A granted plugin's skills join the session's skill index, and a user or project skill of the same name wins. `[[language_servers]]` and `[[debug_adapters]]` declare a process the host spawns under the session sandbox (the plugin never spawns); a package of only `[[mcp]]`, `[[language_servers]]`, `[[debug_adapters]]`, `[[agents]]` and `[[skills]]` needs no `wasm`. `Span.link` accepts an HTTPS URL or workspace-relative path and opens only on a person's click; unsupported links remain text.

## Terminal-only

`[capabilities.terminal] keys = true` grants leader-key bindings declared in `InitOut.keys` (`KeyDecl`) and handled by `cox_key`. The older `capabilities.ui.keys` spelling is an `api = 1` alias. `cox:tui/v1` is reserved and currently has no functions. A desktop session drops returned keys with one informational notice. `cox --plain` maps to the terminal surface but draws no slots.

## Desktop-only

`[capabilities.desktop]` accepts `inspector`, `toolbar`, `palette`, `notify`, and `canvas`. `InitOut.desktop.inspector` declares one `TabDecl`; `cox_render` fills its `desktop.inspector` slot. `toolbar` and `palette` hold `ActionDecl` values whose `command` must name a declared command, run through `cox_command`. `CommandOut.open_inspector` selects the tab. `cox:desktop/v1` exposes `cox_desktop_notify` with a `DesktopNotice` and at most two `NoticeAction` values. `Widget.image` names a package-relative PNG (at most 256 KiB); the terminal shows its `alt` text. A desktop-only host call on another surface returns `AbiError.not_on_this_surface`. Contributions returned on an unavailable surface are dropped with one informational notice, and a plugin excluded by `surfaces` is skipped with a notice. One digest grant covers every surface.

### Canvas

`canvas = true` adds a pane of pages you draw beside the transcript. `InitOut.desktop.canvas` declares one `CanvasDecl`: a `title`, an SF Symbol, and `select_attr`, a `data-` attribute of 1–48 characters from `[a-z0-9-]`. `cox_canvas` returns the frames, each a static HTML page with an `id`, a `title` and a `width_pt` (clamped to 200–2560); the pane shows them side by side, each as tall as its content, and the person can zoom. The grant asks for `desktop.canvas` on its own line, even when the plugin's other desktop parts were granted before, because the pane shows the plugin's own HTML. Only the person opens the pane, with ⌘⇧D or the palette's "Show <title>"; nothing a plugin returns can open it.

Caps: at most 12 frames, 1 MiB of HTML in a frame and 4 MiB in one call; a frame `id` matches `^[a-z0-9_.-]{1,64}$` and is unique. A frame over a cap is dropped with a warning. Three missed deadlines in a row stop the canvas for the session; the last frames stay. Both exports run in render context: no kv writes, no notices, no tools. cox compares each frame's HTML with the last one and reloads only the frames that changed.

What a page can do: lay itself out with HTML, inline `<style>` and `style` attributes, and inline SVG; follow the app's light or dark appearance through `prefers-color-scheme`; mark the elements a click may select with `select_attr`. What it cannot do:

- run script: page JavaScript is off, so `<script>` and `on…` attributes do nothing;
- load anything: images, fonts, style sheets and frames are blocked, from the network, from `data:` URLs and from the page's own scheme alike (a content rule list and a `default-src 'none'` Content-Security-Policy);
- navigate: links and forms do nothing;
- keep state: each frame has a non-persistent store.

A click runs the app's own script, which lives in a content world the page cannot reach: it outlines the nearest element that carries `select_attr` and reports its value. cox drops a value over 256 characters or with control characters, then calls `cox_canvas_select` with the frame's `id` and the value. A returned `CanvasAttach` (`name` up to 80 characters, `text` up to 8 KiB, both sanitized) becomes a `text/plain` chip in the composer, which the person sends with their next message or removes; the next click on the same canvas replaces it. `None` adds nothing. Nothing from the canvas reaches the model unless the person sends it. The design is `docs/design/plugin-canvas.md`.

`plugins/examples/surfaces` (Glance) is a working example: it draws two frames, its glance count and its desktop parts, marks each part with `data-glance`, and turns a click into a note that names the part.

## Cloud agents

A `[[cloud_agents]]` entry runs a background task on a hosted cloud agent. The host drives it: no export of the package is called for it. The package still names a `wasm` file, as `[[external_agents]]` does. The one backend is `cursor`, and no cloud plugin ships in this repository; the offline tests use a fixture package.

```toml
# plugin.toml
api = 1
id = "cloud-bg"
version = "0.1.0"
name = "Cloud background"
wasm = "plugin.wasm"

[[cloud_agents]]
name = "cloud"                  # agent(preset: "cloud")
backend = "cursor"              # closed set; its host is fixed
key_env = "CURSOR_API_KEY"      # the name of an env var, never the key
model = "composer-2"            # optional; the backend's default when absent
description = "Long background runs on Cursor's servers"
```

The fields are `name` (the `agent(preset: "<name>")` dispatch name), `backend`, `key_env` (default `CURSOR_API_KEY`; a value is refused), and the optional `model` and `description` (shown at approval). There is no URL field, and an unknown key such as `url` is refused, so the key cannot be sent to another server.

`cox plugin install` asks for one grant per entry. The line names the backend, the host, the key variable and the model:

```text
cloud-agent:cloud cursor host=api.cursor.com key=CURSOR_API_KEY model=composer-2; sends the prompt and the repository's GitHub URL; the service clones the repository and pushes a branch on its servers, billed to your plan
```

A changed entry asks for the grant again. The key is read by name from the environment, then the platform keyring, as for any provider. Only a dashboard-issued key works.

### What leaves the machine

Each run sends only these:

- the prompt that the model passed to `agent(preset)`;
- the repository's GitHub URL, rebuilt from the `origin` owner and name (credentials, a port, a path or a query in the remote are dropped); a remote on any other host is refused before any request;
- the starting ref: the current branch, or the `HEAD` commit when detached;
- the model id, when one is set;
- the key, as a bearer token, to `api.cursor.com` only.

cox uploads no other file. Cursor clones the repository from GitHub, so commits that are not on `origin` are not seen. When local `HEAD` is not on `origin` (judged from the last fetch), the answer starts with a warning. The requests carry no git name or e-mail. The code the run reads and edits runs on Cursor's servers, outside cox's sandbox and path checks.

### Permission

Every run needs an allow decision for `CloudAgent(<owner>/<repo>)`. Put an exact entry in your own config, for example `allow = ["CloudAgent(acme/widgets)"]` under `[permissions]`; names match ignoring case. A bare `CloudAgent` allow, a session grant or a repository's own project config does not count. Without the entry, with a matching `deny`, or in plan mode, the run is refused before any request, and the refusal names the rule to add. A background task has no approval prompt, so the question is never asked there.

### Running and ending

`agent(preset: "<name>")` always runs in the background, even without `background`. The run appears as a background task whose result is the run's answer, with `thinking` text left out. Cancelling the task cancels the run on Cursor's side. If cox closes before a run ends, the run keeps going; when the session is resumed, cox follows the run again or finalizes it and adds its result to the task.

Each run's tokens, as Cursor reports them, are recorded in the usage ledger at $0 with `billed_externally` set. cox does not price them: the spend is on your Cursor plan.

### Limits

- A follow-up message into a run is not sent. The client can build one, but no driver uses it.
- A background task cannot show an approval prompt. It is refused with the rule to add instead.
- A run reattached after a resume cannot be cancelled.
- A refused run's finish notice can still say that the subagent produced no answer. The task's error carries the refusal reason (T56.12 is open).
- Only Cursor, and only GitHub-hosted repositories.
- The client is tested offline against hand-written fixtures. It has not yet been checked against a live Cursor account (T56.10).

<!-- plugin-api:start -->
## API field index

Generated from [manifest](plugin.schema.json) and [ABI](plugin-abi.schema.json) schemas by `scripts/plugin_api_reference.py`. Each entry lists a type's fields and enum cases; referenced types have their own entries. Run `python3 scripts/plugin_api_reference.py --write` after an intentional schema change.

### Shared

- manifest `PluginManifest`: `api`, `surfaces`, `id`, `version`, `name`, `description`, `wasm`, `wasi`, `limits`, `capabilities`, `provider`, `models`, `mcp`, `language_servers`, `debug_adapters`, `external_agents`, `cloud_agents`, `agents`, `skills`
- manifest `Surface`: `terminal`, `desktop`, `headless`, `acp`
- manifest `Limits`: `memory_mib`, `call_ms`
- manifest `Capabilities`: `events`, `hooks`, `tools`, `invoke`, `context`, `kv`, `model`, `net`, `fs`, `decide`, `ui`, `ide`
- manifest `ModelTier`: `cheap`, `code`
- manifest `FsCaps`: `read`, `write`
- manifest `UiCaps`: `status`, `panel`, `overlay`, `commands`, `render`
- manifest `IdeCaps`: `buffer`, `lsp`, `dap`, `index`, `terminal`, `git`
- manifest `BufferAccess`: `read`, `edit`
- manifest `GitAccess`: `read`, `write`
- manifest `ProviderDecl`: `name`, `api`, `base_url`, `api_key_env`, `auth`
- manifest `ProviderApi`: `chat`, `responses`, `plugin`
- manifest `ProviderAuth`: `bearer`, `x-api-key`, `none`
- manifest `ModelDecl`: `id`, `provider`, `context_window`, `max_output`, `price`
- manifest `PriceDecl`: `input`, `output`, `cache_read`, `cache_write`
- manifest `McpDecl`: `name`, `command`, `args`, `url`
- manifest `LanguageServerDecl`: `name`, `languages`, `globs`, `command`, `args`, `env`, `config`
- manifest `DebugAdapterDecl`: `name`, `languages`, `command`, `args`, `env`, `config`
- manifest `ExternalAgentDecl`: `name`, `command`, `args`, `mode`, `key_env`
- manifest `AgentMode`: `acp`, `stream-json`
- manifest `CloudAgentDecl`: `name`, `backend`, `key_env`, `model`, `description`
- manifest `CloudBackend`: `cursor`
- manifest `AgentDecl`: `name`, `file`
- manifest `SkillDecl`: `name`, `dir`
- ABI `InitIn`: `api`, `plugin_id`, `config`, `session`, `granted`, `surface`
- ABI `SessionInfo`: `id`, `cwd`
- ABI `Surface`: `terminal`, `desktop`, `headless`, `acp`
- ABI `InitOut`: `tools`, `commands`, `status`, `panels`, `renderers`, `subscribe`
- ABI `CommandDecl`: `name`, `description`
- ABI `Slot`: `status.left`, `status.right`, `panel`, `overlay`
- ABI `CanvasDecl`: `title`, `symbol`, `select_attr`
- ABI `EventBatch`: `first_seq`, `dropped`, `events`
- ABI `Effects`: `redraw`, `notices`
- ABI `PluginNotice`: `level`, `text`
- ABI `NoticeLevel`: `info`, `warn`
- ABI `HookCall`: `event`, `payload`
- ABI `ToolCallIn`: `name`, `input`
- ABI `CommandIn`: `name`, `args`
- ABI `CommandOut`: `prompt`, `prompt.text`, `prompt.kind`, `compact`, `compact.focus`, `compact.kind`, `toggle_panel`, `toggle_panel.kind`, `open_overlay`, `open_overlay.kind`, `notice`, `notice.kind`, `nothing`, `nothing.kind`
- ABI `RenderIn`: `slot`, `width`, `height`
- ABI `RenderItemIn`: `target`, `call`, `result`, `width`
- ABI `ProviderCall`: `request`, `model`, `base_url`
- ABI `ModelCall`: `request`, `tier`, `own_provider`
- ABI `ModelTier`: `cheap`, `code`
- ABI `OwnProvider`: `name`, `model`
- ABI `HttpReq`: `method`, `url`, `headers`, `body`
- ABI `HttpResp`: `status`, `headers`, `body`
- ABI `Question`: `point`, `state`, `options`, `items`
- ABI `DecidePoint`: `route`, `risk`, `approve_hint`, `compact`, `rank`, `salience`
- ABI `QuestionItem`: `state`, `options`
- ABI `Advice`: `answer`, `confidence`, `note`
- ABI `Answer`: `choice`, `choice.order`, `choice.kind`, `score`, `score.value`, `score.kind`, `noul`, `noul.p_yes`, `noul.kind`, `scores`, `scores.values`, `scores.kind`, `items`, `items.answers`, `items.kind`
- ABI `DecideOut`: `call`
- ABI `DecideResume`: `question`, `events`
- ABI `AbiError`: `not_in_this_context`, `not_in_this_context.kind`, `not_on_this_surface`, `not_on_this_surface.surface`, `not_on_this_surface.kind`, `not_granted`, `not_granted.capability`, `not_granted.kind`, `denied`, `denied.reason`, `denied.kind`, `budget`, `budget.kind`, `too_large`, `too_large.limit`, `too_large.kind`, `timeout`, `timeout.kind`, `transient`, `transient.message`, `transient.kind`, `failed`, `failed.message`, `failed.kind`
- ABI `Widget`: `text`, `list`, `list.items`, `list.selected`, `table`, `table.header`, `table.rows`, `table.widths`, `key_value`, `gauge`, `gauge.ratio`, `gauge.label`, `stack`, `stack.vertical`, `stack.children`, `stack.sizes`, `block`, `block.title`, `block.child`
- ABI `Span`: `text`, `style`, `bold`, `italic`, `link`
- ABI `StyleToken`: `text`, `dim`, `accent`, `user`, `agent`, `tool`, `ok`, `warn`, `error`, `diff_add`, `diff_del`, `diff_hunk`, `border`, `selection`
- ABI `CanvasIn`: `width_pt`, `height_pt`
- ABI `CanvasOut`: `frames`
- ABI `Frame`: `id`, `title`, `width_pt`, `html`
- ABI `CanvasSelect`: `frame`, `value`
- ABI `CanvasAttach`: `name`, `text`

### Terminal-only

- manifest `Capabilities`: `terminal`
- manifest `UiCaps`: `keys`
- manifest `TerminalCaps`: `keys`
- ABI `InitOut`: `keys`
- ABI `KeyDecl`: `key`, `name`, `description`

### Desktop-only

- manifest `Capabilities`: `desktop`
- manifest `DesktopCaps`: `inspector`, `toolbar`, `palette`, `notify`, `canvas`
- ABI `InitOut`: `desktop`
- ABI `Slot`: `desktop.inspector`
- ABI `DesktopOut`: `inspector`, `toolbar`, `palette`, `canvas`
- ABI `TabDecl`: `title`, `symbol`
- ABI `ActionDecl`: `command`, `title`, `symbol`
- ABI `CommandOut`: `open_inspector`, `open_inspector.kind`
- ABI `Widget`: `image`, `image.path`, `image.alt`
- ABI `DesktopNotice`: `level`, `title`, `body`, `actions`
- ABI `NoticeAction`: `label`, `command`
<!-- plugin-api:end -->
