# Design: scheduled cox runs (T67.13)

A design gate. No scheduler, no config key and no schema change lands with
this card. The creator reviews it before any implementation card is added
to `plan.md`. Behaviour below is `main` at `c0c3b6a1` (2026-10-19).

How the same run sits in GitHub Actions is `docs/design/github-action.md`.
This file is where a schedule lives, how two fires stay apart, and how the
cost is recorded.

## Not the in-session loop

`/loop` and `cox run --loop` repeat a prompt inside a session that is
already open (`docs/getting-started.md`). They stop at `max_iterations` or
`budget.session_usd`, and they die with the process. A schedule starts a
process when nobody is at the keyboard. It is not an extension of
`cox-ext/src/presence.rs` (that file is a presence record; T66.14 already
says it stays one).

A resident supervisor (`docs/design/serve.md`, T66.12) is a different
boundary. This design does not add a timer to it. The first shape is
something outside cox — cron, a GitHub `schedule:` workflow, launchd —
executing `cox run -p`.

## Where the schedule lives

User config only, next to the other files under `COX_HOME` (default
`~/.cox`, override `COX_HOME`):

```text
~/.cox/schedules.toml
```

One table per named job: `prompt` (non-empty), `cwd`, `interval` (the same
`30s` / `5m` / `1h` grammar as `/loop`), `budget_usd` (optional, default
the user's `budget.session_usd`), `max_turns` (optional). No `command` and
no hook. A job is a prompt, not a program the repository chose.

A project `.cox/config.toml`, a `.github/workflows` file, and a repository
`schedules.toml` cannot add or edit a job. A repository that could would
choose when cox spends money and what it is asked. The GitHub workflow in
the other document is the owner's workflow on the default branch, which is
a different trust decision and is not read as this file.

The loader for this file is an implementation card. Until that card, the
owner's crontab holds the same fields as arguments to `cox run -p`.

## One fire

Each fire is a new session. Resume is off by default: a scheduled prompt
that continues yesterday's transcript will also continue yesterday's
allowances, and a fresh session is the smaller grant.

```text
COX_KEYRING=off \
  cox run -p "$PROMPT" --cwd "$CWD" \
  --budget "$BUDGET" --max-turns "$MAX_TURNS" \
  --sandbox workspace-write --approve never \
  --no-hooks --no-mcp --no-plugins
```

`--approve never` matches the action document: an unattended fire must not
wait on stdin. Exit codes stay the ones in `crates/cox/src/run.rs` (`0` ok,
`1` error, `2` denied, `3` budget, `4` interrupted). Cron should treat
anything but 0 as a failure, except the overlap skip below.

`--permission-mode auto` and `danger-full-access` are not the default. The
person who wants them puts them on that one crontab line.

## Locking

Two locks, and they answer different questions.

1. The session lock (`crates/cox-store/src/lock.rs`) is held by the process
   that drives a session id. A second process that resumes that id is
   refused. A fire that creates a new session takes its own lock and does
   not collide with another session.
2. A per-job lock so the same named job does not overlap itself when a fire
   runs longer than `interval`. Path: `$COX_HOME/run/schedule-<name>.lock`,
   same idea as `cox serve`'s `run/serve.lock` (the directory is `0700`,
   the lock is the kernel's, the text in the file only names the holder).
   The name is the table key, restricted to the characters already allowed
   for a target token so the path cannot escape `run/`.

If the job lock is held, the new fire does not start cox. It exits 0 and
writes one line to stderr: `schedule <name> still running`. Exit 0 because
a late cron overlap is not a failed prompt. The implementation card can
choose a distinct exit code only by amending `run.rs`'s table; this design
does not.

The session lock is not a substitute for the job lock. Two fires of the
same job would otherwise open two sessions and spend twice.

## Ledger

A cost that is not a `usage` row does not exist. `cox run` already inserts
one row per provider call through the core, in the `COX_HOME` database.
`cox stats` reads that database. A local schedule uses the owner's real
`COX_HOME`, so `budget.monthly_usd` (default `100`) accumulates across
fires and is the backstop when every job's `--budget` would otherwise sum
past the month. `cheap_counts` stays as configured.

A skipped overlap writes no usage row, because no request was made.

The GitHub Actions shape uses a fresh `COX_HOME` per job, so the monthly
cap does not accumulate there. Its cap is the `--budget` of that job. That
split is deliberate: a runner has no owner database to trust, and a laptop
does.

No new table in this design. A `schedule_runs` row (name, started, exit
code) would make skips visible in `cox stats` and is an implementation
card, not a requirement for the first crontab-shaped fire.

## Out of scope

Parsing `schedules.toml`, a daemon, a change to `cox serve`, archiving
rules for scheduled sessions (T66.14), and pushing from a fire. Pushing is
the action document's rule: cox does not receive a GitHub token, and the
sandbox network stays off unless that one crontab line turns it on.
