-- One row per hosted cloud-agent run (P56, T56.5), so a run outlives the
-- session that started it: a resumed session finds the runs still going and
-- records each run's usage once. `status` is the backend's own word and
-- `terminal` the host's verdict on it, so no backend's status list lives in
-- this crate. `usage_recorded` flips once, in a conditional UPDATE, so two
-- resumers cannot both write the ledger row. `model` is NULL when the
-- backend picked its default.
CREATE TABLE cloud_runs (
  backend TEXT NOT NULL, run_id TEXT NOT NULL,
  session_id TEXT NOT NULL, task_id TEXT NOT NULL,
  agent_id TEXT NOT NULL, repository TEXT NOT NULL, model TEXT,
  status TEXT NOT NULL, terminal INTEGER NOT NULL DEFAULT 0,
  usage_recorded INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
  PRIMARY KEY (backend, run_id)
);

CREATE INDEX cloud_runs_session ON cloud_runs (session_id);
