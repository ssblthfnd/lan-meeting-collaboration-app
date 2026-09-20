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
//! # What the schema enforces by itself
//!
//! The migration is written so that the invariants below hold even if a caller
//! forgets to check them. A disabled button is not enforcement, and neither is
//! a validation function that some future code path skips:
//!
//! - ids are canonical UUIDv7, not arbitrary strings
//! - system timestamps are UTC, fixed width, and sort chronologically as text
//! - a participant's note is unique per meeting (ADR-0003)
//! - a note's participant belongs to the note's meeting, via a composite key
//! - at most one live session per participant identity (ADR-0002)
//! - note history is never rewritten, and audit rows are append-only
//! - a note carries at most five links (PRD section 14)
//!
//! Status: Phase 1, step 1. Schema, migrations, pool and transaction helpers
//! exist. Repositories arrive with the steps that need them; there is
//! deliberately no meeting or note service here yet.

#![forbid(unsafe_code)]

pub mod error;
pub mod migrations;
pub mod pool;
pub mod sql;

pub use error::{DbError, DbResult};
pub use pool::Db;
pub use sql::Sql;
