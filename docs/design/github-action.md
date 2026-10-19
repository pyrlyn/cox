# Design: cox in a GitHub Actions workflow (T67.13)

A design gate. No workflow file, action, or Rust change lands with this card.
The creator reviews it before any implementation card is added to `plan.md`.
Claims about today's binary cite `main` at `c0c3b6a1` (2026-10-19).

The scheduled-run half of the same card is `docs/design/schedule.md`. A
GitHub `schedule:` cron is one caller of the shape below, not a second
binary.

## What the binary already does

`cox run -p <prompt>` is the headless surface (`crates/cox/src/cli.rs`,
`Run`). Exit codes are `0` ok, `1` error, `2` denied, `3` budget, `4`
interrupted (`crates/cox/src/run.rs`).

Flags a workflow can pass today, without new code:

| Flag | Effect |
| --- | --- |
| `--budget <usd>` | Overrides `budget.session_usd` (default `5.0` in `config/default.toml`). |
| `--max-turns` | Stops the loop. |
| `--sandbox workspace-write` | Default. `danger-full-access` is flag-only (`cli.rs`). |
| `--approve never` | A call that would ask is denied. Stdin is not read (`run.rs`). |
| `--approve on-failure` | A confined exec runs without a prompt; a sandbox denial still asks. |
| `--permission-mode auto` | Writes are allowed without a prompt (`cox-permission` `by_risk`). `bypass` is flag-only. |
| `--output-format stream-json` | One JSON event per line for the log. |
| `--no-hooks --no-mcp --no-plugins` | The runner does not load repository programs. |

A project `.cox/config.toml` cannot raise `budget.session_usd` above the
user/default layer (`crates/cox-config/src/load.rs`). It also cannot add
hook commands or `mcp.servers`. The CLI flag layer still can, which is why
the workflow, not the repository file, must set the cap.

`sandbox.network` defaults to false. `git push` from the `bash` tool needs
a network and a credential; neither is on by default.

Keys resolve from the environment, then the platform keyring
(`cox_provider::http::resolve_key`). A cargo dev run sets `COX_KEYRING=off`
and never prompts. An Actions runner must do the same.

## The workflow this design allows

Triggers: `workflow_dispatch` and `schedule` on the default branch only.
`pull_request` from a fork must not receive a provider key. `pull_request_target`
is out: it checks out pull-request code with the base repository's secrets.

Permissions on the job: `contents: read`. No `id-token`, no packages, no
pull-requests, unless a later step outside cox (see below) was reviewed to
open a pull request.

`COX_HOME` is a fresh directory under `RUNNER_TEMP`, not restored from
cache. A previous job's session grants, allow-list edits and monthly spend
must not carry over. The session cap passed as `--budget` is the cap that
fires.

The step runs the `cox` already installed on the runner. This design does
not download a release, pin a URL, or build cox inside the job. Installing
cox on a runner is the same rule as P68: cox installs nothing itself.

Suggested invocation, once an implementation card exists:

```text
COX_KEYRING=off COX_HOME="$RUNNER_TEMP/cox" \
  cox run -p "$PROMPT" \
  --budget "$BUDGET" \
  --max-turns 20 \
  --sandbox workspace-write \
  --approve never \
  --no-hooks --no-mcp --no-plugins \
  --output-format stream-json
```

`--approve never` is the unattended default so the job cannot block on
stdin. A write or an unconfined exec is denied (exit 2) instead of applied.
A workflow that wants unattended edits sets `--permission-mode auto` in the
step, not in the repository config. That is a wider mode and the workflow
file on the default branch has to say so. `danger-full-access` and
`bypass` stay out of the action. A person who wants them types the flag
themselves; a workflow file in the repository is not that person.

## Keys

`ANTHROPIC_API_KEY` or `OPENAI_API_KEY` comes from an Actions secret. The
step maps the secret to the variable the provider already reads. The
workflow must not `echo` it, write it into the workspace, or put it in
`$GITHUB_STEP_SUMMARY`. `COX_KEYRING=off` so a missing secret fails the
provider call instead of opening a keyring prompt on a headless runner.

The cox process does not receive `GITHUB_TOKEN`, `GH_TOKEN`, or
`ACTIONS_RUNTIME_TOKEN`. Those stay on the job for steps that run after
cox, if the owner added such a step. Unsetting them for the cox step is
part of the implementation card; today a workflow author has to do it by
hand, and this design says they must.

## What it may push

Nothing, from inside cox. The model can edit the workspace under
`workspace-write`. It cannot push: no network by default, and no GitHub
token in the process.

A following step, not part of cox, may commit the diff and open a pull
request. That step is ordinary GitHub Actions, reviewed like any other
workflow. It must:

- run only on the default branch (`schedule` / `workflow_dispatch`), never
  on untrusted pull-request code;
- push a new branch, never `main`;
- use a token that was not visible to cox.

If that step is absent, the job ends with the workspace diff only in the
log. Exit 3 is the budget cap and should fail the job. Exit 2 is a denial
and should fail the job. Exit 4 is the runner cancelling the step.

## Out of scope

A published action, a composite action in this repository, caching
`COX_HOME`, building cox in CI, and any new flag. Those wait for an
amendment after this document is approved.
