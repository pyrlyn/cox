# Design: resident sessions and a supervisor (T66.12)

A design gate. It settles a crate boundary before any code is written. Every
claim about today's behaviour cites `path:line` on `main` at `ffa246b5`
(2026-10-17). The proposed cards at the end wait for the creator's approval
in a new `plan.md` §6 amendment. T66.13 and T66.14 wait for that amendment.

## Problem

A cox session lives exactly as long as the process that opened it:

- **The TUI** owns its `cox_core::Session` by value
  (`crates/cox-tui/src/app.rs:93-94`), takes its event receiver once
  (`app.rs:104`), and spawns each turn on the runtime that `run_tui` builds
  (`app.rs:190-195`, `crates/cox/src/session.rs:838-839`). When the TUI exits,
  the turn ends with it.
- **The session lock** is an OS advisory lock that the kernel drops when the
  holder exits (`crates/cox-store/src/lock.rs:5-9`). It is claimed in
  `cox_session::open` before anything is built (`crates/cox-session/src/lib.rs:264-269`),
  and the `Store` that writes the rollout holds it (`crates/cox-store/src/lib.rs:78-80`, `:96-108`).
- **`cox app-server --stdio`** stops when stdin closes. The read loop ends
  (`crates/cox-app/src/server.rs:133-155`), `stop` closes every session
  (`server.rs:156`, `:272-280`), `serve` returns, and `main` drops the tokio
  runtime that the arm built (`crates/cox/src/main.rs:172-176`). tokio drops
  spawned tasks at their next yield once the runtime shuts down
  (<https://docs.rs/tokio/1.53.1/tokio/runtime/struct.Runtime.html>, Shutdown
  section, checked 2026-10-17; `Cargo.lock` pins tokio 1.53.1). A turn is one
  of those tasks (`crates/cox-app/src/live.rs:886-895`), so it is dropped too.

`docs/app-server.md` promises the opposite twice: "A running turn keeps
running on the host" (`docs/app-server.md:20-21`) and "the session itself
keeps running on the host" (`docs/app-server.md:118-120`). Both are false
today (question 7).

The goal is a **resident session**: one that outlives its client. A person
closes the laptop lid, the ssh connection drops, or the desktop app quits,
and the turn keeps running on the host. Later, any client of the same user
reattaches and sees the same timeline.

## The field

- **tmux** keeps its server socket in a directory under `TMUX_TMPDIR` or
  `/tmp`. That `tmux-UID` directory "must not be world readable, writable or
  executable". By default "the access list is empty", and the owner and root
  are always allowed (<https://man7.org/linux/man-pages/man1/tmux.1.html>,
  checked 2026-10-17). This is the model of a per-user socket that a client
  reattaches to.
- **prime-agent** (`PrimeIntellect-ai/prime-agent` at `afe8d14c`,
  2026-10-09; read through the GitHub contents API on 2026-10-17) runs a daemon
  with one worker per session. `crates/pa-daemon/src/supervisor/supervision.rs`
  sets `BASE_BACKOFF_MS = 250`, `MAX_BACKOFF_MS = 30_000`,
  `MAX_CONSECUTIVE_FAILURES = 5` and `STABLE_LIFETIME_MS = 30_000`. Those are
  the numbers the T66.12 card asks for. The same directory has
  `adoption.rs`, which lets a restarted daemon re-adopt workers that outlived
  it. cox does not take adoption (decision D7). The study is idea-only and
  clean-room, as for all of P66: no code is copied.
