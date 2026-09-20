//! Audit policy.
//!
//! Architecture rules section 17: every important mutation is recorded with an
//! actor, an action, a target, metadata and a timestamp, and the log is
//! append-only. The database enforces the append-only part with triggers
//! (ADR-0011); this module decides *what* gets written.
//!
//! Actions are a typed enum rather than free strings, so a typo produces a
//! compile error instead of an audit trail that is silently unqueryable. Only
//! the actions Step 2 actually performs exist here: an action is added with the
//! operation that emits it, never in advance.
//!
//! The actor is never supplied by a caller. Every audit record takes its
//! `actor_type` and `actor_id` from an [`crate::authz::Authorized`], which is
//! derived from the [`crate::actor::Actor`] the transport established.

use core::fmt;

use serde_json::Value;

use crate::id::{MeetingId, NoteId, ParticipantId};
use crate::time::UtcTimestamp;

/// What happened.
///
/// The string forms are stable: they are written to `audit_logs.action` and
/// will be read back by the export and history features.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuditAction {
    /// A meeting was created, in `DRAFT`.
    MeetingCreated,
    /// A `DRAFT` meeting's configuration was changed.
    MeetingUpdated,
    /// A meeting moved from `DRAFT` to `OPEN`.
    MeetingOpened,
    /// A meeting moved from `OPEN` to `LOCKED`.
    MeetingLocked,
    /// A participant was added to the roster.
    ParticipantAdded,
    /// A participant's details were changed.
    ParticipantUpdated,
    /// A participant was removed from the roster.
    ParticipantRemoved,
    /// A participant's note was created for the first time.
    NoteCreated,
    /// An existing note's content was replaced.
    NoteUpdated,
}

impl AuditAction {
    /// The persisted action string.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            AuditAction::MeetingCreated => "meeting.created",
            AuditAction::MeetingUpdated => "meeting.updated",
            AuditAction::MeetingOpened => "meeting.opened",
            AuditAction::MeetingLocked => "meeting.locked",
            AuditAction::ParticipantAdded => "participant.added",
            AuditAction::ParticipantUpdated => "participant.updated",
            AuditAction::ParticipantRemoved => "participant.removed",
            AuditAction::NoteCreated => "note.created",
            AuditAction::NoteUpdated => "note.updated",
        }
    }
}

impl fmt::Display for AuditAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What the action happened to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditTarget {
    Meeting(MeetingId),
    Participant(ParticipantId),
    Note(NoteId),
}

impl AuditTarget {
    /// The persisted `target_type` discriminator.
    #[must_use]
    pub fn target_type(&self) -> &'static str {
        match self {
            AuditTarget::Meeting(_) => "meeting",
            AuditTarget::Participant(_) => "participant",
            AuditTarget::Note(_) => "note",
        }
    }

    /// The persisted `target_id`, in canonical id form.
    ///
    /// `audit_logs.target_id` is deliberately not a foreign key, so a record
    /// describing a removed participant keeps naming them after the row is gone
    /// (ADR-0011).
    #[must_use]
    pub fn target_id(&self) -> String {
        match self {
            AuditTarget::Meeting(id) => id.to_storage(),
            AuditTarget::Participant(id) => id.to_storage(),
            AuditTarget::Note(id) => id.to_storage(),
        }
    }
}

/// One audit record, ready to be written.
///
/// Deliberately has no actor field: the actor is taken from the
/// [`crate::authz::Authorized`] passed alongside it, so a caller cannot
/// attribute an action to someone else.
#[derive(Debug, Clone, PartialEq)]
pub struct AuditEntry {
    pub action: AuditAction,
    pub target: AuditTarget,
    /// Context for the record. Must be a JSON object; the column has a
    /// `json_valid` check.
    pub metadata: Value,
    pub at: UtcTimestamp,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_strings_are_namespaced_and_stable() {
        assert_eq!(AuditAction::MeetingCreated.as_str(), "meeting.created");
        assert_eq!(AuditAction::MeetingUpdated.as_str(), "meeting.updated");
        assert_eq!(AuditAction::MeetingOpened.as_str(), "meeting.opened");
        assert_eq!(AuditAction::MeetingLocked.as_str(), "meeting.locked");
        assert_eq!(AuditAction::ParticipantAdded.as_str(), "participant.added");
        assert_eq!(
            AuditAction::ParticipantUpdated.as_str(),
            "participant.updated"
        );
        assert_eq!(
            AuditAction::ParticipantRemoved.as_str(),
            "participant.removed"
        );
        assert_eq!(AuditAction::NoteCreated.as_str(), "note.created");
        assert_eq!(AuditAction::NoteUpdated.as_str(), "note.updated");
    }

    #[test]
    fn targets_report_the_discriminator_and_canonical_id() {
        let meeting_id = MeetingId::new();
        let target = AuditTarget::Meeting(meeting_id);
        assert_eq!(target.target_type(), "meeting");
        assert_eq!(target.target_id(), meeting_id.to_storage());

        let participant_id = ParticipantId::new();
        let target = AuditTarget::Participant(participant_id);
        assert_eq!(target.target_type(), "participant");
        assert_eq!(target.target_id(), participant_id.to_storage());

        let note_id = NoteId::new();
        let target = AuditTarget::Note(note_id);
        assert_eq!(target.target_type(), "note");
        assert_eq!(target.target_id(), note_id.to_storage());
    }
}
