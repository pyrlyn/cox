# Using the Jev plugin

`plugins/jev` is TypeSafe's System One decision model. It answers yes/no,
score and choice questions about cox's own state. It does not write code,
titles or summaries. A tier that names `typesafe` would receive a JSON
decision where it expects a reply, so no tier should use it. The provider
section exists only so a decision call has a ledger row, a budget gate and
a key.

Both decision points are off until you turn them on. State leaves the
machine only for a point you enabled. `risk` can only raise a call's
severity. `route` can only move a main turn from `code` down to `cheap`.
Neither point can be the reason something becomes allowed.

The live E1 and E2 measurements need the creator's `TYPESAFE_API_KEY` (and
`ANTHROPIC_API_KEY` for E2) and are not in `research.md` yet. Until those
tables exist, leave both points unset, which is what the defaults already do.

## Build and install

There is no `just plugin jev` recipe. The guest crate builds with the
repository toolchain:

```bash
mise exec -- cargo build -p cox-plugin-jev --release --target wasm32-unknown-unknown --manifest-path plugins/Cargo.toml
cp plugins/target/wasm32-unknown-unknown/release/cox_plugin_jev.wasm plugins/jev/jev.wasm
cox plugin install plugins/jev --yes
```

Install checks `plugin.toml`, copies the package into `COX_HOME` and grants
`decide: risk` and `decide: route`. Drop `--yes` to answer the prompt
yourself. `just plugin-test` runs the guest tests and does not produce the
`.wasm`.

## Key

The section's `api_key_env` is `TYPESAFE_API_KEY`. cox looks there first,
then in the keyring entry `cox/typesafe`. A missing key fails that one
call open: the point keeps its local answer and the session continues.
`COX_KEYRING=off` skips the keyring, which is what tests and `cargo run` do.

```bash
export TYPESAFE_API_KEY="<your typesafe key>"
```

## Turn a point on

```toml
[plugins.decide]
risk = "jev"
route = "jev"
risk_ms = 30000
route_ms = 30000
```

A point with no plugin id is not asked. `risk_ms` and `route_ms` are the
budgets in milliseconds (defaults 200 and 300). A late answer is ignored.
`min_confidence` (default 0.6) drops a low-confidence answer. `route` also
refuses anything below 0.8 inside the plugin, and offers `cheap` only when
the predicted turn cost is at most `1 - route_margin` (default 0.15) of the
`code` cost.

`risk` thresholds live in the plugin's own table. These are the defaults:

```toml
[plugins.jev.risk]
irreversible = 0.7
external_effect = 0.7
exfiltration = 0.5
severity = 2.5
min_confidence = 0.6
```

An unknown key or a value outside its scale fails `cox_init`. cox warns and
skips the plugin rather than judging with defaults you tried to replace.

## What is sent

One request per question, to `https://api.typesafe.ai/v1/systemone`, model
`jev-1.13.0`. The plugin forwards an allowlist. Anything else, including
tool output that is not the call's own `input`, stays behind.

`risk` (once per tool batch, and only for a call whose raised risk would
change an allow into an ask):

- shared: `task` (the compaction working-state request, clipped), `cwd`,
  `workspace_roots`, `sandbox`
- each call: `tool`, `subject`, `input`, `classifier_risk`

A call is raised to `Destructive` when `irreversible`, `external_effect` or
`exfiltration` is at least its threshold, or `severity` is at least 2.5
with confidence at least 0.6. No raise means no advice, and the classifier
stands. The core then keeps `max(classifier, advised)`, so an answer cannot
lower a risk.

`route` (once per main user turn, never for a subagent, a `Plan` session,
a `/think` turn, or after `/model` pinned the tier):

- `prompt` (the user message, scrubbed the same way as the rollout, clipped)
- `todo`, `last_turn`, `files_touched_count` when the core sent them
- `static`, the tier the router already picked

The questions are a choice of the offered tiers plus `other`, and a yes/no
"the user asks for careful or deep work". The plugin advises `cheap` only
when the choice is `cheap`, confidence is at least 0.8, that yes/no is
below 0.3, and `last_turn` did not end in an error. `other` and `think`
are ignored. Silence keeps the static pick.

## Cost, failure, doctor

Every request that reaches TypeSafe is a `usage` row with job `plugin:jev`.
Input is $0.042 per million tokens and output is free. `cox stats` groups
that job with the others. The `--budget` cap applies to it.

A late `risk` answer emits a notice that the classifier's risk stands. A
late or empty `route` answer leaves the static tier and records no
`Advised` event. A plugin that fails to load is a warning, not a failed
session.

`cox doctor` warns when `tiers.code.provider` is still `typesafe` and the
plugin is not loaded: the section moved, and the session uses the default
tier instead. With the plugin loaded, that row says the section is served
by a plugin.

## Moving an old `[providers.typesafe]` table

An existing table still loads. It sets the base URL and the key env var the
plugin uses. It does not make Jev a tier.

```toml
[providers.typesafe]
base_url = "https://api.typesafe.ai"
api_key_env = "TYPESAFE_API_KEY"
```

Point `tiers.code.provider` at a model provider (`anthropic`, `openai`,
`local`, or one of your `[providers.*]` sections), not at `typesafe`.
