// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The supervisor's bookkeeping (`docs/design/serve.md` D3, D7): one slot per
//! resident session, each exit fed to `restart::Policy` with the uptime
//! measured here, restarts as timed events, the cap of 16 workers, and the
//! shutdown that ends every worker before it kills the stragglers. Separate
//! from `worker` so the decisions stay readable without the process plumbing.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::restart::{Decision, Policy};
use crate::router::Routes;
use crate::worker::{Spawned, Worker, open_line};

/// At most this many workers run at once (D3); no config key in the first cut.
pub const CAP: usize = 16;

/// Why `open` started no worker.
#[derive(Debug, thiserror::Error)]
pub enum SuperviseError {
    /// [`CAP`] workers already run.
    #[error("cox serve already runs {CAP} sessions; end one before opening another")]
    Cap,
    /// The worker program did not start.
    #[error("could not start the worker: {0}")]
    Spawn(#[from] std::io::Error),
    /// The worker exited without answering `open`.
    #[error("the worker exited before it opened the session")]
    NoReply,
    /// The worker answered `open` with this error.
    #[error("the worker could not open the session: {0}")]
    Refused(String),
}

/// How workers are started and stopped.
#[derive(Debug, Clone)]
pub struct Options {
    /// The worker binary.
    pub program: PathBuf,
    /// Its arguments.
    pub args: Vec<OsString>,
    /// How long `end` may take before SIGKILL (D7).
    pub grace: Duration,
    /// A ceiling on the policy's restart delay, so a test need not wait out
    /// the real backoff; production leaves the policy's own 30 s cap.
    pub max_restart_delay: Duration,
}

impl Options {
    /// `program serve --worker`; production passes `current_exe()`, never a
    /// program found on `PATH` (D3).
    pub fn worker(program: PathBuf) -> Self {
        Self {
            program,
            args: vec!["serve".into(), "--worker".into()],
            grace: Duration::from_secs(10),
            max_restart_delay: Duration::MAX,
        }
    }
}

/// A session as the supervisor lists it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// A worker serves it.
    Running,
    /// Crashed and waiting out its restart delay.
    Restarting,
    /// The policy gave up; only an explicit `open` tries it again.
    Failed,
}

enum State {
    Running(Worker),
    /// The serial of the worker that died, so a stale timer cannot restart a
    /// session that was reopened meanwhile.
    Waiting(u64),
    Failed,
}

struct Slot {
    cwd: PathBuf,
    theme: String,
    policy: Policy,
    state: State,
}

/// Each names a worker by its serial.
pub(crate) enum Event {
    /// It exited; `true` for status 0.
    Exited(u64, bool),
    /// Its restart delay is over.
    Restart(u64),
    /// As a restart, it answered `open` with an error.
    Refused(u64),
}

/// Owns every worker; see the module header.
pub struct Supervisor {
    options: Options,
    slots: HashMap<String, Slot>,
    serial: u64,
    events: mpsc::UnboundedSender<Event>,
    inbox: mpsc::UnboundedReceiver<Event>,
    routes: Option<Arc<Routes>>,
}

impl Supervisor {
    /// A supervisor with no workers yet.
    pub fn new(options: Options) -> Self {
        let (events, inbox) = mpsc::unbounded_channel();
        Self {
            options,
            slots: HashMap::new(),
            serial: 0,
            events,
            inbox,
            routes: None,
        }
    }

    /// Hand every worker's lines after `open` to the router.
    pub(crate) fn route_through(&mut self, routes: Arc<Routes>) {
        self.routes = Some(routes);
    }

    /// Every resident session and its state.
    pub fn list(&self) -> Vec<(String, Status)> {
        let status = |slot: &Slot| match slot.state {
            State::Running(_) => Status::Running,
            State::Waiting(_) => Status::Restarting,
            State::Failed => Status::Failed,
        };
        self.slots
            .iter()
            .map(|(id, s)| (id.clone(), status(s)))
            .collect()
    }

