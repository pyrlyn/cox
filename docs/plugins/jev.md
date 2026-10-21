# Using the Jev plugin

`plugins/jev` connects cox to Jev, TypeSafe's "System One" decision model.
Jev does not write text. It answers typed questions about a small state:
a choice among described options, a score on a rubric, or a yes-probability.
cox asks it at a *decision point* and keeps the final say. The core offers
the options, applies a one-way rule, and runs its own default when Jev is
silent, late or unsure (`docs/design/plugins.md` §4, "Decision points").

What Jev is **not**: it is not a model you chat with or route a tier to.
TypeSafe says so itself: Jev "is not a drop-in replacement" for an agent's
LLM, and there is no `model: 'jev-latest'` setting (research J13,
https://docs.typesafe.ai/introduction/coding-agents.md, checked 2026-09-26).
A tier that names the `typesafe` provider gets one warning per session
("typesafe is a decision model; no tier should route to it") and a JSON
decision where it expects text.

## Build and install

No prebuilt Jev archive ships with a release (PL§14 decision 14). Build it
from the repository and install the package directory:

```bash
just plugin jev
cox plugin install target/plugins/jev
```

`just plugin jev` builds `plugins/jev` for `wasm32-unknown-unknown` (the
target comes from the repository's `mise.toml`) and lays out
`target/plugins/jev/` with `plugin.toml` and `jev.wasm`. Install validates
the manifest, copies it into `COX_HOME`, and prints what the plugin asks
for before you grant it:

```
Jev (0.1.0)
  TypeSafe System One decisions (choice, score, yes/no) over cox's own state
  asks to be able to:
    - decide:risk
    - provider:typesafe https://api.typesafe.ai key=TYPESAFE_API_KEY
plugin jev enabled
```

cox asks `[y/N]` before the last line; `--yes` answers it for you. The
grant lets Jev answer `risk` and send its requests to `api.typesafe.ai`
with your key. It does not turn the point on: no question is sent until
`[plugins.decide]` names Jev (below).

## The key

The plugin's provider section is named `typesafe`, as the built-in client
was, so the key is looked up the same way: the `TYPESAFE_API_KEY`
environment variable first, then the platform keyring entry `cox/typesafe`
(service `cox`, account `typesafe`). Never put the key in a config file.

```bash
export TYPESAFE_API_KEY="<your TypeSafe key>"
```

The host adds the key to the request. The plugin never sees it, and it may
reach only `api.typesafe.ai`, only from inside its provider call.

## Turn on a decision point

Every point is off until `[plugins.decide]` names a plugin for it.
Installing the plugin never turns one on (PL§14 decision 11).

```toml
[plugins.decide]
# Jev may raise a tool call's risk, never lower it.
risk = "jev"
# How long a tool batch waits for the answer, in milliseconds. A later
# answer is ignored, and the classifier's risk stands.
risk_ms = 200
```

`risk` is the only point Jev answers today. The `route` advisor (a main
turn's tier, only downward) is not in the plugin yet (T33.40.9). A
`route = "jev"` line is valid config, but Jev does not declare that point,
so it never answers and the static tier pick runs.

### `risk` thresholds

Jev answers four questions for each tool call. A call is raised to
`Destructive` when any answer crosses its threshold. The defaults are the
J§5.1 ones; change them in the plugin's own table:

```toml
[plugins.jev.risk]
irreversible = 0.7     # P(the call cannot be undone by git or the checkpoint)
external_effect = 0.7  # P(it pushes, publishes, deploys, sends or deletes outside the workspace)
exfiltration = 0.5     # P(it sends workspace content or credentials over the network)
severity = 2.5         # severity score, 0 (only reads) … 3 (destroys data)
min_confidence = 0.6   # the confidence a severity score needs to count
```

The probabilities are between 0 and 1 and `severity` is between 0 and 3.
An unknown key or a value out of range makes the plugin refuse to start.
cox then warns, skips the plugin, and every point it would answer stays off.

A raised call goes back through the permission engine. In the TUI you get
an approval prompt. Headless (`cox run -p`), the call is denied. The core
keeps `max(classifier, advised)`, so Jev can never make a call *more*
allowed.

## What leaves your machine

The core asks `risk` only when raising the call to `Destructive` would
change the outcome: `Exec` under a confining sandbox, `Write` under `auto`,
or a rule that allows by risk. Read-only calls, calls that already ask, and
`bypass` mode never reach Jev. One request covers every such call in a
tool batch. It carries exactly these fields and nothing else:

| Field | What it is |
| --- | --- |
| `task` | your current request (the last message you typed), clipped to 2,000 characters |
| `cwd` | the session's working directory |
| `workspace_roots` | the workspace roots |
| `sandbox` | the sandbox mode |
| `items[].tool` | the tool's name, e.g. `bash` |
| `items[].subject` | what the call acts on, such as the command or the path, clipped to 2,000 characters |
| `items[].input` | the call's input as the model wrote it, clipped to 2,000 characters |
| `items[].classifier_risk` | cox's own risk for the call |

Tool output, file contents and the transcript are never sent. The plugin
keeps a fixed list of fields (`plugins/jev/src/risk.rs`), so a field the
core adds later stays out until a plugin update names it.

TypeSafe says it does not train on user data. It offers zero data
retention only to enterprise customers, through sales. Its docs name no
processing location (research J17, https://docs.typesafe.ai/legal.md,
checked 2026-09-26). Turn on a point only if that is acceptable for the
repository you are in.

## Costs

Every Jev request is an ordinary ledger row: model `jev-1.13.0`, provider
`typesafe`, job `plugin:jev`. `cox stats` lists it under that job, next to
your main turns, and the budget gate applies to it as to any request. The
price comes from the plugin's own `[[models]]` rows: $0.042 per million
input tokens, output free. The research estimate for a session with `risk`
on is 10–30 requests and under $0.002 (J§5.1). The real cost is wall time.
TypeSafe quotes 70–500 ms per request, and a tool batch waits for it up to
`risk_ms`.

## When something goes wrong

Jev fails open. A missing key, a network error, a reply later than
`risk_ms`, a reply that does not parse, or one that flags nothing all leave
the classifier's risk in place. The session goes on. A late answer is
recorded as a notice ("plugin jev gave no risk advice within 200 ms; the
classifier's risk stands"). After three failures in a row, the plugin's
`cox_decide` export is switched off for the rest of the session, with a
warning.
Every answer that was used is an `Advised` event in the rollout, with
Jev's probabilities in its note.

`cox doctor` shows one row for the plugin and its price rows:

```
plugin jev: ✓ loaded; models from plugin:jev: jev-1.13.0, jev-latest
```

and one row for its provider section:

```
provider typesafe: ✓ served by the jev plugin; key TYPESAFE_API_KEY found
```

or, with no key:

```
provider typesafe: ⚠ served by the jev plugin; TYPESAFE_API_KEY is not set and keyring entry 'cox/typesafe' not found
  fix: set TYPESAFE_API_KEY
```

## Measurements and defaults

Two evaluations decide whether a point should be recommended:

- **E1, `risk`** (T33.40.7): about 300 labelled commands, each run with and
  without Jev. It measures the recall gain on must-ask commands, the
  false-raise rate on benign ones, the late-answer rate and p50/p95
  latency, with a $0.10 cap. If more than 10 % of benign commands are
  raised, or more than 20 % of answers arrive late at p95, `risk` is not
  recommended.
- **E2, `route`** (T33.40.10): paired coding tasks on real Anthropic tiers
  with a $3 cap. If any task that passes without Jev fails with it, or the
  median saving per task is below 15 %, `route` stays off.

Neither has run yet: E1 waits for a run with the creator's key, and E2
waits for the `route` advisor and the creator's go-ahead. Until results are
recorded in `research.md` §5, the defaults are the conservative ones:
every point is off, `risk` is opt-in with the J§5.1 thresholds above, and
`route` is not offered. This page will carry both result tables once they
exist.

## Migrating from `[providers.typesafe]`

Jev used to be a built-in provider. It moved into this plugin (J§7), and
an old config still loads:

| You have | What happens now |
| --- | --- |
| a `[providers.typesafe]` table | With the plugin installed, its keys (`base_url`, `api_key_env`, `model`, `timeout_s`, `max_retries`, `models`) configure the plugin's `typesafe` section; yours win over the manifest. Without the plugin, the table is ignored and `cox doctor` warns. |
| a tier with `provider = "typesafe"` | With the plugin installed, it reaches the plugin's section (and warns once that Jev is a decision model). Without it, the tier falls back to its default provider and model for the session, with one warning. |
| `[plugins.decide] risk = "jev"` with no plugin | The point stays off. |
| `TYPESAFE_API_KEY` or the keyring entry `cox/typesafe` | Used as before: the section name did not change. |
| ledger rows with provider `typesafe` | Unchanged. Old rows still read in `cox stats`. New rows carry job `plugin:jev`. |

An old table like this one keeps working once the plugin is installed:

```toml
[providers.typesafe]
base_url = "https://api.typesafe.ai"
api_key_env = "TYPESAFE_API_KEY"
model = "jev-latest"
timeout_s = 30
max_retries = 4
```

Without the plugin, `cox doctor` says:

```
providers.typesafe: ⚠ the table is ignored: plugin jev is not loaded
  fix: build `plugins/jev` and run `cox plugin install <dir>`
```

Install the plugin as above, or delete the table.
