// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The session repo map (P43, `docs/design/v0.2-repomap.md`): built once
//! before the first request of the session the user talks to, archived
//! before use, replayed from the archive on resume (invariant 6), and
//! changed afterwards only by `/repomap refresh` or compaction (T43.5).
//! Separate from `session` because this is the one place `Inner.repomap`
//! is written. The builder is a `RepoMapper` the surface installs, so this
//! crate walks no tree and reads no file; the permission engine is the
//! only filter on what the map may show.

use std::path::Path;

use cox_protocol::errors::CoreError;
use cox_protocol::ids::{ArchiveId, CallId};
use cox_protocol::traits::ArchivePut;
use cox_protocol::types::{Event, Level, RepoMapReason, Risk, ToolCall};

use crate::Outcome;
use crate::session::{Session, State};

/// Tokens → bytes for the budget: the estimator's four-bytes-per-token rule.
const BYTES_PER_TOKEN: usize = 4;

impl Session {
    /// The first submit's step (the `SessionStart` slot): a resumed session
    /// reads its archived map back; a new one builds it when the budget is
    /// on and a mapper is installed. A child never gets here.
    pub(crate) async fn start_repomap(&self) -> Result<(), CoreError> {
        let restored = {
            let inner = self.inner.lock().await;
            if inner.repomap.is_some() {
                return Ok(());
            }
            inner.repomap_archive
        };
        if let Some(id) = restored {
            return self.restore_repomap(id).await;
        }
        match self.render_repomap().await {
            Some(text) => self
                .install_repomap(text, RepoMapReason::SessionStart)
                .await
                .map(|_| ()),
            None => Ok(()),
        }
    }

    /// The one rebuild `/repomap refresh` and compaction share (T43.5).
    /// Identical bytes change nothing — no archive row, no event — so the
    /// cached prefix survives a refresh of an unchanged tree.
    pub(crate) async fn rebuild_repomap(
        &self,
        reason: RepoMapReason,
    ) -> Result<Rebuilt, CoreError> {
        let Some(text) = self.render_repomap().await else {
            return Ok(Rebuilt::Off);
        };
        if self.inner.lock().await.repomap.as_deref() == Some(text.as_str()) {
            return Ok(Rebuilt::Unchanged);
        }
        Ok(match self.install_repomap(text, reason).await? {
            true => Rebuilt::Changed,
            false => Rebuilt::Unchanged,
        })
    }

    /// `/repomap`: the map's size and where to read it; `/repomap refresh`:
    /// a rebuild, only between turns, announced because a changed map
    /// restarts the cached prefix (§1.9 exception, A74).
    pub(crate) async fn repomap_command(&self, args: &[String]) -> Result<(), CoreError> {
        let off = "the repo map is off; set context.repomap_budget_tokens to turn it on";
        let idle = self.inner.lock().await.state == State::Idle;
        let (level, text) = match args.first().map(String::as_str) {
            None => {
                let (bytes, archive) = {
                    let inner = self.inner.lock().await;
                    (
                        inner.repomap.as_ref().map(String::len),
                        inner.repomap_archive,
                    )
                };
                match (bytes, archive) {
                    (Some(bytes), Some(id)) => (
                        Level::Info,
                        format!("repo map: {bytes} bytes in system[2]; `cox expand {id}` shows it"),
                    ),
                    _ if self.config.context.repomap_budget_tokens == 0 => {
                        (Level::Info, off.to_string())
                    }
                    _ => (Level::Info, "this session has no repo map".to_string()),
                }
            }
            Some("refresh") if !idle => (
                Level::Warn,
                "a turn is running; refresh the repo map once it is done".to_string(),
            ),
            Some("refresh") => match self.rebuild_repomap(RepoMapReason::Refresh).await? {
                Rebuilt::Changed => (
                    Level::Info,
                    "repo map refreshed; the cached prefix restarts on the next request"
                        .to_string(),
                ),
                Rebuilt::Unchanged => (Level::Info, "repo map unchanged, cache kept".to_string()),
                Rebuilt::Off => (Level::Info, off.to_string()),
            },
            Some(other) => (
                Level::Warn,
                format!("unknown /repomap argument `{other}`; use /repomap or /repomap refresh"),
            ),
        };
        self.notice(level, text).await
    }