    /// Start a worker for a new session in `cwd`, or for `resume`'s. A failed
    /// session starts over with a fresh failure count (D3).
    pub async fn open(
        &mut self,
        cwd: PathBuf,
        resume: Option<String>,
        theme: String,
    ) -> Result<String, SuperviseError> {
        if let Some(id) = &resume
            && self
                .slots
                .get(id)
                .is_some_and(|s| !matches!(s.state, State::Failed))
        {
            // Already resident; the router (S7) turns this into `attach`.
            return Ok(id.clone());
        }
        let live = self.slots.values();
        if live.filter(|s| !matches!(s.state, State::Failed)).count() >= CAP {
            return Err(SuperviseError::Cap);
        }
        let line = open_line(&cwd, resume.as_deref(), &theme);
        let (worker, reply) = self.spawn(&line).await?;
        let session = reply
            .await
            .map_err(|_| SuperviseError::NoReply)?
            .map_err(SuperviseError::Refused)?;
        let (policy, state) = (Policy::new(), State::Running(worker));
        let slot = Slot {
            cwd,
            theme,
            policy,
            state,
        };
        self.slots.insert(session.clone(), slot);
        Ok(session)
    }

    /// Handle the next worker exit or due restart. Cancelling it drops at
    /// most a restart in progress, which `run` does only to shut down.
    pub async fn watch(&mut self) {
        let event = self.next_event().await;
        self.handle(event).await;
    }

    /// The next worker exit or due restart; cancel-safe, so the router's
    /// loop can wait on it beside its other work.
    pub(crate) async fn next_event(&mut self) -> Option<Event> {
        self.inbox.recv().await
    }

    pub(crate) async fn handle(&mut self, event: Option<Event>) {
        match event {
            Some(Event::Exited(serial, success)) => self.exited(serial, success),
            Some(Event::Restart(serial)) => self.restart(serial).await,
            Some(Event::Refused(serial)) => {
                if let Some((_, worker)) = self.worker(serial) {
                    worker.kill();
                }
            }
            None => {}
        }
    }

    /// Watch workers until `shutdown` resolves (SIGINT or SIGTERM in
    /// production, see [`terminated`]), then shut down. Returns how many
    /// workers needed SIGKILL.
    pub async fn run(&mut self, shutdown: impl Future<Output = ()>) -> usize {
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                () = &mut shutdown => break,
                () = self.watch() => {}
            }
        }
        self.shutdown().await
    }

    /// Send `end` to every worker, wait up to the grace, then SIGKILL the
    /// rest (D7). Returns how many were killed.
    pub async fn shutdown(&mut self) -> usize {
        let deadline = tokio::time::Instant::now() + self.options.grace;
        let mut alive = HashSet::new();
        for (id, slot) in &mut self.slots {
            if let State::Running(worker) = &mut slot.state {
                let _ = tokio::time::timeout_at(deadline, worker.end(id)).await;
                alive.insert(worker.serial);
            }
        }
        while !alive.is_empty() {
            match tokio::time::timeout_at(deadline, self.inbox.recv()).await {
                Ok(Some(Event::Exited(serial, _))) => alive.remove(&serial),
                Ok(_) => continue,
                Err(_) => break,
            };
        }
        let killed = alive.len();
        for serial in &alive {
            if let Some((_, worker)) = self.worker(*serial) {
                worker.kill();
            }
        }
        while !alive.is_empty() {
            if let Some(Event::Exited(serial, _)) = self.inbox.recv().await {
                alive.remove(&serial);
            }
        }
        self.slots.clear();
        killed
    }

    async fn spawn(&mut self, open: &str) -> std::io::Result<Spawned> {
        self.serial += 1;
        let serial = self.serial;
        let events = self.events.clone();
        let on_exit = move |success| {
            let _ = events.send(Event::Exited(serial, success));
        };
        let (options, routes) = (&self.options, self.routes.clone());
        Worker::spawn(
            &options.program,
            &options.args,
            serial,
            open,
            routes,
            on_exit,
        )
        .await
    }

    /// The running worker with this serial, and its session.
    fn worker(&mut self, serial: u64) -> Option<(&String, &mut Worker)> {
        self.slots
            .iter_mut()
            .find_map(|(id, slot)| match &mut slot.state {
                State::Running(w) if w.serial == serial => Some((id, w)),
                _ => None,
            })
    }

    fn exited(&mut self, serial: u64, success: bool) {
        if let Some((id, worker)) = self.worker(serial) {
            let (id, uptime) = (id.clone(), worker.started.elapsed());
            self.decide(id, serial, uptime, success);
        }
    }

    fn decide(&mut self, id: String, serial: u64, uptime: Duration, clean: bool) {
        let Some(slot) = self.slots.get_mut(&id) else {
            return;
        };
        match slot.policy.on_exit(uptime, clean) {
            Decision::Done => {
                self.slots.remove(&id);
            }
            Decision::GiveUp => slot.state = State::Failed,
            Decision::Restart(delay) => {
                slot.state = State::Waiting(serial);
                let delay = delay.min(self.options.max_restart_delay);
                let events = self.events.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(delay).await;
                    let _ = events.send(Event::Restart(serial));
                });
            }
        }
    }

    /// Reopen the session with `resume` and leave it idle: the turn that was
    /// running when the worker died is not run again (D3).
    async fn restart(&mut self, dead: u64) {
        let found = self.slots.iter().find_map(|(id, slot)| match slot.state {
            State::Waiting(serial) if serial == dead => {
                Some((id.clone(), open_line(&slot.cwd, Some(id), &slot.theme)))
            }
            _ => None,
        });
        let Some((id, line)) = found else {
            return;
        };
        match self.spawn(&line).await {
            // The reply is awaited off the event loop. A worker that cannot
            // reopen the session stays up with none, so it is killed and its
            // exit counts as one more failure.
            Ok((worker, reply)) => {
                let (serial, events) = (worker.serial, self.events.clone());
                tokio::spawn(async move {
                    if let Ok(Err(_)) = reply.await {
                        let _ = events.send(Event::Refused(serial));
                    }
                });
                if let Some(slot) = self.slots.get_mut(&id) {
                    slot.state = State::Running(worker);
                }
            }
            Err(_) => self.decide(id, dead, Duration::ZERO, false),
        }
    }
}

