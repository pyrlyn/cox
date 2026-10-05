// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The clap surface and dispatch — nothing else. `config` (T0.3) and
//! `doctor` (T0.5) are wired up; every other subcommand is a stub until its
//! task lands (T2.x run, ...) — each prints a notice and exits 0 rather than
//! erroring, so the binary is a stable target for scripts and CI while the
//! crate fills in.

mod acp_cmd;
mod cli;
mod config_cmd;
mod config_load;
mod doctor;
mod expand_cmd;
mod ext_cmd;
mod mcp_cmd;
mod plain;
#[cfg(feature = "plugins")]
mod plugin_cmd;
mod plugin_fetch;
#[cfg(feature = "plugins")]
mod plugin_new;
#[cfg(feature = "plugins")]
mod plugin_ui;
mod record;
mod resume;
mod run;
mod self_update;
mod session;
mod sessions;
mod stats;
mod status_line;
mod telemetry;
#[cfg(feature = "voice")]
mod voice_cmd;

use clap::Parser;
use cli::{Cli, Command, ConfigAction};

/// `[y/N]` on stdin, same idiom as `session::offer_worktree_removal`; the
/// one prompt `cox plugin` and `cox voice` ask before acting.
#[cfg(any(feature = "plugins", feature = "voice"))]
pub(crate) fn confirm(question: &str) -> bool {
    eprint!("{question} [y/N] ");
    let mut answer = String::new();
    let _ = std::io::stdin().read_line(&mut answer);
    matches!(answer.trim(), "y" | "Y" | "yes")
}

fn main() -> anyhow::Result<()> {
    load_dotenv()?;
    let mut cli = Cli::parse();
    let mut cwd = match &cli.cwd {
        Some(dir) => dir.clone(),
        None => std::env::current_dir().unwrap_or_default(),
    };
    if cli.worktree.is_some() {
        cwd = session::enter_worktree(&mut cli, &cwd)?;
    } else if cli.resume.is_some() {
        // T44.4: a worktree session resumes in its worktree, before config
        // loads, for the same reason `--worktree` runs first.
        cwd = session::resume_worktree(&mut cli, &cwd)?;
    }
    let loaded = config_load::load(&cwd, &cli)?;
    let telemetry_home = cli.home.clone().unwrap_or_else(config_load::cox_home);
    let telemetry = telemetry::init(
        &loaded.config.core.log_level,
        loaded.config.telemetry.otel,
        &loaded.config.telemetry.endpoint,
        &telemetry_home,
    )?;

    match &cli.command {
        Some(Command::Config(args)) => run_config(&cwd, &cli, &args.action),
        Some(Command::Doctor) => {
            let servers = session::mcp_servers(&loaded.config, &cwd).servers;
            let code = doctor::run(
                cli.json,
                &servers,
                &cwd,
                &loaded.config.tui.theme,
                &loaded.config.tui.caps,
                &loaded.config,
            );
            drop(telemetry);
            std::process::exit(code);
        }
        Some(Command::Record(args)) => record::run(&cli, args),
        Some(Command::Stats(args)) => {
            // The store lives under `COX_HOME`, never the working directory:
            // defaulting to `cwd` opened (and created) an empty `cox.db`
            // wherever the command ran, so every read came back empty.
            let home = cli.home.clone().unwrap_or_else(config_load::cox_home);
            stats::run(&home, args)?;
            Ok(())
        }
        Some(Command::Mcp(args)) => mcp_cmd::run(&cli, args, &cwd),
        Some(Command::Ext(args)) => match &args.action {
            None => {
                print!("{}", ext_cmd::report(&cli, &cwd));
                Ok(())
            }
            Some(crate::cli::ExtAction::List { json }) => {
                print!("{}", ext_cmd::list(&cli, &cwd, *json));
                Ok(())
            }
        },
        #[cfg(feature = "plugins")]
        Some(Command::Plugin(args)) => match &args.action {
            None => {
                print!("{}", plugin_cmd::list(&cli, &cwd, false));
                Ok(())
            }
            Some(crate::cli::PluginAction::List { json }) => {
                print!("{}", plugin_cmd::list(&cli, &cwd, *json));
                Ok(())
            }
            Some(crate::cli::PluginAction::Install {
                source,
                sha256,
                rev,
                path,
                yes,
            }) => plugin_cmd::install(
                &cli,
                source,
                sha256.as_deref(),
                rev.as_deref(),
                path.as_deref(),
                *yes,
            ),
            Some(crate::cli::PluginAction::Enable { id, project, yes }) => {
                plugin_cmd::enable(&cli, &cwd, id, *project, *yes)
            }
            Some(crate::cli::PluginAction::Disable { id, project }) => {
                plugin_cmd::disable(&cli, &cwd, id, *project)
            }
            Some(crate::cli::PluginAction::Update {
                ids,
                all,
                check,
                rollback,
                yes,
            }) => plugin_cmd::update(&cli, ids, *all, *check, *rollback, *yes),
            Some(crate::cli::PluginAction::Remove { id, keep_data, yes }) => {
                plugin_cmd::remove(&cli, &cwd, id, *keep_data, *yes)
            }
            Some(crate::cli::PluginAction::Link { dir, yes }) => plugin_cmd::link(&cli, dir, *yes),
            Some(crate::cli::PluginAction::New {
                name,
                lang,
                dir,
                with,
            }) => {
                let dir = dir.clone().unwrap_or_else(|| cwd.join(name));
                let files = plugin_new::scaffold(name, *lang, with)?;
                plugin_new::write(&dir, &files)?;
                println!("scaffolded {name} in {}", dir.display());
                Ok(())
            }
        },
        Some(Command::Run(args)) => {
            let code = run::run(&cli, args, &cwd)?;
            drop(telemetry);
            std::process::exit(code);
        }
        Some(Command::Acp) => acp_cmd::run(&cli, &cwd),
        // T52.19: protocol lines only on stdout; logs go to the log file.
        #[cfg(feature = "app-server")]
        Some(Command::AppServer(_)) => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(cox_app::server::serve_stdio(cli.home.clone()))?;
            Ok(())
        }
        Some(Command::Init(args)) => {
            let code = session::run_init(&cli, &cwd, args.force)?;
            drop(telemetry);
            std::process::exit(code);
        }
        Some(Command::Sessions(args)) => {
            let home = cli.home.clone().unwrap_or_else(config_load::cox_home);
            sessions::run(&home, args)?;
            Ok(())
        }
        Some(Command::Expand(args)) => {
            let home = cli.home.clone().unwrap_or_else(config_load::cox_home);
            expand_cmd::run(&home, &args.id, args.lines.as_deref())
        }
        #[cfg(feature = "voice")]
        Some(Command::Voice(args)) => {
            let home = cli.home.clone().unwrap_or_else(config_load::cox_home);
            voice_cmd::run(&home, &args.action)
        }
        Some(Command::SelfUpdate(args)) => {
            let version = match &args.action {
                Some(crate::cli::SelfUpdateAction::Update { version }) => version.clone(),
                // Bare `cox self` updates to latest, like `update` without a tag.
                None => None,
            };
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(self_update::run(version))?;
            Ok(())
        }
        // T29.1: `--plain`, `tui.screen_reader` or `COX_PLAIN=1`.
        None if loaded.config.tui.screen_reader
            || std::env::var("COX_PLAIN").is_ok_and(|v| v == "1") =>
        {
            plain::run(&cli, &cwd)
        }
        None => session::run_tui(&cli, &cwd),
    }
}

