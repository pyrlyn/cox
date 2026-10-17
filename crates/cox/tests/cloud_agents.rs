// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T56.8 Check: a Cursor Cloud agent end to end with nothing real behind it.
//! The real binary runs against a scratch `COX_HOME`; `cox plugin install
//! --yes` grants a package with a `[[cloud_agents]]` entry
//! (`tests/fixtures/cursor-cloud/plugin`); a wiremock server stands in for
//! `api.cursor.com` through the test-only `COX_CURSOR_CLOUD_URL`; and the
//! repository is a fixture whose `origin` is a GitHub URL and whose commit
//! carries a user name and email that must never leave. Only the parent model
//! is scripted, to issue `agent(preset: "cursor-cloud")`.
//!
//! The responses in `tests/fixtures/cursor-cloud/` are written by hand from
//! the public endpoint docs, never generated from or copied out of Cursor's
//! unlicensed OpenAPI file (plan.md A40 step 3). The key is a fake value.
//!
//! Known gaps, documented rather than fixed here: a run reattached by a
//! resumed session cannot be cancelled, and a stream that ends with no
//! terminal status may deliver its tail twice on resume.

#![cfg(all(feature = "plugins", unix))]

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use cox_protocol::Store as _;
use cox_protocol::ids::SessionId;
use cox_protocol::types::ProviderId;
use serde_json::Value;
use tempfile::TempDir;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SCENARIO: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/scenarios/cloud_agent.toml"
);
const KEY: &str = "not-a-real-key";
const GIT_NAME: &str = "Fixture Person";
const GIT_EMAIL: &str = "fixture.person@example.invalid";
const ALLOW_REPO: &str = "CloudAgent(acme/widgets)";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cursor-cloud")
}

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(fixtures().join(name)).expect("fixture")
}

/// The stand-in for `api.cursor.com`: the three endpoints a finished run
/// touches, each answering a hand-written fixture. The runtime outlives the
/// mounts because the server answers on it.
struct Backend {
    rt: tokio::runtime::Runtime,
    server: MockServer,
}

impl Backend {
    fn start() -> Backend {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let server = rt.block_on(MockServer::start());
        let json =
            |name: &str| ResponseTemplate::new(200).set_body_raw(fixture(name), "application/json");
        let sse =
            ResponseTemplate::new(200).set_body_raw(fixture("stream.sse"), "text/event-stream");
        let mounts = async {
            let bearer = || header("authorization", format!("Bearer {KEY}").as_str());
            Mock::given(method("POST"))
                .and(path("/v1/agents"))
                .and(bearer())
                .respond_with(json("create_agent.json"))
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path("/v1/agents/bc-e2e/runs/run-e2e/stream"))
                .and(bearer())
                .respond_with(sse)
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path("/v1/agents/bc-e2e/usage"))
                .and(bearer())
                .respond_with(json("usage.json"))
                .mount(&server)
                .await;
        };
        rt.block_on(mounts);
        Backend { rt, server }
    }

    fn requests(&self) -> Vec<wiremock::Request> {
        let seen = self.rt.block_on(self.server.received_requests());
        seen.expect("requests are recorded")
    }
}

/// A scratch `COX_HOME` with the fixture package granted, and a repository
/// whose `origin` is `acme/widgets` on GitHub.
struct Rig {
    home: TempDir,
    work: TempDir,
    backend: Backend,
}

impl Rig {
    fn new(allow: &[&str]) -> Rig {
        let (home, work) = (
            tempfile::tempdir().expect("home"),
            tempfile::tempdir().expect("work"),
        );
        let items: Vec<String> = allow.iter().map(|a| format!("{a:?}")).collect();
        let config = format!("[permissions]\nallow = [{}]\n", items.join(", "));
        std::fs::write(home.path().join("config.toml"), config).expect("config");
        let rig = Rig {
            home,
            work,
            backend: Backend::start(),
        };
        rig.git(&["init", "-q", "-b", "main"]);
        rig.git(&[
            "remote",
            "add",
            "origin",
            "https://github.com/acme/widgets.git",
        ]);
        rig.git(&["config", "user.name", GIT_NAME]);
        rig.git(&["config", "user.email", GIT_EMAIL]);
        rig.git(&["config", "commit.gpgsign", "false"]);
        rig.git(&["commit", "-q", "--allow-empty", "-m", "first"]);
        let package = fixtures().join("plugin");
        let package = package.to_str().expect("utf-8 path");
        rig.cox(&["plugin", "install", package, "--yes"])
            .assert()
            .success();
        rig
    }

