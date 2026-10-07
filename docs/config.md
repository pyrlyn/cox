# cox configuration reference

Generated from `config/default.toml` by a test in `cox-protocol/src/config.rs`; do not hand-edit.

## `[core]`

- `home` = `"~/.cox"` — COX_HOME overrides
- `workspace_roots` = `[]` — empty = git root of cwd, else cwd; extra roots via --add-dir
- `max_turns` = `200` — per UserTurn, counts provider calls
- `parallel_tools` = `4`
- `max_concurrent_subagents` = `8` — cap on running TaskKind::Agent tasks, foreground + background (T34.2)
- `log_level` = `"info"` — tracing filter; file log at ~/.cox/logs/cox.log
- `profile` = `""` — "" (default) | "minimal" (T30.1: the lean prefix); also `cox --profile minimal`
- `mode` = `"editor"` — editor (default) | architect (P42: plan + think); also `cox --mode`, `/mode`
## `[tiers.cheap]`

- `provider` = `"anthropic"`
- `model` = `"claude-haiku-4-5"`
- `effort` = `"low"`
- `max_tokens` = `4096`
## `[tiers.code]`

- `provider` = `"anthropic"`
- `model` = `"claude-sonnet-5"`
- `effort` = `"high"`
- `max_tokens` = `16384`
- `thinking` = `"adaptive"`
## `[tiers.think]`

- `provider` = `"anthropic"`
- `model` = `"claude-fable-5-1"`
- `effort` = `"high"`
- `max_tokens` = `32768`
- `thinking` = `"adaptive"`
- `confirm` = `true` — cannot be set false in project config
## `[jobs]`

- `main` = `"code"`
- `plan` = `"think"`
- `compact` = `"cheap"`
- `title` = `"cheap"`
- `summarize` = `"cheap"`
- `commit` = `"cheap"`
- `memory` = `"cheap"`
- `explore` = `"cheap"`
- `shell` = `"cheap"`
- `agent` = `"cheap"`
- `hook` = `"cheap"`
## `[providers.anthropic]`

- `base_url` = `"https://api.anthropic.com"`
- `api_key_env` = `"ANTHROPIC_API_KEY"` — else keyring entry "cox/anthropic"
- `cache_ttl` = `"5m"` — "5m" | "1h"
- `fallbacks` = `true` — fallbacks: "default" + beta header
- `timeout_s` = `120`
- `max_retries` = `4`
- `models` = `[{id="claude-haiku-4-5", display_name="Claude Haiku 4.5 (latest)", context_window=200000, efforts=["low"], images=true}, {id="claude-sonnet-5", display_name="Claude Sonnet 5", context_window=1000000, efforts=["low", "medium", "high", "xhigh"], images=true}, {id="claude-opus-5", display_name="Claude Opus 5", context_window=1000000, efforts=["low", "medium", "high", "xhigh"], images=true}, {id="claude-fable-5-1", display_name="Claude Fable 5.1", context_window=1000000, efforts=["low", "medium", "high", "xhigh"], images=true}]` — id, context window, efforts per model (effort values from models.dev)
## `[providers.openai]`

- `base_url` = `"https://api.openai.com/v1"`
- `api_key_env` = `"OPENAI_API_KEY"` — else keyring entry "cox/openai"
- `api` = `"responses"` — "responses" | "chat"
- `timeout_s` = `120`
- `max_retries` = `4`
- `models` = `[{id="gpt-5.1", display_name="GPT-5.1", context_window=400000, efforts=["low", "medium", "high"], images=true}, {id="gpt-5.5", display_name="GPT-5.5", context_window=1050000, efforts=["low", "medium", "high", "xhigh"], images=true}, {id="gpt-5.6-sol", display_name="GPT-5.6 Sol", context_window=1050000, efforts=["low", "medium", "high", "xhigh"], images=true}]` — id, context window, efforts per model (effort values from models.dev)
## `[providers.local]`

