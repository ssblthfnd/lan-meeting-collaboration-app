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
//!   -> EventSink::publish, after the commit
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
//! Publication is deliberately outside the transaction. Every method below
//! calls [`Domain::publish`] only on the path where `transaction` returned
//! `Ok`, so an event announces a committed fact and nothing else - a rolled
//! back or refused mutation emits nothing (architecture rules section 16).
//!
//! The dependency runs one way only. [`crate::event::EventSink::publish`]
//! cannot fail, so no WebSocket, no window and no channel can affect a write
//! that SQLite has already accepted.

use serde_json::{json, Value};

use crate::actor::Actor;
use crate::audit::{AuditAction, AuditEntry, AuditTarget};
use crate::authz::{authorize, Authorized, Operation};
use crate::error::{DomainError, DomainResult};
use crate::event::{DomainEvent, EventSink, NoEvents};
use crate::id::{
    MeetingId, NoteId, NoteVersionId, ParticipantId, RemoteSubmissionId, SessionId, SubmissionId,
};
use crate::meeting::{MeetingConfiguration, MeetingStatus};
use crate::participant::{ensure_room_for_one_more, ParticipantDetails};
use crate::port::{
    Database, DomainTx, NewMeeting, NewNote, NewNoteVersion, NewParticipant, NewRemoteSubmission,
    NewSession, SubmissionResolution,
};
use crate::session::SessionBinding;
use crate::time::UtcTimestamp;
use crate::token::TokenHash;
use std::sync::Arc;

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

/// Import one remote participant's submission, as the Host confirmed it.
///
/// Every field is already **resolved and validated**: `meeting_id` and
/// `participant_id` are the ids of rows the Host's database returned, not the
/// candidates the file carried, and `content_hash` was computed from the parsed
/// submission rather than read out of it (ADR-0022 decisions 2 and 11).
///
/// `participant_id` names whose note to write. It is a target, never a claim
/// about who is asking: authority comes from the [`Actor`] alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportSubmission {
    pub meeting_id: MeetingId,
    pub participant_id: ParticipantId,
    /// The artefact this submission was exported from.
    pub submission_id: SubmissionId,
    /// SHA-256 over the canonical form, lowercase hex.
    pub content_hash: String,
    /// The note, already line-ending normalised by the canonical pass.
    pub content: String,
    /// The exact submitted bytes, for the ledger. Never re-serialised.
    pub raw_payload: String,
    /// Advisory context only. It never blocks and never becomes the version.
    pub source_version: i64,
}

/// Outcome of a remote submission import.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubmissionImported {
    pub note_id: NoteId,
    /// The history row this import produced, derived inside the transaction.
    pub version: i64,
    /// True when the import created the note rather than replacing its content.
    pub created: bool,
    /// What the ledger recorded. A description, not a mode anyone chose.
    pub resolution: SubmissionResolution,
    pub at: UtcTimestamp,
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

/// Outcome of issuing a join token.
///
/// Carries no token: the plaintext is the transport's, returned to the Host
/// once and never persisted (PRD 22.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JoinTokenIssued {
    pub meeting_id: MeetingId,
    /// True when this replaced a previous token, invalidating the old URL.
    pub replaced_previous: bool,
    pub at: UtcTimestamp,
}

/// Outcome of claiming an identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdentityClaimed {
    pub session_id: SessionId,
    pub meeting_id: MeetingId,
    pub participant_id: ParticipantId,
    pub at: UtcTimestamp,
}

/// Outcome of a Host action on someone's session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionChanged {
    pub session_id: SessionId,
    pub participant_id: ParticipantId,
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

/// Which of the three Phase 1 export formats was generated (Step 12).
///
/// Stable, snake_case wire form via `as_str`/`Display`, matching every other
/// small closed vocabulary this crate persists (compare [`MeetingStatus`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    Markdown,
    Txt,
    AiContext,
}

impl ExportFormat {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            ExportFormat::Markdown => "markdown",
            ExportFormat::Txt => "txt",
            ExportFormat::AiContext => "ai_context",
        }
    }
}

