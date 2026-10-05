// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The clap command tree (plan.md §1.12). Kept separate from `main.rs` so
//! `config_load`'s `every_flag_has_a_config_key` test and the flag→config-key
//! table (`config_load::flag_key_map`) can inspect `Cli::command()` without
//! pulling in dispatch logic.

use std::path::PathBuf;

use clap::{ArgAction, Args, Parser, Subcommand};

/// Root command: `cox [PROMPT] [subcommand] [global flags]`.
#[derive(Parser, Debug, Clone)]
#[command(name = "cox", version, about = "cox — a modular terminal coding agent")]
pub struct Cli {
    /// First-turn prompt for the interactive TUI.
    pub prompt: Option<String>,

    #[command(subcommand)]
    pub command: Option<Command>,

    /// Resume a specific session in the interactive TUI.
    #[arg(long, value_name = "ID", conflicts_with = "continue")]
    pub resume: Option<String>,
    /// Continue the latest session for this cwd in the interactive TUI.
    #[arg(long, conflicts_with = "resume")]
    pub r#continue: bool,

    // ---- Global flags (plan.md §1.12 "Global:" row) ----
    /// Override the `code` tier's provider.
    #[arg(long, global = true, value_name = "NAME")]
    pub provider: Option<String>,
    /// Override the `code` tier's model.
    #[arg(long, global = true, value_name = "ID")]
    pub model: Option<String>,
    /// Override one tier's model, `TIER=MODEL` (repeatable).
    #[arg(long, global = true, value_name = "TIER=MODEL")]
    pub tier: Vec<String>,
    /// Override `sandbox.mode`. `danger-full-access` is flag-only.
    #[arg(long, global = true, value_name = "MODE")]
    pub sandbox: Option<String>,
    /// Override `permissions.mode`. `bypass` is flag-only and shows a persistent banner.
    #[arg(long = "permission-mode", global = true, value_name = "MODE")]
    pub permission_mode: Option<String>,
    /// Override `core.mode`: `architect` (plan + think tier) or `editor`.
    /// In `cox run`, `--mode architect` is also the think-tier consent.
    #[arg(long = "mode", global = true, value_name = "MODE", value_parser = ["architect", "editor"])]
    pub mode: Option<String>,
    /// Override `permissions.approval`.
    #[arg(long, global = true, value_name = "POLICY")]
    pub approve: Option<String>,
    /// Override `budget.session_usd`.
    #[arg(long, global = true, value_name = "USD")]
    pub budget: Option<f64>,
    /// Assembled-prefix profile: `minimal` keeps the core tools, the short
    /// system prompt and no skills/memory index (T30.1).
    #[arg(long, global = true, value_name = "NAME")]
    pub profile: Option<String>,
    /// Run as if started from this directory.
    #[arg(long, global = true, value_name = "DIR")]
    pub cwd: Option<PathBuf>,
    /// Add an extra workspace root (repeatable).
    #[arg(long = "add-dir", global = true, value_name = "DIR")]
    pub add_dir: Vec<PathBuf>,
    /// Work in the git worktree `_worktrees/<repo>-<NAME>` on branch `NAME`,
    /// created if missing; the main checkout stays readable as a second root.
    #[arg(long, global = true, value_name = "NAME")]
    pub worktree: Option<String>,
    /// Override `core.home` (same effect as the `COX_HOME` env var).
    #[arg(long, global = true, value_name = "DIR")]
    pub home: Option<PathBuf>,
    /// Increase log verbosity: `-v` debug, `-vv` trace.
    #[arg(short = 'v', long = "verbose", global = true, action = ArgAction::Count)]
    pub verbose: u8,
    /// Machine-readable output where supported.
    #[arg(long, global = true)]
    pub json: bool,
    /// Disable hooks for this invocation.
    #[arg(long = "no-hooks", global = true)]
    pub no_hooks: bool,
    /// Disable MCP servers for this invocation.
    #[arg(long = "no-mcp", global = true)]
    pub no_mcp: bool,
    /// Load no WASM plugins for this invocation (T33.6).
    #[arg(long = "no-plugins", global = true)]
    pub no_plugins: bool,
    /// Plain surface for screen readers: labelled lines, numbered prompts,
    /// no cursor movement (also `COX_PLAIN=1`, `tui.screen_reader`).
    #[arg(long)]
    pub plain: bool,
}

