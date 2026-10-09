-- The approved contract of one MCP tool (T64.24). Only `approved` is
-- stored: pending and changed are the absence of this row, or a hash that
-- no longer matches, decided in cox-mcp. A changed definition must not
-- overwrite the row by itself.
CREATE TABLE mcp_tool_trust (
  server TEXT NOT NULL,
  tool TEXT NOT NULL,
  hash TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('approved')),
  updated_at TEXT NOT NULL,
  PRIMARY KEY (server, tool)
);