impl std::fmt::Display for ExportFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Outcome of recording that an export was generated.
///
/// Carries nothing about the rendered document itself - the audit row this
/// produces is a fact about *that an export happened*, not a copy of it
/// (Step 12 design freeze, E-2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportRecorded {
    pub meeting_id: MeetingId,
    pub format: ExportFormat,
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
    events: Arc<dyn EventSink>,
}

impl<D: Database> Domain<D> {
    /// Wrap a database in the mutation boundary, publishing nothing.
    ///
    /// What the domain and persistence tests use: those are about what SQLite
    /// holds afterwards, and a channel wired into them would be a second thing
    /// under test.
    pub fn new(db: D) -> Self {
        Self::with_events(db, Arc::new(NoEvents))
    }

    /// Wrap a database and publish every committed mutation to `events`.
    ///
    /// The application builds one of these, with a composite sink reaching both
    /// the LAN broadcast and the Host's own window (ADR-0018). One `Domain` for
    /// both transports is ADR-0012; one sink for both audiences is the same
    /// argument applied to notification.
    pub fn with_events(db: D, events: Arc<dyn EventSink>) -> Self {
        Self { db, events }
    }

    /// Announce a committed fact.
    ///
    /// Called only after `self.db.transaction(..)` returned `Ok`. It is
    /// deliberately infallible: a mutation that is already durable must not be
    /// able to fail afterwards because nobody was listening.
    fn publish(&self, event: DomainEvent) {
        self.events.publish(&event);
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

        let outcome = committed(outcome, "creating a meeting")?;
        self.publish(DomainEvent::MeetingCreated {
            meeting_id: outcome.meeting_id,
            at: outcome.at,
        });
        Ok(outcome)
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

        let outcome = committed(outcome, "updating a meeting")?;
        self.publish(DomainEvent::MeetingUpdated {
            meeting_id: outcome.meeting_id,
            at: outcome.at,
        });
        Ok(outcome)
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

        let outcome = committed(outcome, "adding a participant")?;
        self.publish(DomainEvent::ParticipantAdded {
            meeting_id,
            participant_id: outcome.participant_id,
            at: outcome.at,
        });
        Ok(outcome)
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

        let outcome = committed(outcome, "updating a participant")?;
        self.publish(DomainEvent::ParticipantUpdated {
            meeting_id,
            participant_id: outcome.participant_id,
            at: outcome.at,
        });
        Ok(outcome)
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

        let outcome = committed(outcome, "removing a participant")?;
        self.publish(DomainEvent::ParticipantRemoved {
            meeting_id,
            participant_id: outcome.participant_id,
            at: outcome.at,
        });
        Ok(outcome)
    }

    /// Mint a join token for an `OPEN` meeting. Host only.
    ///
    /// The token itself is generated and hashed by the transport; only the
    /// [`TokenHash`] arrives here, so this method cannot persist a secret even
    /// if a caller tried to hand it one (PRD 22.2).
    ///
    /// Issuing again **replaces** the stored hash, so the previous URL stops
    /// resolving to this meeting. That is the whole of token rotation: there is
    /// no revocation list because there is only ever one live token.
    ///
    /// Locking a meeting does **not** clear the hash. The join URL stops working
    /// because every request re-reads the meeting's status, which is the check
    /// that has to hold anyway - clearing the column would be a second mechanism
    /// that could disagree with the first (ADR-0016).
    pub fn issue_join_token(
        &self,
        actor: &Actor,
        meeting_id: MeetingId,
        token_hash: &TokenHash,
    ) -> DomainResult<JoinTokenIssued> {
        let mut outcome = None;

        self.db.transaction(&mut |tx: &dyn DomainTx| {
            // 1. Current state.
            let meeting = tx
                .find_meeting(meeting_id)?
                .ok_or(DomainError::MeetingNotFound { meeting_id })?;

            // 2. Authorization.
            let proof = authorize(actor, meeting_id, Operation::IssueJoinToken)?;

            // 3. The lock, then the rule that only an open meeting is joinable.
            meeting.ensure_mutable()?;
            meeting.ensure_open()?;

            // 4. Mutation and audit.
            let at = UtcTimestamp::now();
            let replaced_previous = meeting.has_join_token;
            tx.set_join_token_hash(&proof, token_hash, at)?;
            tx.insert_audit(
                &proof,
                &AuditEntry {
                    action: AuditAction::MeetingJoinTokenIssued,
                    target: AuditTarget::Meeting(meeting_id),
                    // Records that a token was issued and whether it displaced
                    // one. Never the token, and never its hash: an audit trail
                    // is read by people, and a credential does not belong in it.
                    metadata: json!({ "replaced_previous": replaced_previous }),
                    at,
                },
            )?;

            outcome = Some(JoinTokenIssued {
                meeting_id,
                replaced_previous,
                at,
            });
            Ok(())
        })?;

        let outcome = committed(outcome, "issuing a join token")?;
        // The Host is told the link changed. Neither the token nor its hash is
        // in the event: the plaintext went to the caller that asked for it, and
        // the hash is nobody's business but the database's (PRD 22.2).
        self.publish(DomainEvent::JoinTokenIssued {
            meeting_id: outcome.meeting_id,
            at: outcome.at,
        });
        Ok(outcome)
    }

