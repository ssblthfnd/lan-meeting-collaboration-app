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
//! - participant sessions and the claim model ([`session`], [`token`])
//! - note versioning and audit policy ([`service`], [`audit`])
//! - domain events and the audience model ([`event`])
//! - the persistence port ([`port`]) that `app-db` implements
//!
//! Explicitly **not** responsible for: HTTP, WebSocket, SQL, file I/O, UI. It
//! has no SQLite and no transport dependency; persistence arrives through
//! [`port::Database`], and notification through [`event::EventSink`], which is
//! a trait with no channel, no socket and no runtime behind it here.
//!
//! Status: Phase 1, step 7. Identifiers, time, the lifecycle, authorization,
//! note versioning, audit policy and the mutation boundary are implemented, and
//! so are meeting creation/configuration, the participant roster, the
//! join-token and identity-claim rules, and the domain event model with its
//! audience derivation.
//!
//! Resolving a presented credential is still the transport's job: this crate
//! holds only [`token::TokenHash`], never a token, and has no way to generate or
//! hash one (architecture rules section 14.1, ADR-0016).

#![forbid(unsafe_code)]

pub mod actor;
pub mod audit;
pub mod authz;
pub mod error;
pub mod event;
pub mod id;
pub mod meeting;
pub mod participant;
pub mod port;
pub mod service;
pub mod session;
mod text;
pub mod time;
pub mod token;

pub use actor::Actor;
pub use audit::{AuditAction, AuditEntry, AuditTarget};
pub use authz::{authorize, Authorized, Operation};
pub use error::{DomainError, DomainResult};
pub use event::{Audience, CompositeSink, DomainEvent, EventSink, NoEvents};
pub use id::{
    AuditLogId, Entity, Id, IdError, MeetingId, NoteId, NoteLinkId, NoteVersionId, ParticipantId,
    RemoteSubmissionId, SessionId, SubmissionId,
};
pub use meeting::{Meeting, MeetingConfiguration, MeetingStatus};
pub use participant::{ParticipantDetails, MAX_PARTICIPANTS, MIN_PARTICIPANTS};
pub use port::{
    Database, DomainTx, NewMeeting, NewNote, NewNoteVersion, NewParticipant, NewSession, NoteRow,
    ParticipantRow,
};
pub use service::{
    Domain, IdentityClaimed, JoinTokenIssued, MeetingCreated, MeetingTransitioned, MeetingUpdated,
    NoteWritten, ParticipantAdded, ParticipantRemoved, ParticipantUpdated, SessionChanged,
    WriteNote,
};
pub use session::{ClaimStatus, SessionBinding};
pub use time::{MeetingDate, MeetingTime, MeetingTimeZone, TimeError, UtcTimestamp};
pub use token::{TokenError, TokenHash};
