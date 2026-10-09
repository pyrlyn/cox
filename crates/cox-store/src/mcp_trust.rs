//! Approved MCP tool contracts (T64.24): the `mcp_tool_trust` table over
//! Diesel's typed DSL. A row exists only after the user (or a user-layer
//! baseline) approved one hash. Pending and changed are not stored: cox-mcp
//! derives them, and a changed definition must not overwrite this row.

use diesel::prelude::*;

use cox_protocol::StoreError;

use crate::models::McpToolTrustRow;
use crate::schema::mcp_tool_trust::dsl as t;
use crate::{Store, now_rfc3339, write_tx};

impl Store {
    /// The approved hash for `(server, tool)`, if the user has one.
    pub fn mcp_trust_get(&self, server: &str, tool: &str) -> Result<Option<String>, StoreError> {
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        let hash: Option<String> = t::mcp_tool_trust
            .filter(t::server.eq(server))
            .filter(t::tool.eq(tool))
            .select(t::hash)
            .first(&mut *conn)
            .optional()
            .map_err(|_| StoreError::Sqlite)?;
        Ok(hash)
    }

    /// Records `hash` as the approved contract. A second call replaces the
    /// hash: `cox mcp trust` is the user accepting the definition they see
    /// now, including one that changed since the previous approval.
    pub fn mcp_trust_approve(
        &self,
        server: &str,
        tool: &str,
        hash: &str,
    ) -> Result<(), StoreError> {
        let row = McpToolTrustRow {
            server: server.to_string(),
            tool: tool.to_string(),
            hash: hash.to_string(),
            status: "approved".to_string(),
            updated_at: now_rfc3339(),
        };
        let mut conn = self.conn.lock().map_err(|_| StoreError::Io)?;
        write_tx(&mut conn, |conn| {
            diesel::delete(
                t::mcp_tool_trust
                    .filter(t::server.eq(server))
                    .filter(t::tool.eq(tool)),
            )
            .execute(&mut *conn)
            .map_err(|_| StoreError::Sqlite)?;
            diesel::insert_into(t::mcp_tool_trust)
                .values(&row)
                .execute(&mut *conn)
                .map_err(|_| StoreError::Sqlite)?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use cox_protocol::Store as _;

    use super::*;

    fn store() -> (tempfile::TempDir, Store) {
        let home = tempfile::tempdir().expect("home");
        let store = Store::open(home.path()).expect("store");
        (home, store)
    }

    #[test]
    fn mcp_trust_approve_replaces_the_hash_and_get_misses_an_unknown_tool() {
        let (_home, store) = store();
        assert_eq!(store.mcp_trust_get("evil", "run").expect("get"), None);
        store
            .mcp_trust_approve("evil", "run", "hash-1")
            .expect("approve");
        assert_eq!(
            store.mcp_trust_get("evil", "run").expect("get").as_deref(),
            Some("hash-1")
        );
        store
            .mcp_trust_approve("evil", "run", "hash-2")
            .expect("re-approve");
        assert_eq!(
            store.mcp_trust_get("evil", "run").expect("get").as_deref(),
            Some("hash-2")
        );
        assert_eq!(store.mcp_trust_get("evil", "other").expect("get"), None);
    }
}