    /// Fail open: a map missing from the archive costs the map, not the
    /// session; nothing is rebuilt, so the prefix never silently changes.
    async fn restore_repomap(&self, id: ArchiveId) -> Result<(), CoreError> {
        match self.archive.get(&id).await {
            Ok(bytes) => {
                self.inner.lock().await.repomap =
                    Some(String::from_utf8_lossy(&bytes).into_owned());
                Ok(())
            }
            Err(_) => {
                self.inner.lock().await.repomap_archive = None;
                self.notice(
                    Level::Warn,
                    format!("repo map {id} is missing from the archive; continuing without one"),
                )
                .await
            }
        }
    }

    /// A fresh map, or `None` when the budget is 0, no mapper is installed,
    /// this is a subagent, or the workspace yields nothing. Each file is
    /// admitted by the engine as a `read` of it in the live mode: a file
    /// the user denied never reaches the model through the map.
    pub(crate) async fn render_repomap(&self) -> Option<String> {
        let tokens = self.config.context.repomap_budget_tokens;
        if tokens == 0 || self.agent.is_some() {
            return None;
        }
        let mapper = self.repo_mapper.get()?.clone();
        let (mode, grants) = {
            let inner = self.inner.lock().await;
            (inner.permission_mode, inner.grants.clone())
        };
        let engine = self.engine.clone();
        let (approval, sandbox) = (self.config.permissions.approval, self.config.sandbox.mode);
        let cwd = self.cwd.clone();
        // Both spellings a model could use, so a relative rule and an
        // absolute one each get their match.
        let admit = move |path: &Path| {
            let relative = path.strip_prefix(&cwd).ok();
            [Some(path), relative].into_iter().flatten().all(|p| {
                let subject = p.display().to_string();
                let call = ToolCall {
                    id: CallId::new(),
                    name: "read".into(),
                    input: serde_json::json!({ "path": subject }),
                    risk: Risk::ReadOnly,
                    subject,
                    segments: None,
                };
                !matches!(
                    engine.decide(&call, mode, approval, sandbox, &grants),
                    Outcome::Deny { .. }
                )
            })
        };
        let budget = usize::try_from(tokens)
            .unwrap_or(usize::MAX)
            .saturating_mul(BYTES_PER_TOKEN);
        let text = mapper.build(&self.cwd, budget, &admit).await;
        (!text.is_empty()).then_some(text)
    }

    /// Archives `text` first (the archive row exists before the model sees
    /// the map), then installs it and records `RepoMapBuilt`. An archive
    /// failure is a warning and leaves the map as it was (`false`).
    async fn install_repomap(
        &self,
        text: String,
        reason: RepoMapReason,
    ) -> Result<bool, CoreError> {
        let put = ArchivePut {
            session: self.id,
            call: CallId::new(),
            tool: "repomap".into(),
            subject: None,
            bytes: text.as_bytes().to_vec(),
        };
        let archive = match self.archive.put(put).await {
            Ok(id) => id,
            Err(error) => {
                self.notice(
                    Level::Warn,
                    format!("repo map not archived ({error}); keeping the previous one"),
                )
                .await?;
                return Ok(false);
            }
        };
        let bytes = text.len() as u64;
        {
            let mut inner = self.inner.lock().await;
            inner.repomap = Some(text);
            inner.repomap_archive = Some(archive);
        }
        self.emit(Event::RepoMapBuilt {
            archive,
            bytes,
            reason,
        })
        .await?;
        Ok(true)
    }
}