- **Unix socket permissions are not portable.** On Linux, connecting to a
  stream socket needs write permission on the socket file, but "on some
  systems (e.g., older BSDs), the socket permissions are ignored", and
  "Portable programs should not rely on this feature for security". The same
  page says `SO_PEERCRED` returns the peer's credentials
  (<https://man7.org/linux/man-pages/man7/unix.7.html>, checked 2026-10-17).
  The path limits are 108 bytes on Linux (same page) and 104 bytes on macOS:
  `char sun_path[104]` in XNU's `bsd/sys/un.h`
  (<https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/un.h>,
  `main`, checked 2026-10-17).
- **tokio** already provides the pieces cox needs:
  `tokio::net::UnixStream::peer_cred()` returns a `UCred` with `uid()`,
  `gid()` and `pid()`, and `pid()` is implemented on Linux and Apple platforms
  (<https://docs.rs/tokio/latest/tokio/net/unix/struct.UCred.html>, 1.53.x,
  checked 2026-10-17). The workspace already enables `features = ["full"]`
  (`Cargo.toml:54`), so the design needs **no new dependency**.

## Decisions

### D1. The attach protocol is the app-server wire (question 1)

**Yes.** The app-server wire becomes the attach protocol. There is no second
enum in `cox-protocol`.

- The wire already covers what a client needs to drive a session: open or
  resume, send an intent, snapshot, expand, complete, changes, plan, plugin
  calls, close (`crates/cox-app/src/wire.rs:55-132`). It also covers the
  pushed patches, inbox, badge, URL and plugin notices (`wire.rs:192-226`).
  The wire is versioned (`wire.rs:30-32`), and a drift test pins its schema
  (`wire.rs:502-512`, `docs/app-server.schema.json`).
- Its payloads are `cox-app` types: `Block`, `TimelinePatch`, `InboxItem`
  (`wire.rs:25-28`). `cox-protocol` sits below `cox-app` and cannot name those
  types, so a copy there would be a second shape that drifts.
- Input is already complete. `Intent` covers everything a person submits:
  send, queue, approve, answer, interrupt, compact, mode, model, effort,
  rewind, revert, fork, handoff, shell, command and rename
  (`crates/cox-app/src/intent.rs:21-135`). `dispatch` turns each intent into
  the core's `Submission`.

The wire is missing three calls, and all three are additive (card S2):

- `attach { session }` sends the stream of a session that is already open
  to a client again. It answers like `open`, with `Opened { session, blocks,
  warnings }`.
- `end { session }` runs `LiveSession::end` (`live.rs:862-870`), which
  `close` deliberately does not do.
- `inbox` returns `App::inbox` (`crates/cox-app/src/app.rs:225`). A client
  that reattaches then sees an approval or a question that arrived while no
  client was attached. Today an inbox item is only pushed once
  (`server.rs:66-71`).

`VERSION` stays `1`, because a v1 client never sends the new methods. An
older server cannot decode a line with a method it does not know, and it
answers with `id` 0 (`server.rs:139-142`). A client that needs `attach` should
therefore check `cox --version` before it relies on it. That costs nothing
today, because no client sends `attach` yet.

**Rejected.** A new `Attach`/`Detach` enum in `cox-protocol` that carries
raw `Event`s. It duplicates `Call`/`Reply`, needs its own schema and drift
test, and the desktop app, which is the one client that needs reattach
today, would still have to fold the events.

### D2. Serve carries patches; the TUI stays an in-process client (question 2)

The TUI consumes **raw `Event`s**, not `cox_app` patches. It turns each event
into `Msg::Event` (`crates/cox-tui/src/app.rs:177-178`, `crates/cox-tui/src/state.rs:492`)
and folds it in its own `update` (`state.rs:998`). That fold is the 4,029-line
`state.rs`, built for its inline-scrollback model (`app.rs:5-11`).

Decision: the first serve cards carry patches only. The clients they serve
are the desktop app (local, or over ssh) and `cox attach --stdio`. The TUI
keeps owning its `Session` exactly as today, and a TUI session is never
resident.

A TUI that attaches to `cox serve` is a separate, later design. It has two
possible paths:

- **(a) The TUI folds patches.** This rewrites the TUI's state model. It is
  out of proportion to this gate.
- **(b) An `events` subscription on the same wire, keyed by rollout `seq`.**
  The core already hands every recorded event and its `seq` to the one
  `EventTap` (`crates/cox-core/src/session.rs:761-765`, `:944-954`), but the
  surface channel carries no `seq` (`session.rs:766`). The tap is a
  `OnceLock` that plugins use. A fan-out tap would be needed so that a
  replay (`rollout_read` up to `seq` N) and the live tail (after N) meet with
  no gap and no duplicate.

(b) is the likely path. It stays proposed until a TUI-attach card is
approved, and no card for it is proposed here.

### D3. One supervisor, one worker process per session (question 3)

`cox serve` is the **supervisor**. Each resident session runs in its own
**worker** process. The supervisor starts a worker by re-executing its own
binary (`std::env::current_exe()`), never a program found on `PATH`, with a
hidden `--worker` flag, and gives it piped stdin and stdout.

**The worker is the existing server loop.** It runs `cox_app::server::serve`
(`server.rs:112-161`) over its pipes, with two changes (card S3):

- It hosts at most one session.
- On EOF it **ends** that session instead of only closing its stream.

The supervisor owns no `Session` and claims no session lock. It reads the
workspace (`projects`, `sessions`, `search`) through its own `App`, which
opens SQLite and nothing else. It routes every call that names a session to
that session's worker.

**Why one process per session.** The lock is per process: claims within one
process share the lock by design (`lock.rs:10-13`, `:77-98`). A crash, an
`abort` or an OOM kill then takes down one session, not all of them. The
kernel frees exactly that session's lock (`lock.rs:7-9`), and the supervisor
can restart exactly that session.

**Restart policy** (card S4, a pure function the supervisor calls on each
worker exit):

- A worker exit is **clean** after an `end` call, after the supervisor's own
  shutdown, or with exit status 0. A clean exit is never restarted.
- Any other exit is a **failure**. A failed worker restarts after 250 ms,
  then 500 ms, 1 s, 2 s and 4 s, doubling up to a 30 s cap. After 5
  consecutive failures the supervisor gives up.
- A worker that lived at least 30 s resets the failure count before its exit
  is counted.
- A restart reopens the session with `resume`. That rebuilds the history
  from the rollout (`crates/cox-session/src/lineage.rs:107-113`) and the
  timeline from the same rollout (`live.rs:115-120`).
- **The restarted worker is idle.** The turn that was running when the
  worker died is **not** run again. A model call and a tool call have side
  effects, and the rollout already records the turn up to the crash, with a
  truncated last line tolerated (`crates/cox-store/src/rollout.rs:5-9`).
- A session the supervisor gave up on shows as **failed** in the
  supervisor's listing. Only an explicit `open` from a client tries it again,
  and that `open` starts a fresh failure count.

**Cap.** The supervisor runs at most 16 workers. An `open` past the cap gets
an `err` response that says so. There is no config key for the cap in the
first cut (YAGNI). A later `[serve]` section, if one is ever added, is
user-layer only, the way `desktop.remote_hosts` is
(`crates/cox-config/src/load.rs:155`).

**Rejected alternatives:**

- **One process for every session**, which is today's `app-server` model. A
  panic in one plugin host or tool kills every session. Two sessions also
  share one lock table (`lock.rs:77-80`), so one process could accidentally
  open the same session twice (finding F1 below).
- **Threads with `catch_unwind`.** An abort or an OOM kill still takes the
  whole process down.
- **Self-daemonizing**, the double fork that tmux does. `cox serve` runs in
  the foreground and logs to cox's log file. Keeping it alive is the job of
  the user's terminal, `nohup`, launchd or systemd. A daemon that starts
  itself would be a process the user never asked for.

### D4. The socket: under `COX_HOME`, owner-only, same uid (question 4)

The supervisor listens on `COX_HOME/run/serve.sock` (card S5):

1. **Run directory.** Create `COX_HOME/run` with mode `0700`. If it already
   exists, require that it is a directory, not a symlink, owned by the
   effective uid, and with no group or other bits. Otherwise refuse to start,
   and name the path and the mode. This is tmux's check on its `tmux-UID`
   directory.
2. **Singleton.** Take an OS lock on `COX_HOME/run/serve.lock` with the same
   mechanism as the session lock (`lock.rs:100-123`). Extract
   `lock::claim_path` from `claim` so that there is one implementation. A
   second `cox serve` on the same `COX_HOME` is refused, and the refusal names
   the holder (`lock.rs:57-67`).
3. **Stale socket.** Only the singleton holder may unlink a leftover
   `serve.sock`. It then binds the socket with umask `077` and sets mode
   `0600` on it.
4. **Path length.** If the path would exceed `sun_path` (104 bytes on macOS,
   108 on Linux), refuse with an error that names the limit. There is no
   fallback to `/tmp`: a shared directory is exactly the surface tmux has to
   defend.
5. **Who may attach.** Only a peer whose `peer_cred().uid()` equals the
   supervisor's effective uid. Every accepted connection is checked before
   its first line is read. A mismatch is dropped and logged. File modes
   alone are not trusted, because unix(7) warns that some systems ignore
   them. Root gets no exception: root can read the socket anyway, so there
   is nothing to defend against root.
6. **Remote clients.** A remote client never gets a network port.
   `ssh <host> cox attach --stdio` runs a bridge on the host as that user.
   The bridge connects to the socket and copies lines in both directions,
   so ssh's own authentication decides who reaches the socket. This keeps
   the remote-control gate's rule that cox opens no inbound port by default
   (`docs/design/v0.3-remote-control.md:67-74`, `plan.md` A123).
7. **Windows.** `cox serve` and `cox attach` refuse to run on Windows with a
   "not supported yet" error. A named pipe with an owner-only DACL is a
   follow-up idea for the Windows host agent, not a card here.

**Rejected:**

- **A TCP loopback port with a token file.** It is reachable by every local
  user up to the token check, and it duplicates what ssh already does.
- **A per-run token on the unix socket.** The uid check already pins the
  socket to the one user, and a token file would add one more secret to keep
  in `COX_HOME`.

### D5. The session lock moves to the worker (question 5)

The worker opens its session through the unchanged path:
`App::open` → `LiveSession::open` → `cox_session::open_with_keys`
(`app.rs:286-304`, `live.rs:94-153`), which claims the lock first
(`cox-session/src/lib.rs:264-269`). The lock then lives exactly as long as
the worker process.

- **The supervisor never claims a session lock.** It never opens a rollout
  writer.
- **A session that another process drives** (a TUI, the desktop app, a
  `cox run -p`) is refused with `SessionBusy` (`lib.rs:83-91`). The client
  gets `AppError::Busy` with the holder (`app.rs:84`, `:134`). Serve never
  takes a session over.
- **The reverse also holds.** `cox --resume <id>` on a resident session is
  refused and names the worker's pid. The read-only `Follow` still works,
  because it never claims (`lineage.rs:115-143`).
- **The worker keeps `surface: "app"`** (`live.rs:149`), because plugins pick
  their surface from it (`crates/cox-session/src/plugins.rs:120-125`). A
  `"serve"` string would map to `Terminal`. As a side effect, the holder text
  reads `cox app (pid …)`.

### D6. Reattach replays only what the store already has (question 6)

There is exactly one record of a session: its JSONL rollout
(`rollout.rs:5-9`). Serve keeps no journal, no transcript copy and no second
dialect.

- **Reattach to a live worker writes nothing.** Today `close` aborts the
  controller's drain (`crates/cox-app/src/controller.rs:217-223`), so a
  timeline that was closed stops folding and could not be streamed again.
  A detach in serve therefore stops only the worker's pump, never the
  controller. The controller keeps folding with bounded memory: one queued
  patch per changed block (`controller.rs:96-124`). `attach` answers with
  `Controller::snapshot` (`controller.rs:128-135`) and starts a new pump. The
  timeline it returns is the rollout folded at open (`live.rs:115-120`) plus
  every live event since then.
- **A restarted worker replays the rollout.** It rebuilds the history and
  the timeline from the rollout through `cox_store::Store::rollout_read`
  (`crates/cox-store/src/lib.rs:554`, `:1201`), as every resume does today.
  The cost is known and accepted: a top-level session appends one
  `StateChanged` line each time it reopens (`session.rs:649-667`). Only a
  restart reopens, so reattach never adds that line, which is what the
  acceptance test checks.
- **Never call `open` with `resume` to reattach a live session.** It would
  build a second core on the same rollout (finding F1). The supervisor routes
  an `open` whose `resume` names a live worker to `attach` instead.

**One attached client per session.** A new `attach` takes the stream over,
and the previous client receives `ended` for that session. That rules out
two people answering one approval. Several clients may still attach to the
supervisor at once, each to different sessions. A stalled client can delay
only its own sessions: each worker has its own pipe and its own 64-line
queue (`server.rs:32-34`), and its controller coalesces behind them.

### D7. Lifetimes and shutdown

The supervisor **owns** its workers. It does not adopt workers that outlived
it, which is prime-agent's approach.

- **SIGINT or SIGTERM to the supervisor.** It sends `end` to every worker,
  closes their stdin, waits up to 10 s, then sends SIGKILL to the ones still
  running.
- **The supervisor dies abruptly.** Each worker reads EOF on stdin, ends its
  session (D3, card S3) and exits. The kernel frees the locks. The next
  `cox serve` starts clean, and every session can be resumed from its
  rollout.

**Rejected: adoption.** A worker that survives its supervisor holds the lock
with no one to talk to. Re-adopting it needs a second channel to that worker
(a socket per worker) and discovery at boot. That is more code and more
surface for a crash case. Ending the session loses at most the turn that was
running, and the rollout records that turn up to its last line.

### D8. Fix the behaviour, and correct the doc first (question 7)

The doc promise in `docs/app-server.md:20-21` and `:118-120` comes true only
with serve. `cox app-server --stdio` stays what it is: one client, sessions
in-process, everything ends when stdin closes. Card S1 corrects the doc now
so it states today's behaviour: the turn stops and the session can be
resumed. Card S8 adds the serve section.

Card S3 also changes one thing in `app-server`: on EOF it **ends** its
sessions instead of closing them. Today's `stop` only calls `close`
(`server.rs:274-277`), so `Session::end`, which kills the bash processes a
turn left detached (`session.rs:795-811`), never runs before the runtime is
dropped.

The desktop app's ssh command stays `cox app-server --stdio` (`docs/app-server.md:100`).
Choosing `cox attach --stdio` for a host that runs `cox serve` is a desktop
card for P52, out of scope here.

## Trust boundaries

Serve adds a process and a socket, and it relaxes no guard:

- **`cox_permission::Engine`.** A worker builds its session through
  `cox_session::open`, the same path the desktop app uses. Every tool call is
  allowed, denied or escalated there. Serve has no approval policy of its
  own.
- **`cox_sandbox::sandbox::Policy` and `path::confine`.** They are resolved
  from the session's effective config, and the project-config guards apply
  (`crates/cox-config/src/load.rs`). Serve adds no config key that a project
  could set.
- **`cox_sanitize::sanitize`.** Patches are the same folded blocks the
  desktop app receives today, and the client shows them the way it shows a
  local session.
- **Keys.** The worker's `Host` is `ServerHost`, which never answers a secret
  (`server.rs:93-96`). Keys come from the host's own environment and
  keyring. No call carries a key, and a test already checks that no schema
  tag names a secret (`wire.rs:476`). Workers inherit the environment that
  the supervisor read once from the login shell (`server.rs:102-104`). A
  worker does not run the login shell again.
- **The socket** is a trust boundary of its own: a peer that connects can do
  anything the user can. It is pinned to the user's uid by an owner-only
  directory, an owner-only socket and a peer-credential check (D4). Every
  line from the socket is still decoded and refused like any other app-server
  line (`server.rs:139-142`).

**What the supervisor may do without the user:**

- restart a failed worker under D3's policy, idle and with nothing
  re-submitted;
- give up after 5 consecutive failures;
- end every worker on its own shutdown;
- refuse an `open` past the cap, or one that is busy elsewhere;
- send `ended` to a client whose stream another `attach` took over.

**What the supervisor may never do:**

- answer an approval or a question;
- submit a prompt, queue a turn or re-run an interrupted turn;
- change a mode, a model, the permission mode or the sandbox;
- write config;
- open a session that no client asked for;
- take over a session that another process holds.

The one planned exception is T66.13's heartbeat. It is a recurring prompt
that the user configures explicitly, and never from a project. Its turn
runs under the same `Engine`, so a heartbeat can never approve its own tool
calls. T66.13 defines it once these cards are approved.

**A detached approval waits.** An approval with no client attached is never
auto-approved and never auto-denied. The turn waits, the item stays in the
worker's inbox, and `inbox` returns it on the next attach (D1).

## Failure modes

| Failure | What happens | Where |
| --- | --- | --- |
| A worker panics, aborts or is OOM-killed | The kernel frees its lock. The supervisor restarts it, idle, under D3. The attached client gets `ended` and may `attach` again. | `lock.rs:7-9` |
| A worker fails 5 times in a row | The supervisor gives up and lists the session as failed. Only an explicit `open` tries again. | S4 |
| The supervisor dies | Workers see EOF, end their sessions and exit. Locks are freed. The rollouts are intact up to the last line written. | D7 |
| A client disconnects mid-turn | The turn keeps running. The controller keeps folding and coalescing. `attach` returns the snapshot. | `controller.rs:96-135` |
| A client stalls | Its worker's 64-line queue fills, then the worker's controller coalesces. Other sessions are unaffected. | `server.rs:32-34`, `:283-306` |
| Two clients attach one session | The newer one takes the stream. The older one gets `ended`. | D6 |
| `open` with `resume` of a session already live | It is routed to `attach`, never to a second core. | D6, F1 |
| The session is held by a TUI or the app | `SessionBusy` with the holder. Serve never takes it over. | `cox-session/src/lib.rs:264-269` |
| A second `cox serve` on the same `COX_HOME` | Refused by `serve.lock`, which names the holder. | D4 |
| A stale `serve.sock` after a crash | The lock holder unlinks it and binds again. | D4 |
| The run directory is open to others, or is a symlink | Refused at start. | D4 |
| The socket path is over `sun_path` | Refused at start, naming the limit. | D4 |
| A peer with another uid | Dropped before its first line is read. | D4 |
| The worker cap is reached | `err` on `open`. | D3 |
| The disk is full when the rollout is written | `emit` fails and the turn reports it, as today. | `session.rs:761-764` |
| A worker is SIGKILLed while a `bash` it detached still runs | The process survives: `bash` makes its own session leader (`crates/cox-tools/src/bash/mod.rs:432-447`), and `Session::end` never ran. The same is true of a SIGKILLed TUI today. A Linux subreaper is a later idea. | known gap |
| `COX_HOME` on NFS | flock is unreliable there, as it already is for the session lock. Not addressed. | out of scope |

## Findings while reading

- **F1.** `cox app-server` can build two cores for one session in one
  process. `Call::Open { resume: Some(id) }` for an `id` that the same
  server already holds calls `App::open` again (`server.rs:189-197`,
  `app.rs:286-304`). Nothing checks `App::is_open` (`app.rs:466`), and a
  second claim in the same process shares the first lock by design
  (`lock.rs:92-97`). The result is two `Store` rollout writers, each with its
  own `next_seq` (`rollout.rs:31-44`). Card S2 makes such an `open` attach
  instead.
- **F2.** On EOF, `app-server` closes its sessions but never ends them
  (`server.rs:274-277`), so detached bash processes are not killed before
  the runtime drops. Card S3 fixes it.
- **T66.14 needs no serve code.** For it, "resident" can mean "its lock is
  held", which `Store::session_holder` already answers without claiming the
  lock (`crates/cox-store/src/lib.rs:110-114`). The archive sweep can skip a
  held session today. Its dependency on T66.12's cards could shrink to this
  design alone. That is the creator's call.

## Out of scope

- Any code (this task).
- A TUI that attaches to serve (D2).
- The desktop app choosing `cox attach` per host (P52).
- Windows named pipes (D4).
- Adopting workers that outlived the supervisor (D7).
- A `[serve]` config section, idle exit for workers, and launchd or systemd
  unit files.
- Heartbeats (T66.13) and archiving (T66.14), beyond the notes above.
- Network listeners of any kind. The remote-control gate is still deferred
  (`plan.md` A123).

## Acceptance test for the later cards (question 8)

This is card S9, against a scratch `COX_HOME` with the scripted provider
(`COX_PROVIDER=scripted`, `COX_SCENARIO`, as `crates/cox/tests/remote.rs:59`
sets them):

1. Start `cox serve` in the background. Wait for `run/serve.sock`.
2. Client A runs `cox attach --stdio`. It sends `open` for a new session in
   a scratch cwd, then counts the rollout's lines (M).
3. Client A sends one prompt. It reads patches until the turn ends, then
   records the rollout's line count (N, with N > M) and the blocks it
   folded with `coalesce::apply`.
4. Kill client A with SIGKILL. Assert that the worker still holds the lock:
   `Store::session_holder` is `Some`.
5. Client B runs `cox attach --stdio` and sends `attach` for that session.
   Assert three things:
   - `Opened.blocks` equals A's folded blocks;
   - the rollout still has exactly N lines, and its `seq` runs 1..=N without
     a repeat;
   - lines M+1..=N are that one turn's events, from `TurnStarted` to
     `TurnDone`.
6. Send SIGTERM to `cox serve`. Assert that the lock is free and that the
   socket was removed.

## Proposed cards

These are proposals only. They are **not** in `plan.md` and get ids when
the creator's §6 amendment adds them. Each card keeps to the cox rules:
at most 200 lines of code and at most 3 code files. A crate's doc rows
(`AGENTS.md` layout, `docs/design/crates.md`, `plan.md` §1) and the
generated schema are listed separately. No card adds a dependency.

**S1. `docs/app-server.md` says what a dropped connection does.**

- Files: `docs/app-server.md`.
- Size: about 10 lines, docs only.
- Content: replace "A running turn keeps running on the host" and "the
  session itself keeps running on the host" with today's behaviour: the
  turn stops, the session can be resumed, and resident sessions are planned.
- Check:

  ```bash
  ! grep -n "keeps running on the host" docs/app-server.md
  mise exec -- cargo nextest run -p cox --test docs
  ```

**S2. Wire: `attach`, `end` and `inbox`; detach keeps the fold; no second core.**

- Depends: S1.
- Files: `crates/cox-app/src/wire.rs`, `crates/cox-app/src/server.rs`,
  `docs/app-server.md` (the call list). Generated:
  `docs/app-server.schema.json`.
- Size: about 140 lines.
- Content: `Call::Attach`, `Call::End` and `Call::Inbox`, with
  `Reply::Inbox`. `close` stops the pump and sends `ended`, but no longer
  closes the controller. `open` with `resume` of an id this server holds
  attaches instead (F1).
- Tests:
  - `close_then_attach_streams_from_the_snapshot`;
  - `open_of_an_open_session_attaches_instead_of_a_second_core`;
  - `end_ends_the_session`;
  - `inbox_returns_an_item_that_arrived_while_detached`.
- Check:

  ```bash
  mise exec -- cargo nextest run -p cox-app server wire
  ```

**S3. Worker mode in `cox-app`'s server: one session, ended on EOF.**

- Depends: S2.
- Files: `crates/cox-app/src/server.rs`.
- Size: about 80 lines.
- Content: a `ServeMode::{Client, Worker}` for `serve`. A worker refuses a
  second `open`. On EOF both modes **end** their sessions (F2). A worker
  skips `load_login_env`.
- Tests:
  - `eof_ends_every_session`;
  - `a_worker_refuses_a_second_session`.
- Check:

  ```bash
  mise exec -- cargo nextest run -p cox-app server
  ```

**S4. New crate `cox-serve`: the restart policy.**

- Files: `crates/cox-serve/Cargo.toml`, `crates/cox-serve/src/lib.rs`,
  `crates/cox-serve/src/restart.rs`. Docs: the workspace `Cargo.toml`
  members line, an `AGENTS.md` layout row, a `docs/design/crates.md` row.
- Size: about 120 lines.
- Why a crate: it owns a trust boundary (who may connect) and process
  supervision. It must stay out of `cox-app`, which `cox-ffi` links into the
  macOS app (`crates/cox/tests/deps.rs:619-622`), and out of `crates/cox`,
  which is clap and dispatch only.
- Content: `restart.rs` is pure. `Policy::on_exit(uptime, clean) ->
  Restart(Duration) | GiveUp | Done`, with 250 ms doubling to 30 s, give-up
  after 5 failures and a reset after 30 s of life.
- Tests:
  - `delays_double_from_250ms_to_a_30s_cap`;
  - `gives_up_after_five_consecutive_failures`;
  - `thirty_seconds_of_life_resets_the_count`;
  - `a_clean_exit_is_never_restarted`.
- Check:

  ```bash
  mise exec -- cargo nextest run -p cox-serve restart
  ```

**S5. The socket: run directory, singleton lock, bind, peer uid.**

- Depends: S4.
- Files: `crates/cox-store/src/lock.rs` (extract `claim_path` from `claim`,
  same behaviour), `crates/cox-serve/src/socket.rs`,
  `crates/cox-serve/src/lib.rs`.
- Size: about 170 lines.
- Content: all of D4 except the bridge and Windows. `socket.rs` is
  `#[cfg(unix)]`.
- Tests:
  - `a_second_supervisor_is_refused_naming_the_holder`;
  - `a_stale_socket_is_replaced`;
  - `a_run_dir_open_to_others_is_refused`;
  - `a_symlinked_run_dir_is_refused`;
  - `a_path_over_sun_path_is_refused`;
  - `the_socket_is_owner_only`;
  - `admit_refuses_another_uid` (a pure predicate, because a test cannot
    fake a peer uid);
  - the existing `lock` tests stay green.
- Check:

  ```bash
  mise exec -- cargo nextest run -p cox-serve socket
  mise exec -- cargo nextest run -p cox-store lock
  ```

**S6. Supervision: spawn workers, watch them, restart them, shut down.**

- Depends: S3, S4.
- Files: `crates/cox-serve/src/worker.rs`,
  `crates/cox-serve/src/supervisor.rs`.
- Size: about 190 lines.
- Content: spawn `current_exe() serve --worker` with piped stdio. Apply S4
  on each exit; a restart reopens the session with `resume`. Enforce the cap
  of 16. On SIGINT or SIGTERM, send `end` to every worker, wait 10 s, then
  SIGKILL. A worker that the supervisor gave up on is listed as failed.
- Tests: a fake worker program, as `crates/cox/tests/remote.rs:51` fakes
  `ssh`, covers:
  - `a_crashed_worker_is_restarted_idle_with_resume`;
  - `give_up_marks_the_session_failed`;
  - `shutdown_ends_every_worker`;
  - `the_cap_refuses_the_seventeenth_open`.
- Check:

  ```bash
  mise exec -- cargo nextest run -p cox-serve supervisor
  ```

**S7. Router and bridge.**

- Depends: S2, S5, S6.
- Files: `crates/cox-serve/src/router.rs`, `crates/cox-serve/src/attach.rs`,
  `crates/cox-serve/src/lib.rs`.
- Size: about 190 lines.
- Content:
  - The accept loop and the per-connection uid check.
  - Workspace calls are answered by the supervisor's `App`.
  - Calls that name a session are routed by session id; `open` with
    `resume` of a live worker becomes `attach`.
  - One attached client per session: the previous client gets `ended`.
  - Worker notifications are forwarded to the attached client.
  - `attach.rs` is the `--stdio` bridge, `tokio::io::copy_bidirectional`
    between stdio and the socket.
- Tests:
  - `workspace_calls_need_no_worker`;
  - `session_calls_reach_their_worker`;
  - `a_second_attach_ends_the_first_stream`;
  - `open_resume_of_a_live_worker_attaches`.
- Check:

  ```bash
  mise exec -- cargo nextest run -p cox-serve router
  ```

**S8. `cox serve` and `cox attach --stdio`.**

- Depends: S7.
- Files: `crates/cox/src/cli.rs`, `crates/cox/src/main.rs`,
  `docs/app-server.md` (a "Resident sessions" section that names both
  commands, which `docs_name_every_subcommand` requires).
- Size: about 80 lines.
- Content: both commands sit behind the existing `app-server` feature. The
  hidden `--worker` flag is not shown in `--help`. On Windows both refuse
  with "not supported yet". `cox attach` never starts a supervisor: with no
  `cox serve` running it fails, naming the socket path.
- Check:

  ```bash
  mise exec -- cargo nextest run -p cox --test docs
  COX_HOME=/tmp/cox-scratch mise exec -- cargo run -- serve --help
  ```

**S9. End-to-end: kill the client, reattach, and the rollout grows only by the turn.**

- Depends: S8.
- Files: `crates/cox/tests/serve.rs` (test only).
- Size: about 200 test lines.
- Content: the acceptance test above. Every process the test starts is
  killed at the end, including on failure, through a drop guard.
- Check:

  ```bash
  mise exec -- cargo nextest run -p cox --test serve
  ```

**Order:**

- S1 can land at once.
- Then S2 → S3.
- S4 runs in parallel with S2 and S3, and S5 follows S4.
- S6 follows S3 and S4.
- S7 follows S2, S5 and S6.
- Then S8 → S9.

After S9:

- T66.13 (heartbeat) gets its files and steps. Its scheduler belongs in the
  supervisor, and its prompt goes through `send` like a client's.
- T66.14 can go earlier, given the T66.14 note under "Findings while reading".
