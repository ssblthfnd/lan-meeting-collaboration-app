//! # app-core
//!
//! Domain core of the LAN Meeting Collaboration App.
//!
//! This crate is the **only** place where business rules live. Both transports
//! (`app-server` for LAN participants, `src-tauri` commands for the Host) must
//! route every mutation through [`service::Domain`], which is the single
//! mutation boundary: authorization, meeting lock enforcement, note versioning
//! and auditing all happen inside one transaction there, so no transport can
//! skip them.
//!
//! Responsibilities:
//! - actor contract ([`actor::Actor`]) and authorization ([`authz`])
//! - identifiers ([`id`]) and time/timezone handling ([`time`])
//! - meeting lifecycle, configuration and the lock rule ([`meeting`])
//! - the participant roster ([`participant`])
//! - note versioning and audit policy ([`service`], [`audit`])
//! - the persistence port ([`port`]) that `app-db` implements
//!
//! Explicitly **not** responsible for: HTTP, WebSocket, SQL, file I/O, UI. It
//! has no SQLite and no transport dependency; persistence arrives through
//! [`port::Database`].
//!
//! Status: Phase 1, step 3. Identifiers, time, the lifecycle, authorization,
//! note versioning, audit policy and the mutation boundary are implemented, and
//! so are meeting creation/configuration and the participant roster.
//! Session/token resolution belongs to the transport layer and is not here.

#![forbid(unsafe_code)]

pub mod actor;
pub mod audit;
pub mod authz;
pub mod error;
pub mod id;
pub mod meeting;
pub mod participant;
pub mod port;
pub mod service;
mod text;
pub mod time;

pub use actor::Actor;
pub use audit::{AuditAction, AuditEntry, AuditTarget};
pub use authz::{authorize, Authorized, Operation};
pub use error::{DomainError, DomainResult};
pub use id::{
    AuditLogId, Entity, Id, IdError, MeetingId, NoteId, NoteLinkId, NoteVersionId, ParticipantId,
    RemoteSubmissionId, SessionId, SubmissionId,
};
pub use meeting::{Meeting, MeetingConfiguration, MeetingStatus};
pub use participant::{ParticipantDetails, MAX_PARTICIPANTS, MIN_PARTICIPANTS};
pub use port::{
    Database, DomainTx, NewMeeting, NewNote, NewNoteVersion, NewParticipant, NoteRow,
    ParticipantRow,
};
pub use service::{
    Domain, MeetingCreated, MeetingTransitioned, MeetingUpdated, NoteWritten, ParticipantAdded,
    ParticipantRemoved, ParticipantUpdated, WriteNote,
};
pub use time::{MeetingDate, MeetingTime, MeetingTimeZone, TimeError, UtcTimestamp};
