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
//!             -> check the lifecycle rule the operation needs
//!             -> validate the input
//!             -> write the entity
//!             -> append note history
//!             -> append the audit record
//!           COMMIT
//!   -> transport may broadcast, after the commit
//! ```
//!
//! [`Domain::create_meeting`] is the one operation with no state to load, since
//! the meeting does not exist until it writes. Every other step is the same.
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

use serde_json::{json, Value};

use crate::actor::Actor;
use crate::audit::{AuditAction, AuditEntry, AuditTarget};
use crate::authz::{authorize, Operation};
use crate::error::{DomainError, DomainResult};
use crate::id::{MeetingId, NoteId, NoteVersionId, ParticipantId};
use crate::meeting::{MeetingConfiguration, MeetingStatus};
use crate::participant::{ensure_room_for_one_more, ParticipantDetails};
use crate::port::{Database, DomainTx, NewMeeting, NewNote, NewNoteVersion, NewParticipant};
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

/// Outcome of creating a meeting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeetingCreated {
    pub meeting_id: MeetingId,
    /// Always [`MeetingStatus::Draft`]. Returned rather than assumed so a caller
    /// reads the state from the outcome instead of hard-coding it.
    pub status: MeetingStatus,
    pub at: UtcTimestamp,
}

/// Outcome of changing a meeting's configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeetingUpdated {
    pub meeting_id: MeetingId,
    pub at: UtcTimestamp,
}

/// Outcome of adding a participant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParticipantAdded {
    pub participant_id: ParticipantId,
    /// Roster size after this addition; never more than
    /// [`crate::participant::MAX_PARTICIPANTS`].
    pub roster_size: i64,
    pub at: UtcTimestamp,
}

/// Outcome of changing a participant's details.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParticipantUpdated {
    pub participant_id: ParticipantId,
    pub at: UtcTimestamp,
}

