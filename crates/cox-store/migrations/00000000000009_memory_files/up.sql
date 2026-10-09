-- Files a memory entry names (T59.10), so a search can rank an entry above an
-- equally text-matching one when the session touched one of its files. `path`
-- is the canonical absolute path, the form the session's touched paths take.
-- Cascade keeps the link rows from outliving their memory row.
CREATE TABLE memory_files (
  memory_id INTEGER NOT NULL REFERENCES memory(id) ON DELETE CASCADE,
  path TEXT NOT NULL,
  PRIMARY KEY (memory_id, path)
);
CREATE INDEX memory_files_path ON memory_files(path);