    fn git(&self, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(self.work.path())
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?}");
    }

    fn cox(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_cox"));
        cmd.current_dir(self.work.path())
            .env("COX_HOME", self.home.path())
            .env("HOME", self.home.path())
            .env("COX_KEYRING", "off")
            .env("COX_PROVIDER", "scripted")
            .env("COX_SCENARIO", SCENARIO)
            .env("CURSOR_API_KEY", KEY)
            .env("COX_CURSOR_CLOUD_URL", self.backend.server.uri())
            // A proxy in the environment must not stand between the test and loopback.
            .env("NO_PROXY", "127.0.0.1,localhost")
            .env_remove("HTTP_PROXY")
            .env_remove("http_proxy")
            .args(["--cwd", self.work.path().to_str().expect("utf-8 path")])
            .args(args);
        cmd
    }

    /// The parent's run as stream-json events.
    fn run(&self) -> Vec<Value> {
        let out = self
            .cox(&["run", "-p", "go", "--output-format", "stream-json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let text = String::from_utf8(out).expect("utf-8");
        text.lines()
            .map(|l| serde_json::from_str(l).expect("a JSON line"))
            .collect()
    }

    /// Every usage row any session of this home wrote (the task's own
    /// session holds the cloud run's).
    fn usage_rows(&self) -> Vec<cox_protocol::UsageRow> {
        let store = cox_store::Store::open(self.home.path()).expect("store");
        let sessions = std::fs::read_dir(self.home.path().join("sessions")).expect("sessions");
        sessions
            .filter_map(|e| {
                let id = e.ok()?.path().file_stem()?.to_str()?.to_string();
                id.parse::<SessionId>().ok()
            })
            .flat_map(|id| store.usage_for_session(&id).expect("usage rows"))
            .collect()
    }
}

fn types(events: &[Value]) -> Vec<&str> {
    events.iter().filter_map(|e| e["type"].as_str()).collect()
}

/// T56.8 Check: the headless run dispatches the cloud task, the run's
/// assistant text arrives as the task's answer (thinking left out), the
/// ledger gets one `$0` external row with Cursor's token counts for the run,
/// and the run row ends `FINISHED`. The approval is the exact
/// `CloudAgent(owner/repo)` allow rule (T56.6): a background task has no
/// prompt to answer, so there is no `--answer` for this call.
#[test]
fn cloud_agent_headless_run_end_to_end() {
    let rig = Rig::new(&["Agent", ALLOW_REPO]);
    let events = rig.run();

    let kinds = types(&events);
    for want in ["task_created", "task_completed"] {
        assert!(kinds.contains(&want), "{want} in {kinds:?}");
    }
    let said: Vec<&str> = events
        .iter()
        .filter(|e| e["type"] == "notice")
        .filter_map(|e| e["text"].as_str())
        .collect();
    let answer = said
        .iter()
        .find(|t| t.contains("The race is in the retry loop."))
        .unwrap_or_else(|| panic!("the run's answer in {said:?}"));
    assert!(answer.contains("is not on origin"), "{answer}");
    let all = events.iter().map(Value::to_string).collect::<String>();
    assert!(!all.contains("private reasoning"), "thinking stays out");
    assert!(events.iter().all(|e| e["type"] != "error"), "{events:#?}");

    let rows = rig.usage_rows();
    let cloud: Vec<_> = rows
        .iter()
        .filter(|r| r.provider == ProviderId::External)
        .collect();
    assert_eq!(cloud.len(), 1, "{rows:#?}");
    let (row, u) = (cloud[0], &cloud[0].usage);
    assert_eq!(row.model.0, "cursor:auto");
    let tokens = (u.input_tokens, u.output_tokens, u.cache_read_tokens);
    assert_eq!((tokens, u.cache_write_tokens), ((120, 45, 60), 30));
    assert_eq!(u.cost_usd, 0.0, "billed on the user's Cursor plan");

    let store = cox_store::Store::open(rig.home.path()).expect("store");
    let run = store.cloud_run("cursor", "run-e2e").expect("read");
    let run = run.expect("the run is recorded");
    assert_eq!((run.status.as_str(), run.terminal), ("FINISHED", true));
    assert_eq!(run.repository, "acme/widgets");
    assert!(run.usage_recorded);
}

/// T56.8 Check: nothing that identifies the user leaves in any request: not
/// the git name or email in a body, a header or a path, no `/v1/me`, and the
/// create body carries the clean GitHub URL, `autoCreatePR: false` and a
/// `cox/<version>` User-Agent.
#[test]
fn cloud_agent_requests_carry_no_git_identity() {
    let rig = Rig::new(&["Agent", ALLOW_REPO]);
    rig.run();

    let requests = rig.backend.requests();
    let paths: Vec<&str> = requests.iter().map(|r| r.url.path()).collect();
    assert_eq!(
        paths,
        [
            "/v1/agents",
            "/v1/agents/bc-e2e/runs/run-e2e/stream",
            "/v1/agents/bc-e2e/usage"
        ],
        "exactly the three endpoints a finished run needs"
    );
    for request in &requests {
        let wire = format!(
            "{} {:?} {}",
            request.url,
            request.headers,
            String::from_utf8_lossy(&request.body)
        )
        .to_lowercase();
        // The message names the path only: `wire` holds the bearer header.
        for identity in [GIT_NAME, GIT_EMAIL, "fixture.person"] {
            assert!(
                !wire.contains(&identity.to_lowercase()),
                "{identity} in the request to {}",
                request.url.path()
            );
        }
        let agent = request.headers.get("user-agent").expect("User-Agent");
        assert!(
            agent.to_str().expect("ascii").starts_with("cox/"),
            "{agent:?}"
        );
    }
    let body: Value = serde_json::from_slice(&requests[0].body).expect("create body");
    assert_eq!(body["repos"][0]["url"], "https://github.com/acme/widgets");
    assert_eq!(body["repos"][0]["startingRef"], "main");
    assert_eq!(body["autoCreatePR"], false);
}

/// T56.8 Check: with no `CloudAgent` rule the ask cannot be answered, so the
/// run is refused before any request and no Cursor tokens are recorded: no code leaves
/// the machine on the strength of a plugin alone. T56.12: the notice that
/// closes the task gives the refusal, which names the rule to add, instead
/// of "produced no answer"; `TaskCompleted` itself has no text field.
#[test]
fn cloud_agent_without_an_allow_rule_makes_no_request() {
    let rig = Rig::new(&["Agent"]);
    let events = rig.run();
    assert!(types(&events).contains(&"task_completed"), "{events:#?}");
    assert!(rig.backend.requests().is_empty());
    let notice = events
        .iter()
        .filter(|e| e["type"] == "notice")
        .filter_map(|e| e["text"].as_str())
        .find(|t| t.contains("background task finished"))
        .unwrap_or_else(|| panic!("the task's closing notice in {events:#?}"));
    assert!(notice.contains("CloudAgent(acme/widgets)"), "{notice}");
    assert!(
        notice.contains("task failed"),
        "marked as a failure: {notice}"
    );
    assert!(!notice.contains("produced no answer"), "{notice}");
    // A row means a request was made (T56.11); the refused run made none.
    let rows = rig.usage_rows();
    let external = rows.iter().filter(|r| r.provider == ProviderId::External);
    assert_eq!(external.count(), 0, "{rows:#?}");
}
