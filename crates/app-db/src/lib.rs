//! # app-db
//!
//! SQLite persistence layer. SQLite is the **source of truth** for the whole
//! application; no important state may live only in a frontend.
//!
//! Responsibilities:
//! - connection/pool management and PRAGMAs (WAL, foreign keys, busy timeout)
//! - embedded schema migrations
//! - repositories (typed read/write access)
//! - transaction helpers; the meeting lock check runs *inside* the mutating
//!   transaction, never as a separate pre-flight query
//!
//! The database file is local to the Host device and is never exposed to the
//! network (PRD 22.7, 22.8).
//!
//! Status: skeleton. No tables, migrations or queries exist yet
//! (Phase 1, step 1).

#![forbid(unsafe_code)]

use thiserror::Error;

pub type DbResult<T> = Result<T, DbError>;

/// Errors produced by the persistence layer.
#[derive(Debug, Error)]
pub enum DbError {
    /// Placeholder until the SQLite driver is introduced in Phase 1, step 1.
    #[error("database error: {0}")]
    Other(String),
}