- `base_url` = `"http://localhost:11434/v1"`
- `api_key_env` = `""` — empty = no key; most local servers need none
- `api` = `"chat"`
- `model` = `"qwen3-coder"`
- `context_window` = `32768` — local servers do not report it
- `timeout_s` = `600` — higher than a remote section's 120: local prefill is slow (T30.23)
- `max_retries` = `4`
- `models` = `[{id="qwen3-coder", context_window=32768, efforts=["low", "high", "xhigh"]}]` — id, context window, efforts per model
## `[providers.lmstudio]`

- `base_url` = `"http://localhost:1234"`
- `api_key_env` = `"LM_API_TOKEN"` — else keyring entry "cox/lmstudio"; neither = no x-api-key header
- `model` = `""` — usually left unset; pin via tiers.code.model / --tier code=<model>
- `context_window` = `0` — 0 = ask the server (GET /api/v1/models), then the model catalog
- `load` = `false` — true = load the model at session start if it is not loaded (with context_window)
- `timeout_s` = `600` — local prefill is slow, same rationale as `local`
- `max_retries` = `4`
## `[providers.typesafe]`

- `base_url` = `"https://api.typesafe.ai"` — client appends /v1/systemone
- `api_key_env` = `"TYPESAFE_API_KEY"` — else keyring entry "cox/typesafe"
- `model` = `"jev-latest"`
- `timeout_s` = `30`
- `max_retries` = `2`
- `models` = `[{id="jev-latest", context_window=128000, efforts=["low"]}]` — decisions are cheap-tier only
## `[providers.deepseek]`

- `base_url` = `"https://api.deepseek.com"` — client appends /chat/completions
- `api_key_env` = `"DEEPSEEK_API_KEY"` — else keyring entry "cox/deepseek"
- `api` = `"chat"`
- `model` = `"deepseek-v4-pro"`
- `context_window` = `1000000`
- `timeout_s` = `120`
- `max_retries` = `4`
- `models` = `[{id="deepseek-v4-flash", display_name="DeepSeek V4 Flash", context_window=1000000, efforts=["low", "high", "xhigh"], images=true}, {id="deepseek-v4-pro", display_name="DeepSeek V4 Pro", context_window=1000000, efforts=["low", "high", "xhigh"], images=false}, {id="deepseek-v4-flash-vision-exp", display_name="DeepSeek V4 Flash Vision Exp", context_window=1000000, efforts=["low", "high", "xhigh"], images=true}]` — id, context window, efforts per model (effort values from models.dev)
## `[providers.openrouter]`

- `base_url` = `"https://openrouter.ai/api/v1"`
- `api_key_env` = `"OPENROUTER_API_KEY"` — else keyring entry "cox/openrouter"
- `api` = `"chat"`
- `model` = `"anthropic/claude-sonnet-5"`
- `context_window` = `1000000`
- `timeout_s` = `120`
- `max_retries` = `4`
- `models` = `[{id="anthropic/claude-sonnet-5", display_name="Claude Sonnet 5", context_window=1000000, efforts=["low", "medium", "high", "xhigh"], images=true}, {id="anthropic/claude-opus-5", display_name="Claude Opus 5", context_window=1000000, efforts=["low", "medium", "high", "xhigh"], images=true}, {id="deepseek/deepseek-v4-pro", display_name="DeepSeek V4 Pro", context_window=1048576, efforts=["high", "xhigh"], images=false}, {id="qwen/qwen3-coder-plus", display_name="Qwen3 Coder Plus", context_window=1000000, efforts=["low", "high", "xhigh"], images=false}, {id="x-ai/grok-4.3", display_name="Grok 4.3", context_window=1000000, efforts=["low", "medium", "high"], images=true}]` — curated coding subset; the full 359-model list lives in models.dev
## `[providers.moonshot]`