/// Resolves on SIGINT or SIGTERM, the production shutdown trigger for `run`.
pub async fn terminated() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut term) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::Instant;
    use tokio::time::timeout;

    /// Stands in for `cox serve --worker`, as `crates/cox/tests/remote.rs`
    /// fakes `ssh`: it logs every line, answers `open` with `resume`'s id (or
    /// a new one), and exits 0 on `end`. `crash` exits 3 after every `open`;
    /// `crash-once` only the first time; `stubborn-once` ignores the first
    /// `end` it gets.
    const FAKE: &str = r#"log="$1"; mode="$2"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$log"
  case "$line" in
    *'"method":"open"'*)
      s=$(printf '%s\n' "$line" | sed -n 's/.*"resume":"\([^"]*\)".*/\1/p')
      [ -n "$s" ] || s="s$$"
      printf '{"type":"response","v":1,"id":1,"outcome":{"status":"ok","value":{"kind":"opened","value":{"session":"%s","blocks":[],"warnings":[]}}}}\n' "$s"
      case "$mode" in
        crash) exit 3 ;;
        crash-once) [ -e "$log.crashed" ] || { : > "$log.crashed"; exit 3; } ;;
      esac ;;
    *'"method":"end"'*)
      # mkdir is atomic, so exactly one of the workers ending at once wins.
      if [ "$mode" = stubborn-once ] && mkdir "$log.stubborn" 2>/dev/null; then
        exec tail -f /dev/null
      fi
      exit 0 ;;
  esac
