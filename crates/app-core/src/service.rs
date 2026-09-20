//! The domain mutation boundary.
//!
//! Every state-changing operation in the application goes through [`Domain`].
//! There is no second path: the write methods on [`crate::port::DomainTx`]
//! require an [`crate::authz::Authorized`], which only this crate can mint, so
//! a transport that tried to write directly would not compile.
//!
//! # The shape of every mutation
//!
//! ```text
//! transport establishes the Actor
//!   -> Domain::<operation>
//!        -> BEGIN IMMEDIATE
//!             -> load current meeting state from the database
//!             -> authorize the actor for the operation
//!             -> check the meeting lock against that loaded state
//!             -> validate the input
//!             -> write the entity
//!             -> append note history
//!             -> append the audit record
//!           COMMIT
//!   -> transport may broadcast, after the commit
//! ```
//!
//! The order matters. The meeting is re-read from the database inside the same
//! transaction that performs the write, so a lock committed a microsecond
//! earlier is seen. Nothing here consults a `Meeting` the caller passed in, a
//! flag, or an earlier check: architecture rules section 15 calls a pre-flight
//! check a race, not an enforcement, and this is the one place where that is
//! avoided for all transports at once.
//!
//! Broadcasting is deliberately outside: WebSocket events are emitted after the
//! transaction commits, never before (architecture rules section 16).

use serde_json::json;

use crate::actor::Actor;
use crate::audit::{AuditAction, AuditEntry, AuditTarget};
use crate::authz::{authorize, Operation};
use crate::error::{DomainError, DomainResult};
use crate::id::{MeetingId, NoteId, NoteVersionId, ParticipantId};
use crate::meeting::MeetingStatus;
use crate::port::{Database, DomainTx, NewNote, NewNoteVersion};
use crate::time::UtcTimestamp;

/// Write (create or replace) one participant's note.
///
/// `participant_id` names **whose note to write**. It is a target, never a
/// claim about who is asking: authority comes from the [`Actor`] alone
/// (architecture rules section 14.1 rule 5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteNote {
    pub meeting_id: MeetingId,
    pub participant_id: ParticipantId,
    pub content: String,
}

/// Outcome of a lifecycle transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeetingTransitioned {
    pub meeting_id: MeetingId,
    pub from: MeetingStatus,
    pub to: MeetingStatus,
    pub at: UtcTimestamp,
}

/// Outcome of a note write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoteWritten {
    pub note_id: NoteId,
    /// The history row this write produced. Starts at 1 and increases by one.
    pub version: i64,
    /// True when this write created the note rather than replacing its content.
    pub created: bool,
    pub at: UtcTimestamp,
}

/// The application's only mutation entry point.
pub struct Domain<D: Database> {
    db: D,
}

impl<D: Database> Domain<D> {
    /// Wrap a database in the mutation boundary.
    pub fn new(db: D) -> Self {
        Self { db }
    }

    /// Move a meeting from `DRAFT` to `OPEN`. Host only.
    pub fn open_meeting(
        &self,
        actor: &Actor,
        meeting_id: MeetingId,
    ) -> DomainResult<MeetingTransitioned> {
        self.transition(
            actor,
            meeting_id,
            MeetingStatus::Open,
            Operation::OpenMeeting,
            AuditAction::MeetingOpened,
        )
    }

    /// Move a meeting from `OPEN` to `LOCKED`. Host only.
    ///
    /// After this commits, every mutation in this module refuses, because each
    /// re-reads the status inside its own transaction.
    pub fn lock_meeting(
        &self,
        actor: &Actor,
        meeting_id: MeetingId,
    ) -> DomainResult<MeetingTransitioned> {
        self.transition(
            actor,
            meeting_id,
            MeetingStatus::Locked,
            Operation::LockMeeting,
            AuditAction::MeetingLocked,
        )
    }

    fn transition(
        &self,
        actor: &Actor,
        meeting_id: MeetingId,
        to: MeetingStatus,
        operation: Operation,
        action: AuditAction,
    ) -> DomainResult<MeetingTransitioned> {
        let mut outcome = None;

        self.db.transaction(&mut |tx: &dyn DomainTx| {
            // 1. Current state, from the database, inside this transaction.
            let meeting = tx
                .find_meeting(meeting_id)?
                .ok_or(DomainError::MeetingNotFound { meeting_id })?;

            // 2. Authorization.
            let proof = authorize(actor, meeting_id, operation)?;

            // 3. The lock, checked against the state just loaded.
            meeting.ensure_mutable()?;

            // 4. Validation: is this transition one the lifecycle allows?
            meeting.ensure_transition(to)?;

            // 5. Mutation and audit, in the same transaction.
            let at = UtcTimestamp::now();
            tx.set_meeting_status(&proof, to, at)?;
            tx.insert_audit(
                &proof,
                &AuditEntry {
                    action,
                    target: AuditTarget::Meeting(meeting_id),
                    metadata: json!({
                        "from": meeting.status.as_str(),
                        "to": to.as_str(),
                    }),
                    at,
                },
            )?;

            outcome = Some(MeetingTransitioned {
                meeting_id,
                from: meeting.status,
                to,
                at,
            });
            Ok(())
        })?;

        committed(outcome, "completing a meeting transition")
    }