- `base_url` = `"https://api.moonshot.ai/v1"`
- `api_key_env` = `"MOONSHOT_API_KEY"` — else keyring entry "cox/moonshot"
- `api` = `"chat"`
- `model` = `"kimi-k2.6"`
- `context_window` = `262144`
- `timeout_s` = `120`
- `max_retries` = `4`
- `models` = `[{id="kimi-k2.6", display_name="Kimi K2.6", context_window=262144, efforts=["low", "medium", "high", "xhigh"], images=true}, {id="kimi-k2.7-code", display_name="Kimi K2.7 Code", context_window=262144, efforts=["low", "high", "xhigh"], images=true}]` — id, context window, efforts per model
## `[providers.z-ai]`

- `base_url` = `"https://api.z.ai/api/paas/v4"`
- `api_key_env` = `"ZHIPU_API_KEY"` — else keyring entry "cox/z-ai"
- `api` = `"chat"`
- `model` = `"glm-5.2"`
- `context_window` = `1000000`
- `timeout_s` = `120`
- `max_retries` = `4`
- `models` = `[{id="glm-5.2", display_name="GLM-5.2", context_window=1000000, efforts=["high", "xhigh"], images=false}, {id="glm-5.3", display_name="GLM-5.3", context_window=1000000, efforts=["low", "high", "xhigh"], images=false}]` — id, context window, efforts per model
## `[providers.gemini]`

- `base_url` = `"https://generativelanguage.googleapis.com/v1beta/openai"` — client appends /chat/completions
- `api_key_env` = `"GEMINI_API_KEY"` — else keyring entry "cox/gemini"
- `api` = `"chat"` — Google calls its OpenAI compatibility beta: https://ai.google.dev/gemini-api/docs/openai
- `model` = `"gemini-3.8-flash"`
- `timeout_s` = `120`
- `max_retries` = `4`
- `models` = `[{id="gemini-3.8-flash", display_name="Gemini 3.8 Flash", context_window=1048576, efforts=["low", "medium", "high"], reasoning_effort=true, images=true}, {id="gemini-3.1-pro-preview", display_name="Gemini 3.1 Pro Preview", context_window=1048576, efforts=["low", "medium", "high"], reasoning_effort=true, images=true}, {id="gemini-3.5-flash-lite", display_name="Gemini 3.5 Flash Lite", context_window=1048576, efforts=["low", "medium", "high"], reasoning_effort=true, images=true}]`
## `[context]`

- `compact_at` = `0.75` — fraction of max_context
- `keep_turns` = `2`
- `microcompact_after_turns` = `6`
- `tool_output_visible_bytes` = `8192`
- `tool_output_head_lines` = `60`
- `tool_output_tail_lines` = `20`
- `dedup_window_turns` = `8`
- `instruction_budget_tokens` = `8000`
- `memory_budget_tokens` = `800`
- `deferred_tools` = `true`
- `system_prompt` = `"default"` — default | minimal (T30.1); `core.profile = "minimal"` implies it
- `repomap_budget_tokens` = `0` — repo map in system[2] (P43); 0 = off until the T43.6 bench
## `[compaction]`

- `strategy` = `"state+llm"` — state+llm | llm (T59.1): state+llm writes files touched and errors seen from the transcript; llm is the opt-out
## `[permissions]`

- `mode` = `"default"` — default | plan | auto | bypass (bypass only via flag)
- `approval` = `"on-request"` — untrusted | on-request | on-failure | never
- `allow` = `[]` — rule strings, §1.8; a project config cannot set allow (its list is ignored)
- `ask` = `[]` — a project config adds rules to this list, never replaces it
- `deny` = `["Read(~/.ssh/**)", "Read(~/.aws/**)", "Bash(rm -rf /*)"]` — a project config adds rules to this list, never replaces it
- `import_claude_settings` = `true`
- `allow_for_session_persists` = `false`
## `[sandbox]`

