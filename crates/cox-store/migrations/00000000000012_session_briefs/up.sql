-- The latest compaction summary per project, keyed by the canonical
-- workspace root (not the folder name), read by the next session there.
CREATE TABLE session_briefs (
  project TEXT PRIMARY KEY NOT NULL,
  body TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
