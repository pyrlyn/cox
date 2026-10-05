# Design: WASM plugins (A52, phase P33)

## Problem

v0.1 extends cox through data and processes: markdown, hook subprocesses and MCP servers (`extensions.md`). The creator has decided that v0.2 adds an in-process WASM host (extism). With it, one package can contribute hooks, tools, event listeners, TUI parts (status segments, panels, commands and keys, custom rendering), model providers, catalog rows, MCP servers and answers to the core's typed questions. No new cox release is needed for any of that.

The measurable question: can one plugin package add a status segment, a hook, a deferred tool and a provider, while

- §1.15 invariants 1, 8 and 10 (`prefix_bytes_identical_between_turns`, `every_request_has_a_usage_row`, `broken_hook_is_skipped_not_fatal`) stay green, and
- each enabled plugin adds at most 50 ms to a warm session start and 5 ms p95 to a render?

## The field

- **Zellij (R§4.3.5 P18–P19).** WASM plugins `subscribe` to event types and ask for permissions through `request_permission`, over 14 kinds; the answer arrives as an event. `update(Event) -> bool` decides whether `render(rows, cols)` runs. There is no render per frame. cox takes this redraw model as is.
- **Zed (P20).** An `extension.toml` manifest and Rust built for `wasm32-wasip2`. Capabilities such as `process:exec` and `download_file` are declared in the manifest, and the user narrows them with `granted_extension_capabilities`. cox takes the "declared in the manifest, narrowed by the user" shape. Zed still reaches agent tools through MCP.
- **Claude Code (P21, `extensions.md`).** "Plugins" are bundles of markdown, hooks, `.mcp.json` and `bin/`, with nothing in process. cox already reads all of those formats (D4). A cox plugin adds only what a bundle cannot do: in-process code with a typed ABI.
- **extism 1.30.0 (P1–P17).** Wasmtime 43 underneath. The host controls `allowed_hosts`, `allowed_paths` (preopens, `ro:` for read-only), `memory.max_pages`, `timeout_ms` and `config`. It also has host functions with `UserData`, a `CancelHandle` that works from another thread, and a SHA-256 check on module bytes. `Plugin` is `Send + Sync`, but `call` takes `&mut self`. The error type is anyhow. HTTP and file loading are default features that cox turns off.

## Decision

1. **One host crate owns extism.** `cox-plugin` holds extism with `default-features = false`, so there is no `ureq`, no URL loading and no file loading (P5). It depends on `cox-protocol`, `cox-plugin-api` and `cox-sanitize`, and implements the protocol traits (`Hook`, `Tool`, `Provider`, `EventTap`, `Advisor`).
2. **The core does not know plugins exist.** `cox-core` sees plugins only through traits in `cox-protocol` (D2), and `crates/cox` wires everything together.
3. **Plugin runs are recorded as ordinary events.** Everything a plugin does to a session appears as an event already in the rollout: a hook outcome, a tool result, provider events, a notice, or the new `Advised`. Replaying a session without the plugin therefore gives the same log (D12).
4. **Approval is per digest.** A plugin runs only under a grant that names its package digest and its capabilities. Changed bytes or wider capabilities ask again.
5. **Plugins get no new trust path.** Every tool call a plugin makes passes `cox_core::permission::Engine`. Every path passes `confine`, every process runs under `sandbox::Policy`, and every string that reaches the terminal passes `cox_sanitize`. No host function goes around any of these.
6. **Plugins fail open (D14).** A trap, a timeout or a malformed reply is a `Notice(Warn)` and an absence. Three failures in a row disable that export for the session, and `cox doctor` and `cox ext` report it.

## 1. Package, discovery and commands

A plugin is a directory. Its id is `^[a-z][a-z0-9-]{1,23}$` with no `__`.

```text
<id>/
  plugin.toml          manifest (§2)
  plugin.wasm          the module the manifest names
  bin/…                optional: executables an [[mcp]] entry names (§7c)
  README.md            optional
```

Locations:

- **User plugins** live in `~/.cox/plugins/<id>/versions/<digest12>/`. A `current` file names the active version, and at most one `previous` is kept (§1b).
- **Project plugins** live in `<git root>/.cox/plugins/<id>/` and are read in place.

If both locations have the same id, the user plugin wins and the project plugin is skipped with a notice. A repository must not shadow code the user installed.

**Project plugins are repository content, and repository content is untrusted (D14).** A project plugin never loads until the user enables it for that repository with `cox plugin enable <id> --project` or the TUI dialog. That grant is keyed on the plugin id, the repository root and the digest. This mirrors how a project `.mcp.json` must be approved. Cloning a repository never runs its plugins.

**Digest.** SHA-256 over the whole package tree: `(relative path, length, bytes)` for every file, sorted by path. It covers the manifest, the wasm and any `bin/` executable, so swapping a shipped MCP binary cannot avoid a new approval. `sha2` is already a dependency of `cox`, `cox-store` and `cox-provider`.

**CLI.** A new `cox plugin` group, because `cox ext` today has only `list` (`crates/cox/src/cli.rs:311-325`, `ext_cmd.rs:16,61`). `cox ext list` gains a *plugins* section so one command still lists every extension.

| Command | Effect |
| --- | --- |
| `cox plugin list [--json]` | discovered plugins, source, version, digest, grant state, disabled exports |
| `cox plugin install <dir>` | validate (§2), copy into `versions/<digest12>/`, record the source, then run `enable`'s approval |
| `cox plugin enable <id> [--project] [--yes]` | show the capabilities in words and ask. `--yes` is for a user's own provisioning script |
| `cox plugin disable <id>` | clear `enabled` on the grant row; files and data stay |
| `cox plugin update [<id>… \| --all] [--check] [--rollback]` | §1b |
| `cox plugin remove <id> [--keep-data] [--yes]` | §1c |
| `cox plugin new <name> [--lang …] [--dir …] [--with …]` | §13 |

The global switch is `plugins.enabled`, as a config key, an env var and `--no-plugins` (D13).

**Install sources: a local directory, an https URL, or a git repository (A121 §3, P53).** Fewer is better, so the added sources end in the same install this section already describes — validate the manifest, digest the tree, copy into `versions/<digest12>/`, ask for the grant — no second path, no plugin runs before its grant:

- **A local directory:** `cox plugin install <dir>`, as above. No network, no download UX, no trust-on-first-download; the dev loop (`new` → build → `install` or `link`) and every test.
- **An https URL:** `cox plugin install <https-url> --sha256 <hex>` fetches a `.tar.gz` archive. `--sha256` is required — a URL with none, `http://`, `file://` and every other scheme are all refused before a byte is fetched — and the download is refused the moment its digest fails to match, before anything is unpacked.
- **A git repository:** `cox plugin install git+<url> --rev <tag|commit> [--path <subdir>]`. `--rev` is required and must name a tag or a commit; a branch name is refused, so `update` (§1b) never follows a moving target silently. `--path` selects a subdirectory of the clone as the package root, confined inside the clone.

Both downloads land under `~/.cox/plugins/.staging/` first, removed on every exit — success, refusal or a crash — before anything moves into `versions/<digest12>/`. A downloaded archive or a cloned repository is repository content, and repository content is untrusted (D14): nothing in it runs during install, and any symlink or path entry that resolves outside the staging directory is refused, for the archive's entries and the clone's alike.

Git is shelled to exactly as `crates/cox-tools/src/git.rs` shells to it for the status line and worktrees (A13, `git_or_err`/`git`, `crates/cox-tools/src/git.rs:511-529`): `git` found on `PATH` through `Command::new("git")`, never linked in via `git2` or `gix`, and never read from a cloned repository's own config for how to run git itself. Install runs `git clone --depth 1 --no-recurse-submodules --branch <tag>` (or a fetch of a commit when `--rev` is not a tag) with `GIT_TERMINAL_PROMPT=0`, so a private repository fails instead of prompting, into the staging directory; the resolved commit is what gets recorded. Archive extraction reuses the `tar` shell-out `crates/cox/src/self_update.rs` already runs to unpack a release (`unpack_cox`, `std::process::Command::new("tar")`, `crates/cox/src/self_update.rs:118-155`) rather than adding a second extraction path.

`install` records the source it used:

- local: `{kind: "path", path, digest}`
- URL: `{kind: "url", url, sha256}`
- git: `{kind: "git", url, rev, commit, path}`

`update` (§1b) re-reads that recorded source, per kind: a local path is re-read from disk, as today; a URL source is re-fetched at the same URL and the same hash — a changed file at that URL is a hash mismatch, refused, never a silent update, and a new version is a new `install` with a new hash; a git source is re-fetched at the same tag or commit — a tag that now resolves to a different commit yields a new digest and asks for the grant again with the capability diff, exactly like a bytes-changed local update. `--check` (§1b) applies to all three: fetch, compute, print, change nothing.

