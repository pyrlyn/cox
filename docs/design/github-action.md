# Design: `cox run -p` in a GitHub Action (T67.13)

A design gate. No workflow, action or cox code is added here. Claims about
today's behaviour cite `path:line` on `main` at `c0c3b6a1` (2026-10-19).
The proposed cards at the end wait for the creator's approval in a new
`plan.md` §6 amendment.

## Problem

A repository wants a headless cox turn in CI: a prompt in, a transcript and
a cost out, the job red when the run fails. The runner is untrusted from
the model's point of view (a pull request's code is untrusted input) and
the model is untrusted from the runner's point of view (it must not spend
without a cap, must not read the provider key out of the process, and must
not push).

## What already exists

`cox run -p` is the headless surface (`crates/cox/src/cli.rs:277-323`).
Useful flags, all already parsed:

| Flag | Role |
| --- | --- |
| `--prompt` / `--prompt-stdin` | The prompt. `--prompt-stdin` keeps it off the process list (`cli.rs:281-284`). |
| `--output-format text\|json\|stream-json` | `stream-json` is the log a job can upload (`cli.rs:286-288`). |
| `--max-turns` | Caps provider calls (`cli.rs:289-291`). |
| `--budget <usd>` | Overrides `budget.session_usd` (`cli.rs:54-56`). |
| `--approve` | Overrides `permissions.approval` (`cli.rs:51-53`). |
| `--permission-mode` | `bypass` is flag-only (`cli.rs:44-46`). |
| `--sandbox` | `danger-full-access` is flag-only (`cli.rs:41-43`). |
| `--autonomous` | Gate loop. Gates come from the user's own `[autonomous]` table (`run.rs:601-610`). |

When `--approve` is omitted, a headless run sets `permissions.approval` to
`never` (`run.rs:248-275`). `never` turns an ask into a denial
(`crates/cox-permission/src/policy.rs:32-39` for exec; the engine's
`ApprovalPolicy::Never` arm denies every remaining ask). Nobody is waiting
on stdin.

Exit codes (`run.rs:36-48`, mapped in `run.rs:80-90`): `0` end of turn or
max turns with no denial, `1` error, `2` denied or refused, `3` budget,
`4` interrupted. A non-zero status fails the job.

A project config cannot raise `budget.*` (`crates/cox-config/src/load.rs:164-188`),
cannot set `sandbox.mode = danger-full-access` or `sandbox.network = true`
(`load.rs:147-152`), and cannot set `[autonomous]` at all (`load.rs:487-515`).
`permissions.allow` from a project file is ignored (`default.toml` comment
on `allow`). The checkout under test cannot widen the run.

`bash` clears the child environment and copies only
`CHILD_ENV_ALLOWLIST` (`crates/cox-tools/src/bash/mod.rs:409-414`,
`crates/cox-protocol/src/config.rs:145-147`: `PATH`, `HOME`, `LANG`,
`LC_ALL`, `LC_CTYPE`, `TERM`, `TMPDIR`, `USER`, `SHELL`). Provider keys
and `GITHUB_TOKEN` are not on that list.

## The action

One composite action, published only after this design is approved. It
runs on `ubuntu-latest` (Linux sandbox: bwrap, else landlock, else the
session forces `on-request` and says so — but headless `never` then
denies those asks). It does not install a toolchain beyond the released
`cox` binary. It does not run `cargo`.

### Inputs

| Input | Default | Constraint |
| --- | --- | --- |
| `prompt` | required | Passed on stdin via `--prompt-stdin`, not as an argv element. |
| `max-turns` | `4` | `--max-turns`. |
| `budget-usd` | `1` | `--budget`. A project file cannot raise it. |
| `output` | `stream-json` | `text` or `json` or `stream-json`. |
| `approve` | omitted | Empty, or `never`, or `on-failure`. `untrusted` and `on-request` are rejected: they wait for a person. |
| `permission-mode` | omitted (config default) | `plan`, `default` or `auto`. `bypass` is rejected. |
| `sandbox` | omitted (`workspace-write`) | `read-only` or `workspace-write`. `danger-full-access` is rejected. |