/// Top-level subcommands (plan.md §1.12); `main.rs` dispatches each one.
#[derive(Subcommand, Debug, Clone)]
pub enum Command {
    /// Headless run: `cox run -p <prompt>`.
    Run(RunArgs),
    /// List / search rollouts.
    Sessions(SessionsArgs),
    /// Print archived tool output by id.
    Expand(ExpandArgs),
    /// Usage and cost stats.
    Stats(StatsArgs),
    /// Read or write config.
    Config(ConfigArgs),
    /// Report why cox will or will not work on this machine.
    Doctor,
    /// Re-record a provider cassette.
    Record(RecordArgs),
    /// Serve built-in tools over MCP stdio.
    Mcp(McpArgs),
    /// Agent Client Protocol server on stdio.
    Acp,
    /// Serve this machine's sessions to a remote desktop app (T52.19).
    #[cfg(feature = "app-server")]
    AppServer(AppServerArgs),
    /// Scaffold an AGENTS.md for the repo.
    Init(InitArgs),
    /// Instruction files, skills, commands, agents, hooks, MCP servers in effect.
    Ext(ExtArgs),
    /// Discovered plugins and their declared capabilities (T33.4).
    #[cfg(feature = "plugins")]
    Plugin(PluginArgs),
    /// Self-update the binary.
    #[command(name = "self")]
    SelfUpdate(SelfUpdateArgs),
    /// Push-to-talk dictation: the local whisper models (T54.5).
    #[cfg(feature = "voice")]
    Voice(VoiceArgs),
}

/// `cox voice model list|download <name>` (T54.5).
#[cfg(feature = "voice")]
#[derive(Args, Debug, Clone)]
pub struct VoiceArgs {
    #[command(subcommand)]
    pub action: VoiceAction,
}

/// `cox voice` subcommands.
#[cfg(feature = "voice")]
#[derive(Subcommand, Debug, Clone)]
pub enum VoiceAction {
    /// The whisper models cox can download and use.
    #[command(subcommand)]
    Model(VoiceModelAction),
}

/// `cox voice model` subcommands.
#[cfg(feature = "voice")]
#[derive(Subcommand, Debug, Clone)]
pub enum VoiceModelAction {
    /// Every pinned model with its size and whether it is downloaded.
    List,
    /// Download one pinned model and verify its SHA-256.
    Download {
        /// Model name from `cox voice model list` (`base.en`).
        name: String,
        /// Download without asking (required when stdin is not a terminal).
        #[arg(long)]
        yes: bool,
    },
}

/// `cox app-server --stdio` (T52.19, DT§4.4): the app-server protocol for
/// one client on stdin and stdout; stdio is the only transport, so the flag
/// is required and a later transport gets its own.
#[cfg(feature = "app-server")]
#[derive(Args, Debug, Clone)]
pub struct AppServerArgs {
    /// Serve on stdin and stdout (what `ssh <host> cox app-server --stdio` runs).
    #[arg(long, required = true)]
    pub stdio: bool,
}

/// `cox self update [--version v]` (plan.md §1.12/T12.2).
#[derive(Args, Debug, Clone)]
pub struct SelfUpdateArgs {
    #[command(subcommand)]
    pub action: Option<SelfUpdateAction>,
}

/// `cox self` subcommands.
#[derive(Subcommand, Debug, Clone)]
pub enum SelfUpdateAction {
    /// Update to the latest release (or `--version vX.Y.Z`).
    Update {
        /// Release tag to install (`v0.1.0`); default is latest.
        #[arg(long, value_name = "TAG")]
        version: Option<String>,
    },
}

/// `cox expand <id>` (plan.md T2.5).
#[derive(Args, Debug, Clone)]
pub struct ExpandArgs {
    /// Archive id printed alongside a truncated tool result.
    pub id: String,
    /// Optional 1-based inclusive line range (`START-END`).
    #[arg(long)]
    pub lines: Option<String>,
}