No new dependency: the URL source reuses the `reqwest` client and the SHA-256 helper `crates/cox/src/self_update.rs` already has, archive extraction reuses its `tar` shell-out, and git is already shelled to the same way (A13).

### 1b. Update and rollback

`cox plugin update <id>` steps:

1. Re-read the recorded source.
2. Validate the manifest against the schema and the `api` version.
3. Compute the digest. If it equals `current`, stop and print "up to date".
4. Compute the capability diff against the stored grant.
5. `--check` prints steps 3–4 and changes nothing.
6. Otherwise, write the new tree to `versions/<new12>/.tmp`, fsync it, and rename it to `versions/<new12>/`.
7. Ask for a grant, showing the capability diff with added capabilities highlighted and removed ones listed. This covers both "bytes changed" and "widened".
8. Only after approval, write `current` (temp file plus rename, which is atomic on one filesystem). The old version becomes `previous`, and older versions are deleted.

Headless and ACP never approve. Given a widening or a new digest, they keep `current` and print `Notice(Warn)`: "update for <id> waits for approval: run `cox plugin update <id>`".

`--rollback` swaps `current` and `previous`. Grants are stored per `(id, scope, digest)` (§3), so the previous grant is still valid unless it was revoked. If it was revoked, the rollback asks again.

**A running session never picks up an update.** Tool schemas are in the cache-stable prefix. The new version loads at the next session, or at `/plugin reload`, which is the same as `/clear`: a new session in the same cwd, with a notice that the cache prefix starts over.

### 1c. Remove

`cox plugin remove <id>` steps:

1. Ask for confirmation (TUI modal or stdin; `--yes` skips it).
2. Disable the grant, so a concurrent session no longer loads the plugin at its next open.
3. Delete `~/.cox/plugins/<id>/` recursively after checking that the resolved path is inside `~/.cox/plugins/`. It never follows a symlink out of that path.
4. Delete every grant row for the id.
5. Delete the plugin's kv rows unless `--keep-data` is given.
6. Report config that still references the plugin, and edit none of it: `[plugins.<id>]`, a `tiers.*.provider` naming one of its provider sections, `[plugins.decide]` entries, and `keybindings.toml` rows for `plugin.<id>.*`.

In the current TUI session, `/plugin remove` stops the instance, so UI, hooks and events stop at once. Its tools stay in the frozen prefix but answer `ToolError::Denied { why: "plugin removed" }` until the next session. A project plugin's files belong to the repository, so for one, `remove` revokes the grant and deletes kv, and prints the path for the user to delete with git.

`/plugin new|update|remove|reload|list` in the TUI palette go through `Cmd` to the same `crates/cox` module the CLI uses. There is no second implementation.

## 2. Manifest `plugin.toml`

The types live in `cox-plugin-api::manifest` (§9). The committed schema is `docs/plugin.schema.json`, generated with `schemars::schema_for!`. Its drift test copies the protocol-schema test (`crates/cox-protocol/src/lib.rs:65-81`). One module in `cox-plugin` owns loading and validation (AGENTS "Config files").

```toml
api = 1                          # ABI major; the host refuses a different major
id = "git-glance"
version = "0.3.0"
name = "Git glance"
description = "Branch and CI state in the status line"
wasm = "plugin.wasm"
wasi = false                     # true for wasip1 guests (Go); no preopens unless [capabilities.fs]

[limits]                         # shown at approval; the host clamps to its maxima
memory_mib = 16                  # → max_pages; host max 64
call_ms = 200                    # default per-call budget; per-kind caps in §5–§8 are tighter

[capabilities]
events   = ["turn_started", "tool_call_done", "usage"]   # Event serde tags
hooks    = ["PreToolUse", "PostToolUse"]                 # HookEvent names
tools    = ["summarise"]                                 # become wasm__git-glance__summarise
invoke   = ["read", "grep"]                              # tools the plugin may call (each still passes Engine)
context  = true                                          # the event-folded context snapshot
kv       = true
model    = "cheap"                                       # cox_model_call tier ceiling; "code" must be named; never "think"
net      = ["api.github.com"]                            # allow-list for cox_http
fs       = { read = ["$WORKSPACE"], write = [] }         # WASI preopens, each through confine
decide   = ["risk"]                                      # §6b decision points
ui       = { status = true, panel = true, overlay = false, commands = true, keys = true, render = ["tool:wasm__git-glance__summarise"] }

[[provider]]                     # §7a
name = "typesafe"                # plugin id stays "jev"; the section is named "typesafe" (§14 decision 13)
api = "plugin"                   # "chat" | "responses" | "plugin"
base_url = "https://api.typesafe.ai"
api_key_env = "TYPESAFE_API_KEY"
auth = "bearer"                  # "bearer" | "x-api-key" | "none"

[[models]]                       # §7b
id = "jev-latest"
provider = "typesafe"
context_window = 64000           # 64k/request, 32k for state + longest question (R§4.3.6 J6)
price = { input = 0.042, output = 0.0 }

[[mcp]]                          # §7c
name = "gh"
command = "bin/gh-mcp-${target}" # inside the package, or a PATH program shown verbatim at approval
args = ["--stdio"]

[[agents]]                       # T45.3: a subagent definition, same format as .cox/agents/*.md
name = "reviewer"                # dispatched as agent(preset: "reviewer")
file = "agents/reviewer.md"
```

Validation (T33.1 and T33.4):