    /// Bind a participant identity to a new session (ADR-0002).
    ///
    /// The actor is an [`Actor::Claimant`], established by the transport from a
    /// valid join token. `token_hash` is the hash of a session token the
    /// transport generated and will return to the browser exactly once.
    ///
    /// First-claim-wins is enforced twice, for the reason ADR-0011 gives for
    /// every other invariant: the live session is read inside this transaction,
    /// and the partial unique index refuses a second live row regardless. A
    /// constraint violation is mapped back to the same refusal, so a lost race
    /// and a plain second attempt look identical to the caller.
    ///
    /// A claim is usable immediately. `approved_at` stays null until the Host
    /// acknowledges it, and no authorization decision consults it (ADR-0016).
    pub fn claim_identity(
        &self,
        actor: &Actor,
        meeting_id: MeetingId,
        participant_id: ParticipantId,
        token_hash: &TokenHash,
    ) -> DomainResult<IdentityClaimed> {
        let mut outcome = None;

        self.db.transaction(&mut |tx: &dyn DomainTx| {
            // 1. Current state.
            let meeting = tx
                .find_meeting(meeting_id)?
                .ok_or(DomainError::MeetingNotFound { meeting_id })?;

            // 2. Authorization. A claimant may claim the one identity it names.
            let proof = authorize(
                actor,
                meeting_id,
                Operation::ClaimIdentity { participant_id },
            )?;

            // 3. The lock, then the rule that only an open meeting is joinable.
            meeting.ensure_mutable()?;
            meeting.ensure_open()?;

            // 4. Validation: the identity must exist in this meeting and must
            //    not already be held.
            if tx.find_participant(meeting_id, participant_id)?.is_none() {
                return Err(DomainError::ParticipantNotFound {
                    meeting_id,
                    participant_id,
                });
            }
            if tx.find_live_session(meeting_id, participant_id)?.is_some() {
                return Err(DomainError::IdentityAlreadyClaimed {
                    meeting_id,
                    participant_id,
                });
            }

            // 5. Mutation and audit.
            let at = UtcTimestamp::now();
            let session_id = SessionId::new();
            tx.insert_session(
                &proof,
                &NewSession {
                    id: session_id,
                    participant_id,
                    token_hash: token_hash.clone(),
                    at,
                },
            )
            .map_err(|error| claim_conflict(error, meeting_id, participant_id))?;

            tx.insert_audit(
                &proof,
                &AuditEntry {
                    action: AuditAction::ParticipantClaimed,
                    target: AuditTarget::Session(session_id),
                    metadata: json!({
                        "participant_id": participant_id.to_storage(),
                        "approved": false,
                    }),
                    at,
                },
            )?;

            outcome = Some(IdentityClaimed {
                session_id,
                meeting_id,
                participant_id,
                at,
            });
            Ok(())
        })?;

        let outcome = committed(outcome, "claiming an identity")?;
        // No session id in the event. The Host's roster wants to know somebody
        // joined; a session identifier is not part of that fact, and this event
        // does not target a socket the way revocation does.
        self.publish(DomainEvent::IdentityClaimed {
            meeting_id: outcome.meeting_id,
            participant_id: outcome.participant_id,
            at: outcome.at,
        });
        Ok(outcome)
    }

