//! The Cursor Cloud Agents API client crate (P56, A123). Its own crate under
//! D1 because it is the one place a socket to `api.cursor.com` opens (T56.2),
//! and it is neither a `Provider` (an agent run is not a chat completion) nor
//! session assembly (the host driver that maps runs onto task events lives in
//! `cox-session`). This slice (T56.1) holds only the hand-written wire types.

pub mod wire;
