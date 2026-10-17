// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T33.40.4 Check: the Jev plugin as a provider, end to end and offline.
//! The real binary installs the Jev package built by `cox-plugin-fixtures`
//! into a scratch `COX_HOME`, `[providers.typesafe] base_url` points it at a
//! local wiremock, and a scripted `write` in `auto` mode asks the `risk`
//! point, which Jev answers through a call-out on its own `typesafe` section.
//!
//! The package copy adds `decide = ["risk"]` to the manifest: the shipped
//! manifest declares no decision point until T33.40.6 lands the advisor.

#![cfg(feature = "plugins")]

use std::path::PathBuf;

use assert_cmd::Command;
use cox_plugin_fixtures::JEV_DIR;
use cox_protocol::ids::SessionId;
use cox_protocol::traits::Store as _;
use cox_protocol::types::{Job, ProviderId};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEY: &str = "jev-e2e-key-do-not-leak";
const TABLE_KEY: &str = "jev-table-key-do-not-leak";

/// One wiremock standing in for `api.typesafe.ai`, on its own runtime so
/// the blocking `cox` runs below need no async test.
struct Typesafe {
    rt: tokio::runtime::Runtime,
    server: MockServer,
}

impl Typesafe {
    fn answering(response: ResponseTemplate) -> Self {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let server = rt.block_on(MockServer::start());
        rt.block_on(
            Mock::given(method("POST"))
                .and(path("/v1/systemone"))
                .respond_with(response)
                .mount(&server),
        );
        Self { rt, server }
    }

    fn requests(&self) -> Vec<wiremock::Request> {
        self.rt.block_on(self.server.received_requests()).unwrap()
    }
}

/// A System One reply whose `severity` is low, so the write stays allowed.
fn reply(input_tokens: u32) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "model": "jev-1.13.0",
        "answers": { "severity": {
            "type": "score", "score": 0.4, "confidence": 0.9,
            "legend": { "0": "only reads" }, "probabilities": { "0": 0.9 }
        }},
        "usage": { "input_tokens": input_tokens, "output_tokens": 0 }
    }))
}

/// A scratch home with the Jev package installed and enabled, and a
/// scripted run that writes each of `files` in a tool batch of its own, so
/// each write is one `risk` question (the core asks once per batch,
/// T33.40.6.1).
struct Rig {
    home: tempfile::TempDir,
    cwd: tempfile::TempDir,
    scenario: PathBuf,
}

impl Rig {
    fn new(server: &Typesafe, files: &[&str]) -> Self {
        Self::with_table(server, files, "")
    }

    /// `table` is extra `[providers.typesafe]` lines after `base_url`.
    fn with_table(server: &Typesafe, files: &[&str], table: &str) -> Self {
        let rig = Self {
            home: tempfile::tempdir().unwrap(),
            cwd: tempfile::tempdir().unwrap(),
            scenario: PathBuf::new(),
        };
        let scenario = rig.home.path().join("scenario.toml");
        let turns: String = files
            .iter()
            .map(|f| {
                format!(
                    "[[turn]]\ntext = \"writing\"\ntool_calls = \
                     [{{ name = \"write\", input = {{ path = \"{f}\", content = \"x\" }} }}]\n\n"
                )
            })
            .collect();
        std::fs::write(&scenario, format!("{turns}[[turn]]\ntext = \"done\"\n")).unwrap();
        std::fs::write(
            rig.home.path().join("config.toml"),
            format!(
                "[providers.typesafe]\nbase_url = \"{}\"\n{table}\n[permissions]\nmode = \"auto\"\n\n\
                 [plugins.decide]\nrisk = \"jev\"\nrisk_ms = 30000\n",
                server.server.uri()
            ),
        )
        .unwrap();
        let package = rig.home.path().join("jev-package");
        std::fs::create_dir(&package).unwrap();
        for entry in std::fs::read_dir(JEV_DIR).unwrap() {
            let entry = entry.unwrap().path();
            std::fs::copy(&entry, package.join(entry.file_name().unwrap())).unwrap();
        }
        let manifest = package.join("plugin.toml");
        let text = std::fs::read_to_string(&manifest).unwrap();
        std::fs::write(
            &manifest,
            format!("{text}\n[capabilities]\ndecide = [\"risk\"]\n"),
        )
        .unwrap();
        let rig = Self { scenario, ..rig };
        let package = package.to_str().unwrap();
        rig.cox()
            .args(["plugin", "install", package, "--yes"])
            .assert()
            .success();
        rig.cox()
            .args(["plugin", "enable", "jev", "--yes"])
            .assert()
            .success();
        rig
    }

    fn cox(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_cox"));
        cmd.current_dir(self.cwd.path())
            .env("COX_HOME", self.home.path())
            .env("HOME", self.home.path())
            .env("COX_KEYRING", "off")
            .env("COX_PROVIDER", "scripted")
            .env("COX_SCENARIO", &self.scenario)
            .env("TYPESAFE_API_KEY", KEY)
            .env("JEV_TABLE_KEY", TABLE_KEY)
            .args(["--cwd", self.cwd.path().to_str().unwrap()]);
        cmd
    }