    /// Acknowledge a participant's claim. Host only.
    ///
    /// Sets `approved_at`. It grants nothing: the session could already act, and
    /// still can (ADR-0016). This exists so the Host can mark a roster as
    /// checked, not so they can let someone in.
    pub fn approve_claim(
        &self,
        actor: &Actor,
        meeting_id: MeetingId,
        participant_id: ParticipantId,
    ) -> DomainResult<SessionChanged> {
        let outcome = self.act_on_session(
            actor,
            meeting_id,
            participant_id,
            Operation::ApproveClaim { participant_id },
            AuditAction::ParticipantClaimApproved,
            &|tx, proof, session, at| tx.approve_session(proof, session.session_id, at),
        )?;
        self.publish(Self::session_event(&outcome, meeting_id, false));
        Ok(outcome)
    }

    /// End a participant's session. Host only.
    ///
    /// The identity becomes claimable again and the revoked row stays as
    /// history. This is what makes first-claim-wins operable: an identity taken
    /// by the wrong person, or stranded on a closed laptop, can be freed
    /// (ADR-0002 rule 5).
    pub fn revoke_session(
        &self,
        actor: &Actor,
        meeting_id: MeetingId,
        participant_id: ParticipantId,
    ) -> DomainResult<SessionChanged> {
        let outcome = self.act_on_session(
            actor,
            meeting_id,
            participant_id,
            Operation::RevokeSession { participant_id },
            AuditAction::ParticipantSessionRevoked,
            &|tx, proof, session, at| tx.revoke_session(proof, session.session_id, at),
        )?;
        // Published after the commit, naming the one session that ended. Only
        // the socket authenticated with that session id closes; the same
        // participant's other devices are untouched (ADR-0002 rule 5).
        self.publish(Self::session_event(&outcome, meeting_id, true));
        Ok(outcome)
    }

    /// The shared pipeline behind approving and revoking.
    ///
    /// Both load the live session inside the transaction, so neither can act on
    /// a session that was revoked a moment earlier.
    fn act_on_session(
        &self,
        actor: &Actor,
        meeting_id: MeetingId,
        participant_id: ParticipantId,
        operation: Operation,
        action: AuditAction,
        write: SessionWrite<'_>,
    ) -> DomainResult<SessionChanged> {
        let mut outcome = None;

        self.db.transaction(&mut |tx: &dyn DomainTx| {
            // 1. Current state.
            let meeting = tx
                .find_meeting(meeting_id)?
                .ok_or(DomainError::MeetingNotFound { meeting_id })?;

            // 2. Authorization.
            let proof = authorize(actor, meeting_id, operation)?;

            // 3. The lock. A locked meeting is finished, sessions included.
            meeting.ensure_mutable()?;

            // 4. Validation: there must be a live session to act on.
            let session = tx
                .find_live_session(meeting_id, participant_id)?
                .ok_or(DomainError::SessionNotFound { meeting_id })?;

            // 5. Mutation and audit.
            let at = UtcTimestamp::now();
            write(tx, &proof, session, at)?;
            tx.insert_audit(
                &proof,
                &AuditEntry {
                    action,
                    target: AuditTarget::Session(session.session_id),
                    metadata: json!({ "participant_id": participant_id.to_storage() }),
                    at,
                },
            )?;

            outcome = Some(SessionChanged {
                session_id: session.session_id,
                participant_id,
                at,
            });
            Ok(())
        })?;

        committed(outcome, "changing a session")
    }

    /// The event a session action produced.
    ///
    /// Approving and revoking share the pipeline above but not the event: one
    /// is display state belonging to a person, the other closes exactly one
    /// socket. Publishing from each caller rather than from the shared pipeline
    /// is what keeps that difference visible.
    fn session_event(
        outcome: &SessionChanged,
        meeting_id: MeetingId,
        revoked: bool,
    ) -> DomainEvent {
        if revoked {
            DomainEvent::SessionRevoked {
                meeting_id,
                participant_id: outcome.participant_id,
                // From the row the transaction acted on, never from a caller.
                session_id: outcome.session_id,
                at: outcome.at,
            }
        } else {
            DomainEvent::ClaimApproved {
                meeting_id,
                participant_id: outcome.participant_id,
                at: outcome.at,
            }
        }
    }

