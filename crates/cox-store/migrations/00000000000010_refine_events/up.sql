-- One row per refine of the harness entries (T66.4): proposed, applied,
-- rejected or rolled back. `edits` is a JSON array of
-- {action, kind, id, before_sha, after_sha}; `evidence` is the model's
-- rationale for the whole refine, not per edit. `rollback_of` names the event
-- an undo reverses. `session_id` has no foreign key so the history outlives
-- the session row, as in `cloud_runs`.
CREATE TABLE refine_events (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL,
  scope TEXT NOT NULL CHECK (scope IN ('local','global')),
  trigger TEXT NOT NULL CHECK (trigger IN ('manual','auto')),
  evidence TEXT NOT NULL,
  edits TEXT NOT NULL,
  outcome TEXT NOT NULL CHECK (outcome IN ('applied','rejected','rolled_back')),
  rollback_of TEXT,
  fingerprint TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE INDEX refine_events_session ON refine_events (session_id, created_at);
