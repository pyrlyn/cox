# Design: scheduled `cox run -p` (T67.13)

A design gate, paired with `docs/design/github-action.md`. No scheduler
and no cox code are added here. Claims cite `path:line` on `main` at
`c0c3b6a1` (2026-10-19). Proposed cards wait for a `plan.md` §6 amendment.

## Problem

A prompt should run when a clock fires, without a person at the keyboard,
and without a second copy of the same run starting because the clock
fired twice. The cost of that run has to land in the ledger. The schedule
itself must not be something a repository can turn on inside a checkout
the user merely cloned.

## Where the schedule lives

Two places, and only the first one is in scope to build after approval.

**GitHub Actions cron.** The schedule is `on.schedule` in a workflow file
in the repository that opted in. cox stores no cron expression for this
path. The run is the action in `github-action.md`: a new session, one
`cox run -p`, then exit. The workflow sets

```yaml
concurrency:
  group: cox-schedule
  cancel-in-progress: false
```

so a second fire waits or is skipped by GitHub rather than starting a
second cox. `contents: read`, the same key and budget rules, and no push
from the cox job.

**Not a project file.** A `.cox/` or `.github/` entry that cox itself
read as "run this prompt every morning" would let a cloned repo spend
the user's provider key. That is the same reason `[autonomous]` is
reverted from project config (`crates/cox-config/src/load.rs:487-515`).
This design does not add a project-level schedule key.

**A user-level schedule is a later amendment, not a card here.** If it
is ever built, the expressions live in the user config under `COX_HOME`,
which a project file cannot replace, and a resident supervisor (T66.13,
still waiting on the T66.12 amendment) is the process that would wake
them. This document does not specify that file.

## Locking

Each scheduled fire opens a **new** session. It does not resume and it
does not attach to a resident session.

The session lock is `cox_store::lock::claim` (`crates/cox-store/src/lock.rs:84`).
A second cox in the same `COX_HOME` that tries to open the same session
id is refused with the holder's surface. A cron fire does not reuse an
id, so the lock's job is the usual one: a person who opened that session
in a TUI is not fighting the job. The job never passes `--resume` or
`--continue`.

On a GitHub runner the lock is local to the job's `COX_HOME`. Two
concurrent jobs do not share a filesystem, so the lock cannot see the
other job. The `concurrency` group above is what stops the overlap.
Both are required: the group for two runners, the session lock for a
runner that somehow started cox twice.

A run that dies leaves a stale lock. The existing lock recovers when
the holding process is gone (`lock.rs` drops the lock with the process).
The design adds no new stale-lock policy.

## The ledger

A scheduled run is an ordinary session. Every provider request writes a
`usage` row before the model sees the next step. `--budget` (default
`1` in the action) is the cap; exit code `3` is the budget stop
(`crates/cox/src/run.rs:47`, `run.rs:87`).

The runner disk is discarded. The action uploads that session's
`cox.db` and the `cox stats --json --session <id>` summary as the
artifact named `cox-ledger-<run_id>`. The artifact does not create a
second ledger. It is how a person reads the rows after the runner is
gone. A missing artifact with a green job is a failed action: the row
existed in the job, and the action's contract is to keep a copy.

`budget.monthly_usd` does not carry across GitHub jobs, because each
job has a fresh `COX_HOME`. The schedule therefore treats
`--budget` per fire as the only cap. Stacking a monthly cap across
runs would mean a durable `COX_HOME` (a self-hosted runner with a
kept directory). That is allowed — point `COX_HOME` at a directory
the workflow caches — and then the existing monthly cap applies with
no new code. The cache must be the user's, not a file committed in
the repository.

## What the prompt may do

The same posture as the action: headless `never` unless the workflow
opts into `on-failure`, no `bypass`, no `danger-full-access`, no
push from the cox job. A scheduled prompt is repository text once it
lives in the workflow file, so it is untrusted input to the model and
is passed on stdin. It is not a system instruction and it does not
enter the cached prefix of some other session.

## Relationship to T66.13

T66.13 is a heartbeat that re-enters a **resident** session with a
recurring prompt. This design starts a process and exits. The two
must not both fire for the same prompt: a repository picks one. The
GitHub cron does not talk to `cox serve`. A later amendment that
folds cron into the supervisor says so explicitly and does not extend
this document in place.

## Proposed cards

Not added to `plan.md`.

1. **Example schedule workflow.** File:
   `.github/workflows/cox-schedule.example.yml`, `workflow_dispatch`
   and a commented `schedule` block, `concurrency` as above, the
   composite action from the github-action card, artifact upload of
   `cox.db`. Check: `scripts/test_cox_action.py` grows one assertion
   for the concurrency group and the artifact name. Still no Rust.
2. **No user-config scheduler in this amendment.** Called out so a
   later card does not grow one inside T67.13's implementation.

## Out of scope

The composite action's input table (`docs/design/github-action.md`).
Resident heartbeats (T66.13). A project-writable cron. Pushing from
the scheduled job. Windows Task Scheduler and launchd.