    /// Create or replace a participant's note, recording a new version.
    ///
    /// Exactly one note exists per participant per meeting (ADR-0003), so this
    /// is an upsert by construction: it never produces a second note. Every
    /// successful call appends one `note_versions` row, whose number is derived
    /// from current state inside this transaction.
    pub fn write_note(&self, actor: &Actor, command: WriteNote) -> DomainResult<NoteWritten> {
        let WriteNote {
            meeting_id,
            participant_id,
            content,
        } = command;

        let mut outcome = None;

        self.db.transaction(&mut |tx: &dyn DomainTx| {
            // 1. Current state.
            let meeting = tx
                .find_meeting(meeting_id)?
                .ok_or(DomainError::MeetingNotFound { meeting_id })?;

            // 2. Authorization. A participant may write only their own note;
            //    the Host may write anyone's; an import is confined to its own.
            let proof = authorize(actor, meeting_id, Operation::WriteNote { participant_id })?;

            // 3. The lock.
            meeting.ensure_mutable()?;

            // 4. Validation.
            validate_note_content(&content)?;
            if !tx.participant_exists(meeting_id, participant_id)? {
                return Err(DomainError::ParticipantNotFound {
                    meeting_id,
                    participant_id,
                });
            }

            // 5. Upsert the single note.
            let at = UtcTimestamp::now();
            let (note_id, created) = match tx.find_note(meeting_id, participant_id)? {
                Some(existing) => {
                    tx.update_note_content(&proof, existing.id, &content, at)?;
                    (existing.id, false)
                }
                None => {
                    let note_id = NoteId::new();
                    tx.insert_note(
                        &proof,
                        &NewNote {
                            id: note_id,
                            participant_id,
                            content: content.clone(),
                            at,
                        },
                    )?;
                    (note_id, true)
                }
            };

            // 6. History. The version comes from the database, never from the
            //    caller. `UNIQUE(note_id, version)` makes a lost race a
            //    conflict rather than a duplicate.
            let version = tx.latest_note_version(note_id)? + 1;
            tx.insert_note_version(
                &proof,
                &NewNoteVersion {
                    id: NoteVersionId::new(),
                    note_id,
                    version,
                    content: content.clone(),
                    at,
                },
            )?;

            // 7. Audit, same transaction.
            tx.insert_audit(
                &proof,
                &AuditEntry {
                    action: if created {
                        AuditAction::NoteCreated
                    } else {
                        AuditAction::NoteUpdated
                    },
                    target: AuditTarget::Note(note_id),
                    metadata: json!({
                        "participant_id": participant_id.to_storage(),
                        "version": version,
                    }),
                    at,
                },
            )?;

            outcome = Some(NoteWritten {
                note_id,
                version,
                created,
                at,
            });
            Ok(())
        })?;

        committed(outcome, "writing a note")
    }
}

/// Unwrap the value a committed transaction produced.
///
/// `transaction` returns `Ok` only when the closure returned `Ok`, and every
/// closure above sets `outcome` on that path. A `None` here would mean the
/// adapter committed without running the closure, which is a broken
/// implementation rather than a domain refusal - so it is reported as a
/// persistence failure rather than by panicking.
fn committed<T>(outcome: Option<T>, operation: &'static str) -> DomainResult<T> {
    outcome.ok_or(DomainError::Persistence {
        operation,
        detail: "the transaction committed without producing an outcome".to_owned(),
    })
}

/// Note content rules.
///
/// Only what this step can justify: a write must carry content. Clearing a note
/// is a separate operation the PRD does not define, and the Markdown subset
/// itself is validated by the shared editor and renderer (ADR-0007), not here.
fn validate_note_content(content: &str) -> DomainResult<()> {
    if content.trim().is_empty() {
        return Err(DomainError::Validation {
            field: "note content",
            expected: "non-empty content",
            detected: if content.is_empty() {
                "an empty string".to_owned()
            } else {
                "only whitespace".to_owned()
            },
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_or_whitespace_note_content_is_rejected() {
        for bad in ["", "   ", "\n\t "] {
            let err = validate_note_content(bad).unwrap_err();
            assert!(matches!(err, DomainError::Validation { .. }), "{err:?}");
        }
    }

    #[test]
    fn ordinary_note_content_passes() {
        assert!(validate_note_content("## Agenda\n\nBudget discussed.").is_ok());
    }

    #[test]
    fn a_transaction_that_produced_nothing_is_a_persistence_failure_not_a_panic() {
        let err = committed::<()>(None, "testing").unwrap_err();
        assert!(matches!(err, DomainError::Persistence { .. }));
        assert!(!err.is_refusal());
    }
}