/// `cox run` (plan.md §1.12). Only the clap shape and the flag→config-key
/// mapping land in T0.3; execution is T2.x.
#[derive(Args, Debug, Clone)]
pub struct RunArgs {
    /// The prompt to run headlessly.
    #[arg(short = 'p', long = "prompt", value_name = "TEXT")]
    pub prompt: Option<String>,
    /// Output shape: `text` | `json` | `stream-json`.
    #[arg(long = "output-format", value_name = "FORMAT")]
    pub output_format: Option<String>,
    /// Cap provider calls for this run (overrides `core.max_turns`).
    #[arg(long = "max-turns", value_name = "N")]
    pub max_turns: Option<u32>,
    /// Comma-separated tool allow-list for this run.
    #[arg(long = "allowed-tools", value_name = "A,B")]
    pub allowed_tools: Option<String>,
    /// Pre-supplied answer to the next approval prompt, if one is asked.
    #[arg(long, value_name = "TEXT")]
    pub answer: Option<String>,
    /// Continue the most recent session.
    #[arg(long, conflicts_with = "resume")]
    pub r#continue: bool,
    /// Resume a specific session by id.
    #[arg(long, value_name = "ID")]
    pub resume: Option<String>,
    /// Route this run through the `think` tier (implies confirmation).
    #[arg(long)]
    pub deep: bool,
    /// Repeat the prompt on a timer; requires `--max-iterations` (T27.6,
    /// same `<n>s|m|h` grammar as the TUI's `/loop`, T27.4).
    #[arg(long = "loop", value_name = "INTERVAL")]
    pub r#loop: Option<String>,
    /// Stop `--loop` after this many turns; required together with `--loop`.
    #[arg(long = "max-iterations", value_name = "N")]
    pub max_iterations: Option<u32>,
    /// Attach an image (PNG, JPEG, GIF or WebP, at most 3.75 MB) to the
    /// first turn; repeat for more. Checked before any request (T40.7).
    #[arg(long = "image", value_name = "PATH")]
    pub images: Vec<PathBuf>,
}

/// `cox stats` (plan.md §1.12/T1.7). Print usage and cost statistics.
#[derive(Args, Debug, Clone)]
pub struct StatsArgs {
    /// Print stats for a specific session by id.
    #[arg(long, value_name = "ID")]
    pub session: Option<String>,
    /// Only cache diagnostics: per-turn read ratio plus cache-miss turns.
    #[arg(long)]
    pub cache: bool,
    /// Group usage by day, broken down by tier and job.
    #[arg(long)]
    pub day: bool,
    /// Group usage by month, broken down by tier and job.
    #[arg(long)]
    pub month: bool,
    /// Print totals for one project slug (or every project when omitted).
    #[arg(long, value_name = "SLUG")]
    pub project: Option<Option<String>>,
    /// Machine-readable JSON output.
    #[arg(long)]
    pub json: bool,
    /// Machine-readable CSV output.
    #[arg(long)]
    pub csv: bool,
}

/// `cox sessions [ID] [--grep <q>] [--json] [--limit N]` (plan.md §1.12/T10.3,
/// A13). A bare id prints that one session's record instead of the list.
#[derive(Args, Debug, Clone)]
pub struct SessionsArgs {
    /// Print one session's record (times, provider, model, effort, tokens).
    #[arg(value_name = "ID", conflicts_with = "grep")]
    pub id: Option<String>,
    /// Full-text search over indexed session text.
    #[arg(long, value_name = "QUERY")]
    pub grep: Option<String>,
    /// Machine-readable JSON output.
    #[arg(long)]
    pub json: bool,
    /// Max sessions listed (default 20).
    #[arg(long, value_name = "N")]
    pub limit: Option<usize>,
}

/// `cox mcp [--allow-write] [--tools a,b]` (plan.md T6.2): read-only tools
/// by default; writes are opt-in and `bash` only by name.
/// `cox mcp login|logout <server>` (T22.5) manage an HTTP server's OAuth token.
#[derive(Args, Debug, Default, Clone)]
pub struct McpArgs {
    #[command(subcommand)]
    pub action: Option<McpAction>,
    /// Also serve `edit`, `write` and `apply_patch`.
    #[arg(long)]
    pub allow_write: bool,
    /// Serve exactly these tools (comma-separated); the only way to get `bash`.
    #[arg(long, value_name = "A,B")]
    pub tools: Option<String>,
}

#[derive(Subcommand, Debug, Clone)]
pub enum McpAction {
    /// Log in to an HTTP MCP server (OAuth in the browser); the token goes to the keyring.
    Login {
        /// Server name from config, `.mcp.json` or `~/.claude.json`.
        server: String,
    },
    /// Forget an HTTP MCP server's token.
    Logout {
        /// Server name from config, `.mcp.json` or `~/.claude.json`.
        server: String,
    },
}

