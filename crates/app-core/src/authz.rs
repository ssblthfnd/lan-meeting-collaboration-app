//! Authorization: who may do what.
//!
//! Every rule here is decided in Rust, never in a UI (architecture rules
//! section 14). The actor is established at the transport boundary and handed
//! in; nothing in this module reads an identity out of a payload. A
//! `participant_id` naming *whose note to write* is a target, never a claim
//! about who is asking.
//!
//! # The proof
//!
//! [`authorize`] is the only way to obtain an [`Authorized`], and an
//! `Authorized` is the only way to reach a write method on
//! [`crate::port::DomainTx`]. Its fields are private to this crate, so the
//! persistence adapter that *implements* those methods still cannot mint one.
//! Bypassing authorization therefore is not a matter of remembering to call the
//! right function: there is no way to spell it.

use crate::actor::Actor;
use crate::error::{DomainError, DomainResult};
use crate::id::{MeetingId, ParticipantId};

/// A state-changing operation the domain layer offers.
///
/// This enum lists only what is actually implemented. Operations are added
/// alongside their implementation, not in advance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    /// Create a new meeting, in `DRAFT`.
    CreateMeeting,
    /// Change a `DRAFT` meeting's configuration.
    UpdateMeeting,
    /// Move a meeting from `DRAFT` to `OPEN`.
    OpenMeeting,
    /// Move a meeting from `OPEN` to `LOCKED`.
    LockMeeting,
    /// Add a participant to a `DRAFT` meeting's roster.
    AddParticipant,
    /// Change a participant's details.
    UpdateParticipant,
    /// Remove a participant from the roster.
    RemoveParticipant,
    /// Create or replace the note belonging to `participant_id`.
    WriteNote { participant_id: ParticipantId },
}

impl Operation {
    /// Short verb used in refusal messages and audit records.
    #[must_use]
    pub fn action(&self) -> &'static str {
        match self {
            Operation::CreateMeeting => "create a meeting",
            Operation::UpdateMeeting => "change meeting configuration",
            Operation::OpenMeeting => "open a meeting",
            Operation::LockMeeting => "lock a meeting",
            Operation::AddParticipant => "add a participant",
            Operation::UpdateParticipant => "change a participant",
            Operation::RemoveParticipant => "remove a participant",
            Operation::WriteNote { .. } => "write a note",
        }
    }

    fn target(&self) -> &'static str {
        match self {
            Operation::CreateMeeting
            | Operation::UpdateMeeting
            | Operation::OpenMeeting
            | Operation::LockMeeting => "the meeting",
            Operation::AddParticipant
            | Operation::UpdateParticipant
            | Operation::RemoveParticipant => "the participant roster",
            Operation::WriteNote { .. } => "another participant's note",
        }
    }
}

/// Proof that [`authorize`] approved a specific operation.
///
/// Carries the facts the persistence layer needs and nothing the caller could
/// have chosen for themselves: the actor discriminator and acting participant
/// come from the [`Actor`], not from a payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Authorized {
    actor_type: &'static str,
    acting_participant: Option<ParticipantId>,
    meeting_id: MeetingId,
    operation: Operation,
}

impl Authorized {
    /// The meeting this approval is scoped to.
    #[must_use]
    pub fn meeting_id(&self) -> MeetingId {
        self.meeting_id
    }

    /// `HOST`, `PARTICIPANT` or `REMOTE_IMPORT`.
    ///
    /// Written verbatim to `audit_logs.actor_type` and
    /// `note_versions.created_by_type`, whose `CHECK` constraints accept
    /// exactly these three values.
    #[must_use]
    pub fn actor_type(&self) -> &'static str {
        self.actor_type
    }

    /// The acting participant, or `None` for the Host.
    ///
    /// This is what `audit_logs.actor_id` and `note_versions.created_by` store.
    /// The schema requires `created_by IS NULL` exactly when
    /// `created_by_type = 'HOST'` (ADR-0008), and this method is the single
    /// source of that value.
    #[must_use]
    pub fn created_by(&self) -> Option<ParticipantId> {
        self.acting_participant
    }

    /// The approved operation.
    #[must_use]
    pub fn operation(&self) -> Operation {
        self.operation
    }
}

