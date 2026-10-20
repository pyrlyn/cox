-- T66.14: a sweep sets this instead of deleting the row, so the rollout
-- and `cox expand` keep working. NULL means the session is still listed.
ALTER TABLE sessions ADD COLUMN archived_at TEXT;
