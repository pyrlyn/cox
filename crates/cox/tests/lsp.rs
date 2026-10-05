// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T41.8 Check: the `diagnostics` tool end to end, with the test-only
//! `fake_lsp` binary (`tests/support/fake_lsp.rs`) as the rust server.
//! Everything else is the real binary: the scripted model finds the
//! deferred tool through `tool_search` and calls it on a workspace file,
//! the session starts the server under the same sandbox wrap as a stdio MCP
//! server, and `cox run -p` kills it when the session ends. No network, no
//! real language server — except the ignored T41.9 live check at the
//! bottom, which runs the real `rust-analyzer` and only on request.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

const SCENARIO: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/scenarios/lsp_diagnostics.toml"
);

/// A scratch `COX_HOME` whose user config makes `fake_lsp` the rust server
/// and allows `diagnostics` (the first call starts a server, so it is
/// `Exec` and would otherwise be denied headless), and a workspace with
/// `src/a.rs`.
struct Rig {
    home: TempDir,
    work: TempDir,
    /// Where the server writes its pid. Its path is also a token unique to
    /// this rig in the server's command line.
    pid_file: PathBuf,
}

impl Rig {
    fn new() -> Rig {
        let dir = || tempfile::tempdir().expect("tempdir");
        let (home, work) = (dir(), dir());
        std::fs::create_dir_all(work.path().join("src")).expect("src");
        std::fs::write(work.path().join("src/a.rs"), "fn main() {}\n").expect("a.rs");
        let pid_file = work.path().join("fake_lsp.pid");
        let config = format!(
            "[permissions]\nallow = [\"Diagnostics\"]\n\n\
             [lsp.servers.rust]\ncommand = {:?}\nargs = [\"--pid-file\", {:?}]\n",
            env!("CARGO_BIN_EXE_fake_lsp"),
            pid_file.to_str().expect("utf-8 path"),
        );
        std::fs::write(home.path().join("config.toml"), config).expect("config");
        Rig {
            home,
            work,
            pid_file,
        }
    }

    fn cox(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_cox"));
        cmd.current_dir(self.work.path())
            .env("COX_HOME", self.home.path())
            .env("HOME", self.home.path())
            .env("COX_PROVIDER", "scripted")
            .env("COX_SCENARIO", SCENARIO)
            .args(["--cwd", self.work.path().to_str().expect("utf-8 path")])
            .args(args);
        cmd
    }
}

fn json_lines(text: &str) -> Vec<Value> {
    text.lines()
        .map(|l| serde_json::from_str(l).expect("a JSON line"))
        .collect()
}

/// Whether this host can wrap a server's argv at all: `sandboxed_argv`
/// refuses on a Landlock-only host or one with no backend.
fn sandbox_wraps() -> bool {
    use cox_tools::sandbox::{Backend, backend};
    matches!(
        backend(cox_protocol::LinuxBackend::Auto),
        Some(Backend::Seatbelt | Backend::Bwrap)
    )
}

/// Whether any process's command line still names `token`. By command
/// line, not by the recorded pid: under bwrap the server records its pid
/// in its own pid namespace, which the host does not share.
fn running(token: &Path) -> bool {
    let out = std::process::Command::new("pgrep")
        .arg("-f")
        .arg(token)
        .output()
        .expect("pgrep");
    out.status.success()
}

/// T41.8 Check: the model discovers `diagnostics`, gets the fake server's
/// diagnostic for `src/a.rs` back as `path:line:col: severity: message`,
/// and once `cox run -p` has exited no server process is left. On a host
/// that cannot wrap the server, the call is refused with the `bash`
/// fallback instead.
#[test]
fn diagnostics_come_back_from_a_sandboxed_server_that_is_gone_after_exit() {
    let rig = Rig::new();
    let out = rig
        .cox(&["run", "-p", "check a.rs", "--output-format", "stream-json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let events = json_lines(&String::from_utf8(out).expect("utf-8"));
    let results: Vec<&str> = events
        .iter()
        .filter(|e| e["type"] == "tool_call_done")
        .filter_map(|e| e["result"]["visible"].as_str())
        .collect();
    assert_eq!(
        results.len(),
        2,
        "tool_search, then diagnostics: {events:#?}"
    );
    assert!(results[0].contains("\"diagnostics\""), "{}", results[0]);

    if !sandbox_wraps() {
        assert!(results[1].contains("`bash`"), "{}", results[1]);
        return;
    }
    assert!(
        results[1].contains("src/a.rs:1:1: error: fake: fn main() {}"),
        "{}",
        results[1]
    );
    let pid = std::fs::read_to_string(&rig.pid_file).expect("the server wrote its pid");
    assert!(pid.trim().parse::<u32>().is_ok(), "{pid:?}");
    // The group kill has been sent before `cox` exits; allow the kernel a
    // moment to tear the processes down.
    let deadline = Instant::now() + Duration::from_secs(10);
    while running(&rig.pid_file) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        !running(&rig.pid_file),
        "a fake_lsp process outlived cox (pid {})",
        pid.trim()
    );
}

/// T41.1's guard through the real binary: a repository's `.cox/config.toml`
/// cannot choose the program cox runs as a language server.
#[test]
fn project_config_cannot_set_lsp_servers() {
    let rig = Rig::new();
    std::fs::create_dir_all(rig.work.path().join(".git")).expect(".git");
    std::fs::create_dir_all(rig.work.path().join(".cox")).expect(".cox");
    std::fs::write(
        rig.work.path().join(".cox/config.toml"),
        "[lsp.servers.rust]\ncommand = \"/bin/false\"\n",
    )
    .expect("project config");
    let assert = rig
        .cox(&["config", "get", "lsp.servers.rust.command"])
        .assert()
        .success();
    let output = assert.get_output();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.contains(env!("CARGO_BIN_EXE_fake_lsp")), "{stdout}");
    assert!(!stdout.contains("/bin/false"), "{stdout}");
    assert!(
        stderr.contains("project config ignores lsp.servers"),
        "{stderr}"
    );
}

