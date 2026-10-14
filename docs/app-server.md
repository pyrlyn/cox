# `cox app-server` wire protocol

`cox app-server --stdio` serves the desktop app's session calls over stdin and
stdout, one JSON object per line. The app starts it on another machine through
a target's launcher (ssh, docker, limactl or colima); `docs/targets.md` covers
the targets, how to put cox on them and what they may and may not receive. This
file is the wire between the app and that server.

The session, its tools, its sandbox and its keys stay on the machine that runs
the server. Only the protocol lines cross the connection.

## Serving

The server reads requests until stdin closes, then closes every session stream
it opened. A running turn keeps running on the host. It writes nothing to
stdout except protocol lines; logs go to cox's log file under `COX_HOME` on the
host. At start it reads the login shell's environment, as the app does locally,
so tools such as `cargo` or `mise` and env-var keys resolve as they would in a
terminal there.

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
  - `close` stops a session's stream.

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

## Handshake

The first request on a link is `hello`. It takes no parameters and answers the
host's protocol version (`wire`), cox version, OS, architecture and sandbox
backend, or no backend when the host has none. The app refuses a host that
speaks another protocol version and says which side to update, for example:

> devbox speaks app-server protocol 2 but this app speaks 1; update this app
> so the two match

A cox that is too old to know `hello` gives a plain error instead:

> the cox on devbox is too old for this app (protocol 1); update cox on that
> machine (see docs/targets.md)

The app also reads the launcher's stderr, keeping the last 8 KiB and stripping
escape sequences and bidi controls from it, so a link that closes before
`hello` says why. When the shell reports exit status 127 or `cox: command not
found`, the message is:

> cox is not on the PATH of devbox; install cox there so that a
> non-interactive login can run `cox` (see docs/targets.md)

Any other early close shows the last lines the launcher wrote, for example
`could not reach devbox: ssh: Could not resolve hostname devbox`. A drop after
a successful `hello` is the usual disconnected state.

A stalled client never delays a turn. The server keeps at most 64 lines queued
for the writer. Past that, each session's patches fold into the latest state
per block until the client reads again.

**What never crosses it:**

- **Keys.** The server's host answers no secret request. A session on the host
  reads the host's own env vars and keyring, and the protocol has no message
  that carries a key. A test checks that no schema tag names a secret.
- **The launch environment and the ssh agent.** How a link is started, what
  the launcher is given and what it may forward are the target's rules and are
  in `docs/targets.md`.
