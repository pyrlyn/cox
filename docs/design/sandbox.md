# Design: the sandbox (T4.4)

## Problem

Instruction files are requests; the sandbox is the guarantee (D7). The
measurable claim: under `sandbox.mode = workspace-write`, a command that `bash`
spawns cannot (1) write outside the workspace roots and the scratch
directories, (2) write `.git`, `.cox`, `.claude`, `.agents`, `.codex` or `.aws` inside
them, (3) open a
network socket unless `[sandbox].network = true` — on macOS, and on Linux both
with and without bubblewrap. Invariant 11
(`sandbox_denies_write_outside_workspace`), proved by `sandbox_macos_*` and
`sandbox_linux_*` through the real `bash` tool.

## The field

**Claude Code (R§2.1, ledger #11).** macOS: Seatbelt via `sandbox-exec`.
Linux and WSL2: bubblewrap. The sandbox has no network of its own; outbound
traffic leaves over a unix socket that `socat` bridges to a proxy running
*outside* the sandbox, and the proxy enforces a per-domain allowlist. Gain: a
domain-level network policy. Cost: two extra processes per command, a proxy
that must speak every protocol the tools use, and anything that ignores proxy
environment variables goes dark without saying why.

**Codex (R§1.4).** The same pair, no proxy. macOS: generated Seatbelt profiles.
Linux: `codex-linux-sandbox` — bubblewrap when on `PATH` (`--unshare-user
--unshare-pid`, read-only root, network namespace when restricted, `PR_SET_NO_NEW_PRIVS`,
seccomp), else Landlock. `.git` and `.codex` are re-applied read-only inside
writable roots. Modes `read-only` / `workspace-write` / `danger-full-access`;
policies `untrusted` / `on-request` / `on-failure` / `never`. Network is a
boolean per policy.

**Pi (R§2.2).** No in-process sandbox at all; isolation is a container or
micro-VM around the whole agent, which is the user's job.

## cox

One front door: `sandbox::command(policy, roots, cmd) -> Command` is the only
place a shell string becomes an argv, and `bash` has no other way to spawn.
`sandbox::backend()` decides once per session: macOS → `seatbelt`; Linux
`auto` → `bwrap` if a probe run with the same namespaces succeeds (a binary on
`PATH` is not enough — Docker, AppArmor and hardened kernels refuse user
namespaces), else `landlock` if `landlock_create_ruleset` answers, else `None`.
`cox doctor` prints the answer.

| guarantee | seatbelt | bwrap | landlock + seccomp |
| --- | --- | --- | --- |
| no write outside roots | `(deny default)`, `file-write*` allowed per root | `/` bound read-only, writable set bound read-write on top | ruleset: write only on the writable set |
| `.git` read-only inside a root | later `deny file-write* (subpath …)` wins | re-bound read-only after the read-write bind | **cannot** — Landlock only grants (see limits) |
| no network unless allowed | `(allow network*)` only when `network` | `--unshare-net` | seccomp: `connect` and `socket(AF_INET*)` fail with `EPERM` |

The writable set is the roots plus `[sandbox].writable`, the temp dir in every
mode, and `/tmp` and `~/.cache` only in `workspace-write`; `read-only` allows
nothing but the temp dir. Roots are canonicalised before any rule is written
(`/tmp` is `/private/tmp` on macOS) so a rule and the kernel agree on the path.

The sandbox and the permission engine meet at §1.8 step 8 (T4.3):
`on-failure` runs an `Exec` call confined *without asking*; `bash` reports
`structured.sandbox_denied` only when a backend actually confined the run and
the command failed with a denial marker; the loop turns that into
`ApprovalRequired { SandboxDenied }` and reruns unconfined only on an explicit
`Allow`. `danger-full-access` is flag-only, cannot come from project config,
and emits `Notice { Security }` right after `SessionStarted`, which every
surface pins.

Borrowed from Codex: the mode and policy vocabulary, the bwrap → Landlock
fallback, `.git` re-applied read-only. Borrowed from Claude Code: Seatbelt as
generated text that is unit-tested on every platform. Dropped: the socat proxy
— cox's network is a boolean. A domain allowlist needs a proxy outside the
sandbox and is a §6 amendment if a user ever needs it, not a v0.1 promise.
Dropped from Pi: container-as-sandbox — cox must be safe on a laptop without
Docker.

Why at least as good: the same two kernel mechanisms as both incumbents, plus
(a) the bwrap probe, so a host that cannot create user namespaces falls to
Landlock instead of failing every command at run time; (b) denial → ask →
rerun is a tested state-machine transition in `cox-core`, not TUI logic, so
`cox run -p` and ACP get it for free; (c) `doctor` names the backend, so "is
`.git` protected here?" has a one-line answer.

## Windows

None in v0.1. `backend()` returns `None`, `doctor` warns, and the surface that
builds the session forces `on-request` and emits `Notice { Security }` (P5/P6
wiring, noted in T4.2). Recommended: WSL2, where bubblewrap is one package
away and the Linux path applies unchanged. AppContainer or job objects are not
a plan item; adding them is a §6 amendment.

## Known limits (documented, not escapes)

- Landlock cannot re-deny a subpath of a granted directory, so on a Linux host
  without bwrap `.git` inside a writable root is writable.
  `sandbox_linux_keeps_git_read_only_inside_the_root` skips there and `doctor`
  says `landlock`.
- Every backend allows `process-exec`: a confined shell may run any binary,
  and confinement is inherited by the whole process tree — that is the point,
  not a gap.
- Denial detection is textual (`Operation not permitted`, `Read-only file
  system`, …). A plain mode-bit refusal reads the same and costs one extra
  question under `on-failure`; the miss would cost a silent failure.
- Symlinks inside a root that point outside it are resolved by the kernel at
  open time (Seatbelt subpaths, bind mounts, Landlock rules all see the target),
  so `path::confine` is not the last line of defence for the shell — the
  sandbox is.

## Falsifiers

This design is wrong if any of the following turns out true:

1. Any documented escape: a command under `workspace-write` on macOS, or on
   Linux with bwrap, that writes outside the roots, changes `.git/HEAD`, or
   opens a socket with `network = false`.
2. A host where `backend()` reports `bwrap` or `landlock` but the confined
   command runs unconfined — the probe passed, the real run did not apply.
3. A rerun under `danger-full-access` without a matching
   `ApprovalDecided { Allow }` earlier in the same event stream.
4. A common developer command (`cargo build`, `npm install`, `git commit`)
   that cannot complete under `workspace-write` with `network = false` for a
   reason other than the network — the writable set is then wrong, and users
   will reach for `danger-full-access` by default, which defeats D7.

## Review

Think-tier review (Fable 5.1, 2026-09-03). Agree with the shape: one front
door, backend chosen by a probe, denial as a loop transition. Two points to
watch. First, falsifier 4 is the one most likely to fire: toolchains write to
`~/.cargo`, `~/.npm`, `~/.rustup` and the like, and none of those is in the
writable set — expect `on-failure` to ask on the first build of every session
until `[sandbox].writable` learns the common caches or the defaults grow.
Second, the textual denial markers include `Permission denied`, which
ordinary `chmod`-shaped failures also print; the design accepts one spurious
question, but a scripted `bash` test that fails on a real mode bit under
`on-failure` should exist so the false positive stays a question and never
becomes an unconfined rerun without an `Allow`. No escape found in the
profile, the bwrap argv or the Landlock ruleset as written.