/// `cox record <name> -p <prompt> [--sse FILE] [--redact]` (plan.md T1.5).
#[derive(Args, Debug, Clone)]
pub struct RecordArgs {
    /// Cassette name (`cassettes/<name>/` under `COX_HOME`).
    pub name: String,
    /// The prompt recorded as the request's user message.
    #[arg(short = 'p', long = "prompt", value_name = "TEXT")]
    pub prompt: Option<String>,
    /// Redact `sk-` keys and `Bearer ` prefixes.
    #[arg(long)]
    pub redact: bool,
    /// Raw SSE body to store (live capture waits on a session loop).
    #[arg(long, value_name = "FILE")]
    pub sse: Option<PathBuf>,
}

/// `cox config <action>` (plan.md §1.12/§1.6).
#[derive(Args, Debug, Clone)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub action: ConfigAction,
}

/// `cox config` subcommands.
#[derive(Subcommand, Debug, Clone)]
pub enum ConfigAction {
    /// Print every effective config key.
    Show {
        /// Also print which layer set each key (`default|user|project|env|flag`).
        #[arg(long)]
        sources: bool,
    },
    /// Print one config key's effective value.
    Get {
        /// Dotted config key, e.g. `tiers.code.model`.
        key: String,
    },
    /// Set one config key in the user config file (preserves comments).
    Set {
        /// Dotted config key, e.g. `tiers.code.model`.
        key: String,
        /// The new value, parsed as TOML (so `5`, `true`, `"text"`, `[1,2]` all work).
        value: String,
    },
    /// Print the user config file path.
    Path,
}

/// `cox init [--force]` (plan.md T25.6): scaffold `AGENTS.md` headlessly.
#[derive(Args, Debug, Clone)]
pub struct InitArgs {
    /// Overwrite an existing `AGENTS.md` instead of refusing.
    #[arg(long)]
    pub force: bool,
}

/// `cox ext [list]` (plan.md §1.12/T9.3): bare `ext` prints the human
/// report; `ext list` lists definitions, `--json` for machine output.
#[derive(Args, Debug, Clone)]
pub struct ExtArgs {
    #[command(subcommand)]
    pub action: Option<ExtAction>,
}

/// `cox ext` subcommands.
#[derive(Subcommand, Debug, Clone)]
pub enum ExtAction {
    /// List instruction-adjacent definitions in effect.
    List {
        /// Machine-readable JSON output.
        #[arg(long)]
        json: bool,
    },
}

/// `cox plugin [list]` (plan.md T33.4): bare `plugin` lists too, since
/// `list` is its only action so far.
#[cfg(feature = "plugins")]
#[derive(Args, Debug, Clone)]
pub struct PluginArgs {
    #[command(subcommand)]
    pub action: Option<PluginAction>,
}

