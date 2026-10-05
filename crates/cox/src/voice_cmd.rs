// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `cox voice model list|download <name>` (T54.5, A123): the whisper models
//! push-to-talk can use. Names, pinned URLs, SHA-256 digests and sizes come
//! from `cox-voice`'s vendored table; a model outside it is never fetched.
//! Here rather than in `cox-voice` because that crate never opens a socket,
//! and a model lands only through this explicit command, which asks first.

use std::fs;
use std::io::{IsTerminal as _, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};
use serde::Deserialize;

use crate::cli::{VoiceAction, VoiceModelAction};
use crate::plugin_fetch::{http_client, sha256_read};

/// One row of `whisper-models.json`.
#[derive(Debug, Clone, Deserialize)]
struct ModelRow {
    name: String,
    file: String,
    url: String,
    sha256: String,
    size: u64,
}

#[derive(Deserialize)]
struct Table {
    models: Vec<ModelRow>,
}

fn table() -> anyhow::Result<Vec<ModelRow>> {
    let table: Table =
        serde_json::from_str(cox_voice::MODELS_JSON).context("the pinned whisper model table")?;
    Ok(table.models)
}

/// Where downloaded models live: `$COX_HOME/models/whisper/`.
pub(crate) fn models_dir(home: &Path) -> PathBuf {
    home.join("models").join("whisper")
}

/// Where the pinned model `name` lives once downloaded; `None` for a name
/// the table does not list (T54.7: the TUI and `cox doctor`).
pub(crate) fn model_path(home: &Path, name: &str) -> Option<PathBuf> {
    let models = table().ok()?;
    let row = models.into_iter().find(|m| m.name == name)?;
    Some(models_dir(home).join(row.file))
}

pub fn run(home: &Path, action: &VoiceAction) -> anyhow::Result<()> {
    let VoiceAction::Model(action) = action;
    let models = table()?;
    let dir = models_dir(home);
    match action {
        VoiceModelAction::List => {
            print!("{}", list(&models, &dir));
            Ok(())
        }
        VoiceModelAction::Download { name, yes } => {
            let Some(row) = models.iter().find(|m| m.name == *name) else {
                bail!("no pinned model named {name}; `cox voice model list` shows them");
            };
            let mut ask = |q: &str| crate::confirm(q);
            let confirm: Option<&mut dyn FnMut(&str) -> bool> = if std::io::stdin().is_terminal() {
                Some(&mut ask)
            } else {
                None
            };
            let client = http_client()?;
            tokio::runtime::Runtime::new()?.block_on(download(
                &client,
                row,
                &dir,
                *yes,
                confirm,
                &mut std::io::stdout(),
            ))
        }
    }
}

fn mib(bytes: u64) -> String {
    format!("{:.0} MiB", bytes as f64 / f64::from(1 << 20))
}

/// A model counts as present when its file has the pinned size; `download`
/// checks the digest, which is too slow to run over every model here.
fn present(row: &ModelRow, dir: &Path) -> bool {
    fs::metadata(dir.join(&row.file)).is_ok_and(|m| m.len() == row.size)
}

fn list(models: &[ModelRow], dir: &Path) -> String {
    let mut out = String::new();
    for m in models {
        let state = if present(m, dir) { "present" } else { "-" };
        out.push_str(&format!("{:<9} {:>8}  {state}\n", m.name, mib(m.size)));
    }
    out.push_str(&format!("models live in {}\n", dir.display()));
    out
}

/// Downloads `row` into `dir` after consent: `yes`, or `confirm` answering
/// y on a terminal; `confirm` is `None` when stdin is not one. The bytes go
/// to `<file>.part`, which is renamed only once its SHA-256 matches the
/// table and deleted on any failure. A file already there that verifies is
/// left alone and nothing is fetched.
async fn download(
    client: &reqwest::Client,
    row: &ModelRow,
    dir: &Path,
    yes: bool,
    confirm: Option<&mut dyn FnMut(&str) -> bool>,
    out: &mut dyn Write,
) -> anyhow::Result<()> {
    let dest = dir.join(&row.file);
    if dest.is_file() {
        if sha256_read(fs::File::open(&dest)?)? == row.sha256 {
            writeln!(
                out,
                "{} is already downloaded: {}",
                row.name,
                dest.display()
            )?;
            return Ok(());
        }
        writeln!(
            out,
            "{} does not match its pinned SHA-256; downloading it again",
            dest.display()
        )?;
    }
    writeln!(out, "{} ({}) from {}", row.name, mib(row.size), row.url)?;
    out.flush()?;
    let approved = match confirm {
        _ if yes => true,
        Some(ask) => ask(&format!("download {} ({})?", row.name, mib(row.size))),
        None => bail!(
            "stdin is not a terminal: pass --yes to download {}",
            row.name
        ),
    };
    if !approved {
        writeln!(out, "{} not downloaded", row.name)?;
        return Ok(());
    }
    fs::create_dir_all(dir)?;
    let part = dir.join(format!("{}.part", row.file));
    if let Err(e) = fetch_to(client, row, &part).await {
        let _ = fs::remove_file(&part);
        return Err(e);
    }
    fs::rename(&part, &dest)?;
    writeln!(out, "downloaded {} to {}", row.name, dest.display())?;
    Ok(())
}