/// T41.9's scenario: a cold call, a wait long enough for rust-analyzer to
/// load the crate, then a warm call.
const LIVE_SCENARIO: &str = r#"
[[turn]]
text = "looking for a checker"
tool_calls = [{ name = "tool_search", input = { query = "diagnostics" } }]
[[turn]]
text = "cold"
tool_calls = [{ name = "diagnostics", input = { path = "src/a.rs" } }]
[[turn]]
text = "let it load"
tool_calls = [{ name = "bash", input = { command = "sleep 20" } }]
[[turn]]
text = "warm"
tool_calls = [{ name = "diagnostics", input = { path = "src/a.rs" } }]
[[turn]]
text = "done"
"#;

/// T41.9: the real `rust-analyzer`, started by the real binary under the
/// real sandbox, reports a type error in a scratch crate. Proves the
/// Seatbelt/bwrap profile lets it read `~/.cargo`, `~/.rustup` and the
/// toolchain's sysroot; prints the backend, each call's latency, whether
/// the server was asked (pull) or waited for (push), and when its first
/// push and first `workspace/diagnostic/refresh` arrived, for research.md.
/// `HOME` stays real: the rustup proxy finds the toolchain through it. The
/// server's stdout is tee'd to a file so the exchange can be read back.
#[test]
#[ignore = "needs rust-analyzer on PATH; run with --run-ignored only (T41.9)"]
fn real_rust_analyzer_reports_a_type_error_under_the_sandbox() {
    let found = std::process::Command::new("rust-analyzer")
        .arg("--version")
        .output();
    let Some(version) = found.ok().filter(|o| o.status.success()) else {
        eprintln!("skipped: rust-analyzer is not on PATH");
        return;
    };
    let dir = || tempfile::tempdir().expect("tempdir");
    let (home, work) = (dir(), dir());
    let w = work.path();
    std::fs::create_dir_all(w.join("src")).expect("src");
    std::fs::write(
        w.join("Cargo.toml"),
        "[package]\nname = \"scratch\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("Cargo.toml");
    std::fs::write(w.join("src/main.rs"), "mod a;\nfn main() {}\n").expect("main.rs");
    std::fs::write(w.join("src/a.rs"), "pub fn f() -> u32 {\n    \"one\"\n}\n").expect("a.rs");
    let scenario = home.path().join("scenario.toml");
    std::fs::write(&scenario, LIVE_SCENARIO).expect("scenario");
    let capture = w.join("lsp.out");
    let config = format!(
        "[permissions]\nallow = [\"Diagnostics\", \"Bash(sleep:*)\"]\n\n\
         [lsp.servers.rust]\ncommand = \"/bin/sh\"\nargs = [\"-c\", {:?}]\n",
        format!("rust-analyzer | tee {}", capture.display()),
    );
    std::fs::write(home.path().join("config.toml"), config).expect("config");

    let start = Instant::now();
    let child = std::process::Command::new(env!("CARGO_BIN_EXE_cox"))
        .current_dir(w)
        .env("COX_HOME", home.path())
        .env("COX_PROVIDER", "scripted")
        .env("COX_SCENARIO", &scenario)
        .args(["--cwd", w.to_str().expect("utf-8 path")])
        .args(["run", "-p", "check a.rs", "--output-format", "stream-json"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("spawn cox");
    // Poll the capture for the first push and the first refresh request.
    let watch = capture.clone();
    let watcher = std::thread::spawn(move || {
        let (mut push, mut refresh) = (None, None);
        while start.elapsed() < Duration::from_secs(60) && (push.is_none() || refresh.is_none()) {
            let raw = std::fs::read_to_string(&watch).unwrap_or_default();
            if push.is_none() && raw.contains("textDocument/publishDiagnostics") {
                push = Some(start.elapsed());
            }
            if refresh.is_none() && raw.contains("workspace/diagnostic/refresh") {
                refresh = Some(start.elapsed());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        (push, refresh)
    });
    let out = child.wait_with_output().expect("cox ran");
    assert!(out.status.success(), "{out:?}");
    let (push_at, refresh_at) = watcher.join().expect("watcher");

    let events = json_lines(&String::from_utf8(out.stdout).expect("utf-8"));
    let done: Vec<&Value> = events
        .iter()
        .filter(|e| e["type"] == "tool_call_done")
        .collect();
    assert_eq!(done.len(), 4, "search, cold, sleep, warm: {events:#?}");
    let (cold, warm) = (&done[1]["result"], &done[3]["result"]);
    let warm_text = warm["visible"].as_str().unwrap_or_default();
    assert!(warm_text.contains("src/a.rs:2:5: error:"), "{warm_text}");
    assert!(warm_text.contains("E0308"), "{warm_text}");

    // cox pulls when `initialize` advertised `diagnosticProvider`.
    let raw = std::fs::read_to_string(&capture).expect("the server's stdout was captured");
    let pull = raw.contains("\"diagnosticProvider\":{");
    eprintln!(
        "T41.9: {} | backend {:?} | {} | cold call {} ms: {:?} | warm call {} ms | \
         first push at {push_at:?}, first refresh at {refresh_at:?} after cox started\n{warm_text}",
        String::from_utf8_lossy(&version.stdout).trim(),
        cox_tools::sandbox::backend(cox_protocol::LinuxBackend::Auto),
        if pull { "pull" } else { "push" },
        cold["duration_ms"],
        cold["visible"].as_str().unwrap_or_default(),
        warm["duration_ms"],
    );
}