- `id` matches the directory name;
- every name fits the model tool-name rule `[a-zA-Z0-9_-]{1,64}` after prefixing;
- `net` entries are host patterns, not URLs;
- `fs` roots are `$WORKSPACE`, `$PLUGIN_DATA` or paths inside them;
- a `ui.render` target outside the plugin's own tools needs an explicit `tool:<name>`, which approval shows as "changes how <name> looks";
- an `[[agents]]` `file` is a relative `.md` path with no `..`, `\` or `:` component, and names are unique (T45.3);
- `wasm` may be omitted only by a data-only package: `[[mcp]]` and/or `[[agents]]`, nothing else;
- unknown keys are an error. The manifest is ours, so `deny_unknown_fields` applies, as in `Config` (`crates/cox-protocol/src/config.rs:35`);
- `surfaces` and the surface tables `[capabilities.terminal]` and `[capabilities.desktop]` follow §15.5; `ui.keys` is an alias of `terminal.keys`.

The capability list is the unit of approval. Each entry becomes one line in the dialog, for example "Can call the model on the cheap tier (costs appear in `cox stats` as `plugin:git-glance`)".

## 3. Grants

The grants live in `cox-store`, the only crate with SQL (D9). Migration `00000000000004_plugins` follows `…001_init`, `…002_usage_effort` and `…003_checkpoints`. Two Diesel tables, with typed models like `UsageDbRow` (`crates/cox-store/src/models.rs:29`):

| Table | Columns | Key |
| --- | --- | --- |
| `plugin_grants` | `plugin_id`, `scope` (`user` or `project:<root>`), `digest`, `capabilities` (JSON of the granted list), `enabled`, `source` (JSON), `decided_at` | (`plugin_id`, `scope`, `digest`) |
| `plugin_kv` | `plugin_id`, `key`, `value` (blob), `updated_at` | (`plugin_id`, `key`) |

There is no general kv table today (`schema.rs:13-87`). A narrow `PluginStore` trait sits in `cox-protocol` next to `Store` (`grant_get`, `grant_put`, `grant_set_enabled`, `grants_delete`, `kv_get`, `kv_put`, `kv_delete_all`), so `Store` itself does not grow. Quota: 64 KiB per value and 1 MiB per plugin. extism's own vars are switched off (`max_var_bytes = 0`, P12), so kv is the only state that outlives a call. It is persistent, has a quota, and is deleted by `remove`.

`grant::check(manifest, digest, stored) -> Verdict` is a pure function in `cox-plugin`, with three results:

- `Granted`: the digest matches and the requested capabilities are a subset of the granted ones. Asking for fewer never re-asks.
- `NeedsApproval { added, removed }`: a new digest, or wider capabilities.
- `Disabled`.

The granted list (T33.6) is a sorted JSON array of strings, one line per capability: `events:<tag>`, `hooks:<name>`, `tools:<name>`, `invoke:<name>`, `net:<host>`, `fs.read:<root>`, `fs.write:<root>`, `decide:<point>`, `ui.render:<target>`, the flags `wasi`, `context`, `kv`, `ui.status`, `ui.panel`, `ui.overlay`, `ui.commands`, `ui.keys`, `model:cheap` or `model:code`, plus one line per `[[provider]]` (`provider:<name> <base_url> key=<env>`), `[[mcp]]` (`mcp:<name> <command args | url>`) `[[external_agents]]` entry (`agent:<name> <argv> key=<env>`) and `[[agents]]` entry (`subagent:<name> <file>`, T45.3). The model tier is the one ordered entry: a `model:code` grant covers a `model:cheap` request. A row whose `capabilities` is not such an array grants nothing, and a store read error counts as no grant. `Disabled` wins over the digest check. A project `.cox/config.toml` may turn `plugins.enabled` off but never on (the project-config guard list).

By surface:

- **TUI.** At session open, each `NeedsApproval` plugin gets a `Modal::PluginGrant`. The TUI has one modal slot (`crates/cox-tui/src/state.rs:114-135`), so the dialogs queue. It lists the capabilities in words and, for project plugins, the repository in warning style. The keys are `y` (grant) and `n` (skip for this session). There is no "always" key: a grant is always per digest.
- **Headless `run -p` and ACP.** Only `Granted` plugins load. Anything else is one `Notice(Warn)` naming the command to run. This matches headless approval, which denies when there is no approver (`crates/cox/src/run.rs:331`).
- **CLI.** `enable` and `install` prompt on stdin, and `--yes` accepts.

## 4. Host↔guest ABI (`api = 1`)

Every payload is JSON through extism's `Json<T>` conversion. The types sit in `cox-plugin-api` with `JsonSchema`, `cox-protocol` re-exports them as `cox_protocol::plugin`, and the committed `docs/plugin-abi.schema.json` has a drift test. Existing protocol types cross the ABI unchanged: `Event` (`types.rs:775`), `HookEvent`/`HookOutcome` (`types.rs:275,572`), `ToolSpec` (`types.rs:1183`), `ToolOutput` (`types.rs:1200`), `Request`/`ProviderEvent`/`Usage`. They already derive `JsonSchema` (`types.rs:14`).

**Guest exports.** Only `cox_init` is required. The rest are probed with `function_exists` (P8).

| Export | In → out | Called |
| --- | --- | --- |
| `cox_init` | `InitIn { api, plugin_id, config: Value, session: SessionInfo, granted }` → `InitOut { tools: Vec<ToolSpec>, commands, keys, status, panels, renderers, subscribe }` | once per session; anything not granted is dropped with a notice |
| `cox_on_event` | `EventBatch { first_seq, dropped, events }` → `Effects { redraw, notices }` | §5 |
| `cox_hook` | `HookCall { event, payload }` → `HookOutcome` | §6 |
| `cox_decide` | `Question` → `DecideOut { Advice(Option<Advice>) \| Call(ModelCall) }` | §6b; `ModelCall.target = Tier(t) \| OwnProvider { name, model }` (2026-09-26, T33.40.1) |
| `cox_decide_resume` (optional) | `DecideResume { question, events }` → `Option<Advice>` | §6b; called after the host runs a `Call`'s `ModelCall` through the plugin's own provider, budget-gated and ledgered (2026-09-26, T33.40.1) |
| `cox_tool_subject` / `cox_tool_risk` / `cox_tool_call` | the `Tool` trait, serialised (`traits.rs:212-234`) | §7 |
| `cox_provider_stream` | `ProviderCall { request: Request, model }` → `Vec<ProviderEvent>` | §7a |
| `cox_command` / `cox_key` | `CommandIn { name, args }` → `CommandOut` | §8 |
| `cox_render` | `RenderIn { slot, width, height }` → `Widget` | §8 |
| `cox_render_item` | `RenderItemIn { target, call, result, width }` → `Option<Widget>` | §8 |
| `cox_shutdown` | `()` → `()` | session end, best effort |

`CommandOut` is deliberately narrow: `Prompt { text }` (submitted as `UserTurn`, like a markdown command), `Compact { focus }`, `TogglePanel`, `OpenOverlay`, `Notice`, or nothing. A plugin can never send an arbitrary `Submission`. `SetPermissionMode(bypass)`, `Approve` and `SwitchModel` up are out of reach.

**Host functions** live in namespace `cox:host/v1` and are registered with `with_function_in_namespace` (P7). Each call checks the grant and the calling context.

| Host fn | Capability | Allowed from | What it goes through |
| --- | --- | --- | --- |
| `cox_log(level, text)` | — | all | `tracing` only; rate-limited |
| `cox_notify(level, text)` | — | all but render | `Event::Notice`; the level is capped at `Warn` (a plugin can never raise `Security` or `Budget`); sanitized when shown |
| `cox_kv_get/put/delete` | `kv` | all but render | `PluginStore`, quota |
| `cox_context()` | `context` | all | the host's event-folded snapshot (§5) |
| `cox_invoke_tool(name, input)` | `invoke` lists the name | `on_event`, `command`, `key`, `tool_call` | builds a `ToolCall` and runs `PreToolUse` → `Engine::decide` (`permission/mod.rs:106`) → sandbox → archive, like a model call; `Ask` shows "plugin <id> asks to run …" in the TUI and is denied in headless mode (`run.rs:331`) |
| `cox_model_call(ModelCall)` | `model` | all but render | router → provider registry with `Job::Plugin(id)` at or below the granted tier: budget gate first, then one `usage` row (§7d) |
| `cox_http(HttpReq)` to a `net`-listed host | `net` | all but render | host-side reqwest; host must match the allow-list; body cap `max_http_response_bytes` |
| `cox_http(HttpReq)` to a provider section's `base_url` host | that `[[provider]]` section | `cox_provider_stream` only (2026-09-26; `NotInThisContext` everywhere else, including `cox_decide`) | host-side reqwest; the host injects the provider's auth header (§7a); the only path a decision plugin may reach its own paid provider from (T33.40.1) |
| `cox_output(line)` / `cox_cancelled()` | inside `tool_call` | `tool_call` | `ToolCx.output` / `ToolCx.cancel` (`traits.rs:187`) |
| `cox_redraw()` | any `ui` | all | marks the plugin's slots dirty (§8) |

Configuration comes in `InitIn.config`. That is the plugin's `[plugins.<id>]` table from `Config`, which gains `plugins: PluginsConfig { enabled, decide, #[serde(flatten)] entries }`, the same flatten pattern as `HooksConfig` and `McpConfig` (`config.rs:844,876`). The plugin validates its own table.

**Why some calls are banned in some contexts.** The core awaits `cox_hook`, `cox_decide` and `cox_provider_stream`. A tool invocation from inside them would need the loop that is waiting on them, which is a deadlock. So those contexts get `Err(NotInThisContext)`. `cox_render` has a strict time cap, so it may only read. The same reasoning gates `cox_http` to a provider host: reaching it from inside `cox_decide` directly would either deadlock (the plugin's own worker is blocked waiting on `cox_decide`, P8) or bypass the budget gate and the ledger, so the host allows it only inside `cox_provider_stream`. A decision plugin reaches its own provider instead by returning `DecideOut::Call(ModelCall)`, which the host runs before calling `cox_decide_resume` (2026-09-26, T33.40.1, R§4.3.6 J§4).

**Versioning.** `api` is the ABI major. A minor addition (a new optional export, host function or field) keeps `api = 1`, because unknown JSON fields are ignored in both directions. A breaking change raises the major. The host supports the current major and the previous one for one minor release of cox, and `cox plugin list` flags plugins on the old major. The surface parts and their namespaces (`cox:tui/v1`, `cox:desktop/v1`) follow the same rule (§15.7).

**Threading.** One OS thread per plugin owns its `Plugin` (`call` is `&mut self`, P8) and serves two bounded queues:

- *control*: hooks, decide, tools, provider, render and commands; depth 16, served first;
- *events*: a ring of depth 256 (§5).

Each call runs under its own deadline through a `CancelHandle` timer (P9). The manifest `timeout_ms` is set to the plugin's `limits.call_ms` as the outer cap (P13). Calls into one plugin run one at a time, so its tools carry `Concurrency::Exclusive`. Different plugins run in parallel. `Pool` (P10) is not used, because extra instances would split plugin state.

### Decision points for a decision model such as Jev

A decision plugin (A25/P21; Jev is the first) needs to *answer* questions the core asks, not only intercept them. That is `cox_decide` behind a new `Advisor` trait in `cox-protocol`, set on `Session` like `set_hook` (`crates/cox-core/src/session.rs:469`). The core still decides. It offers the options, applies a monotone rule, and uses the local default on silence, lateness or low confidence.

| Point | Where in the core today | Question | Rule the core enforces |
| --- | --- | --- | --- |
| `route` | `router::pick`, `Job::Main` only, once per `UserTurn` | Choice among tiers ≤ the static pick | never up, never `think` (D5); opt-in per session config; the core offers a downgrade only when its own cost estimate predicts a saving over the static pick (2026-09-26, T33.40.8) |
| `risk` | after the classifier, before `Engine::decide` (`turn.rs:244-259`) | Score of severity | may raise `Risk`, never lower it; asked only when raising to `Destructive` would change the engine's outcome (2026-09-26, T33.40.6) |
| `approve_hint` | approval modal | Noul, a caution note only | warning-only: may add "caution", never say a call "looks safe" — monotone the same direction as `risk` (2026-09-26, §14 decision 10; bare "looks safe" display is unsafe against a steered state, R§4.3.6 J12) |
| `compact` | after `TurnDone`, before the threshold check | Noul "compact now?" | may only compact earlier; never skips a mandatory compaction |
| `rank` | `tool_search`, skill match | Choice or reject over candidates cox found | reorders or filters; never adds |
| `salience` | memory extraction | Score per item | thresholds stay in config |

`[plugins.decide]` names exactly one plugin per point (`route = "jev"`), with `min_confidence` and a latency budget (defaults: route 300 ms, risk 200 ms, compact 500 ms, rank 300 ms). Without an entry the point is off. A new `Event::Advised { point, plugin, advice, applied }` records every answer, so replay and `cox stats` can see which ones were used. A model call inside `cox_decide` is an ordinary `cox_model_call` with its own ledger row.

These points are not a `HookEvent`, on purpose. A hook's `Modify` rewrites input and has no monotone rule, so a hook-shaped router could route up.

`Question` carries `items: Vec<QuestionItem>`, so one request covers every item a point needs in one tool batch or one turn instead of one request per item — 12× cheaper and 10× faster for a 13-item batch, measured against System One's own batching cookbook (2026-09-26, T33.40.1, R§4.3.6 J10).

## 5. Events

**The tap.** `cox-protocol` gains `trait EventTap: Send + Sync { fn offer(&self, seq: u64, ev: &Event); }`, which must not block. `Session::emit` calls it right after the rollout append, with the same scrubbed event the rollout gets (`crates/cox-core/src/session.rs:419-450`, scrub at 431). Plugins therefore see redacted events, in rollout order, from one place for all four surfaces. `Session::events()` has exactly one consumer (`session.rs:406-408`), so tapping a surface's consumer would have meant three copies of the tap.

**Delivery.** Each plugin gets the `Event` variants named in `capabilities.events`, as read-only JSON copies. That is any of the 21 variants at `types.rs:775`. `text_delta`, `thinking_delta` and `tool_call_output` are allowed but arrive batched.

**Queue.** `offer` pushes into the plugin's ring of 256 under a short lock and returns. When the ring is full it drops the *oldest* event and increments `dropped`. The next `EventBatch` carries `first_seq` and `dropped`, so the plugin knows about the gap. Recent state matters more to a status segment than history. Order within one plugin is emission order. Nothing is ordered across plugins.

**Never blocking.** The core loop's cost per event per plugin is one `try_lock` and one push. A slow plugin loses events. It never slows a turn.

**The context snapshot** (`cox_context`) is folded by the host from the same tap. It holds the session id, cwd, tier and model, usage totals, the last 50 items with assembled text (redacted), the todo list and compactions. It is not the assembled `Request` (§12).

## 6. Hooks

Plugin hooks are one more hook source, with no second mechanism.

- `PluginHooks` implements `cox_protocol::Hook` (`traits.rs:349`) with the same `HookOutcome` model: Continue, Block, Modify, Failed.
- A `HookChain(Vec<Arc<dyn Hook>>)` replaces the single wrapped `ShellHooks` (`crates/cox/src/session.rs:150-165`). The chaining loop moves out of `ShellHooks::run` (`crates/cox-ext/src/hooks.rs:37-73`) into one shared function: the first `Block` or `Failed` stops the chain, and a `Modify` feeds its `tool_input` to the next hook. `PresenceHook` keeps wrapping the chain (`presence.rs:159,234`).
- **Order.** Config shell hooks run first, so the user's own rules win. Plugin hooks follow in plugin-id order.
- **Dispatch fix.** `fire_configured` today dispatches only events present in `[hooks]` (`crates/cox-core/src/hooks.rs:17-25`), so a plugin hook would never run. `Hook` gains `fn interested(&self, event: HookEvent) -> bool`, defaulting to the config check. The chain answers true when any source is interested.
- **Timeout.** A plugin hook's deadline is the smaller of `hooks.timeout_s` and `limits.call_ms`. When the deadline passes, the host cancels the call and returns `Failed`. The core then fails open (`hooks.rs:70-83`).
- **Safety of `Modify`.** A plugin's `Modify` on `PreToolUse` cannot get past the engine, because `PreToolUse` runs before `Engine::decide` (`turn.rs:244-259`).

## 7. Tools, providers, models, MCP

**Tools.** Each granted tool is a `WasmTool` implementing `Tool`, named `wasm__<id>__<tool>` as in the earlier sketch (`v0.2-wasm.md`).

- It is always `deferred: true`, so it reaches the model only through `tool_search`.
- Its `ToolSpec` is taken from `cox_init` once and frozen for the session.
- Specs are sorted by (plugin id, tool name) and appended after MCP tools (`crates/cox/src/session.rs:122-124`).
- Discovered tools join `Request.tools` in discovery order after the stable set (`crates/cox-core/src/context.rs:58-62,92`), so the prefix changes once per discovery, as it does for MCP.
- A plugin that returns different specs on its next `init` changes the next session, never the current one.
- Output goes through the normal path: the core archives before it truncates (D6a).

**7a. Providers.** Two forms, both recommended.

- **Declarative (`api = "chat" | "responses"`).** The section merges into `providers.custom` as one more `CompatibleProviderConfig` (`config.rs:560`), and cox drives it with its own wire clients. There is no wasm on the request path. A user config section with the same name wins, and the plugin's section is skipped with a notice. This is the zero-code path for any OpenAI-shaped endpoint.
- **ABI (`api = "plugin"`).** A `PluginProvider` implements `Provider`: `stream()` calls `cox_provider_stream` with the provider-neutral `Request` and forwards the returned `ProviderEvent`s. It exists for wires none of cox's clients parse, such as Jev's System One (`crates/cox-provider/src/jev.rs:8-13`).
  - The guest reaches the network only through `cox_http`, only to `base_url`'s host.
  - The **host** resolves the key with `resolve_key(api_key_env, <section>)` (`crates/cox-provider/src/http.rs:49`) and adds it as the declared `auth` header. The key never enters wasm memory, and the plugin never sees the keyring.
  - v1 returns one batch per request. Streaming through a `cox_emit` host function is a later minor addition.

For both forms:

- The core writes one `usage` row per request and checks the budget before it, as for any provider (invariant 8).
- The price comes from the catalog, never from the plugin.
- A plugin's `Usage` is untrusted. If it is missing, cox estimates (`estimated: true`). If the reported input is below half of cox's own token estimate for the request, cox records the estimate and warns once.

**7b. Models.** `[[models]]` rows join the `cox-models` catalog as a new layer. Today the order is built-in < config < user `prices.toml` (`crates/cox-models/src/catalog.rs:119-198`). It becomes: **built-in < plugin < config < user prices**.

- The plugin layer only *fills*. For an id the built-in rows already have, a plugin may set only fields that are still `None`. A conflicting value, above all a price, is ignored with a `Notice(Warn)` and a `cox doctor` row: "plugin jev tried to change the price of claude-haiku-4-5".
- For a new id the plugin row is taken whole. If two plugins define the same new id, the lower plugin id wins and the other gets a warning.
- `Catalog::load` takes the plugin rows as an argument, so `cox-models` stays pure. `ModelRow` gains a `source` field for `cox doctor`.

**7c. MCP servers.** `[[mcp]]` entries join `cox_mcp::discovery::discover` (`crates/cox-mcp/src/discovery.rs:22`) as one more source (`sources["<id>-<name>"] = "plugin:<id>"`), with the lowest precedence: `.mcp.json`, `~/.claude.json` and config all win on a name clash. Tools appear as `mcp__<id>-<name>__<tool>`.

- **A stdio server must be a real process.** rmcp's stdio transport speaks to a child process (`crates/cox-mcp/src/client.rs:91-100`), and a wasm plugin that wants tools already has the in-process `tools` capability, which is cheaper.
- A command inside the package is covered by the digest. A PATH program (`npx`, `uvx`) is shown verbatim at approval.
- `crates/cox` wraps the argv with the platform sandbox (`sandbox::Policy`, `cox-sandbox` after T32.3) before `cox-mcp` spawns it, so `cox-mcp` keeps depending on `cox-protocol` only. `client.rs:92` already clears the environment.
- **Every MCP stdio server is sandboxed, not only a plugin's (resolved 2026-09-26, §14 decision 4).** `client.rs:91-98`'s existing, user-configured servers get the same `sandbox::Policy` wrap as a plugin's, with a per-server `sandbox = false` opt-out in config for setups that need it (T33.42 — its own card, so this behaviour change does not ride in on T33.19's landing).
- An HTTP `url` needs its host in `net`.

**7d. Model calls by plugins.** A plugin that wants a model answer, for example a hook asking Jev, uses `cox_model_call`, never raw HTTP to a provider:

- the router picks from the granted tier, which is at most `cheap` unless `code` is granted, and never `think` (D5);
- the budget gate runs, and one `usage` row is written with `job = plugin:<id>` (D5: "every request carries a job tag");
- the key stays in the host.

Raw `cox_http` to a paid API would bypass the ledger and the budget, and would need the key inside the guest. Manifest validation therefore refuses a `net` entry that matches the host of any configured provider section. Talking to a provider goes through a provider section or `cox_model_call`.

## 8. TUI contributions

**Widget tree** (`cox-plugin-api::ui`). It is declarative and closed:

- `Widget = Text(Vec<Line>) | List { items, selected } | Table { header, rows, widths } | KeyValue(Vec<(Span, Vec<Span>)>) | Gauge { ratio, label } | Stack { vertical, children, sizes } | Block { title, child }`
- `Span { text, style: StyleToken, bold, italic }`
- `StyleToken` names the `Theme` fields (`crates/cox-render/src/theme.rs:29-46`: `text`, `dim`, `accent`, `ok`, `warn`, `error`, `diff_add`, `diff_del`, `border`, `selection`, …). Plugins get no raw colours, so themes, `NO_COLOR` and colour downgrade keep working.
- Limits: at most 512 nodes, depth 8 and 16 KiB of text per render. Anything over the limit becomes a one-line "plugin output too large".
- The conversion to ratatui lives in `cox-tui` and runs every string through `cox_sanitize::sanitize_with` (`crates/cox-sanitize/src/lib.rs:27`), the same boundary as `cells::cell_lines` (`cells.rs:100-101`).

**Slots.**

| Slot | Where | Budget |
| --- | --- | --- |
| `status.left`, `status.right` | the one status row (`status.rs:1-8`) | ≤ 24 columns per segment; plugin segments drop first on a narrow terminal |
| `panel` | rows above the composer, like the todo panel | ≤ 8 rows; toggled by the plugin's command or key |
| `overlay` | `Modal::Plugin { id }`, full screen like `Modal::Diff` and `Modal::Transcript` | Esc closes |

There is no sidebar. D10's inline viewport has no side area. A sidebar would need the alternate screen (§12).

**Renderers.** A `ui.render` target is `tool:<name>` or `item:assistant_message`. When a `Cell::Tool` completes (`ToolCallDone`), the TUI asks the matching renderer once and caches the result in the cell. `None`, a timeout or an error falls back to the built-in rendering (`cells.rs:87-193`). The model-visible text and the archive are never touched, and headless, ACP and the rollout never see renderer output.

**Commands.** Plugin commands appear in the palette as `/<id>:<name>` with the plugin as their source. They are appended to `State.commands` after the built-ins (`state.rs:189-191`) and dispatched like file commands (`state.rs:1217,1395-1408`), so a built-in always wins.

**Keys.** Plugin keys are reachable only as `<leader> <key>`. The leader is the new keymap action `plugin.leader`, rebindable in `keybindings.toml` (`keymap.rs:308`). Built-in and user bindings always win. A clash between two plugins goes to the lower id and is reported by `Keymap::conflicts()` (`keymap.rs:278`).

**Redraw model (Zellij's, P18).** `view` never calls a plugin. `cox_render` runs only when:

- the plugin asked through `Effects.redraw` or `cox_redraw()`;
- a resize happened;
- a slot became visible.

The result is cached in `State` through `Msg::Plugin(PluginUiMsg)`, and `Cmd::Plugin(PluginRequest)` carries commands, keys and slot sizes back to the host. This fits `update(&mut State, Msg) -> Vec<Cmd>` (`state.rs:364,643`) and the draw-per-`Msg` loop (`crates/cox-tui/src/app.rs:151`). A render has 20 ms. Past that, the last good render stays on screen, and three misses in a row show "⚠ <id> slow" and stop the slot for the session. `cox-tui` depends only on the ABI types through `cox-protocol`, never on `cox-plugin`.

Headless and ACP do not call UI exports.

The desktop app draws the same slots, renderers and commands. Keys are terminal-only, and the app adds its own contributions (§15).

## 9. Where the code lives

| Crate | Rule (`crates.md`) | Depends on | Holds |
| --- | --- | --- | --- |
| `cox-plugin-api` | (d): the guest SDK needs the ABI and manifest types without `cox-protocol`'s tokio and `CancellationToken` (`crates/cox-protocol/Cargo.toml`) | serde, serde_json, schemars | manifest, ABI payloads, widget tree, capability names; builds for `wasm32-unknown-unknown` |
| `cox-plugin` | (a): the only user of extism and wasmtime | protocol, plugin-api, sanitize | loader, digest, grant check, workers, host fns, `PluginHooks`, `WasmTool`, `PluginProvider`, `EventTap`, `Advisor` impls |
| `plugins/` (separate cargo workspace, not `crates/*`) | guest code must never build for the host in `cargo nextest run --workspace` | extism-pdk 1.4.1 | `sdk/` (`cox-plugin-sdk`), `examples/<lang>/`, `templates/<lang>/` |

`cox-protocol` re-exports `cox_plugin_api` as `cox_protocol::plugin`, so every type that crosses a crate boundary is still reachable from the protocol crate. New `deps.rs` rules follow `only_store_depends_on_diesel` (`crates/cox/tests/deps.rs:85-99`):

- only `cox-plugin` depends on `extism`;
- `cox-plugin-api` depends on no workspace crate;
- `cox-core` does not depend on `cox-plugin`.

`extism::Error` is anyhow (P6), and `cox-plugin` maps it to its own `thiserror` enum `PluginError` at the boundary.

## 10. Security

- **Resources.**
  - Memory: `limits.memory_mib`, host max 64 MiB.
  - Per-call deadlines: render 20 ms, event batch 50 ms, hook ≤ `hooks.timeout_s`, tool ≤ the tool timeout, decide ≤ its point budget.
  - Queue depths as in §4, a kv quota, and vars off.
  - No fuel metering: epoch interruption (P13) is cheaper and enough.
- **Filesystem.** Only WASI preopens from `fs`. Each root passes `cox_tools::path::confine` at load. `.git` and `.cox` are never writable (D7), and read grants mount `ro:` (P11). WASI is on only when `wasi = true` or `fs` is set. The `WasiCtx` has no environment and no arguments (P16).
- **Network.** Only `cox_http`, only to allow-listed hosts, capped by `max_http_response_bytes`. extism's own `http_request` is compiled out (P5), and a test proves it refuses.
- **No bypass.** No host function spawns a process, opens a file, writes the terminal, reads the keyring or decides a permission. MCP stdio servers (§7c) and external-agent CLI processes (`[[external_agents]]`, P35, `docs/design/external-agents.md`) are the only processes a plugin brings, and both run under the same sandbox wrap, spawned by the host, never by a WASM guest.
- **Untrusted output.** Everything a plugin returns is untrusted like MCP output: tool descriptions and results go to the model through the normal archive and truncate path, and everything shown in the terminal is sanitized.
- **Fail open (D14).**
  - A load failure, a trap, a timeout or invalid JSON is `Notice(Warn)` plus absence.
  - Three failures in a row disable that export for the session.
  - `cox doctor` gains a plugins row: loaded, skipped, not granted, disabled exports and their reasons. `cox ext list` shows the same state, so "my plugin stopped working" always leaves a visible trace (`extensions.md` falsifier 2).

## 11. Performance budget and how it is measured

| Metric | Budget | Measured by |
| --- | --- | --- |
| release binary growth from extism and wasmtime | ≤ 22 MiB (was 20 MiB, and 10 MiB before that; T33.3 measured +16.8 MiB on 2026-09-26, and the whole `plugins` feature measured +20.38 MiB on 2026-09-27, A65) | `scripts/footprint.sh` before and after T33.3 (R§4.3.5 P25) |
| clean build growth | ≤ 60 s | `cargo build --timings` before and after T33.3 (R§4.3.5 P25) |
| warm session start per plugin (wasmtime cache on) | ≤ 50 ms | `just bench` timing over the Rust example: open, then `cox_init` |
| cold compile of a 1 MiB module | ≤ 500 ms | same, cache cleared |
| `cox_on_event` batch of 16 | p50 ≤ 1 ms | same, 1 000 calls |
| `cox_render` of a status segment | p95 ≤ 5 ms | same |
| hook round trip | p95 ≤ 5 ms (a shell hook is tens of ms, `extensions.md`) | same |

The compilation cache uses `with_cache_config` (P7), pointed at `~/.cox/cache/wasmtime/`.

## 12. Falsifiers and out of scope

**Falsifiers.**

1. **Footprint.** If T33.3 measures more than 10 MiB of binary or 60 s of clean build for extism, the host moves behind a default-on cargo feature `plugins`, a lean build ships without it, and D1 is amended. **Fired (2026-09-26).** T33.3 measured +16.8 MiB (53 713 168 → 71 277 728 B, R§4.3.5 P25); clean-build growth could not be separated from machine load (wall time 159 s before, 139 s after; +463 CPU-s of new units). The creator's decision: the host sits behind the cargo feature `plugins` on `crates/cox`, on by default, which gates the optional `cox-plugin` dependency. The slim build is `--no-default-features --features otel`, and `crates/cox/tests/deps.rs` (`slim_build_has_no_wasm_runtime`) checks that it has no extism or wasmtime. The binary budget became 20 MiB. On 2026-09-27 the whole `plugins` feature measured +20.38 MiB (77 780 912 B full against 56 409 008 B slim, macOS arm64 release), and the creator raised the budget to 22 MiB (A65); the falsifier fires again above that.
2. **Latency.** If warm start exceeds 50 ms per plugin or a status render exceeds 5 ms p95 on the example, status segments become push-only: a `cox_set_status` host function replaces `cox_render` for the status slot.
3. **The ABI is too small.** If Jev-as-a-plugin (T33.40) needs a host function that bypasses the engine, the ledger or the sandbox, the ABI is wrong and the decision-point design is revisited before `api = 1` freezes.
4. **The wasmtime pin.** If a wasmtime advisory affects the version extism pins (43, P3) and extism ships no fix within 30 days, embedding wasmtime directly is re-evaluated.
5. **Context access.** If three plugin requests need the assembled `Request` (the system prompt and instruction files) and the event-folded snapshot cannot serve them, a read-only `cox_request_view` is designed. It must not break the cache-stable prefix.

**Out of scope.**

- A marketplace or registry.
- Signatures.
- The component model and WIT.
- Plugin-to-plugin calls.
- Plugins adding text to the system prompt (skills do that).
- A sidebar in the inline viewport.
- Changing tool schemas within a running session.
- Loading Claude Code plugin bundles as cox plugins.
- JS, Python, C# and other PDKs.

## 13. Scaffolding and languages

**`cox plugin new <name> [--lang rust|go|kotlin|dart] [--dir <path>] [--with status,panel,command,key,renderer,hook,event,tool,provider,models,mcp]`**

- One pure module, `crates/cox/src/plugin_new.rs`, maps (name, lang, with) to a list of (path, content). The CLI writes that list. The TUI palette's `/plugin new` reaches the same function through `Cmd`, and asks for the language with the existing `Modal::Picker` when `--lang` is missing. Headless defaults to `rust`.
- The name is validated as a plugin id. An existing target directory is an error, never an overwrite.
- The output is:
  - `plugin.toml`, whose `[capabilities]` match `--with` exactly;
  - stub exports only for the chosen capabilities;
  - a build recipe (a `justfile`);
  - a `README.md`;
  - a smoke test that installs the built package into a scratch `COX_HOME` with `--yes` and checks that `cox plugin list --json` reports it `loaded` with the chosen contributions. `list` compiles each granted plugin and runs `cox_init` against a stub session.
- Templates live in `plugins/templates/<lang>/` with `.tmpl` names, so cargo, go and gradle never pick them up. They are embedded with `include_str!` in one table, which needs no new dependency. Capability blocks are marked `cox:with=<cap>` … `cox:end` and removed when not chosen.

**Dev loop.** `cox plugin link <dir>` (optional card) uses a built plugin in place. A linked plugin asks again only when its capabilities widen, never on byte changes, because the user builds it. It is marked `dev` in every listing and in `cox doctor`. `/plugin reload` is the same as `/clear` (§1b). `cox plugin build` is not needed, because the template's `justfile` is the build. `cox plugin dev` (watch, rebuild and reload) is not needed either, because `link` plus `/plugin reload` covers it.

**Languages (sources R§4.3.5 P26–P37).**

| Lang | PDK | Target | Loads in extism today? | Plan |
| --- | --- | --- | --- | --- |
| Rust | `extism-pdk` 1.4.1, official | `wasm32-unknown-unknown` | yes (P17) | the reference: SDK, example and template first |
| Go | `github.com/extism/go-pdk` v1.1.3, official (maintained, pushed 2026-01-22) | TinyGo `-target wasip1 -buildmode=c-shared`, or Go ≥ 1.24 `GOOS=wasip1` with `//go:wasmexport` | yes by the PDK's docs, with WASI on | SDK wrapper, example and template, built in the `plugins` CI job |
| Kotlin | none maintained (`LizAinslie/extism-kotlin-pdk`, last push 2023-11-30) | Kotlin/Wasm `wasmWasi`, Beta, WASI preview 1, needs GC and exception handling; `@WasmImport`/`@WasmExport` are experimental | **refuted (T33.35, 2026-09-26, R§4.3.5 P40–P43)**: a minimal `cox_init` module (Kotlin 2.4.20) fails to *parse* under extism 1.30.0's default engine config — `exceptions proposal not enabled` — regardless of WASI; Kotlin's `wasmWasi` output unconditionally uses the wasm exception-handling proposal, which extism enables only behind its non-default `wasmtime-exceptions` feature. With that feature on it instantiates and calls `cox_init` cleanly, with WASI off exactly as the real host runs today (A55/T33.43 is not the blocker here) | `wasmtime-exceptions` approved and on (A61), but any real module also imports `wasi_snapshot_preview1::random_get` from Kotlin's stdlib (R§4.3.5 P45), so Kotlin waits for WASI (T33.43, A63); the draft PDK and example are on branch `wip/t33.36-kotlin` |
| Dart | none | `dart2wasm` targets JS environments only: "doesn't support execution in standard Wasm run-times like wasmtime" (dart.dev, Dart 3.13) | **no — refuted again 2026-09-26** (T33.37, R§4.3.5 P44): Dart 3.13.4's `dart compile wasm` still emits a JS bootstrap, and the module fails to parse under the workspace's pinned extism/wasmtime 43 (`exceptions proposal not enabled`) before even reaching its `wasm:js-string`/`dart2wasm` JS imports | documented exception: the Dart template scaffolds a plugin whose only capability is an `[[mcp]]` stdio server (`dart compile exe`, `dart_mcp` 0.5.2), so `--with` accepts only `tool` and `mcp` for Dart; dart-lang/sdk#56366 (WASI support) is still open — re-run the spike when it closes |

Every language implements the same example: a status segment showing the turn count, a `PostToolUse` hook that counts failures, a `/<id>:reset` command and a `turn_started` subscription. The Dart MCP variant implements one tool, `count`.

**Building examples and templates.**

- The Rust example is built by `crates/cox-plugin-fixtures/build.rs` into `OUT_DIR`. The target comes from `mise.toml` (`rust = { version = "1.97.1", targets = ["wasm32-unknown-unknown"] }`, P22) and CI's `dtolnay/rust-toolchain` `targets:` input (P23). It needs no network once the registry cache is warm, like any other dependency. No `.wasm` is committed, so nothing drifts from its source and A48 is untouched.
- Host unit tests use inline WAT (P15), with no build at all.
- Go, Kotlin and Dart toolchains go in `plugins/mise.toml` (mise reads per-directory config), so host-only contributors never install them. Their e2e tests are `#[ignore = "needs <toolchain>: run `just plugin-examples <lang>`"]`: nextest lists them as skipped with the reason, never as passed. A `plugin-examples` CI job installs the toolchains and runs `--run-ignored only`. In that job a missing toolchain is a failure.

## 14. Decisions (2026-09-26)

The creator resolved every open question this design and the Jev use case (A25/A52, T33.40) raised. Recorded here so a later reader does not re-litigate them; each is cross-referenced from where it changes the design above.

1. **SDK/API publishing.** `cox-plugin-api` and `cox-plugin-sdk` are not published while `api` may still change. Publishing them, once the ABI is stable, is a `roadmap.md` item, not a P33 card.
2. **Dart.** Stays the documented exception (§13): its only v1 capability is an `[[mcp]]` stdio server. T33.37 re-checked dart-lang/sdk#56366 on 2026-09-26 (still open) and tried Dart 3.13.4 against the workspace's extism/wasmtime pins (R§4.3.5 P44): refuted — `dart compile wasm` still emits a JS bootstrap, and the module fails to instantiate (`exceptions proposal not enabled`) before its `wasm:js-string`/`dart2wasm` JS imports even come into play. Re-run the spike when #56366 closes.
3. **Kotlin.** T33.35's spike (2026-09-26, R§4.3.5 P40–P43) found that a minimal Kotlin/Wasm `wasmWasi` module needs extism's non-default `wasmtime-exceptions` feature just to parse (unrelated to WASI/A55). The creator approved turning it on for the whole workspace on 2026-09-26 (A61), so it applies to every plugin, not only Kotlin ones; see "Engine features" in `docs/plugins.md`. A real Kotlin module also imports `wasi_snapshot_preview1::random_get` from Kotlin's own stdlib (R§4.3.5 P45), and the host keeps WASI off until T33.43 (A55), so T33.36 waits for that bump, like Go (A63).
4. **Sandboxing MCP stdio servers.** Every MCP stdio server — not only ones a plugin ships — runs under `sandbox::Policy` (§7c). This resolves §7c's open question the other way from the status quo: instead of leaving existing user-configured servers unsandboxed, it wraps all of them, with a per-server opt-out in config (T33.42, its own card).
5. **`route`.** A plugin may only downgrade the tier (D5 still holds: never up, never `think`). The core offers a downgrade to the plugin only when its own cost estimate (catalog prices, prefix size, the cache-read/write formula, T33.40.8) predicts a saving over the static pick. A plugin cannot even be asked when the core's own estimate says downgrading would cost more.
6. **Install sources in v1.** Confirmed: local folder only (§1). Git and URL sources move to `roadmap.md`.
7. **CI.** A separate `plugin-examples` job builds go, tinygo, java, gradle, kotlin and dart (§13) rather than folding them into the main job.
8. **The built-in `typesafe` section.** Once the Jev plugin (T33.40) reaches parity, the built-in `[providers.typesafe]` client (`crates/cox-provider/src/jev.rs`) is removed from the core, per the Jev cards' tombstone config type, fail-open notice and removal cards (T33.40.12–T33.40.16). Until parity, both exist side by side (T33.40.5).
9. **Jev evals.** Only E1 (`risk`, Jev only, capped at $0.10, T33.40.7) is approved to run. E2 (`route`, real Anthropic plus Jev, capped at $3, T33.40.10) stays in the plan but needs the creator's explicit go-ahead before each run.
10. **`approve_hint`.** Warnings only. A plugin may add "caution"; it may never say "looks safe" — the same monotone shape as `risk`, just never usable to grant quiet approval (§4's decision-points table, above).
11. **`risk` advisor.** Enabled only by an explicit line in `[plugins.decide]` — never automatically on install, even after E1 passes. The grant dialog states exactly what data leaves the machine for that point.
12. **T32.15 (`cox-provider-jev`) is dropped.** After parity, `jev.rs` is deleted outright rather than extracted into its own crate; see `plan.md` §6 A52 and `done.md`.
13. **The ABI fix from the Jev research.** Writing the Jev plugin against `api = 1` as first drafted exposed a deadlock/ledger-bypass gap (T33.40.1, §4 above): `cox_decide` now returns either an `Advice` or a `ModelCall`, the host runs the call through the plugin's own provider with the budget gate and ledger, then calls `cox_decide_resume`; `cox_http` to a provider host is allowed only inside `cox_provider_stream`; `Question` is batched. The example provider throughout this document is named `typesafe`, not `jev` — the plugin id stays `jev`, the provider section it declares is `typesafe` (§2).
14. **No prebuilt Jev plugin archive ships with the release.** Users build it from `plugins/jev` (`just plugin jev`) and install it with `cox plugin install <dir>` (§1).
15. **Plugin agent definitions are grant-gated, local definitions win** (T45.3, 2026-09-29). An `[[agents]]` file is loaded only for a `Granted` plugin, and each file is its own approval line, so an update that adds or renames one asks again. A `.cox/agents/*.md` or `~/.cox/agents/*.md` definition of the same name wins over the plugin's, with a notice (T45.4). The definition's `permissionMode` can only narrow the parent's mode (T45.2).

## 15. Surfaces: shared, terminal-only and desktop-only (T33.45, A134)

Plugins were designed for the terminal (§8), but they already run in every session cox opens, and the desktop app draws the shared slots through `cox-app` (`crates/cox-app/src/plugin_ui.rs`, T52.14–T52.17, T52.23). Until this section, nothing said which surface a feature belongs to: a plugin could not declare where it runs, `[capabilities.ui]` mixed a terminal-only part (`keys`) with shared ones, and there was no desktop-only API. This section splits the API into three parts, says how a plugin declares them and what happens when it asks for a part the current surface does not have. Build cards: §6 A134 in `plan.md`.

### 15.1 Surfaces

The session already names who drives it (`SessionSpec.surface`, `crates/cox-session/src/lib.rs`, set in `crates/cox/src/session.rs`, `crates/cox/src/acp_cmd.rs` and `crates/cox-app/src/live.rs`). `cox-session` maps it to the plugin `Surface` in one function; the plugin host never parses the string.

| `SessionSpec.surface` | `Surface` | Draws plugin UI |
| --- | --- | --- |
| `tui` | `terminal` | yes, §8 |
| `plain` (`cox --plain`) | `terminal` | no: it appends lines and has no slots, as today |
| `app`, local or through `cox app-server --stdio` on an ssh host (`crates/cox-app/src/server.rs` opens its sessions through the same `App::open`) | `desktop` | yes, `cox-app` `plugin_ui` |
| `headless` (`cox run -p`) | `headless` | no |
| `acp` | `acp` | no |

The surface is fixed when the session opens. A remote session is a desktop session: the UI is the app, wherever the plugin runs.

### 15.2 Shared API (both surfaces)

Everything `api = 1` had before this section, minus `keys`:

- **Exports:** `cox_init`, `cox_on_event`, `cox_hook`, `cox_decide`/`cox_decide_resume`, `cox_tool_subject`/`cox_tool_risk`/`cox_tool_call`, `cox_provider_stream`, `cox_command`, `cox_render`, `cox_render_item`, `cox_shutdown` (§4).
- **Manifest:** `[capabilities]` `events`, `hooks`, `tools`, `invoke`, `context`, `kv`, `model`, `net`, `fs`, `decide`, `ui.status`, `ui.panel`, `ui.overlay`, `ui.commands`, `ui.render`; `[[provider]]`, `[[models]]`, `[[mcp]]`, `[[external_agents]]`, `[[agents]]` (§2).
- **Host functions** in `cox:host/v1`: `cox_log`, `cox_notify`, `cox_kv_*`, `cox_context`, `cox_invoke_tool`, `cox_model_call`, `cox_http`, `cox_output`/`cox_cancelled`, `cox_redraw` (§4).
- **Widgets and slots:** the closed `Widget` tree with `StyleToken` roles (§8); the `status.left`/`status.right`, `panel` and `overlay` slots; `tool:`/`item:` renderers; `/<id>:<name>` commands. Sizes are in cells: the TUI's grid, the app's `font.monoCode` cells (`PluginWidgetView`, DS§6.3).
- **Layout rules both hosts already apply alike** are shared, not terminal-only: a status segment is at most 24 cells and plugin segments drop first when the row is narrow (`SEGMENT_COLS` in `crates/cox-tui/src/status.rs` and in `crates/cox-app/src/plugin_ui.rs`; the app's `SessionToolbar` drops them first through `ViewThatFits`, DS§6.4); the panel is at most 8 rows (`PANEL_ROWS` in both); the overlay is `Modal::Plugin` in the TUI and a sheet that Esc closes in the app (DS§6.4 `PluginPanel`). They are host layout, not API: a plugin only receives `RenderIn { width, height }`.

Two shared additions:

- **`InitIn.surface: Option<Surface>`** — `terminal`, `desktop`, `headless` or `acp`, so a plugin adapts at run time. Absent from a cox older than this section; the SDK reads absence as `terminal`, the only surface plugins were written for.
- **`Span.link: Option<String>`** — a link on a run of text: an `https://` URL or a workspace-relative path; anything else is dropped to plain text when the tree is converted. The terminal draws it with `cox-render`'s OSC 8 marking (`crates/cox-render/src/link.rs`) when `term::Caps.osc8` is on, and as underlined text otherwise; the app draws a link that opens on a click — `Host::open_url` for a local session, `Host::confirm_open_url` for a remote one (`crates/cox-app/src/app.rs`). Nothing opens without the person's click. An older host ignores the field (no ABI type denies unknown fields) and draws the text.

### 15.3 Terminal-only API

- **Leader keys:** `KeyDecl`, `InitOut.keys` and the `cox_key` export, reachable only as `<leader> <key>` under the keymap action `plugin.leader` (§8 "Keys", `crates/cox-tui/src/keymap.rs`). The manifest spelling moves from `ui.keys` to `[capabilities.terminal] keys` (§15.5). Why terminal-only: the leader is a binding in `keybindings.toml`; the app's keys belong to the menu bar and ⌘K (DT§5.5), which have no leader. `cox-app` exposes `plugin_keys`/`plugin_key` (`crates/cox-app/src/live.rs`) and `cox-ffi` forwards them, but no Swift view calls them; the app-core card removes them. A plugin that wants the same action on the desktop declares it as a command and a palette action (§15.4).
- **Namespace `cox:tui/v1`** is reserved for terminal-only host functions and has none in the first build. The candidate, `cox_tui_caps()` returning the probed `term::Caps` subset a plugin could use (`osc8`, `truecolor`, `images`), waits for a plugin that needs it: the host already downgrades colours, glyphs and links on its own.
- **Not API:** what depends on D10's inline viewport (no sidebar, §8) and the narrow-terminal drop order stay host behaviour.

### 15.4 Desktop-only API

Each contribution is drawn natively by the app from declarative data; a plugin never supplies UI code (§10). Each maps to a control the design system already has.

| Contribution | Manifest `[capabilities.desktop]` | ABI | Native control | On the terminal |
| --- | --- | --- | --- | --- |
| Inspector tab | `inspector = true` | `InitOut.desktop.inspector: Option<TabDecl { title, symbol }>`; filled by `cox_render` with the new slot `desktop.inspector` and the inspector's width in cells; one tab per plugin; §8's redraw model applies (it renders while selected) | DS§6.4 `Inspector`: a tab in the strip after Info (`InspectorTab` gains a plugin case), its tree in a `PluginWidgetView` inside an `InspectorSection` | not offered (§15.6) |
| Toolbar items | `toolbar = true` | `InitOut.desktop.toolbar: Vec<ActionDecl { command, title, symbol }>`, at most 2 per plugin | DS§6.1 `CapsuleStyle(.plain, isIcon: true)` in DS§6.4 `SessionToolbar`, after the plugin segments, the title as tooltip; dropped with the segments when the bar is narrow | not offered |
| Palette actions | `palette = true` | `InitOut.desktop.palette: Vec<ActionDecl>` | DS§6.4 `CommandPalette`, Actions group: a `PaletteKind::Action` row (`crates/cox-app/src/palette.rs`) with id `plugin:<id>:<command>` and the symbol in its well | the same command is already in the TUI palette as `/<id>:<name>` |
| Actionable notifications | `notify = true` | host function `cox_desktop_notify(DesktopNotice { level, title, body, actions })` in `cox:desktop/v1`; at most 2 actions `{ label, command }`; allowed in every export but render; level capped at `warn`; at most one per 10 s per plugin | DT§5.6: posted through `cox_app::app::Host` like an inbox item, its actions mapped back by CoxPlatform's `NotificationActions` (T37.27); posted only when the app is not frontmost or the session is not visible, otherwise shown as a notice | `Err(NotOnThisSurface)`; `cox_notify` is the shared notice |
| Images in widgets | none (shows only bytes the digest covers) | `Widget::Image { path, alt }`: a package-relative PNG path (no `..`, `\` or `:` component, as an `[[agents]]` file), at most 256 KiB, counted by §8's node cap; never a URL, so the app never fetches for a plugin | DS§6.3 `PluginWidgetView` draws the image, `alt` as its accessibility label | the `alt` text as one dim line |

Rules for all of them:

- `ActionDecl.command` names one of the plugin's `InitOut.commands`, so a click goes through `cox_command` and its closed `CommandOut` (§4) — no new export, no new way to submit. An action naming an undeclared command is dropped with a notice. Commands still need `ui.commands`.
- `symbol` is an SF Symbol name matching `[a-z0-9.]{1,64}`. A name the system does not know draws `puzzlepiece.extension`. That `NSImage(systemSymbolName:accessibilityDescription:)` returns nil for an unknown name is **unverified**: Apple's page (https://developer.apple.com/documentation/appkit/nsimage/init(systemsymbolname:accessibilitydescription:)) did not render its text to a fetch on 2026-10-03; the Swift card proves it with a test.
- `CommandOut` gains `OpenInspector`, the desktop counterpart of `OpenOverlay`.
- Every string passes `cox_sanitize::sanitize` in `cox-app` before Swift sees it, as `WidgetView` does today.
- **Not offered (open: the creator has not confirmed this, A134):** a sidebar section — the sidebar lists the window's sessions across projects (DS§6.4 `Sidebar`), while a plugin instance lives in one session (T33.44), so it would need a window-scoped plugin lifetime the host does not have; menu-bar items — the palette covers them and the menu bar is app-wide; plugin-supplied SwiftUI, HTML or web views — never; HTML from MCP servers is `v0.3-mcp-apps.md`'s separate path.

### 15.5 Declaring surfaces

```toml
api = 1
id = "glance"
surfaces = ["terminal", "desktop"]   # where it loads; absent = every surface

[capabilities]                       # shared
ui = { status = true, commands = true }

[capabilities.terminal]              # terminal-only
keys = true

[capabilities.desktop]               # desktop-only
inspector = true
toolbar = true
palette = true
notify = true
```

- `surfaces` takes `terminal`, `desktop`, `headless` and `acp`; absent means all four, so every existing manifest loads where it loads today. An empty list is an error. A UI-only plugin lists the two UI surfaces and skips `cox_init` in headless and ACP runs.
- A surface table whose surface `surfaces` excludes is an error: it asks for something that can never run.
- `ui.keys` stays accepted under `api = 1` as an alias of `terminal.keys` (the loader ORs them). `cox plugin new` and the docs write the new spelling; the alias goes at the next ABI major.
- **Grant lines** (T33.6) gain `terminal.keys`, `desktop.inspector`, `desktop.toolbar`, `desktop.palette` and `desktop.notify`. A stored `ui.keys` line reads as `terminal.keys`, so nobody is asked again. `surfaces` adds no line: it only narrows.
- **One grant covers both surfaces.** A grant stays per digest. The TUI modal and the CLI prompt (§3) list every line, the surface parts marked "terminal only" or "desktop only"; the app has no grant dialog and loads only `Granted` plugins, like headless (§3), so a grant given in the terminal or the CLI covers the app too, and nobody is asked twice.
- `cox plugin list` (text and `--json`) and `cox doctor` show each plugin's surfaces.

### 15.6 Calls to an unavailable surface

Every case fails open (D14) and none traps:

1. **Load.** A plugin whose `surfaces` excludes the session's surface is not loaded. One `Notice(Info)` names where it runs: the author chose it, so it is not a warning.
2. **Grant.** `InitIn.granted` leaves out the other surface's table. Headless and ACP leave out both surface tables and the shared `ui`, since they call no UI export (§8).
3. **`InitOut`.** Entries for a part this surface lacks — `keys` on the desktop, `desktop.*` on the terminal — are dropped like ungranted ones, with one `Notice(Info)` per plugin per session listing them. A plugin that checks `InitIn.surface` returns only its surface's parts and gets no notice.
4. **Host functions.** Every namespace is linked on every surface, because extism resolves a module's imports when it instantiates it and `hostfn.rs` registers one fixed import list. On a surface without that part a surface function returns `Err(AbiError::NotOnThisSurface { surface })`, checked before the grant: saying "not granted" for something that can never work here would mislead.
5. **Values.** `Widget::Image` on the terminal draws its `alt`; `CommandOut::OpenInspector` on the terminal is dropped with a notice; a `Span.link` the surface cannot open draws as text.

### 15.7 Versioning

- **The ABI parts stay `api = 1`** under §4's minor rule. New fields are ignored both ways (`abi.rs` denies no unknown field). A new enum value reaches only a peer that asked for it: `Slot::DesktopInspector` only after the guest declared the tab, `NotOnThisSurface` only from surface functions an older guest never imports, `Widget::Image` only from a guest that saw `surface = desktop`, since an older host rejects the whole tree and counts a miss.
- **The manifest is the exception.** It denies unknown keys (`manifest.rs`), so a cox older than this section refuses a package that uses `surfaces` or a surface table, with "unknown field", and skips it with the usual notice. Accepted: that strictness is a trust property (a misspelt capability must never grant less or more), and the fix is updating cox. An old manifest loads unchanged in a new cox.
- **Namespaces:** `cox:host/v1` (shared), `cox:tui/v1` (reserved) and `cox:desktop/v1` (`cox_desktop_notify`), so a surface part grows without touching `cox:host/v1`. A module that imports `cox:desktop/v1` does not instantiate in an older cox and is skipped with a notice.
- **A breaking change to any part raises the one ABI major** (`api`); the parts do not version apart in the manifest.

### 15.8 Where the code lives

| Crate | Gains |
| --- | --- |
| `cox-plugin-api` | `Surface`; `surfaces`, `TerminalCaps`, `DesktopCaps` in the manifest; `TabDecl`, `ActionDecl`, `InitOut.desktop`, `InitIn.surface`, `DesktopNotice`, `Slot::DesktopInspector`, `CommandOut::OpenInspector`, `AbiError::NotOnThisSurface`; `Span.link`, `Widget::Image` |
| `cox-plugin` | the grant lines and the `ui.keys` alias (`grant.rs`); the session's `Surface` on `HostEnv`, `cox:desktop/v1` and the `NotOnThisSurface` check (`hostfn.rs`); the `InitOut` filter (`live.rs`) |
| `cox-session` | the `SessionSpec.surface` → `Surface` map and the load filter |
| `cox-tui` | links and image `alt` in the widget conversion; `OpenInspector` dropped |
| `cox-app`, `cox-ffi` | the inspector slot, toolbar and palette actions, notifications through `Host`, links and images in `WidgetView`; `plugin_keys`/`plugin_key` removed; the app-server wire carries the new payloads |
| `desktop/macos` | the views in §15.4's table, with DS§6 rows |

`cox-app` stays free of terminal toolkits and `cox-tui` keeps depending on the ABI types only through `cox-protocol` (§8, `deps.rs`).
