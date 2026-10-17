# `cox app-server` and remote sessions

`cox app-server --stdio` serves the macOS app's session calls over stdin and
stdout, one JSON object per line. The app runs it on another machine over your
own `ssh`. That machine's sessions then show in the app's sidebar as a group of
their own, and they open, stream and take prompts like local ones. The session,
its tools, its sandbox and its keys stay on that machine. Only the protocol
lines cross the connection.

## Running it on a host

Install cox on the host as you would locally (see `docs/getting-started.md`),
so that `cox` is on the `PATH` of a non-interactive ssh login. Then check it by
hand:

```sh
ssh devbox cox app-server --stdio
```

The server reads requests until stdin closes, then closes every session stream
it opened. A running turn stops too; the session can be resumed later with
`open` and `resume`. It writes nothing to stdout except protocol lines; logs go
to cox's log file under `COX_HOME` on the host. At start it reads the login
shell's environment, as the app does locally, so tools such as `cargo` or
`mise` and env-var keys resolve as they would in a terminal there.

A one-line request to try it by hand is in
`crates/cox/tests/fixtures/app-server/projects.jsonl`:

```sh
cox app-server --stdio < crates/cox/tests/fixtures/app-server/projects.jsonl
```

The subcommand is behind the `app-server` cargo feature, which is on by
default and pulls in the `plugins` feature.

## What crosses the wire

Each line is a `request`, a `response` or a `notification`, tagged by `type`
and carrying the protocol version `v` (now `1`). A line with another version
is refused. Every line decodes against the JSON Schema in
`docs/app-server.schema.json`. That file is generated from `cox-app`'s `wire`
module, and a drift test keeps it current.

```json
{"type":"request","v":1,"id":1,"call":{"method":"projects","params":{"limit":20}}}
{"type":"response","v":1,"id":1,"outcome":{"status":"ok","value":{"kind":"projects","value":[]}}}
{"type":"notification","v":1,"event":{"event":"patches","data":{"session":"…","patches":[]}}}
```

- **Requests** are the calls the app makes on a local core:
  - `projects`, `sessions` and `search` read the workspace.
  - `open` opens or resumes a session in a cwd on the host.
  - `send` sends one intent to a session: a prompt, an approval, an answer, a
    mode or model switch.
  - `snapshot`, `expand`, `complete`, `changes` and `plan` read one session.
  - `plugin_action` runs the command a plugin button or action (`plugin:<id>:<command>`)
    names, and answers whether the session's plugins granted it.
  - `close` stops a session's stream and sends `ended`. The server still holds
    the session, and its timeline keeps folding.
  - `attach` streams a session the server still holds again, from its snapshot,
    and answers like `open`. An `open` that resumes a session this server
    already holds attaches to it instead of building a second core.
  - `end` ends a session: its turns stop and what they detached is killed.
  - `inbox` returns the approvals and questions that wait, so a client that
    attaches sees those that arrived while it was away.

  The server answers each with a `response` that carries the same `id`. The
  response is either `ok` with a reply, or `err` with a message. A line that is
  not a request gets `id` 0 and an error.
- **Notifications** are pushed without a request:
  - `patches`: a session's timeline patches, coalesced as they are for the local
    app.
  - `ended`: a session's stream is over.
  - `inbox`: an approval or a question that waits, with the Dock badge.
  - `badge`: the badge alone.
  - `open_url`: a page a tool asked to show. The app drops anything but an
    `http` or `https` link, then asks the person first, naming the host the
    link came from; it opens the page only on Open.
  - `plugin_notice`: a plugin's desktop notice (PL§15.4), already sanitized,
    with at most two buttons that name granted commands. The app posts it as a
    system notification; a button comes back as `plugin_action`. A link in a
    plugin's widget is checked again on the app and goes through the same
    confirmation as `open_url`.

A stalled client never delays a turn. The server keeps at most 64 lines queued
for the writer. Past that, each session's patches fold into the latest state
per block until the client reads again.

**What never crosses it:**

- **Keys.** The server's host answers no secret request. A session on the host
  reads the host's own env vars and keyring, and the protocol has no message
  that carries a key. A test checks that no schema tag names a secret.
- **Your ssh agent.** The app runs `ssh` with `ForwardAgent=no` and
  `ClearAllForwardings=yes`, so neither the agent nor a port reaches the host.
- **Your environment.** `ssh` starts with an empty environment apart from
  `HOME`, `USER`, `LOGNAME`, `PATH` and `SSH_AUTH_SOCK`. The last lets the
  local `ssh` itself authenticate; it is not forwarded.

## How the app connects

**File › Connect to Host…** takes an ssh host alias, such as a `Host` entry in
`~/.ssh/config`. The app then runs:

```sh
/usr/bin/ssh -T -o BatchMode=yes -o ForwardAgent=no -o ClearAllForwardings=yes -- <alias> cox app-server --stdio
```

The alias is refused before anything runs if it is empty, starts with `-`, or
contains whitespace or control characters. `BatchMode=yes` means ssh never
prompts, so the host must accept a key from your agent or your ssh config. A
host that cannot connect shows in the sidebar as disconnected, and the core's
reason shows in the sheet.

A host that connects is saved to `desktop.remote_hosts` in your user config
(see `docs/config.md`), and the app reconnects to it at the next launch. A
project's `.cox/config.toml` cannot set this key: the project-config guard
reverts it and reports the change, because a repository must not choose where
the app opens a shell.

Each host is a sidebar group under a host badge. The app re-reads the group
every few seconds. When the connection drops, the group shows **Disconnected**
with a **Reconnect** button, and its rows are read-only until it reconnects.
A session opened from a host streams through that host's connection. When the
connection drops, the session's stream ends and its running turn stops. The
session and its rollout stay on the host, and `open` with `resume` brings it
back (see *What crosses the wire*). Resident sessions, which would keep running
with no client attached, are planned in `docs/design/serve.md` (T66.12). They
are not built yet.

A remote session in the app does not yet have:

- completions or prompt history;
- the Plan and Info tabs;
- Review, or turn costs;
- expanding a truncated output;
- a terminal pane.

The protocol has no message for these yet, or the app asks for them
synchronously.