done
"#;

    const WAIT: Duration = Duration::from_secs(5);

    struct Fake {
        dir: tempfile::TempDir,
    }

    impl Fake {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            std::fs::write(dir.path().join("worker.sh"), FAKE).expect("fake worker");
            Self { dir }
        }

        /// Run through `/bin/sh` so no freshly written file is executed.
        fn supervisor(&self, mode: &str) -> Supervisor {
            let dir = self.dir.path();
            let mut options = Options::worker("/bin/sh".into());
            options.args = vec![
                dir.join("worker.sh").into(),
                dir.join("log").into(),
                mode.into(),
            ];
            options.grace = Duration::from_millis(300);
            options.max_restart_delay = Duration::from_millis(10);
            Supervisor::new(options)
        }

        async fn open(&self, sup: &mut Supervisor, resume: Option<&str>) -> String {
            let cwd = self.dir.path().to_path_buf();
            let resume = resume.map(str::to_owned);
            let opened = sup.open(cwd, resume, "dark".into());
            timeout(WAIT, opened).await.expect("in time").expect("open")
        }

        fn log(&self) -> Vec<String> {
            let log = std::fs::read_to_string(self.dir.path().join("log")).expect("log");
            log.lines().map(str::to_owned).collect()
        }
    }

    async fn watch(sup: &mut Supervisor) {
        timeout(WAIT, sup.watch()).await.expect("an event in time");
    }

    #[tokio::test]
    async fn a_crashed_worker_is_restarted_idle_with_resume() {
        let fake = Fake::new();
        let mut sup = fake.supervisor("crash-once");
        let id = fake.open(&mut sup, None).await;

        watch(&mut sup).await;
        assert_eq!(sup.list(), [(id.clone(), Status::Restarting)]);
        watch(&mut sup).await;
        assert_eq!(sup.list(), [(id.clone(), Status::Running)]);

        // Ending flushes the fake's log: it reads its lines in order.
        assert_eq!(timeout(WAIT, sup.shutdown()).await.expect("in time"), 0);
        let log = fake.log();
        assert_eq!(log.len(), 3, "{log:?}");
        assert!(log[0].contains(r#""resume":null"#), "{log:?}");
        assert!(log[1].contains(&format!(r#""resume":"{id}""#)), "{log:?}");
        // Idle: nothing but `end` reached the restarted worker.
        assert!(log[2].contains(r#""method":"end""#), "{log:?}");
    }

    #[tokio::test]
    async fn give_up_marks_the_session_failed() {
        let fake = Fake::new();
        let mut sup = fake.supervisor("crash");
        let id = fake.open(&mut sup, None).await;

        while sup.list() != [(id.clone(), Status::Failed)] {
            watch(&mut sup).await;
        }
        let resumed = format!(r#""resume":"{id}""#);
        let log = fake.log();
        assert_eq!(log.len(), 5, "the first open and four restarts: {log:?}");
        assert_eq!(log.iter().filter(|l| l.contains(&resumed)).count(), 4);

        // Only an explicit open tries again, with a fresh failure count.
        assert_eq!(fake.open(&mut sup, Some(&id)).await, id);
        assert_eq!(sup.list(), [(id.clone(), Status::Running)]);
        watch(&mut sup).await;
        assert_eq!(sup.list(), [(id, Status::Restarting)]);
        timeout(WAIT, sup.shutdown()).await.expect("in time");
    }

    #[tokio::test]
    async fn shutdown_ends_every_worker() {
        let fake = Fake::new();
        let mut sup = fake.supervisor("stubborn-once");
        let mut ids = Vec::new();
        for _ in 0..3 {
            ids.push(fake.open(&mut sup, None).await);
        }

        let started = Instant::now();
        let killed = timeout(WAIT, sup.run(std::future::ready(()))).await;
        assert_eq!(killed.expect("in time"), 1, "the stubborn one");
        assert!(
            started.elapsed() >= Duration::from_millis(300),
            "it got its grace"
        );
        assert!(sup.list().is_empty());
        let log = fake.log();
        for id in &ids {
            let end = format!(r#"{{"method":"end","params":{{"session":"{id}"}}}}"#);
            assert!(log.iter().any(|l| l.contains(&end)), "{id}: {log:?}");
        }
    }

    #[tokio::test]
    async fn the_cap_refuses_the_seventeenth_open() {
        let fake = Fake::new();
        let mut sup = fake.supervisor("idle");
        for _ in 0..CAP {
            fake.open(&mut sup, None).await;
        }
        let opened = sup.open(fake.dir.path().to_path_buf(), None, "dark".into());
        let refused = timeout(WAIT, opened).await.expect("in time");
        assert!(matches!(refused, Err(SuperviseError::Cap)), "{refused:?}");
        assert_eq!(sup.list().len(), CAP);
        assert_eq!(timeout(WAIT, sup.shutdown()).await.expect("in time"), 0);
    }
}