/// Streams `row.url` into `part` and checks its SHA-256. A body longer than
/// the pinned size is cut off early rather than filling the disk.
async fn fetch_to(client: &reqwest::Client, row: &ModelRow, part: &Path) -> anyhow::Result<()> {
    let mut response = client.get(&row.url).send().await?.error_for_status()?;
    let mut file = fs::File::create(part)?;
    let mut written = 0u64;
    while let Some(chunk) = response.chunk().await? {
        written += chunk.len() as u64;
        if written > row.size {
            bail!("{} is larger than its pinned {} bytes", row.url, row.size);
        }
        file.write_all(&chunk)?;
    }
    file.sync_all()?;
    drop(file);
    let got = sha256_read(fs::File::open(part)?)?;
    if got != row.sha256 {
        bail!(
            "SHA-256 mismatch for {}: expected {}, got {got}; nothing was kept",
            row.url,
            row.sha256
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    const BODY: &[u8] = b"ggml pretend model";

    fn row(server: &MockServer, body: &[u8]) -> ModelRow {
        ModelRow {
            name: "tiny.en".into(),
            file: "ggml-tiny.en.bin".into(),
            url: format!("{}/ggml-tiny.en.bin", server.uri()),
            sha256: sha256_read(body).expect("hash"),
            size: body.len() as u64,
        }
    }

    async fn serve(server: &MockServer, body: &[u8], times: u64) {
        Mock::given(method("GET"))
            .and(path("/ggml-tiny.en.bin"))
            .and(header(
                "user-agent",
                concat!("cox/", env!("CARGO_PKG_VERSION")),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
            .expect(times)
            .mount(server)
            .await;
    }

    #[test]
    fn the_embedded_table_parses() {
        let models = table().expect("parses");
        assert!(models.iter().any(|m| m.name == "base.en"));
        assert!(models.iter().all(|m| m.sha256.len() == 64 && m.size > 0));
    }

    #[tokio::test]
    async fn voice_model_download_verifies_sha256() {
        let server = MockServer::start().await;
        serve(&server, BODY, 1).await;
        let dir = tempdir().expect("tempdir");
        let row = row(&server, BODY);
        let client = http_client().expect("client");
        let mut out = Vec::new();

        download(&client, &row, dir.path(), true, None, &mut out)
            .await
            .expect("downloads");
        assert_eq!(fs::read(dir.path().join(&row.file)).expect("file"), BODY);
        assert!(!dir.path().join("ggml-tiny.en.bin.part").exists());

        // A present, verified file is a no-op: the mock expects one GET.
        let mut again = Vec::new();
        download(&client, &row, dir.path(), true, None, &mut again)
            .await
            .expect("no-op");
        assert!(String::from_utf8_lossy(&again).contains("already downloaded"));
    }

    #[tokio::test]
    async fn voice_model_download_hash_mismatch_leaves_no_file() {
        let server = MockServer::start().await;
        serve(&server, b"ggml pretend modeX", 1).await;
        let dir = tempdir().expect("tempdir");
        let row = row(&server, BODY);
        let client = http_client().expect("client");

        let err = download(&client, &row, dir.path(), true, None, &mut Vec::new())
            .await
            .expect_err("mismatch");
        assert!(err.to_string().contains("SHA-256 mismatch"), "{err}");
        assert_eq!(fs::read_dir(dir.path()).expect("dir").count(), 0);
    }

    #[tokio::test]
    async fn voice_model_download_without_a_tty_needs_yes() {
        let server = MockServer::start().await;
        serve(&server, BODY, 0).await;
        let dir = tempdir().expect("tempdir");
        let row = row(&server, BODY);
        let client = http_client().expect("client");

        let err = download(&client, &row, dir.path(), false, None, &mut Vec::new())
            .await
            .expect_err("refuses");
        assert!(err.to_string().contains("--yes"), "{err}");

        // On a terminal, a "no" fetches nothing either.
        let mut no = |_: &str| false;
        download(
            &client,
            &row,
            dir.path(),
            false,
            Some(&mut no),
            &mut Vec::new(),
        )
        .await
        .expect("declined");
        assert!(!dir.path().join(&row.file).exists());
    }

    #[test]
    fn voice_model_list_marks_present_models() {
        let home = tempdir().expect("tempdir");
        let dir = models_dir(home.path());
        let models = table().expect("parses");
        let base = models
            .iter()
            .find(|m| m.name == "base.en")
            .expect("base.en");
        fs::create_dir_all(&dir).expect("mkdir");
        let file = fs::File::create(dir.join(&base.file)).expect("create");
        file.set_len(base.size)
            .expect("sparse file of the pinned size");

        let printed = list(&models, &dir);
        let line = |name: &str| {
            printed
                .lines()
                .find(|l| l.split_whitespace().next() == Some(name))
                .unwrap_or_default()
                .to_string()
        };
        assert!(line("base.en").ends_with("present"), "{printed}");
        assert!(line("tiny.en").ends_with('-'), "{printed}");
        assert!(line("base.en").contains("141 MiB"), "{printed}");
        assert_eq!(printed.lines().count(), models.len() + 1);
    }
}