/// Loads local secrets and `COX_*` overrides before clap reads the process
/// environment. `from_filename` walks upward from the process cwd and never
/// overwrites a value supplied by the shell or CI; `.env.local` therefore
/// fills only still-unset keys after `.env`.
fn load_dotenv() -> anyhow::Result<()> {
    for filename in [".env", ".env.local"] {
        if let Err(error) = dotenvy::from_filename(filename)
            && !error.not_found()
        {
            return Err(error.into());
        }
    }
    Ok(())
}

fn run_config(cwd: &std::path::Path, cli: &Cli, action: &ConfigAction) -> anyhow::Result<()> {
    match action {
        ConfigAction::Show { sources } => {
            let loaded = config_load::load(cwd, cli)?;
            config_cmd::show(&loaded, *sources)
        }
        ConfigAction::Get { key } => {
            let loaded = config_load::load(cwd, cli)?;
            match config_cmd::get(&loaded, key) {
                Some(value) => {
                    println!("{value}");
                    Ok(())
                }
                None => Err(anyhow::anyhow!("no such config key: {key}")),
            }
        }
        ConfigAction::Set { key, value } => {
            let path = config_cmd::set(key, value)?;
            println!("{}", path.display());
            Ok(())
        }
        ConfigAction::Path => {
            println!("{}", config_cmd::path().display());
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    fn config_cli() -> Cli {
        Cli::parse_from(["cox", "config", "show"])
    }

    #[test]
    fn config_dotenv_fills_unset_cox_key() {
        let home = tempdir().expect("home tempdir");
        let env_file = tempdir().expect("dotenv tempdir");
        let cwd = tempdir().expect("cwd tempdir");
        let path = env_file.path().join(".env");
        fs::write(&path, "COX_TIERS_CODE_MODEL=dotenv-model\n").expect("write dotenv");

        crate::config_load::temp_env(
            &[
                ("COX_HOME", Some(home.path().to_str().expect("utf-8 home"))),
                ("COX_TIERS_CODE_MODEL", None),
            ],
            || {
                dotenvy::from_path(&path).expect("load dotenv");
                let loaded =
                    crate::config_load::load(cwd.path(), &config_cli()).expect("load config");
                assert_eq!(loaded.config.tiers.code.model, "dotenv-model");
                assert_eq!(loaded.source_of("tiers.code.model"), "env");
            },
        );
    }

    #[test]
    fn config_dotenv_does_not_override_set_env() {
        let home = tempdir().expect("home tempdir");
        let env_file = tempdir().expect("dotenv tempdir");
        let cwd = tempdir().expect("cwd tempdir");
        let path = env_file.path().join(".env");
        fs::write(&path, "COX_TIERS_CODE_MODEL=dotenv-model\n").expect("write dotenv");

        crate::config_load::temp_env(
            &[
                ("COX_HOME", Some(home.path().to_str().expect("utf-8 home"))),
                ("COX_TIERS_CODE_MODEL", Some("shell-model")),
            ],
            || {
                dotenvy::from_path(&path).expect("load dotenv");
                let loaded =
                    crate::config_load::load(cwd.path(), &config_cli()).expect("load config");
                assert_eq!(loaded.config.tiers.code.model, "shell-model");
                assert_eq!(loaded.source_of("tiers.code.model"), "env");
            },
        );
    }
}