/// Decide whether `actor` may perform `operation` in `meeting_id`.
///
/// The two refusals are deliberately different facts:
///
/// - [`DomainError::Unauthorized`] - the actor is not a party to this meeting
///   at all. A participant session is bound to one meeting (ADR-0002), so
///   reaching for another one is not a permissions question.
/// - [`DomainError::Forbidden`] - the actor belongs here, but not to this
///   operation.
pub fn authorize(
    actor: &Actor,
    meeting_id: MeetingId,
    operation: Operation,
) -> DomainResult<Authorized> {
    let approve = |actor_type, acting_participant| {
        Ok(Authorized {
            actor_type,
            acting_participant,
            meeting_id,
            operation,
        })
    };

    let refuse = |actor_type: &'static str| {
        Err(DomainError::Forbidden {
            actor_type,
            action: operation.action(),
            target: operation.target(),
        })
    };

    // Everything except writing a note belongs to the Host alone: the meeting
    // itself, its lifecycle and its roster (PRD section 5.1, and section 5.2's
    // list of what a participant cannot do).
    let host_only = |actor_type: &'static str| match operation {
        Operation::WriteNote { .. } => None,
        _ => Some(refuse(actor_type)),
    };

    match actor {
        // The Host owns the meeting and may edit any participant's note
        // (PRD section 5.1, section 15).
        Actor::Host => approve("HOST", None),

        Actor::Participant {
            meeting_id: actor_meeting,
            participant_id,
            ..
        } => {
            if *actor_meeting != meeting_id {
                return Err(DomainError::Unauthorized { meeting_id });
            }
            // Meeting configuration, lifecycle and roster belong to the Host.
            if let Some(refusal) = host_only("PARTICIPANT") {
                return refusal;
            }
            match operation {
                // A participant may write their own note and no other
                // (PRD section 5.2, architecture rules section 14).
                Operation::WriteNote {
                    participant_id: target,
                } => {
                    if target == *participant_id {
                        approve("PARTICIPANT", Some(*participant_id))
                    } else {
                        refuse("PARTICIPANT")
                    }
                }
                // Unreachable: `host_only` has already refused every other
                // operation. It is a refusal rather than a panic so that the
                // default for a new operation is "no", and `Operation::action`
                // and `Operation::target` are the exhaustive matches that force
                // a new variant to be considered here at all.
                _ => refuse("PARTICIPANT"),
            }
        }

        Actor::RemoteImport {
            meeting_id: actor_meeting,
            participant_id,
        } => {
            if *actor_meeting != meeting_id {
                return Err(DomainError::Unauthorized { meeting_id });
            }
            if let Some(refusal) = host_only("REMOTE_IMPORT") {
                return refusal;
            }
            match operation {
                // An import is confined to the participant its validated
                // context names. It carries no wider authority than the
                // participant whose submission it is (ADR-0008).
                Operation::WriteNote {
                    participant_id: target,
                } => {
                    if target == *participant_id {
                        approve("REMOTE_IMPORT", Some(*participant_id))
                    } else {
                        refuse("REMOTE_IMPORT")
                    }
                }
                // Unreachable, as above.
                _ => refuse("REMOTE_IMPORT"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::SessionId;

    fn participant_actor(meeting_id: MeetingId, participant_id: ParticipantId) -> Actor {
        Actor::Participant {
            meeting_id,
            participant_id,
            session_id: SessionId::new(),
        }
    }

    /// Every operation except writing a note, which is the only one a
    /// participant can ever be approved for.
    const HOST_ONLY: [Operation; 6] = [
        Operation::CreateMeeting,
        Operation::UpdateMeeting,
        Operation::OpenMeeting,
        Operation::LockMeeting,
        Operation::AddParticipant,
        Operation::UpdateParticipant,
    ];

    #[test]
    fn the_host_may_perform_every_operation() {
        let meeting_id = MeetingId::new();
        let someone = ParticipantId::new();

        for operation in HOST_ONLY.into_iter().chain([
            Operation::RemoveParticipant,
            Operation::WriteNote {
                participant_id: someone,
            },
        ]) {
            let approval = authorize(&Actor::Host, meeting_id, operation).unwrap();
            assert_eq!(approval.actor_type(), "HOST");
            // The Host is not a participant, so history records no author id.
            assert_eq!(approval.created_by(), None);
        }
    }

    #[test]
    fn a_participant_may_write_only_their_own_note() {
        let meeting_id = MeetingId::new();
        let me = ParticipantId::new();
        let actor = participant_actor(meeting_id, me);

        let approval = authorize(
            &actor,
            meeting_id,
            Operation::WriteNote { participant_id: me },
        )
        .unwrap();
        assert_eq!(approval.actor_type(), "PARTICIPANT");
        assert_eq!(approval.created_by(), Some(me));

        let someone_else = ParticipantId::new();
        let err = authorize(
            &actor,
            meeting_id,
            Operation::WriteNote {
                participant_id: someone_else,
            },
        )
        .unwrap_err();
        assert!(matches!(err, DomainError::Forbidden { .. }), "{err:?}");
    }

    #[test]
    fn a_participant_may_not_change_the_meeting_lifecycle() {
        let meeting_id = MeetingId::new();
        let actor = participant_actor(meeting_id, ParticipantId::new());

        for operation in [Operation::OpenMeeting, Operation::LockMeeting] {
            let err = authorize(&actor, meeting_id, operation).unwrap_err();
            assert_eq!(
                err,
                DomainError::Forbidden {
                    actor_type: "PARTICIPANT",
                    action: operation.action(),
                    target: "the meeting",
                }
            );
        }
    }

    #[test]
    fn a_participant_may_not_change_configuration_or_the_roster() {
        let meeting_id = MeetingId::new();
        let actor = participant_actor(meeting_id, ParticipantId::new());

        for operation in HOST_ONLY.into_iter().chain([Operation::RemoveParticipant]) {
            let err = authorize(&actor, meeting_id, operation).unwrap_err();
            assert_eq!(
                err,
                DomainError::Forbidden {
                    actor_type: "PARTICIPANT",
                    action: operation.action(),
                    target: operation.target(),
                },
                "{operation:?} must be refused"
            );
        }
    }

    #[test]
    fn a_remote_import_may_not_change_configuration_or_the_roster() {
        // An import carries exactly the authority of the participant whose
        // submission it is, which is none over the meeting or its roster.
        let meeting_id = MeetingId::new();
        let actor = Actor::RemoteImport {
            meeting_id,
            participant_id: ParticipantId::new(),
        };

        for operation in HOST_ONLY.into_iter().chain([Operation::RemoveParticipant]) {
            let err = authorize(&actor, meeting_id, operation).unwrap_err();
            assert!(
                matches!(err, DomainError::Forbidden { .. }),
                "{operation:?}: {err:?}"
            );
        }
    }

    #[test]
    fn creating_a_meeting_is_not_something_a_participant_has_standing_for() {
        // A create is scoped to an id that does not exist yet, so a participant
        // session - which is bound to one existing meeting (ADR-0002) - is not
        // a party to it at all.
        let actor = participant_actor(MeetingId::new(), ParticipantId::new());
        let err = authorize(&actor, MeetingId::new(), Operation::CreateMeeting).unwrap_err();
        assert!(matches!(err, DomainError::Unauthorized { .. }), "{err:?}");
    }

    #[test]
    fn a_participant_reaching_into_another_meeting_is_not_a_party_to_it() {
        // A session binds to one meeting (ADR-0002), so this is not a
        // permissions question - the actor simply has no standing there.
        let own_meeting = MeetingId::new();
        let other_meeting = MeetingId::new();
        let me = ParticipantId::new();
        let actor = participant_actor(own_meeting, me);

        let err = authorize(
            &actor,
            other_meeting,
            Operation::WriteNote { participant_id: me },
        )
        .unwrap_err();

        assert_eq!(
            err,
            DomainError::Unauthorized {
                meeting_id: other_meeting
            }
        );
    }

    #[test]
    fn a_remote_import_is_confined_to_its_own_participant_and_meeting() {
        let meeting_id = MeetingId::new();
        let subject = ParticipantId::new();
        let actor = Actor::RemoteImport {
            meeting_id,
            participant_id: subject,
        };

        let approval = authorize(
            &actor,
            meeting_id,
            Operation::WriteNote {
                participant_id: subject,
            },
        )
        .unwrap();
        assert_eq!(approval.actor_type(), "REMOTE_IMPORT");
        assert_eq!(approval.created_by(), Some(subject));

        // Not a licence to write anyone else's note...
        assert!(authorize(
            &actor,
            meeting_id,
            Operation::WriteNote {
                participant_id: ParticipantId::new(),
            },
        )
        .is_err());

        // ...nor to touch the meeting itself.
        assert!(authorize(&actor, meeting_id, Operation::LockMeeting).is_err());

        // ...nor to act in another meeting.
        assert!(matches!(
            authorize(
                &actor,
                MeetingId::new(),
                Operation::WriteNote {
                    participant_id: subject
                },
            ),
            Err(DomainError::Unauthorized { .. })
        ));
    }

    #[test]
    fn approval_records_the_actor_not_the_target() {
        // The Host writing a participant's note is recorded as a HOST edit with
        // no author id, which is what the schema's
        // `created_by_type = 'HOST' XOR created_by IS NOT NULL` check requires.
        let meeting_id = MeetingId::new();
        let target = ParticipantId::new();

        let approval = authorize(
            &Actor::Host,
            meeting_id,
            Operation::WriteNote {
                participant_id: target,
            },
        )
        .unwrap();

        assert_eq!(approval.actor_type(), "HOST");
        assert_ne!(approval.created_by(), Some(target));
        assert_eq!(approval.created_by(), None);
    }
}