/// What a rebuild did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Rebuilt {
    /// No map: the budget is 0, no mapper, a subagent, or nothing to map.
    Off,
    /// Same bytes as the map in place; nothing recorded.
    Unchanged,
    /// A new map is archived, installed and recorded.
    Changed,
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex as StdMutex};

    use async_trait::async_trait;
    use cox_protocol::errors::ProviderError;
    use cox_protocol::traits::{Provider, RepoMapper, Store as _};
    use cox_protocol::types::{
        Caps, Job, ProviderEvent, ProviderId, Request, SlashCommand, Submission, Tier, Usage,
    };
    use cox_provider::scripted::Scripted;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::MemoryStore;

    /// Offers two files and lists the ones `admit` takes, tagged with how
    /// many times it was asked, so a rebuild shows up as new bytes — unless
    /// `frozen`, which stands for a tree that did not change.
    #[derive(Default)]
    pub(crate) struct FakeMapper {
        pub(crate) calls: AtomicUsize,
        pub(crate) frozen: bool,
    }

    #[async_trait]
    impl RepoMapper for FakeMapper {
        async fn build(
            &self,
            root: &Path,
            _budget_bytes: usize,
            admit: &(dyn for<'p> Fn(&'p Path) -> bool + Send + Sync),
        ) -> String {
            let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            let mut out = if self.frozen {
                "v0\n".to_string()
            } else {
                format!("v{n}\n")
            };
            for rel in ["src/lib.rs", "secrets/key.txt"] {
                if admit(&root.join(rel)) {
                    out.push_str(rel);
                    out.push('\n');
                }
            }
            out
        }
    }

    /// Scripted replies, every request kept.
    pub(crate) struct Recording {
        inner: Scripted,
        pub(crate) seen: StdMutex<Vec<Request>>,
    }

    impl Recording {
        pub(crate) fn turns(n: usize) -> Self {
            let toml = "[[turn]]\ntext = \"ok\"\n".repeat(n);
            Self {
                inner: Scripted::from_toml(&toml, "").expect("scenario"),
                seen: StdMutex::new(Vec::new()),
            }
        }

        /// system[2] of each turn request; the compaction summary's
        /// request has one system block and is left out.
        pub(crate) fn system_two(&self) -> Vec<String> {
            self.seen
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .filter(|r| r.system.len() > 2)
                .map(|r| r.system[2].text.clone())
                .collect()
        }
    }

    #[async_trait]
    impl Provider for Recording {
        fn id(&self) -> ProviderId {
            self.inner.id()
        }
        fn capabilities(&self) -> Caps {
            self.inner.capabilities()
        }
        async fn stream(
            &self,
            req: Request,
            sink: mpsc::Sender<ProviderEvent>,
            cancel: CancellationToken,
        ) -> Result<Usage, ProviderError> {
            self.seen
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(req.clone());
            self.inner.stream(req, sink, cancel).await
        }
        async fn count_tokens(&self, req: &Request) -> Result<u32, ProviderError> {
            self.inner.count_tokens(req).await
        }
    }

    pub(crate) fn config() -> cox_protocol::Config {
        let mut config = cox_protocol::Config::default();
        config.context.repomap_budget_tokens = 100;
        config
    }

    pub(crate) fn cwd() -> PathBuf {
        PathBuf::from("/tmp/cox-turn")
    }

    pub(crate) fn turn(text: &str) -> Submission {
        Submission::UserTurn {
            text: text.into(),
            attachments: vec![],
            confirm_think: false,
        }
    }

    /// A session over `turns` scripted replies with `mapper` installed.
    pub(crate) fn open(
        config: cox_protocol::Config,
        turns: usize,
        mapper: Arc<FakeMapper>,
    ) -> (Session, Arc<Recording>, Arc<MemoryStore>) {
        let store = Arc::new(MemoryStore::new());
        let provider = Arc::new(Recording::turns(turns));
        let session = Session::new(
            config,
            provider.clone(),
            vec![],
            store.clone(),
            store.clone(),
            cwd(),
        )
        .expect("session");
        session.set_repo_mapper(mapper);
        (session, provider, store)
    }

    pub(crate) fn built(store: &MemoryStore, session: &Session) -> Vec<RepoMapReason> {
        store
            .rollout_read(&session.id())
            .expect("rollout")
            .into_iter()
            .filter_map(|e| match e {
                Event::RepoMapBuilt { reason, .. } => Some(reason),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn repomap_is_built_once_per_session() {
        let mapper = Arc::new(FakeMapper::default());
        let (session, provider, store) = open(config(), 2, mapper.clone());
        session.submit(turn("one")).await.expect("one");
        session.submit(turn("two")).await.expect("two");
        assert_eq!(mapper.calls.load(Ordering::SeqCst), 1);
        assert_eq!(built(&store, &session), [RepoMapReason::SessionStart]);
        let two = provider.system_two();
        assert_eq!(two.len(), 2);
        assert!(two[0].ends_with("<repo_map>\nv1\nsrc/lib.rs\nsecrets/key.txt\n</repo_map>"));
    }

    /// The tree changing under the session (the fake answers `v2` if asked
    /// again) does not change the map: nothing rebuilds it on its own.
    #[tokio::test]
    async fn repomap_is_not_rebuilt_after_edit() {
        let mapper = Arc::new(FakeMapper::default());
        let (session, provider, _store) = open(config(), 3, mapper.clone());
        for text in ["one", "edit a file", "three"] {
            session.submit(turn(text)).await.expect("turn");
        }
        let two = provider.system_two();
        assert_eq!(two.len(), 3);
        assert!(two.iter().all(|t| *t == two[0]), "{two:?}");
        assert!(!two[0].contains("v2"));
        assert_eq!(mapper.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn repomap_is_off_at_budget_zero() {
        let mapper = Arc::new(FakeMapper::default());
        let (session, provider, store) = open(cox_protocol::Config::default(), 1, mapper.clone());
        session.submit(turn("one")).await.expect("one");
        assert_eq!(mapper.calls.load(Ordering::SeqCst), 0);
        assert!(built(&store, &session).is_empty());
        assert!(!provider.system_two()[0].contains("repo_map"));
    }

    #[tokio::test]
    async fn repomap_skips_denied_paths() {
        let mut config = config();
        config.permissions.deny.push("Read(secrets/**)".into());
        let (session, provider, _store) = open(config, 1, Arc::new(FakeMapper::default()));
        session.submit(turn("one")).await.expect("one");
        let two = &provider.system_two()[0];
        assert!(two.contains("src/lib.rs"), "{two}");
        assert!(!two.contains("secrets"), "{two}");
    }

    #[tokio::test]
    async fn subagent_gets_no_repomap() {
        let mapper = Arc::new(FakeMapper::default());
        let (session, _provider, _store) = open(config(), 1, mapper.clone());
        let child = session
            .spawn_child(
                config(),
                vec![],
                Job::Explore,
                Tier::Cheap,
                None,
                None,
                "explore-1".into(),
                "explore".into(),
            )
            .expect("child");
        child
            .submit(Submission::SetEffort { effort: None })
            .await
            .expect("submit");
        assert!(child.render_repomap().await.is_none());
        assert!(child.inner.lock().await.repomap.is_none());
        assert_eq!(mapper.calls.load(Ordering::SeqCst), 0);
    }

    /// Invariant 6: a resumed session sends the map it had, read back from
    /// the archive, without asking the mapper (which would now answer `v2`).
    #[tokio::test]
    async fn resume_builds_identical_request_with_repomap() {
        let mapper = Arc::new(FakeMapper::default());
        let (session, live, store) = open(config(), 1, mapper.clone());
        session.submit(turn("one")).await.expect("one");
        let events = store.rollout_read(&session.id()).expect("rollout");
        let history = crate::History::from_events(&events);
        assert!(history.repomap.is_some());

        let replay = Arc::new(Recording::turns(1));
        let resumed = Session::resume(
            config(),
            replay.clone(),
            vec![],
            store.clone(),
            store.clone(),
            cwd(),
            session.id(),
            history,
        )
        .expect("resume");
        resumed.set_repo_mapper(mapper.clone());
        resumed.submit(turn("two")).await.expect("two");
        assert_eq!(mapper.calls.load(Ordering::SeqCst), 1, "not rebuilt");
        assert_eq!(live.system_two()[0], replay.system_two()[0]);
        assert_eq!(built(&store, &resumed), [RepoMapReason::SessionStart]);
    }

    fn refresh() -> Submission {
        Submission::Command {
            command: SlashCommand {
                name: "repomap".into(),
                args: vec!["refresh".into()],
            },
        }
    }

    #[tokio::test]
    async fn repomap_refresh_with_changed_tree_emits_one_event() {
        let mapper = Arc::new(FakeMapper::default());
        let (session, provider, store) = open(config(), 2, mapper.clone());
        session.submit(turn("one")).await.expect("one");
        session.submit(refresh()).await.expect("refresh");
        session.submit(turn("two")).await.expect("two");
        assert_eq!(
            built(&store, &session),
            [RepoMapReason::SessionStart, RepoMapReason::Refresh]
        );
        let two = provider.system_two();
        assert!(two[0].contains("<repo_map>\nv1\n"), "{}", two[0]);
        assert!(two[1].contains("<repo_map>\nv2\n"), "{}", two[1]);
    }

    /// An unchanged tree records nothing, so the next request's system[2]
    /// is byte-identical and the cached prefix survives.
    #[tokio::test]
    async fn repomap_refresh_unchanged_keeps_prefix_bytes() {
        let mapper = Arc::new(FakeMapper {
            frozen: true,
            ..FakeMapper::default()
        });
        let (session, provider, store) = open(config(), 2, mapper.clone());
        session.submit(turn("one")).await.expect("one");
        session.submit(refresh()).await.expect("refresh");
        session.submit(turn("two")).await.expect("two");
        assert_eq!(mapper.calls.load(Ordering::SeqCst), 2, "the refresh asked");
        assert_eq!(built(&store, &session), [RepoMapReason::SessionStart]);
        let two = provider.system_two();
        assert_eq!(two[0], two[1]);
    }

    #[tokio::test]
    async fn repomap_refresh_is_refused_mid_turn() {
        let mapper = Arc::new(FakeMapper::default());
        let (session, _provider, store) = open(config(), 1, mapper.clone());
        session.submit(turn("one")).await.expect("one");
        session.inner.lock().await.state = State::Streaming;
        session.submit(refresh()).await.expect("refresh");
        assert_eq!(mapper.calls.load(Ordering::SeqCst), 1, "not rebuilt");
        assert_eq!(built(&store, &session), [RepoMapReason::SessionStart]);
        let map = session.inner.lock().await.repomap.clone();
        assert!(map.is_some_and(|m| m.starts_with("v1\n")));
    }

    /// Three turns over the default `keep_turns = 2` leave one to summarise;
    /// the fourth scripted reply is the summary.
    #[tokio::test]
    async fn compaction_rebuilds_repomap() {
        let mapper = Arc::new(FakeMapper::default());
        let (session, provider, store) = open(config(), 5, mapper.clone());
        for text in ["one", "two", "three"] {
            session.submit(turn(text)).await.expect("turn");
        }
        session
            .submit(Submission::Compact { focus: None })
            .await
            .expect("compact");
        session.submit(turn("four")).await.expect("four");
        assert_eq!(
            built(&store, &session),
            [RepoMapReason::SessionStart, RepoMapReason::Compaction]
        );
        let two = provider.system_two();
        let last = two.last().expect("a request");
        assert!(last.contains("<repo_map>\nv2\n"), "{last}");
    }
}
