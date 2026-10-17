# Run targets

A **target** is a machine or a container where a whole cox session runs: the
agent loop, the model calls, the tools, the sandbox and the keys are all on the
target. Your machine, the client, sends what you type and shows what comes
back. The wire between them is described in `docs/app-server.md`.

A target is one string, `kind:name`:

| Kind | Written as | The client reaches it with | The target must be |
| --- | --- | --- | --- |
| ssh | `ssh:devbox` | `ssh` | a host that accepts your key without a prompt |
| docker | `docker:web` | `docker exec` | a running container |
| lima | `lima:default` | `limactl shell` | a running Lima instance |
| colima | `colima:dev` | `colima ssh` | a running Colima profile (its VM) |

Four surfaces use a target:

- `cox --on <target>` opens the TUI on the target;
- `cox run --on <target> -p "…"` runs a headless turn there;
- the desktop app's **File > Connect to Target**;
- an `agent` preset with a `target` (see below).

## Put cox on the target

cox does not install itself on a target, and none of these commands installs
anything. You install cox there, as you would on any machine:

- Install a build for the **target's** OS and CPU, not your client's. Install
  paths are in `docs/getting-started.md` (build from source) and the release
  installer in `docs/site.md`.
- Make sure `cox` is on the PATH of the non-interactive login the client uses.
  Each of these should print a version with no prompt:

  ```sh
  ssh devbox cox --version
  docker exec web cox --version
  limactl shell default cox --version
  colima ssh --profile dev -- cox --version
  ```

- `cox --on` and `cox run --on` need only `cox`. The desktop app also needs
  `cox app-server`, which a default build includes.
- In the desktop app, a missing `cox` is reported with the target's name and an
  install hint, and a target that speaks another wire version is refused by
  name. Both come from the `hello` handshake before a session opens
  (`docs/app-server.md`). `cox --on` and `cox run --on` show whatever the
  launcher prints.

## Name a target

A target can be typed as its literal `kind:name`, or given a name in your user
config:

```toml
[targets.devbox]
target = "ssh:devbox"
cwd = "/srv/app"
```

`cwd` is the directory on the target the session starts in. A name in
`[targets.*]` wins over a literal `kind:name` with the same text. The full keys
and the `[launchers]` table are in `docs/config.md` under
`[targets.<name>]` and `[launchers]`. A saved desktop host in
`desktop.remote_hosts` reads as an `ssh:<alias>` target.

## Each kind

Every example below uses the same four surfaces. The `agent` preset is a
`[external_agents.<name>]` entry with a `target`; the allow rule it needs is
explained under "What the trust rules mean for you".

### ssh

cox runs the target's program through your local `ssh`:

```sh
ssh -T -o BatchMode=yes -o ForwardAgent=no -o ClearAllForwardings=yes -- devbox cox …
```

The host is anything `ssh` accepts: an alias from `~/.ssh/config`, or
`user@host`. `BatchMode=yes` means `ssh` never asks for a password or a
passphrase, so the host must accept your key from your ssh agent or your ssh
config. The TUI (`cox --on`) asks for a terminal with `-t`; the desktop app and
`cox run --on` run without one, with `-T`.

Worked example:

```toml
[targets.devbox]
target = "ssh:devbox"
cwd = "/srv/app"

[external_agents.devbox-agent]
target = "devbox"

[permissions]
allow = ["TargetAgent(ssh:devbox)"]
```

```sh
cox --on devbox
cox run --on devbox -p "fix the failing test" --output-format stream-json
```

- Desktop: **File > Connect to Target**, kind SSH, name `devbox`.
- Agent: the model calls `agent` with `preset: "devbox-agent"`.

### docker

cox runs the target's program through the client's `docker`:

```sh
docker [--context <context>] exec -i [-t] web cox …
```

The container must be running. `--context` is docker's global flag, so it comes
before `exec`; it is set in config only, with the `context` key. `docker exec`
here is given no `-e`, so the container receives none of your environment: its
keys are the ones in its own environment.

Worked example:

```toml
[targets.web]
target = "docker:web"
context = "staging"

[external_agents.web-agent]
target = "web"

[permissions]
allow = ["TargetAgent(docker:web)"]
```

```sh
cox --on web
cox run --on web -p "fix the failing test" --output-format stream-json
```

- Desktop: **File > Connect to Target**, kind Docker, pick `web` from the
  running containers the app found (`docker ps`), or type the name.
- Agent: `agent` with `preset: "web-agent"`.

A container is not a sandbox. See "The sandbox inside a container".

### lima

cox runs the target's program through `limactl shell`:

```sh
limactl shell [--tty=false] default cox …
```

`--tty=false` is set for `cox run --on`, where no terminal is wanted. Lima has
a launcher on macOS and Linux only.

Worked example:

```toml
[targets.dev-vm]
target = "lima:default"

[external_agents.vm-agent]
target = "dev-vm"

[permissions]
allow = ["TargetAgent(lima:default)"]
```

```sh
cox --on dev-vm
cox run --on dev-vm -p "fix the failing test" --output-format stream-json
```

- Desktop: **File > Connect to Target**, kind Lima, name `default`.
- Agent: `agent` with `preset: "vm-agent"`.

The rule is `TargetAgent(lima:default)`, not `TargetAgent(lima:dev-vm)`: the
rule names the target the preset resolves to, which is `kind:name`.

