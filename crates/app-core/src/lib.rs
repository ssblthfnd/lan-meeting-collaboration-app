//! # app-core
//!
//! Domain core of the LAN Meeting Collaboration App.
//!
//! This crate is the **only** place where business rules live. Both transports
//! (`app-server` for LAN participants, `src-tauri` commands for the Host) must
//! route every mutation through this crate so that authorization, meeting lock
//! enforcement, auditing and note versioning cannot be bypassed.
//!
//! Responsibilities:
//! - actor resolution contract ([`actor::Actor`])
//! - identifiers ([`id`]) and time/timezone handling ([`time`])
//! - authorization decisions
//! - meeting lock enforcement (inside the same transaction as the mutation)
//! - audit log policy
//! - note versioning policy
//!
//! Explicitly **not** responsible for: HTTP, WebSocket, SQL, file I/O, UI. In
//! particular this crate has no SQLite dependency; the canonical *formats* it
//! defines are what `app-db` stores, but the SQL lives there.
//!
//! Status: Phase 1, step 1. Identifiers and time are implemented; the rule
//! engine itself arrives in step 2.

#![forbid(unsafe_code)]

pub mod actor;
pub mod error;
pub mod id;
pub mod time;

pub use actor::Actor;
pub use error::{CoreError, CoreResult};
pub use id::{
    AuditLogId, Entity, Id, IdError, MeetingId, NoteId, NoteLinkId, NoteVersionId, ParticipantId,
    RemoteSubmissionId, SessionId, SubmissionId,
};
pub use time::{MeetingDate, MeetingTime, MeetingTimeZone, TimeError, UtcTimestamp};