/// Outcome of removing a participant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParticipantRemoved {
    pub participant_id: ParticipantId,
    /// Roster size after the removal.
    pub roster_size: i64,
    pub at: UtcTimestamp,
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

    /// Create a meeting in `DRAFT`. Host only.
    ///
    /// The only mutation with no current state to load, because the meeting does
    /// not exist yet. The authorization proof is scoped to the id about to be
    /// created, so a participant - whose session binds to one existing meeting
    /// (ADR-0002) - has no standing for it at all.
    ///
    /// A new meeting always starts in `DRAFT`. There is no way to ask for any
    /// other status: `OPEN` is reached only through [`Domain::open_meeting`], so
    /// the transition is always audited (PRD section 6).
    pub fn create_meeting(
        &self,
        actor: &Actor,
        configuration: MeetingConfiguration,
    ) -> DomainResult<MeetingCreated> {
        let meeting_id = MeetingId::new();
        let mut outcome = None;

        self.db.transaction(&mut |tx: &dyn DomainTx| {
            // 1. Authorization.
            let proof = authorize(actor, meeting_id, Operation::CreateMeeting)?;

            // 2. Validation. There is no lock to check: a meeting that does not
            //    exist cannot be locked.
            let configuration = configuration.validated()?;

            // 3. Mutation and audit, in the same transaction.
            let at = UtcTimestamp::now();
            tx.insert_meeting(
                &proof,
                &NewMeeting {
                    configuration: configuration.clone(),
                    status: MeetingStatus::Draft,
                    at,
                },
            )?;
            tx.insert_audit(
                &proof,
                &AuditEntry {
                    action: AuditAction::MeetingCreated,
                    target: AuditTarget::Meeting(meeting_id),
                    metadata: configuration_metadata(&configuration),
                    at,
                },
            )?;

            outcome = Some(MeetingCreated {
                meeting_id,
                status: MeetingStatus::Draft,
                at,
            });
            Ok(())
        })?;

        committed(outcome, "creating a meeting")
    }

    /// Replace a `DRAFT` meeting's configuration. Host only.
    ///
    /// Configuration is settled when the meeting opens (ADR-0013), so this
    /// refuses an `OPEN` meeting with [`DomainError::MeetingNotDraft`] and a
    /// `LOCKED` one with [`DomainError::MeetingLocked`]. The status is read from
    /// the database inside this transaction, never from the caller.
    pub fn update_meeting(
        &self,
        actor: &Actor,
        meeting_id: MeetingId,
        configuration: MeetingConfiguration,
    ) -> DomainResult<MeetingUpdated> {
        let mut outcome = None;

        self.db.transaction(&mut |tx: &dyn DomainTx| {
            // 1. Current state.
            let meeting = tx
                .find_meeting(meeting_id)?
                .ok_or(DomainError::MeetingNotFound { meeting_id })?;

            // 2. Authorization.
            let proof = authorize(actor, meeting_id, Operation::UpdateMeeting)?;

            // 3. The lock, then the narrower rule that configuration is only
            //    mutable while the meeting is still being prepared.
            meeting.ensure_mutable()?;
            meeting.ensure_draft()?;

            // 4. Validation.
            let configuration = configuration.validated()?;

            // 5. Mutation and audit.
            let at = UtcTimestamp::now();
            tx.update_meeting_configuration(&proof, &configuration, at)?;
            tx.insert_audit(
                &proof,
                &AuditEntry {
                    action: AuditAction::MeetingUpdated,
                    target: AuditTarget::Meeting(meeting_id),
                    metadata: configuration_metadata(&configuration),
                    at,
                },
            )?;

            outcome = Some(MeetingUpdated { meeting_id, at });
            Ok(())
        })?;

        committed(outcome, "updating a meeting")
    }

    /// Add a participant to a `DRAFT` meeting's roster. Host only.
    ///
    /// The roster is counted inside this transaction, so concurrent additions
    /// cannot both see room for the 99th (PRD section 7). A trigger in the
    /// migration refuses a 100th row independently, which turns a bypass of this
    /// method into a [`DomainError::Conflict`] rather than a silent overflow.
    pub fn add_participant(
        &self,
        actor: &Actor,
        meeting_id: MeetingId,
        details: ParticipantDetails,
    ) -> DomainResult<ParticipantAdded> {
        let mut outcome = None;

        self.db.transaction(&mut |tx: &dyn DomainTx| {
            // 1. Current state.
            let meeting = tx
                .find_meeting(meeting_id)?
                .ok_or(DomainError::MeetingNotFound { meeting_id })?;

            // 2. Authorization.
            let proof = authorize(actor, meeting_id, Operation::AddParticipant)?;

            // 3. The lock, then the DRAFT-only roster rule.
            meeting.ensure_mutable()?;
            meeting.ensure_draft()?;

            // 4. Validation, including the limit against committed state.
            let details = details.validated()?;
            let roster = tx.count_participants(meeting_id)?;
            ensure_room_for_one_more(roster)?;

            // 5. Mutation and audit.
            let at = UtcTimestamp::now();
            let participant_id = ParticipantId::new();
            tx.insert_participant(
                &proof,
                &NewParticipant {
                    id: participant_id,
                    details: details.clone(),
                    at,
                },
            )?;

            let roster_size = roster + 1;
            let mut metadata = participant_metadata(&details);
            metadata["roster_size"] = json!(roster_size);
            tx.insert_audit(
                &proof,
                &AuditEntry {
                    action: AuditAction::ParticipantAdded,
                    target: AuditTarget::Participant(participant_id),
                    metadata,
                    at,
                },
            )?;

            outcome = Some(ParticipantAdded {
                participant_id,
                roster_size,
                at,
            });
            Ok(())
        })?;

        committed(outcome, "adding a participant")
    }

    /// Replace a participant's details. Host only, `DRAFT` only.
    pub fn update_participant(
        &self,
        actor: &Actor,
        meeting_id: MeetingId,
        participant_id: ParticipantId,
        details: ParticipantDetails,
    ) -> DomainResult<ParticipantUpdated> {
        let mut outcome = None;

        self.db.transaction(&mut |tx: &dyn DomainTx| {
            // 1. Current state.
            let meeting = tx
                .find_meeting(meeting_id)?
                .ok_or(DomainError::MeetingNotFound { meeting_id })?;

            // 2. Authorization.
            let proof = authorize(actor, meeting_id, Operation::UpdateParticipant)?;

            // 3. The lock, then the DRAFT-only roster rule.
            meeting.ensure_mutable()?;
            meeting.ensure_draft()?;

            // 4. Validation. The participant must belong to *this* meeting.
            let details = details.validated()?;
            let existing = tx.find_participant(meeting_id, participant_id)?.ok_or(
                DomainError::ParticipantNotFound {
                    meeting_id,
                    participant_id,
                },
            )?;

            // 5. Mutation and audit. History records both sides, because a
            //    changed name is only meaningful next to the previous one.
            let at = UtcTimestamp::now();
            tx.update_participant(&proof, participant_id, &details)?;
            tx.insert_audit(
                &proof,
                &AuditEntry {
                    action: AuditAction::ParticipantUpdated,
                    target: AuditTarget::Participant(participant_id),
                    metadata: json!({
                        "from": participant_metadata(&existing.details),
                        "to": participant_metadata(&details),
                    }),
                    at,
                },
            )?;

            outcome = Some(ParticipantUpdated { participant_id, at });
            Ok(())
        })?;

        committed(outcome, "updating a participant")
    }

    /// Remove a participant from a `DRAFT` meeting's roster. Host only.
    ///
    /// The audit record carries the removed participant's details, because the
    /// row itself is gone afterwards and `audit_logs.target_id` is deliberately
    /// not a foreign key (ADR-0011). Without them the trail would name an id
    /// nothing can resolve.
    pub fn remove_participant(
        &self,
        actor: &Actor,
        meeting_id: MeetingId,
        participant_id: ParticipantId,
    ) -> DomainResult<ParticipantRemoved> {
        let mut outcome = None;

        self.db.transaction(&mut |tx: &dyn DomainTx| {
            // 1. Current state.
            let meeting = tx
                .find_meeting(meeting_id)?
                .ok_or(DomainError::MeetingNotFound { meeting_id })?;

            // 2. Authorization.
            let proof = authorize(actor, meeting_id, Operation::RemoveParticipant)?;

            // 3. The lock, then the DRAFT-only roster rule.
            meeting.ensure_mutable()?;
            meeting.ensure_draft()?;

            // 4. Validation.
            let existing = tx.find_participant(meeting_id, participant_id)?.ok_or(
                DomainError::ParticipantNotFound {
                    meeting_id,
                    participant_id,
                },
            )?;

            // 5. Mutation and audit.
            let at = UtcTimestamp::now();
            tx.delete_participant(&proof, participant_id)?;

            let roster_size = tx.count_participants(meeting_id)?;
            let mut metadata = participant_metadata(&existing.details);
            metadata["roster_size"] = json!(roster_size);
            tx.insert_audit(
                &proof,
                &AuditEntry {
                    action: AuditAction::ParticipantRemoved,
                    target: AuditTarget::Participant(participant_id),
                    metadata,
                    at,
                },
            )?;

            outcome = Some(ParticipantRemoved {
                participant_id,
                roster_size,
                at,
            });
            Ok(())
        })?;

        committed(outcome, "removing a participant")
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
            if tx.find_participant(meeting_id, participant_id)?.is_none() {
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

/// Audit context for a meeting's configuration.
///
/// Records the values the meeting now has, which is what the record is about.
/// It deliberately does not claim to be a diff: [`crate::meeting::Meeting`] is
/// the thin lifecycle projection the lock rule needs (ADR-0012), so the previous
/// configuration is not loaded, and an audit record must not imply knowledge the
/// mutation did not have.
///
/// The timezone is included with the schedule, never separately from it: a date
/// and time without the zone they are read in is ambiguous (PRD section 25.4).
fn configuration_metadata(configuration: &MeetingConfiguration) -> Value {
    json!({
        "title": configuration.title,
        "topic": configuration.topic,
        "date": configuration.date.to_storage(),
        "start_time": configuration.start_time.to_storage(),
        "end_time": configuration.end_time.to_storage(),
        "timezone": configuration.timezone.name(),
        "location": configuration.location,
        "description": configuration.description,
    })
}

/// Audit context for one participant.
fn participant_metadata(details: &ParticipantDetails) -> Value {
    json!({
        "name": details.name,
        "department": details.department,
        "position": details.position,
        "meeting_role": details.meeting_role,
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