### colima

cox runs the target's program through `colima ssh`, which reaches the VM of a
Colima profile:

```sh
colima ssh --profile dev -- cox …
```

Colima has a launcher on macOS and Linux only. Its VM is Linux, so the sandbox
the remote `cox` finds is whatever that VM's kernel gives it.

Worked example:

```toml
[targets.colima-dev]
target = "colima:dev"

[external_agents.colima-agent]
target = "colima-dev"

[permissions]
allow = ["TargetAgent(colima:dev)"]
```

```sh
cox --on colima-dev
cox run --on colima-dev -p "fix the failing test" --output-format stream-json
```

- Desktop: **File > Connect to Target**, kind Colima, name `dev`.
- Agent: `agent` with `preset: "colima-agent"`.

## Desktop app

**File > Connect to Target** opens a sheet with a kind picker (SSH, Docker,
Lima, Colima), a list of the targets of that kind, and a free-text name. The
list holds the targets in your config and, for Docker, Lima and Colima, the
running ones the app finds; for SSH it lists your saved hosts
(`desktop.remote_hosts`) and does not search for others. Before the first
session opens, the sheet shows the target's `hello` result: its OS, CPU, cox
version and sandbox backend, or that it has none, in which case cox asks before
every command.

Each connected target is a sidebar group, named by its ssh alias or by its
`kind:name`. Only ssh hosts are saved and reconnected when the app starts; a
container or VM comes from `[targets.*]` or from the running list. When its connection drops, the group
shows **Disconnected** with a **Reconnect** button, and its rows are read-only
until it reconnects. A session streams through its target's connection; when
the connection drops, the stream ends, and the session keeps running on the
target.

A remote session in the app does not yet have:

- completions or prompt history;
- the Plan and Info tabs;
- Review, or turn costs;
- expanding a truncated output;
- a terminal pane.

## What the trust rules mean for you

- **No key crosses the link.** The launcher starts with an empty environment
  apart from a short list per kind (`HOME`, `USER`, `LOGNAME`, `PATH`, `LANG`,
  `TERM`, `COLORTERM`, and the Docker, Lima or Colima variables that the tool
  needs). Your provider keys are not in that list. The remote `cox` reads its
  keys from its own environment and keyring; the protocol has no message that
  carries a key.
- **No ssh agent crosses the link.** `ForwardAgent=no` and
  `ClearAllForwardings=yes` keep your agent and any port off the host. Your
  local `ssh` still uses `SSH_AUTH_SOCK` to log in to the host; that is the
  only use.
- **Your prompt never goes on the remote command line.** `cox run --on` and an
  `agent` preset send it as one JSON line on stdin. The remote command line
  holds only fixed words and checked names.
- **Names and arguments are checked.** A target name cannot start with `-` or
  contain whitespace or control characters. A `cwd`, a `context` and each
  argument that goes to the remote `cox` are limited to letters, digits and
  `- _ . , / : = @ + \`. `ssh`, `limactl shell` and `colima ssh` join their
  arguments into one string for the remote shell, so a quote or a `;` would
  otherwise be read there.
- **Targets and launchers are your decision.** `[targets.*]`, `[launchers]`,
  `[external_agents.*]` and `desktop.remote_hosts` are user config only. A
  project's `.cox/config.toml` cannot set them; the loader reverts them with a
  warning. A repository cannot choose where your session runs.
- **Launchers are found at fixed absolute paths.** `ssh`, `docker`, `limactl`
  and `colima` are looked up at paths such as `/usr/bin/ssh`, the Homebrew
  directories and Docker Desktop's directory on Windows. `PATH` is never
  searched. To use another copy, set `[launchers]` or the target's `program`;
  a path that is relative or does not exist is refused.
- **`--on` forwards only what it knows.** `cox --on` and `cox run --on` forward
  `--model`, `--mode`, `--permission-mode`, `--sandbox`, `--max-turns`,
  `--output-format`, `--resume` and `--continue`. Any other flag is refused by
  name, not dropped.
- **Agent presets on a target need an allow rule.** Every call to such a preset
  needs `TargetAgent(<kind:name>)` in your own `permissions.allow`. `bypass` and
  `auto` do not waive it, because the work runs on a machine the local sandbox
  does not cover. Without the rule the call is refused and the refusal names
  the rule to add. The preset's own `model`, `mode` and `sandbox` are not
  forwarded; the remote `cox` uses its own configuration.

## The sandbox inside a container

A container is not a sandbox. The remote `cox` applies the sandbox that it finds
on its own machine, under its own `[sandbox]` settings:

- on Linux, bubblewrap or Landlock (`linux_backend`, `auto` by default);
- on macOS, Seatbelt.

A target with no sandbox backend, such as a container without bubblewrap or
Landlock, or a Windows host, has commands that run unconfined. The remote
session then forces `on-failure` to `on-request` and shows a security notice
saying so (D7), so a command asks before it runs. `hello` reports which backend
the target has, and the desktop sheet shows it before the first session. Running
`cox doctor` on the target prints its sandbox check too.

## Limits

- Windows: an OpenSSH server on Windows is an `ssh:` target. It has no sandbox
  backend, so its sessions run `on-request`. Windows targets are still being
  verified on a real machine.
- Docker `context` is set in config only; the desktop app lists running
  containers but not contexts.
- Lima and Colima have no Windows launcher.