    /// Move a meeting from `DRAFT` to `OPEN`. Host only.
    pub fn open_meeting(
        &self,
        actor: &Actor,
        meeting_id: MeetingId,
    ) -> DomainResult<MeetingTransitioned> {
        let outcome = self.transition(
            actor,
            meeting_id,
            MeetingStatus::Open,
            Operation::OpenMeeting,
            AuditAction::MeetingOpened,
        )?;
        self.publish(DomainEvent::MeetingOpened {
            meeting_id,
            at: outcome.at,
        });
        Ok(outcome)
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
        let outcome = self.transition(
            actor,
            meeting_id,
            MeetingStatus::Locked,
            Operation::LockMeeting,
            AuditAction::MeetingLocked,
        )?;
        // The whole room is told, because the lock changes what every
        // participant's screen should offer. It changes nothing about what the
        // backend permits: that was re-read inside the transaction, and is
        // re-read inside every transaction afterwards (section 15).
        self.publish(DomainEvent::MeetingLocked {
            meeting_id,
            at: outcome.at,
        });
        Ok(outcome)
    }

    /// Record that a Host generated an export of a meeting. Host only.
    ///
    /// Step 12's one departure from the shape every other mutation here
    /// follows: there is no entity to write, because export produces a file
    /// outside this database, not a row inside it. What this method writes
    /// is the fact that it happened - one `meeting.exported` audit row - and
    /// nothing else. The rendered bytes never reach this method and never
    /// reach `audit_logs.metadata`; only which format was rendered does.
    ///
    /// This is the actual enforcement point for the `DRAFT` refusal
    /// (`Meeting::ensure_exportable`), re-read inside this transaction exactly
    /// as every lifecycle rule in this module is. A caller is expected to
    /// have already checked eligibility before doing the (potentially
    /// expensive, and definitely filesystem-touching) work of rendering and
    /// writing a file, but that earlier check is not what makes `DRAFT`
    /// refused - this one is (architecture rules section 15; Step 12 design
    /// freeze, corrected E-2/E-3).
    ///
    /// Deliberately publishes no [`DomainEvent`]: nothing in this application
    /// needs to be told an export happened - not a LAN participant, and not
    /// a second Host window, since there is only ever one (Step 12 design
    /// freeze, E-2's rejected alternatives).
    ///
    /// The filesystem write this audit row records is **not** atomic with
    /// this transaction: `std::fs` cannot participate in a SQLite
    /// transaction, and no distributed-transaction mechanism is introduced
    /// to fake one. The caller is expected to call this only after the file
    /// has already been written successfully; if this call then fails, the
    /// file remains on disk without a matching audit row, which is an
    /// accepted, documented non-atomic edge (Step 12 design freeze, E-2).
    pub fn record_export(
        &self,
        actor: &Actor,
        meeting_id: MeetingId,
        format: ExportFormat,
    ) -> DomainResult<ExportRecorded> {
        let mut outcome = None;

        self.db.transaction(&mut |tx: &dyn DomainTx| {
            let meeting = tx
                .find_meeting(meeting_id)?
                .ok_or(DomainError::MeetingNotFound { meeting_id })?;

            let proof = authorize(actor, meeting_id, Operation::GenerateExport)?;

            meeting.ensure_exportable()?;

            let at = UtcTimestamp::now();
            tx.insert_audit(
                &proof,
                &AuditEntry {
                    action: AuditAction::MeetingExported,
                    target: AuditTarget::Meeting(meeting_id),
                    metadata: json!({ "format": format.as_str() }),
                    at,
                },
            )?;

            outcome = Some(ExportRecorded {
                meeting_id,
                format,
                at,
            });
            Ok(())
        })?;

        committed(outcome, "recording an export")
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
    ///
    /// The work happens in [`authorize_note_write`] and [`apply_note_write`],
    /// which a remote import shares. Two implementations of the lock re-read or
    /// the version derivation would be two places for them to disagree, and
    /// those are exactly the two that must not (ADR-0022 decision 5).
    pub fn write_note(&self, actor: &Actor, command: WriteNote) -> DomainResult<NoteWritten> {
        let WriteNote {
            meeting_id,
            participant_id,
            content,
        } = command;

        let mut outcome = None;

        self.db.transaction(&mut |tx: &dyn DomainTx| {
            let at = UtcTimestamp::now();
            let proof = authorize_note_write(tx, actor, meeting_id, participant_id, &content)?;
            let written = apply_note_write(tx, &proof, meeting_id, participant_id, &content, at)?;

            // Audit belongs to the caller, not to the helper: what a note write
            // *is* differs between a person editing and an import arriving, and
            // the action recorded should say which.
            tx.insert_audit(
                &proof,
                &AuditEntry {
                    action: if written.created {
                        AuditAction::NoteCreated
                    } else {
                        AuditAction::NoteUpdated
                    },
                    target: AuditTarget::Note(written.note_id),
                    metadata: json!({
                        "participant_id": participant_id.to_storage(),
                        "version": written.version,
                    }),
                    at,
                },
            )?;

            outcome = Some(written);
            Ok(())
        })?;

        let outcome = committed(outcome, "writing a note")?;
        // The version, never the Markdown. A client that may read the note
        // fetches it through the boundary that decides whether it may.
        self.publish(DomainEvent::NoteChanged {
            meeting_id,
            participant_id,
            note_id: outcome.note_id,
            version: outcome.version,
            at: outcome.at,
        });
        Ok(outcome)
    }

    /// Import a remote participant's submission, as the Host confirmed it.
    ///
    /// The whole of it is **one** transaction: the note, its version, the
    /// idempotency ledger row and the audit entry commit together or not at all
    /// (architecture rules section 11, ADR-0022 decision 5). Calling
    /// [`Domain::write_note`] and then writing the ledger would be two
    /// transactions, and could leave an imported note with no ledger row -
    /// idempotency quietly broken.
    ///
    /// Everything security-critical is decided against state read **inside**
    /// that transaction: the meeting's lock, the actor's authority, the
    /// participant's membership, the content rules and the artefact's identity.
    /// A Host preview is informational, and nothing it concluded is carried in
    /// here as authority (ADR-0022 decision 4).
    ///
    /// `actor` must be [`Actor::RemoteImport`] built from database-resolved
    /// identifiers. `authorize` refuses a target participant that is not the
    /// actor's own regardless, which is what makes a mistake at the transport
    /// boundary a refusal rather than a breach.
    pub fn import_remote_submission(
        &self,
        actor: &Actor,
        command: ImportSubmission,
    ) -> DomainResult<SubmissionImported> {
        let ImportSubmission {
            meeting_id,
            participant_id,
            submission_id,
            content_hash,
            content,
            raw_payload,
            source_version,
        } = command;

        let mut outcome = None;

        self.db.transaction(&mut |tx: &dyn DomainTx| {
            // 1. The gate: the meeting, the actor's authority, the lock, the
            //    content rules and the participant's membership - all judged
            //    against state read inside this transaction.
            //
            //    **Before artefact identity, and that order is load-bearing.** A
            //    locked meeting must be refused as locked even when the file
            //    offered is also a duplicate, and an actor with no standing must
            //    be refused as unauthorized even when the artefact it carries is
            //    one this application has seen. Identity is never an
            //    authorization shortcut and never a lifecycle one.
            let at = UtcTimestamp::now();
            let proof = authorize_note_write(tx, actor, meeting_id, participant_id, &content)?;

            // 2. Artefact identity, now that this import is permitted at all.
            //    Inside the transaction, so a concurrent import cannot slip
            //    between the check and the write (ADR-0022 decision 6).
            //
            //    Before the write rather than after: a duplicate should cost
            //    nothing, and a refusal that had already written a version would
            //    depend on rollback to be correct.
            if let Some(seen) =
                tx.find_remote_submission(meeting_id, participant_id, submission_id)?
            {
                return Err(if seen.content_hash == content_hash {
                    DomainError::DuplicateSubmission {
                        submission_id,
                        note_version: seen.note_version,
                    }
                } else {
                    DomainError::ModifiedArtifact { submission_id }
                });
            }

            // 3. The same artefact under somebody else. Changing
            //    `participant_id` changes the canonical hash, so the lookup
            //    above cannot see it.
            if let Some(foreign) =
                tx.find_foreign_remote_submission(meeting_id, participant_id, submission_id)?
            {
                return Err(DomainError::CrossParticipantArtifact {
                    submission_id,
                    expected: foreign.participant_id,
                    detected: participant_id,
                });
            }

            // 4. The note itself: the upsert and the version, shared with a
            //    participant's own write so neither can drift.
            let written = apply_note_write(tx, &proof, meeting_id, participant_id, &content, at)?;

            let resolution = if written.created {
                SubmissionResolution::Imported
            } else {
                SubmissionResolution::Replaced
            };

            // 5. The ledger, under the same proof, so `proof.meeting_id()`
            //    scopes the row and no caller can supply a different meeting.
            tx.insert_remote_submission(
                &proof,
                &NewRemoteSubmission {
                    id: RemoteSubmissionId::new(),
                    participant_id,
                    submission_id,
                    content_hash: content_hash.clone(),
                    resolution,
                    note_version: written.version,
                    raw_payload: raw_payload.clone(),
                    at,
                },
            )?;

            // 6. Audit. The note body is never in metadata, and neither is the
            //    payload: both are already stored where they belong.
            tx.insert_audit(
                &proof,
                &AuditEntry {
                    action: AuditAction::RemoteSubmissionImported,
                    target: AuditTarget::Note(written.note_id),
                    metadata: json!({
                        "participant_id": participant_id.to_storage(),
                        "submission_id": submission_id.to_storage(),
                        "content_hash": content_hash,
                        "source_version": source_version,
                        "note_version": written.version,
                        "resolution": resolution.as_str(),
                    }),
                    at,
                },
            )?;

            outcome = Some(SubmissionImported {
                note_id: written.note_id,
                version: written.version,
                created: written.created,
                resolution,
                at,
            });
            Ok(())
        })?;

        let outcome = committed(outcome, "importing a remote submission")?;
        // After the commit, and only then. A failed import returns above and
        // never reaches this line, so it announces nothing.
        self.publish(DomainEvent::NoteChanged {
            meeting_id,
            participant_id,
            note_id: outcome.note_id,
            version: outcome.version,
            at: outcome.at,
        });
        Ok(outcome)
    }
}

/// The gate every note write passes, inside the caller's transaction.
///
/// Split out of the write itself so an import can put its **security and
/// lifecycle** decisions before its **artefact identity** decisions without
/// either caller growing its own copy of them (ADR-0022 decision 5).
///
/// That ordering is the point. A locked meeting must be refused *as locked*,
/// even when the artefact offered also happens to be a duplicate - and an actor
/// with no standing must be refused *as unauthorized*, even when the artefact it
/// carries is one this application has seen before. Artefact identity is never
/// an authorization shortcut, and never a lifecycle one.
///
/// Returns the proof, so the caller may write the note, a ledger row and an
/// audit record under the same authorization, in the same transaction.
fn authorize_note_write(
    tx: &dyn DomainTx,
    actor: &Actor,
    meeting_id: MeetingId,
    participant_id: ParticipantId,
    content: &str,
) -> DomainResult<Authorized> {
    // 1. Current state, read inside this transaction.
    let meeting = tx
        .find_meeting(meeting_id)?
        .ok_or(DomainError::MeetingNotFound { meeting_id })?;

    // 2. Authorization. A participant may write only their own note; the Host
    //    may write anyone's; an import is confined to its own.
    //
    //    Before the lock, as step 8 established: someone with no standing in
    //    this meeting learns that they have none, and not what state it is in.
    let proof = authorize(actor, meeting_id, Operation::WriteNote { participant_id })?;

    // 3. The lock, judged against the state just loaded. A pre-flight check
    //    elsewhere is a race; this is the enforcement.
    meeting.ensure_mutable()?;

    // 4. Validation.
    validate_note_content(content)?;
    if tx.find_participant(meeting_id, participant_id)?.is_none() {
        return Err(DomainError::ParticipantNotFound {
            meeting_id,
            participant_id,
        });
    }

    Ok(proof)
}

/// Write the note and append its history row, once the gate has passed.
///
/// The shared mechanics, so that the upsert, the version derivation and the
/// history row exist in exactly one implementation. It decides nothing: an
/// [`Authorized`] is the evidence that every decision has already been made,
/// and only [`authorize_note_write`] can produce one.
///
/// Deliberately does **not** write audit and does **not** publish: what the
/// write means differs between callers, and an event may only follow a commit
/// this function cannot see.
fn apply_note_write(
    tx: &dyn DomainTx,
    proof: &Authorized,
    meeting_id: MeetingId,
    participant_id: ParticipantId,
    content: &str,
    at: UtcTimestamp,
) -> DomainResult<NoteWritten> {
    // Upsert the single note. Exactly one exists per participant per meeting
    // (ADR-0003), so this never produces a second.
    let (note_id, created) = match tx.find_note(meeting_id, participant_id)? {
        Some(existing) => {
            tx.update_note_content(proof, existing.id, content, at)?;
            (existing.id, false)
        }
        None => {
            let note_id = NoteId::new();
            tx.insert_note(
                proof,
                &NewNote {
                    id: note_id,
                    participant_id,
                    content: content.to_owned(),
                    at,
                },
            )?;
            (note_id, true)
        }
    };

    // History. The version comes from the database, never from the caller.
    // `UNIQUE(note_id, version)` makes a lost race a conflict rather than a
    // duplicate.
    let version = tx.latest_note_version(note_id)? + 1;
    tx.insert_note_version(
        proof,
        &NewNoteVersion {
            id: NoteVersionId::new(),
            note_id,
            version,
            content: content.to_owned(),
            at,
        },
    )?;

    Ok(NoteWritten {
        note_id,
        version,
        created,
        at,
    })
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

/// The write half of a session action, shared by approving and revoking.
///
/// Both do the same thing to a different column, so the pipeline around them -
/// load, authorize, check the lock, find the live session, audit - is written
/// once and the difference is passed in.
type SessionWrite<'a> =
    &'a dyn Fn(&dyn DomainTx, &Authorized, SessionBinding, UtcTimestamp) -> DomainResult<()>;

/// Translate a lost first-claim-wins race into the refusal it actually is.
///
/// The read a moment earlier said the identity was free, so a constraint
/// violation here means another browser committed in between. The database is
/// the arbiter (ADR-0002), and its answer is "someone else has it" - which is
/// exactly [`DomainError::IdentityAlreadyClaimed`], not a generic conflict
/// carrying a SQLite message.
///
/// Anything that is *not* a conflict is passed through untouched: a disk
/// failure during a claim is still a disk failure.
fn claim_conflict(
    error: DomainError,
    meeting_id: MeetingId,
    participant_id: ParticipantId,
) -> DomainError {
    match error {
        DomainError::Conflict { .. } => DomainError::IdentityAlreadyClaimed {
            meeting_id,
            participant_id,
        },
        other => other,
    }
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
/// Delegated to [`crate::note`], where the rules and their reasoning live.
/// Kept as a named step in this module so the mutation pipeline still reads
/// the same as every other one: current state, authorization, lock,
/// validation, write, history, audit.
///
/// The shared editor runs the same rules in the browser, but this is the copy
/// that decides whether content is stored: it runs inside the transaction, and
/// content can also arrive from a transport that never saw the editor
/// (architecture rules section 3, ADR-0019).
fn validate_note_content(content: &str) -> DomainResult<()> {
    crate::note::validate(content)
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
    fn the_mutation_boundary_applies_the_note_content_rules() {
        // The rules belong to `crate::note`; this asserts the pipeline calls
        // them, so content the editor would refuse cannot be stored by a
        // transport that never ran the editor.
        for bad in [
            "<script>alert(1)</script>",
            "[x](javascript:alert(1))",
            "before\u{0}after",
        ] {
            let err = validate_note_content(bad).unwrap_err();
            assert!(
                matches!(err, DomainError::Validation { .. }),
                "{bad}: {err:?}"
            );
        }
        assert!(validate_note_content(&"a".repeat(crate::note::MAX_NOTE_BYTES + 1)).is_err());

        // And the regression that matters most: arithmetic is not HTML.
        assert!(validate_note_content("a < b").is_ok());
    }

    #[test]
    fn a_transaction_that_produced_nothing_is_a_persistence_failure_not_a_panic() {
        let err = committed::<()>(None, "testing").unwrap_err();
        assert!(matches!(err, DomainError::Persistence { .. }));
        assert!(!err.is_refusal());
    }
}