`auto` plus the headless default `never` still allows a `Write` that the
engine allows without asking (`crates/cox-permission/src/lib.rs:483`,
`Risk::Write` in Auto) and still denies every remaining ask
(`lib.rs:198-201`). `on-failure` allows a confined `Exec` without asking
and still denies an unconfined one (`policy.rs:33-39`). That pair is the
widest posture this action offers, and it is opt-in.

### Keys

- The provider key is a GitHub Actions secret, exported as
  `ANTHROPIC_API_KEY` (or the env var the configured provider already
  reads). `COX_KEYRING=off`, so the runner never touches a keyring.
- The action step must not print the environment. `stream-json` is the
  only log. cox already redacts the rollout copy of secret-shaped tool
  output; the action does not add a second redactor.
- The key stays in the cox process. It is not copied into the child
  allowlist. A follow-up card tests that a sandboxed `bash` cannot print
  `ANTHROPIC_API_KEY` or `GITHUB_TOKEN`, including by reading
  `/proc/<parent>/environ` when the backend leaves `/proc` visible. If a
  backend fails that test, the action is not published until the sandbox
  fix lands.
- `actions/checkout` is called with `persist-credentials: false`. A token
  written into `.git/config` would be readable by the model even though
  `.git` is on `readonly_in_workspace`.

### What it may push

Nothing, in the job that runs cox.

- `permissions: contents: read`. No `id-token`, no packages, no
  pull-requests on that job.
- cox has no push command. `git push` from `bash` has no credential: the
  child never sees `GITHUB_TOKEN`, and checkout did not persist one.
- A later job may open a pull request. That job does not have the
  provider key, does not run cox, and pushes only a new branch. It does
  not push the default branch, does not force-push, and does not tag.
  The model never sees that job's token.

### Cost and the ledger

Every provider request inside the run is a `usage` row, as for any other
session. `--budget` is the session cap; `budget::decide` stops the run
and the process exits `3`. The runner's `COX_HOME` dies with the job, so
the action uploads `cox.db` as an artifact and writes `cox stats --json`
into the step summary. The artifact is a copy for a person. The row was
written before the model saw the answer, same as a local run.

`--autonomous` is off unless the user config baked into `COX_HOME` has
gates. A workflow file in the repository cannot name those gates.

## Threats

| Threat | Holds because |
| --- | --- |
| A pull request raises the budget or turns the sandbox off | Project guard list reverts those keys. The action also refuses the two flag-only widenings. |
| The prompt or a file tells the model to print the key | The key is not in the child environment. The `/proc` test is a publish gate. |
| The model pushes to the default branch | The cox job has no write token and no persisted checkout credential. |
| A hook or MCP server from the repo runs on the runner | Fail-open still applies: a broken one is skipped. A stdio MCP server is sandboxed (`doctor`'s mcp-sandbox row). The action does not pass `--approve untrusted`. |
| Two jobs spend the same monthly cap | Each job has its own `COX_HOME` and its own `--budget`. The monthly cap is inside that home, so it does not span jobs. The per-run `--budget` is the cap that matters. |

## Proposed cards

Not added to `plan.md`. They wait for the creator.

1. **Composite action and example workflow.** Files: `.github/actions/cox-run/action.yml`, `.github/workflows/cox-run.example.yml` (workflow_dispatch only, so it does not run on every pull request), `scripts/test_cox_action.py`. The script asserts the input rejections (`bypass`, `danger-full-access`, `on-request`), `persist-credentials: false`, `contents: read`, and `--prompt-stdin`. Check: `python3 scripts/test_cox_action.py`. No new Rust dependency.
2. **Sandbox env regression.** One nextest in `cox-tools`: a sandboxed `bash` on Linux cannot read `ANTHROPIC_API_KEY` from its own environment. The `/proc` case is part of that test and, if it fails, is a sandbox fix in the same card only when it fits the size limit; otherwise it is named and left open.

## Out of scope

Scheduled runs (`docs/design/schedule.md`). A Windows runner. Building cox
inside the action. Publishing the action to a marketplace. Any change to
the permission engine.
