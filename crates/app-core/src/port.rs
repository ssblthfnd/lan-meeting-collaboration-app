//! The persistence port.
//!
//! `app-core` owns the rules and must not own SQL, but the rules can only be
//! enforced against current database state inside the mutating transaction
//! (architecture rules section 15). These traits are the seam: the domain
//! describes what it needs, `app-db` supplies it, and neither knows the other's
//! internals. `app-core` has no SQLite dependency as a result.
//!
//! # Reads and writes are not symmetric
//!
//! Read methods are free to call. Every **write** method takes an
//! [`Authorized`], which only [`crate::authz::authorize`] can produce. The
//! adapter implementing these methods cannot construct one, and neither can a
//! transport. That is what makes the mutation boundary hard to bypass by
//! accident rather than merely by convention.

use crate::audit::AuditEntry;
use crate::authz::Authorized;
use crate::error::DomainResult;
use crate::id::{MeetingId, NoteId, NoteVersionId, ParticipantId};
use crate::meeting::{Meeting, MeetingStatus};
use crate::time::UtcTimestamp;

/// A participant's current note, as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteRow {
    pub id: NoteId,
    pub content: String,
}

/// A note that does not exist yet.
///
/// Carries no meeting id on purpose: the adapter takes it from the
/// [`Authorized`] passed alongside, so a note cannot be written into a meeting
/// the actor was not authorized for even if the payload said otherwise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewNote {
    pub id: NoteId,
    pub participant_id: ParticipantId,
    pub content: String,
    pub at: UtcTimestamp,
}

/// One row of note history.
///
/// `version` is computed by the domain from current state inside the
/// transaction; a caller never chooses it (ADR-0003, architecture rules 18.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewNoteVersion {
    pub id: NoteVersionId,
    pub note_id: NoteId,
    pub version: i64,
    pub content: String,
    pub at: UtcTimestamp,
}

/// Everything the domain can do to the database within one transaction.
///
/// Implementations contain persistence mechanics only. They must not decide
/// authorization, re-check the meeting lock, or interpret business rules: those
/// decisions have already been made by the time an [`Authorized`] exists.
pub trait DomainTx {
    /// Load a meeting's current lifecycle state.
    ///
    /// Called inside the mutating transaction, every time. The returned
    /// [`Meeting`] is the only state a mutation may be judged against.
    fn find_meeting(&self, meeting_id: MeetingId) -> DomainResult<Option<Meeting>>;

    /// Whether this participant belongs to this meeting.
    fn participant_exists(
        &self,
        meeting_id: MeetingId,
        participant_id: ParticipantId,
    ) -> DomainResult<bool>;

    /// The participant's single note, if they have written one (ADR-0003).
    fn find_note(
        &self,
        meeting_id: MeetingId,
        participant_id: ParticipantId,
    ) -> DomainResult<Option<NoteRow>>;

    /// The highest version recorded for a note, or 0 when it has no history.
    ///
    /// Read inside the transaction so the next version is derived from current
    /// state. The database additionally holds `UNIQUE(note_id, version)`, so a
    /// lost race cannot produce two rows with the same number.
    fn latest_note_version(&self, note_id: NoteId) -> DomainResult<i64>;

    /// Set a meeting's lifecycle status, and its `locked_at` when locking.
    fn set_meeting_status(
        &self,
        proof: &Authorized,
        status: MeetingStatus,
        at: UtcTimestamp,
    ) -> DomainResult<()>;

    /// Insert a participant's first note.
    fn insert_note(&self, proof: &Authorized, note: &NewNote) -> DomainResult<()>;

    /// Replace an existing note's content.
    fn update_note_content(
        &self,
        proof: &Authorized,
        note_id: NoteId,
        content: &str,
        at: UtcTimestamp,
    ) -> DomainResult<()>;

    /// Append a row to a note's history.
    fn insert_note_version(&self, proof: &Authorized, version: &NewNoteVersion)
        -> DomainResult<()>;

    /// Append an audit record, attributed to the actor in `proof`.
    fn insert_audit(&self, proof: &Authorized, entry: &AuditEntry) -> DomainResult<()>;
}

/// A database the domain can open a write transaction against.
///
/// The transaction is `BEGIN IMMEDIATE`: it takes the write lock up front, so
/// the state a mutation reads cannot be changed by another writer before it
/// commits. `f` returning `Err` rolls the whole transaction back, which is what
/// makes "a failed mutation leaves no audit record" true by construction rather
/// than by discipline.
pub trait Database {
    /// Run `f` inside exactly one write transaction.
    ///
    /// Takes `&mut dyn FnMut` rather than a generic closure so the trait stays
    /// object-safe; the domain services wrap this in a typed API.
    fn transaction(&self, f: &mut dyn FnMut(&dyn DomainTx) -> DomainResult<()>)
        -> DomainResult<()>;
}

/// So a shared handle can back the mutation boundary.
///
/// A Host application keeps one database for the lifetime of the process and
/// shares it with the Tauri command layer and the LAN server; both hold the
/// same [`crate::service::Domain`] through an `Arc`.
impl<T: Database + ?Sized> Database for std::sync::Arc<T> {
    fn transaction(
        &self,
        f: &mut dyn FnMut(&dyn DomainTx) -> DomainResult<()>,
    ) -> DomainResult<()> {
        (**self).transaction(f)
    }
}