    /// One headless turn; a budget stop may end it unsuccessfully, which
    /// the budget test means to see, so the status is not asserted here.
    fn run(&self) {
        self.cox()
            .args(["run", "-p", "write them", "--output-format", "json"])
            .output()
            .unwrap();
    }

    /// Each session once: a rollout and its sidecar files share a stem.
    fn sessions(&self) -> Vec<SessionId> {
        let mut ids: Vec<String> = std::fs::read_dir(self.home.path().join("sessions"))
            .unwrap()
            .filter_map(|e| Some(e.ok()?.path().file_stem()?.to_str()?.to_owned()))
            .collect();
        ids.sort();
        ids.dedup();
        ids.iter().filter_map(|id| id.parse().ok()).collect()
    }

    /// Every rollout event of every session in this home.
    fn events(&self) -> Vec<Value> {
        self.sessions()
            .iter()
            .flat_map(|id| {
                let path = self
                    .home
                    .path()
                    .join("sessions")
                    .join(format!("{id}.jsonl"));
                let text = std::fs::read_to_string(path).unwrap();
                text.lines()
                    .map(|l| serde_json::from_str::<Value>(l).unwrap()["event"].clone())
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn notices(&self) -> Vec<String> {
        self.events()
            .iter()
            .filter(|e| e["type"] == "notice")
            .filter_map(|e| e["text"].as_str().map(str::to_owned))
            .filter(|t| t.starts_with("plugin jev: "))
            .collect()
    }

    fn rollout_text(&self) -> String {
        self.events().iter().map(Value::to_string).collect()
    }

    fn rows(&self) -> Vec<cox_protocol::UsageRow> {
        let store = cox_store::Store::open(self.home.path()).unwrap();
        self.sessions()
            .iter()
            .flat_map(|id| store.usage_for_session(id).unwrap())
            .collect()
    }

    fn jev_rows(&self) -> Vec<cox_protocol::UsageRow> {
        self.rows()
            .into_iter()
            .filter(|row| row.job == Job::Plugin("jev".into()))
            .collect()
    }

    fn wrote(&self, file: &str) -> bool {
        self.cwd.path().join(file).exists()
    }
}

fn decide_skipped(notices: &[String]) -> Vec<&String> {
    notices
        .iter()
        .filter(|n| n.contains("decision skipped"))
        .collect()
}

#[test]
fn jev_request_carries_host_injected_bearer() {
    let server = Typesafe::answering(reply(10));
    let rig = Rig::new(&server, &["a.txt"]);
    rig.run();

    let requests = server.requests();
    assert_eq!(requests.len(), 1, "{:?}", rig.notices());
    let auth: Vec<_> = requests[0]
        .headers
        .get_all("authorization")
        .iter()
        .collect();
    assert_eq!(auth, [format!("Bearer {KEY}").as_str()]);
    let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["model"], "jev-1.13.0");
    assert_eq!(body["state"]["items"][0]["tool"], "write");
    assert_eq!(body["questions"]["severity"]["type"], "score");
    let advised: Vec<_> = rig
        .events()
        .into_iter()
        .filter(|e| e["type"] == "advised")
        .collect();
    assert_eq!(advised.len(), 1, "{}", rig.rollout_text());
    assert_eq!(advised[0]["plugin"], "jev");
    assert!(rig.wrote("a.txt"));
}

#[test]
fn jev_key_never_reaches_guest() {
    let server = Typesafe::answering(reply(10));
    let rig = Rig::new(&server, &["a.txt"]);
    rig.run();

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    // The guest built the body; only the host's header carries the key.
    let body = String::from_utf8_lossy(&requests[0].body);
    assert!(!body.contains(KEY), "{body}");
    assert!(!rig.rollout_text().contains(KEY));
}

#[test]
fn jev_call_writes_usage_row_with_plugin_job() {
    let server = Typesafe::answering(reply(1_000_000));
    let rig = Rig::new(&server, &["a.txt"]);
    rig.run();

    let rows = rig.jev_rows();
    assert_eq!(rows.len(), 1, "{:?}", rig.rows());
    let row = &rows[0];
    assert_eq!(row.provider, ProviderId::Plugin("typesafe".into()));
    assert_eq!(row.model.0, "jev-1.13.0");
    assert_eq!(row.usage.input_tokens, 1_000_000);
    // $0.042/MTok comes from the plugin's own `[[models]]` row: the
    // embedded price table has no `jev-1.13.0`.
    assert!((row.usage.cost_usd - 0.042).abs() < 1e-9, "{row:?}");
}

#[test]
fn jev_call_blocked_by_budget_falls_back() {
    // The first call costs $8.40 against the default $5 session cap. The
    // core asks once per tool batch (T33.40.6.1), and every batch follows a
    // main provider call that passes the same gate, so no second question
    // is asked over the cap: the session stops on its budget instead, and
    // the batch already asked keeps the static pick. The refused call-out
    // itself is `cox-core`'s `decide_call_out_over_budget_falls_back`.
    let server = Typesafe::answering(reply(200_000_000));
    let rig = Rig::new(&server, &["a.txt", "b.txt"]);
    rig.run();

    assert_eq!(server.requests().len(), 1);
    assert_eq!(rig.jev_rows().len(), 1);
    let notices = rig.notices();
    assert!(decide_skipped(&notices).is_empty(), "{notices:?}");
    let stops: Vec<_> = rig
        .events()
        .into_iter()
        .filter(|e| e["type"] == "turn_done")
        .map(|e| e["stop"].clone())
        .collect();
    assert_eq!(
        stops,
        [json!({ "type": "budget" })],
        "{}",
        rig.rollout_text()
    );
    assert!(rig.wrote("a.txt"), "the static pick stands");
    assert!(!rig.wrote("b.txt"), "no batch runs past the cap");
}

#[test]
fn jev_401_is_one_notice_and_fail_open() {
    let server = Typesafe::answering(ResponseTemplate::new(401).set_body_string("bad key"));
    let rig = Rig::new(&server, &["a.txt"]);
    rig.run();

    assert_eq!(server.requests().len(), 1, "a 401 is not retried");
    let notices = rig.notices();
    let skipped = decide_skipped(&notices);
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(skipped[0].contains("401"), "{notices:?}");
    assert!(rig.wrote("a.txt"), "fail open: the write still runs");
    assert!(rig.jev_rows().is_empty());
}

#[test]
fn jev_529_retries_max_retries_times() {
    let server = Typesafe::answering(ResponseTemplate::new(529));
    let rig = Rig::new(&server, &["a.txt"]);
    rig.run();

    // `[providers.typesafe] max_retries = 2`: the first try and two more.
    assert_eq!(server.requests().len(), 3);
    assert_eq!(decide_skipped(&rig.notices()).len(), 1);
    assert!(rig.wrote("a.txt"));
}

#[test]
fn three_failures_disable_decide_export() {
    let server = Typesafe::answering(ResponseTemplate::new(401));
    let rig = Rig::new(&server, &["a.txt", "b.txt", "c.txt", "d.txt"]);
    rig.run();

    assert_eq!(server.requests().len(), 3, "the fourth is never asked");
    let notices = rig.notices();
    assert_eq!(decide_skipped(&notices).len(), 1, "{notices:?}");
    assert!(
        notices
            .iter()
            .any(|n| n.contains("cox_decide disabled for this session after 3 failures")),
        "{notices:?}"
    );
    assert!(
        ["a.txt", "b.txt", "c.txt", "d.txt"]
            .iter()
            .all(|f| rig.wrote(f))
    );
}

#[test]
fn plugin_list_json_shows_provider_and_model_contributions() {
    let server = Typesafe::answering(reply(10));
    let rig = Rig::new(&server, &[]);
    let out = rig
        .cox()
        .args(["plugin", "list", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let list: Value = serde_json::from_slice(&out).unwrap();
    let row = &list["plugins"][0];
    assert_eq!(row["id"], "jev", "{list}");
    assert_eq!(row["providers"], json!(["typesafe"]), "{list}");
    assert_eq!(row["models"], json!(["jev-1.13.0", "jev-latest"]), "{list}");
    assert!(server.requests().is_empty());
}

#[test]
fn typesafe_table_overrides_plugin_transport() {
    // The reply outlives `timeout_s`, so the call ends on the table's
    // deadline, not the client's 30 s; `max_retries = 0` leaves one request.
    let server = Typesafe::answering(reply(10).set_delay(std::time::Duration::from_secs(4)));
    let rig = Rig::with_table(
        &server,
        &["a.txt"],
        "api_key_env = \"JEV_TABLE_KEY\"\ntimeout_s = 1\nmax_retries = 0\n",
    );
    let started = std::time::Instant::now();
    rig.run();

    let requests = server.requests();
    assert_eq!(requests.len(), 1, "{:?}", rig.notices());
    let auth: Vec<_> = requests[0]
        .headers
        .get_all("authorization")
        .iter()
        .collect();
    assert_eq!(auth, [format!("Bearer {TABLE_KEY}").as_str()]);
    assert!(started.elapsed() < std::time::Duration::from_secs(4));
    assert_eq!(decide_skipped(&rig.notices()).len(), 1);
    assert!(rig.wrote("a.txt"), "fail open: the write still runs");
    assert!(!rig.rollout_text().contains(TABLE_KEY));
}

#[test]
fn doctor_names_the_plugin_that_serves_typesafe() {
    let server = Typesafe::answering(reply(10));
    let rig = Rig::new(&server, &[]);
    let out = rig
        .cox()
        .args(["doctor", "--json"])
        .output()
        .unwrap()
        .stdout;
    let rows: Vec<Value> = serde_json::from_slice(&out).unwrap();
    let row = rows
        .iter()
        .find(|r| r["check"] == "typesafe client")
        .unwrap_or_else(|| panic!("{rows:?}"));
    assert!(
        row["detail"]
            .as_str()
            .unwrap()
            .starts_with("served by the jev plugin"),
        "{row}"
    );
}