- `mode` = `"workspace-write"` — read-only | workspace-write | danger-full-access
- `network` = `false`
- `writable` = `[]` — extra writable roots
- `readonly_in_workspace` = `[".git", ".cox", ".claude"]`
- `linux_backend` = `"auto"` — auto | bwrap | landlock | none
## `[budget]`

- `session_usd` = `5.0`
- `monthly_usd` = `100.0`
- `warn_at` = `0.8`
- `cheap_counts` = `true`
## `[tui]`

- `vim` = `false`
- `theme` = `"auto"` — auto | dark | light | a built-in (cox-dark, cox-light, system) or a `~/.cox/themes/<name>.toml` file's stem; `auto` queries the terminal's OSC 11 background colour once, before raw mode, with a 100 ms timeout (T22.6) — tmux, a query error, or no reply within the timeout falls back to `dark`, same as before this query existed. An explicit `dark`/`light` (config file or `COX_TUI_THEME`) always wins over detection; a named theme follows the same read unless its file pins a `variant`. `cox doctor` reports what `auto` resolved to. `/theme` previews and writes this (T24.2).
- `inline` = `true`
- `show_thinking` = `"collapsed"` — collapsed | hidden | full
- `screen_reader` = `false` — the plain surface (T29.1): flat labelled lines, numbered prompts, no cursor movement, BEL when a turn ends; same as `--plain` or `COX_PLAIN=1`
- `mouse` = `true` — the wheel scrolls the transcript, the diff view and pickers 3 lines/rows a tick; no in-app toggle key, so false leaves the terminal's own mouse reporting (and text selection) untouched (T22.4)
- `glyphs` = `"auto"` — auto | unicode | ascii
- `icons` = `{}` — [tui.icons] name = "glyph" overrides one symbol
- `color` = `"auto"` — auto | none | 16 | 256 | true (NO_COLOR forces none)
- `syntax_theme` = `""` — syntect theme for code, diffs and file output ("" follows theme); a `.tmTheme` file in `~/.cox/themes/` is merged in at startup and offered by `/theme` under a `syntax: ` prefix (T24.2)
- `diff` = `"auto"` — auto | side | stacked — edit cards, the approval modal and Ctrl+G split old and new side by side from 120 columns (auto and side alike; narrower stays stacked), stacked never splits; replaced lines highlight the changed words (T24.5)
- `git` = `true` — branch and +n -m in the status line, polled every 2 s
- `notify` = `"auto"` — auto | always | off — OSC 9 (OSC 777 on VTE) plus BEL when a turn ends, an approval waits or ask_user asks; auto only while the terminal is unfocused (focus reporting), always regardless, off never (T23.5)
- `motion` = `"full"` — full | reduced — reduced draws a running tool's spinner as one still glyph and replaces its ticking elapsed time with `running` (T24.7)
- `caps` = `{}` — [tui.caps] name = bool overrides one detected cox_tui::term::Caps field (truecolor, kitty_keyboard, osc8, osc52, osc9, osc9_4, focus, images) for a terminal detection guesses wrong about; unset fields are auto-detected, `cox doctor` shows the source of each (T23.0)
## `[tui.status_line]`

