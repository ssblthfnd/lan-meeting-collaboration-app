//! # app-db
//!
//! SQLite persistence layer. SQLite is the **source of truth** for the whole
//! application; no important state may live only in a frontend.
//!
//! Responsibilities:
//! - connection/pool management and PRAGMAs (WAL, foreign keys, busy timeout)
//! - embedded schema migrations
//! - repositories: the domain-port write adapter, and typed read queries
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
//! - a note carries at most five links, and a meeting at most 99 participants
//!   (PRD sections 7 and 14)
//!
//! # Adapter, not decision-maker
//!
//! [`repository`] implements `app_core::port::DomainTx`, the persistence port
//! the domain defines. It holds persistence mechanics only: authorization and
//! the meeting-lock check live in `app-core`, and every write method here
//! requires an `Authorized` that only `app-core` can mint. This crate cannot
//! grant itself permission to write.
//!
//! # Two sides, one crate
//!
//! [`repository`] is the write adapter behind the domain's mutation boundary;
//! [`query`] answers what the Host UI displays and [`participant_query`] what a
//! LAN participant may see. Reads deliberately do not travel through the domain
//! (ADR-0014), but every half is the same kind of thing: mechanics with no rules
//! in them.
//!
//! The two read types are separate on purpose. The Host may see everything in
//! their own database and a participant may not, so reusing one convenient read
//! model for both audiences is how a participant ends up holding something only
//! the Host should have (architecture rules section 16).
//!
//! [`session_store`] is neither: it answers "who is asking" by resolving a
//! credential hash, and its output becomes an `Actor`.
//!
//! [`presence`] is the other exception. It records that a participant's socket
//! opened or closed, which is an observation rather than an action somebody
//! took: it is not authorized, not audited, and not subject to the meeting
//! lock. Its module documentation says exactly what `last_seen_at` promises and
//! what it does not (ADR-0018).
//!
//! Status: Phase 1, step 8. Schema, migrations, pool, transaction helpers, the
//! domain-port adapter, the Host read queries (meetings, participants, audit
//! and notes), the participant read queries, credential resolution and the
//! presence store all exist.

#![forbid(unsafe_code)]

pub mod error;
pub mod migrations;
pub mod participant_query;
pub mod pool;
pub mod presence;
pub mod query;
pub mod repository;
pub mod session_store;
pub mod sql;

pub use error::{DbError, DbResult};
pub use participant_query::{ClaimableIdentity, JoinableMeeting, OwnIdentity, ParticipantQueries};
pub use pool::Db;
pub use presence::{ParticipantPresence, PresenceStore};
pub use query::{
    AuditEntryView, HostQueries, MeetingDetail, MeetingSummary, NoteDetail, NoteOverview,
    NoteVersionDetail, NoteVersionSummary, ParticipantSummary,
};
pub use repository::DbTx;
pub use session_store::{JoinTarget, SessionStore};
pub use sql::Sql;