/// `cox plugin` subcommands (T33.4, T33.7, T33.31).
#[cfg(feature = "plugins")]
#[derive(Subcommand, Debug, Clone)]
pub enum PluginAction {
    /// Discovered plugins: source, version, digest, grant state, declared capabilities.
    List {
        /// Machine-readable JSON output.
        #[arg(long)]
        json: bool,
    },
    /// Validate a package, copy it into `versions/<digest12>/`, write
    /// `current`, then ask for its capabilities (PL§1). An https URL is
    /// downloaded and unpacked, and a git repository cloned, into staging
    /// first.
    Install {
        /// A local plugin package directory, an `https://` URL of a
        /// `.tar.gz` package archive (needs `--sha256`), or `git+<url>`
        /// (needs `--rev`).
        source: String,
        /// The archive's SHA-256; required with a URL. A mismatch is
        /// refused before anything is unpacked.
        #[arg(long, value_name = "HEX", conflicts_with_all = ["rev", "path"])]
        sha256: Option<String>,
        /// The tag or full commit hash to install from a `git+<url>`; a
        /// branch is refused.
        #[arg(long, value_name = "TAG|COMMIT")]
        rev: Option<String>,
        /// The package's directory inside the git repository.
        #[arg(long, value_name = "SUBDIR", requires = "rev")]
        path: Option<String>,
        /// Skip the stdin prompt and grant what the manifest asks for.
        #[arg(long)]
        yes: bool,
    },
    /// Show a discovered plugin's capabilities in words and, on approval,
    /// grant it (PL§3).
    Enable {
        /// The plugin id, as its directory is named.
        id: String,
        /// Look for a project plugin in this repository instead of a user
        /// one; the grant is scoped to the repository root.
        #[arg(long)]
        project: bool,
        /// Skip the stdin prompt and grant what the manifest asks for.
        #[arg(long)]
        yes: bool,
    },
    /// Clear `enabled` on the grant; files and stored data stay.
    Disable {
        /// The plugin id, as its directory is named.
        id: String,
        /// The grant to clear is the project one, not the user one.
        #[arg(long)]
        project: bool,
    },
    /// Re-read each plugin's recorded source, show the capability diff
    /// and, on approval, switch to it; one previous version is kept (PL§1b).
    Update {
        /// The plugin ids to update.
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        ids: Vec<String>,
        /// Update every installed user plugin.
        #[arg(long)]
        all: bool,
        /// Print what would change and change nothing.
        #[arg(long, conflicts_with = "rollback")]
        check: bool,
        /// Switch back to the kept previous version.
        #[arg(long)]
        rollback: bool,
        /// Skip the stdin prompt and grant what the new manifest asks for.
        #[arg(long)]
        yes: bool,
    },
    /// Disable, delete the plugin's own directory and grants, and delete
    /// its kv unless `--keep-data` (PL§1c). A project plugin's files are
    /// repository content and stay; only its grant and kv go.
    Remove {
        /// The plugin id, as its directory is named.
        id: String,
        /// Keep the plugin's stored kv rows.
        #[arg(long)]
        keep_data: bool,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
    },
    /// Use a built plugin from `<dir>` in place, without installing it
    /// (T33.41, PL§13's dev loop). `discover` reads `<dir>` directly, and
    /// the grant re-asks only when `<dir>`'s capabilities widen, never on
    /// a rebuild's changed bytes.
    Link {
        /// The built plugin's package directory (holds `plugin.toml`).
        dir: PathBuf,
        /// Skip the stdin prompt and grant what the manifest asks for.
        #[arg(long)]
        yes: bool,
    },
    /// Scaffold a fresh plugin package from the PL§13 templates: `plugin.toml`
    /// with `[capabilities]` matching `--with`, stub exports for those
    /// capabilities only, a `justfile`, a `README.md` and a smoke test.
    /// Never overwrites an existing directory.
    New {
        /// The plugin id; also the scaffolded directory's default name.
        name: String,
        /// Only `rust` has a template today (PL§13); headless defaults to it.
        #[arg(long, default_value = "rust")]
        lang: crate::plugin_new::Lang,
        /// Where to write the package; defaults to `./<name>`.
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Capabilities to scaffold stubs for, comma-separated (PL§13).
        #[arg(long, value_delimiter = ',')]
        with: Vec<crate::plugin_new::Capability>,
    },
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn config_cli_parses_run_and_config_subcommands() {
        let cli = Cli::parse_from(["cox", "--model", "x", "run", "-p", "hi", "--deep"]);
        assert_eq!(cli.model.as_deref(), Some("x"));
        match cli.command {
            Some(Command::Run(run)) => {
                assert_eq!(run.prompt.as_deref(), Some("hi"));
                assert!(run.deep);
            }
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn positional_prompt_is_the_first_turn() {
        let cli = Cli::parse_from(["cox", "hello"]);
        assert_eq!(cli.prompt.as_deref(), Some("hello"));
        assert!(cli.command.is_none());
    }

    #[test]
    fn config_cli_command_builds_without_panicking() {
        // `debug_assert()` catches clap arg-definition mistakes (duplicate
        // ids, conflicting short/long names, ...) that only surface when the
        // `Command` is actually built.
        Cli::command().debug_assert();
    }

    #[test]
    fn resume_opens_the_tui_cli() {
        let cli = Cli::parse_from(["cox", "--resume", "01BX5ZZKBKACTAV9WEVGEMMVRZ"]);
        assert_eq!(cli.resume.as_deref(), Some("01BX5ZZKBKACTAV9WEVGEMMVRZ"));
        assert!(!cli.r#continue);
        assert!(cli.command.is_none());

        let cli = Cli::parse_from(["cox", "--continue"]);
        assert!(cli.r#continue);
        assert!(cli.resume.is_none());
        assert!(cli.command.is_none());
    }
}