- `command` = `""` — a /bin/sh -c command fed the status JSON (Claude Code's statusLine field names) on stdin; its first output line is one row above the status line, re-run 300 ms after the status changes; runs sandboxed read-only without network, output passes sanitize (colours and links stripped); "" is off; a project config cannot set it (T46.1)
- `refresh_s` = `0` — also re-run every this many seconds (0 = off, at most 3600)
- `timeout_ms` = `2000` — a run that takes longer is killed and the row goes blank (100 to 10000)
## `[hooks]`

- `timeout_s` = `60` — seconds per [[hooks.<Event>]] process (a hook's own timeout_s overrides); stdin carries the Claude Code JSON payload, exit 2 blocks, stdout may carry updatedInput or additionalContext
- `fail_open` = `true` — a hook that crashes, times out or has an invalid matcher regex is warned about and skipped, never fatal (D14). matcher is an exact tool name, or — when it carries a regex metacharacter — a regex over the tool name (T22.3). Events: UserPromptSubmit, PreToolUse, PostToolUse, PostToolUseFailure, Stop, PreCompact, PostCompact, SessionStart (payload source: startup | resume | clear; stdout additionalContext joins the volatile system block), SessionEnd, PermissionRequest, SubagentStart, SubagentStop, Notification (observe-only kind/message/title payload on ApprovalRequired, TurnDone and ask_user)
## `[mcp]`

- `timeout_s` = `30`
- `deferred` = `true`
- `servers` = `{}` — [mcp.servers.<name>] command/args/url/env/sandbox — same shape as .mcp.json, plus sandbox=false to opt a named stdio server out of the sandbox wrap (default true, T33.42)
## `[lsp]`

- `enabled` = `true` — the deferred ReadOnly `diagnostics` tool (P41): one sandboxed stdio LSP server per language per session, killed when the session ends
- `timeout_s` = `30` — seconds per diagnostics request
- `quiet_ms` = `500` — after the last pushed publishDiagnostics, wait this long before taking the result as complete
## `[lsp.servers.rust]`

- `command` = `"rust-analyzer"` — program to spawn, found on PATH, under the same sandbox wrap as an MCP stdio server; [lsp.servers.<name>] is user config only — a project config cannot set lsp.servers (a repository must not choose a program cox runs)
- `args` = `[]` — arguments to command
- `extensions` = `["rs"]` — file extensions, without the dot, this server is asked about
## `[lsp.servers.typescript]`

- `command` = `"typescript-language-server"`
- `args` = `["--stdio"]`
- `extensions` = `["ts", "tsx", "js", "jsx"]`
## `[lsp.servers.python]`

- `command` = `"pyright-langserver"`
- `args` = `["--stdio"]`
- `extensions` = `["py"]`
## `[lsp.servers.go]`

- `command` = `"gopls"`
- `args` = `[]`
- `extensions` = `["go"]`
## `[tools]`

- `project` = `false` — offer the `project` tool: one call runs the project's own check, test, lint or format-check command (detected from justfile, Cargo.toml, package.json, go.mod or pyproject.toml) through the same sandbox and approval path as `bash`, so it asks exactly as `bash` does for that command (T59.5)
## `[project]`

- `check` = `""` — command for `project` action check; empty detects it from the manifests. Runs under `bash`'s sandbox and permission rules, so a project config may set it
- `test` = `""` — command for `project` action test; empty detects it
- `lint` = `""` — command for `project` action lint; empty detects it
- `fmt` = `""` — command for `project` action fmt, a format check that must not rewrite files; empty detects it
## `[voice]`

- `enabled` = `false` — push-to-talk dictation in the TUI (P54), transcribed on this machine with whisper; only in a cox built with the `voice` feature. User config only: a project config cannot set any voice.* key (a repository must not switch the microphone on or choose the model file)
- `model` = `"base.en"` — whisper model: tiny.en, base.en, small.en, tiny, base or small; fetched only by `cox voice model download <name>` after you confirm
- `language` = `"en"` — ISO-639-1 code for whisper, or "auto" to detect
- `key` = `"alt+v"` — press to record, press again to stop; where the terminal reports key releases, hold to record
- `auto_submit` = `true` — submit the transcript like Enter, only when the draft was empty before recording
- `max_seconds` = `120` — longest recording kept in memory; audio is never written to disk
## `[plugins]`

- `enabled` = `true` — WASM plugins (docs/design/plugins.md); only a plugin granted for its exact package digest loads; a project config can turn this off, never on; env COX_PLUGINS_ENABLED, flag --no-plugins. A [plugins.<id>] table is that plugin's own config, passed unchanged to its cox_init as InitIn.config; the plugin validates it
## `[plugins.decide]`

- `min_confidence` = `0.6` — advice with lower or no confidence is ignored and the static pick stands
- `route_ms` = `300` — route's latency budget; a later answer is ignored
- `route_margin` = `0.15` — route offers cheap only when its predicted turn cost (catalog prices, last prefix size, cache write vs read) is at most (1 - route_margin) x the code tier's
- `risk_ms` = `200` — risk = "<plugin id>" may raise a tool call to destructive, never lower it; asked only when that changes the permission outcome
- `approve_hint_ms` = `200` — approve_hint = "<plugin id>" may add a caution note to an approval prompt, never say a call looks safe
- `compact_ms` = `500` — compact = "<plugin id>" may compact before the threshold, never skip a due compaction
- `rank_ms` = `300` — rank = "<plugin id>" may reorder or drop tool_search hits, never add one
- `salience_ms` = `300` — salience = "<plugin id>" scores each extracted memory item; may drop against memory.salience_min, never add or edit one
## `[memory]`

- `enabled` = `true`
- `extract` = `false` — end-of-session extraction on cheap tier
- `dir` = `""` — default ~/.cox/projects/<slug>/memory
- `salience_min` = `0.3` — an extracted item scoring below this against salience's Score is dropped; the plugin cannot move this bar
## `[session]`

- `auto_title` = `true` — after the first turn, one cheap `title` job names the session (A113); a title the user set is never replaced
## `[telemetry]`

- `otel` = `false`
- `endpoint` = `""`
## `[record]`

- `redact` = `true`
## `[desktop]`

- `menu_bar` = `true` — show cox's menu-bar extra: what needs you, what runs, today's spend (T51.14)
- `remote_hosts` = `[]` — ssh host aliases the app connects to, each its own sidebar group (T52.21); user config only — a project config cannot set it
## `[desktop.appearance]`

- `material` = `"frosted"` — frosted | glossy | solid — solid is forced by Reduce Transparency
- `opacity` = `0.42` — window and pane background opacity, 0 (clear) … 1 (opaque); text panels never drop below 0.8
- `blur` = `34` — background blur in pt, 0 … 60 (frosted); glossy reads it as reflection
- `depth` = `1.0` — elevation scale, 0 (flat, standard macOS look) … 1 (full shadows and highlights)
- `specular` = `1.0` — glare: the specular sweep's strength, 0 (none) … 1 (the material's full sweep); Solid and Increase Contrast stay at 0 (T60.8)
- `tint` = `true` — tint the glass from the wallpaper
- `dark_highlight` = `"none"` — none | subtle — the top-edge highlight in dark mode: none (the dark mockup) or white at 10 % of the light one (A109)
- `dark_highlight_scope` = `"controls"` — controls | all — dark_highlight applies to controls (e1) or every lifted level (e1–e4, the user bubble too) (A109)
## `[desktop.transcript]`

- `cross_block_selection` = `true` — a text selection runs across blocks like one document; false clamps it to one block (A67)
- `text_size` = `13.5` — the transcript's prose size in pt at 100 % text size, 10 … 24; code, headings and thoughts keep their size relative to it (A93)
- `line_height` = `1.55` — the prose line height as a multiple of the text size, 1 … 2.5 (A93)
## `[desktop.context]`

- `cache_hit` = `"turn"` — turn | session — the Context tab's cache hit: the last turn's reads, or every call's so far (A104)
## `[desktop.review]`

- `send` = `"queue"` — queue | now — the Review pane's "Send to agent" while a turn runs: queue it behind the turn like a prompt, or send it at once (A108)
## `[external_agents.<name>]`

An ACP agent a new session can be driven by instead of cox's own loop (T52.2, DT§3.3.1). None by default. **User config only**: a project `.cox/config.toml` cannot add, change or widen an entry, because an entry runs a program; the loader reverts it with a warning. cox never installs the agent: you install it, and cox runs the installed program.

- `command` — the program that speaks ACP on stdio: a name found on `PATH`, or an absolute path
- `args` — arguments to `command`
- `key_env` — the agent's own API-key variable. Only this one variable is passed through, by name, next to the child allowlist (`PATH`, `HOME`, `LANG`, `LC_*`, `TERM`, `TMPDIR`, `USER`, `SHELL`); cox's own provider keys stay behind. A key that does not resolve (env var, then keychain) leaves the agent out with one warning
- `writable` — extra directories under your home the agent may write, for its own state (for example `~/.claude`). Each must resolve inside your home, never to your home itself; one that does not refuses the entry with a warning. A project config cannot set it

An external agent always runs under the sandbox wrap. Its file limits are the session's `[sandbox]` ones plus `writable`, but it always has network access, whatever `sandbox.network` says: it has to reach its vendor's API. The agent's model, cost and context are its own: cox writes no usage row for it and shows its cost as "—".

Examples (install the program first; these are not defaults):

```toml
[external_agents.claude]            # "Claude Agent": npm install -g @agentclientprotocol/claude-agent-acp
command = "claude-agent-acp"
args = ["--hide-claude-auth"]       # API key only, never a claude.ai login
key_env = "ANTHROPIC_API_KEY"
writable = ["~/.claude"]

[external_agents.codex]             # "Codex": npm install -g @agentclientprotocol/codex-acp
command = "codex-acp"
key_env = "CODEX_API_KEY"
writable = ["~/.codex"]

[external_agents.gemini]            # "Gemini CLI": npm install -g @google/gemini-cli
command = "gemini"
args = ["--acp"]
key_env = "GEMINI_API_KEY"
writable = ["~/.gemini"]
```

Cursor (`agent acp`, `CURSOR_API_KEY`) comes from the granted Cursor plugin's `[[external_agents]]` entry (`plugins/cursor`), not from this table.

## `~/.cox/keybindings.toml`

Rebinds the TUI's keys (T25.5). Each line is an action id and a key, or a list of keys; dotted ids may be written as TOML tables. The keys you give replace the action's defaults, in every context the action has (`idle`, `running`), and take the key from whatever action held it by default. A missing file means the defaults in `docs/getting-started.md`.

```toml
send = "ctrl+enter"
newline = ["enter", "shift+enter"]
mode.cycle = "shift+tab"
```

- Actions: `send`, `newline`, `send.now`, `interrupt`, `mode.cycle`, `transcript`, `help`, `thinking`, `expand`, `diff`, `plugin.leader`, `background`, `unqueue`, `quit`, `copy`, `copy.all`, `voice`. `@`, `/`, `Ctrl+R` and the keys inside a picker or overlay are fixed; so is `Ctrl+C`.
- `plugin.leader` (default `ctrl+k`) rebinds the leader itself; a plugin's own keys, reachable only as `<leader> <key>`, come from the plugin's manifest, not from here — a clash between two plugins goes to the lower plugin id and `cox doctor` reports it.
- Keys: modifiers `ctrl`, `alt` (`opt`, `meta`), `shift`, `cmd` (`super`), then one key: a character, `enter`, `esc`, `tab`, `space`, `backspace`, `delete`, arrows, `pageup`, `pagedown`, `home`, `end`, `f1`–`f12`. Any case. Chords (`ctrl+x ctrl+s`) are not supported.
- A plain terminal sends the same byte for `Enter` and `Ctrl+Enter`; `ctrl+enter` needs a terminal that reports it (kitty keyboard protocol, see `cox doctor`).
- `~/.claude/keybindings.json` is read first, for the actions both tools have: `chat:submit` → `send`, `chat:newline` → `newline`, `chat:sendNow` → `send.now`, `chat:cancel` → `interrupt`, `chat:cycleMode` → `mode.cycle`, `app:toggleTranscript` → `transcript`, `task:background` → `background`, `app:exit` → `quit`. Its keys are added beside the defaults; this file still wins. Other Claude actions and chords are skipped.
- An unknown action, a bad key or a file that is not TOML is a warning in the transcript and is skipped. `cox doctor` lists those and any key two of your bindings both claim.

## Accessibility

- `--plain` (or `tui.screen_reader = true`, or `COX_PLAIN=1`) swaps the TUI for flat labelled lines a screen reader can follow: numbered prompts, no cursor movement, and a BEL when a turn ends.
- `tui.motion = "reduced"` stops everything that moves by itself. A running tool shows one still glyph and `running` instead of a spinner and a ticking clock.
- `tui.theme = "cox-dark-daltonized"` or `"cox-light-daltonized"` are the built-in themes without a red/green pair. Added lines and success are blue; removed lines and failure are orange. Every state also keeps its glyph (`✓`, `✗`, `+`, `−`), so colour is never the only signal. `/theme` previews both.
- `NO_COLOR` (set and non-empty, while `tui.color` is `"auto"`), or `tui.color = "none"`, prints no colour at all and leaves the terminal's own.

## Status line command

`[tui.status_line]` (T46.4) runs your own command and draws the first line it prints as one row above the built-in status line; the built-in segments stay. An empty `command` is off.

- stdin is one JSON object with Claude Code's statusline field names, so an existing script runs unchanged: `session_id`, `cwd`, `workspace.current_dir`, `workspace.project_dir`, `model.id`, `model.display_name`, `cost.total_cost_usd`, `context_window.used_percentage`, `context_window.context_window_size` and `version`. cox adds `permission_mode`, `sandbox_mode`, `git.branch` and `busy`. `COLUMNS` is the terminal width.
- It runs 300 ms after any of those or the width changes, a newer change kills a run still going, and with `refresh_s` set it also re-runs on that period.
- A run longer than `timeout_ms`, a non-zero exit or empty output blanks the row; it is never fatal.
- It runs under the sandbox, read-only and without network (the session's own policy only under `danger-full-access`), with the environment cleared to the child allowlist plus `COLUMNS`. A host with no sandbox backend gets one warning and no row; the command never runs bare.
- Its output is untrusted: every escape sequence is stripped, so colours and links are dropped, and the row is drawn dim.
- A project `.cox/config.toml` cannot set `command`: it would run on every start in a cloned repository, so the value is reverted with a warning, like the other guarded keys.

## Theme editor

`Ctrl+E` on a colour row of the `/theme` picker (T46.7) opens that theme's 17 tokens with a swatch and the current value. `Up`/`Down` (or `Tab`) move, typing edits the selected value, and every colour that parses is drawn at once; one that does not is marked `invalid colour` and not applied. `Esc` puts back what was drawn before `/theme` opened.

- `Enter` (or `Ctrl+S`) writes `~/.cox/themes/<stem>.toml`, selects it as `tui.theme` and lists it in `/theme` without a restart. It edits each token's half for the background in use (`dark` or `light`).
- A built-in is never overwritten: its edits go to `<name>-custom.toml`, which starts as a copy of the built-in's own file. A user theme is edited in place, keeping its comments and every other key.
- A stem must match `[a-z0-9][a-z0-9._-]{0,63}` with no `..`; any other is refused with a warning, and a failed write is a warning too.
- The file: optional `variant = "dark"` (or `"light"`) and `syntax = "<.tmTheme name>"`, then `[tokens]` with `<token> = { dark = "<colour>", light = "<colour>" }`. A colour is `#rrggbb`, an ANSI index `0`-`255` or one of the sixteen ANSI names. The tokens are text, dim, accent, user, agent, tool, ok, warn, error, diff_add, diff_del, diff_hunk, border, selection, mode_plan, mode_auto and mode_bypass.
