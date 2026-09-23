//! The persistence adapter for the domain port.
//!
//! This module is the *only* implementation of [`app_core::port::DomainTx`]. It
//! contains persistence mechanics and nothing else: no authorization decision,
//! no meeting-lock check, no business rule. Those have already been made by the
//! time a write method here is reachable, because every write takes an
//! [`Authorized`], which only `app-core` can mint.
//!
//! Where a write needs to know *which meeting*, it reads it from the proof
//! rather than from its payload. A note therefore cannot be inserted into a
//! meeting the actor was not authorized for, even if a caller assembled a
//! payload that said otherwise.

use app_core::audit::AuditEntry;
use app_core::authz::Authorized;
use app_core::error::{DomainError, DomainResult};
use app_core::id::{MeetingId, NoteId, ParticipantId, SessionId, SubmissionId};
use app_core::meeting::{Meeting, MeetingConfiguration, MeetingStatus};
use app_core::participant::ParticipantDetails;
use app_core::port::{
    Database, DomainTx, NewMeeting, NewNote, NewNoteVersion, NewParticipant, NewRemoteSubmission,
    NewSession, NoteRow, ParticipantRow, SubmissionRecord,
};
use app_core::session::SessionBinding;
use app_core::time::UtcTimestamp;
use app_core::token::TokenHash;
use rusqlite::{params, OptionalExtension, Transaction};

use crate::error::DbError;
use crate::pool::Db;
use crate::sql::Sql;

impl From<DbError> for DomainError {
    /// Translate a persistence failure into the domain's vocabulary.
    ///
    /// A constraint violation becomes [`DomainError::Conflict`]: the schema's
    /// uniqueness rules are how concurrent writers are separated, so hitting
    /// one means another writer got there first. Everything else is an
    /// uninterpretable failure, and the SQLite detail is carried only as
    /// opaque diagnostic text.
    fn from(error: DbError) -> Self {
        match error {
            DbError::Constraint { detail } => DomainError::Conflict { detail },
            other => DomainError::Persistence {
                operation: "accessing the database",
                detail: other.to_string(),
            },
        }
    }
}

/// One domain transaction, backed by a SQLite transaction.
pub struct DbTx<'a, 'conn> {
    tx: &'a Transaction<'conn>,
}

impl<'a, 'conn> DbTx<'a, 'conn> {
    /// Wrap a SQLite transaction.
    pub fn new(tx: &'a Transaction<'conn>) -> Self {
        Self { tx }
    }
}

impl DomainTx for DbTx<'_, '_> {
    fn find_meeting(&self, meeting_id: MeetingId) -> DomainResult<Option<Meeting>> {
        let row = self
            .tx
            .query_row(
                // `join_token_hash IS NOT NULL` rather than the hash itself: the
                // domain needs to know whether issuing would displace a token,
                // and nothing more. A credential never enters the domain.
                "SELECT id, title, status, join_token_hash IS NOT NULL
                   FROM meetings WHERE id = ?1",
                params![Sql(meeting_id)],
                |row| {
                    Ok((
                        row.get::<_, Sql<MeetingId>>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, bool>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(DbError::from)?;

        row.map(|(id, title, status, has_join_token)| {
            Ok(Meeting {
                id: id.into_inner(),
                title,
                // A status the domain does not recognise means the database
                // disagrees with this binary about the lifecycle. Surface it
                // rather than guessing.
                status: status.parse::<MeetingStatus>()?,
                has_join_token,
            })
        })
        .transpose()
    }

    fn find_participant(
        &self,
        meeting_id: MeetingId,
        participant_id: ParticipantId,
    ) -> DomainResult<Option<ParticipantRow>> {
        let row = self
            .tx
            .query_row(
                "SELECT id, name, department, position, meeting_role
                   FROM participants
                  WHERE meeting_id = ?1 AND id = ?2",
                params![Sql(meeting_id), Sql(participant_id)],
                |row| {
                    Ok(ParticipantRow {
                        id: row.get::<_, Sql<ParticipantId>>(0)?.into_inner(),
                        details: ParticipantDetails {
                            name: row.get(1)?,
                            department: row.get(2)?,
                            position: row.get(3)?,
                            meeting_role: row.get(4)?,
                        },
                    })
                },
            )
            .optional()
            .map_err(DbError::from)?;
        Ok(row)
    }

    fn find_live_session(
        &self,
        meeting_id: MeetingId,
        participant_id: ParticipantId,
    ) -> DomainResult<Option<SessionBinding>> {
        // `revoked_at IS NULL` is the definition of live, and it is the same
        // predicate the partial unique index uses - so what this read sees and
        // what the index enforces cannot disagree (ADR-0002).
        //
        // The token hash is deliberately not selected. Nothing in the domain
        // needs it, and a credential hash that is never loaded cannot be leaked
        // by a later change to a struct that carries it.
        let row = self
            .tx
            .query_row(
                "SELECT id, approved_at
                   FROM participant_sessions
                  WHERE meeting_id = ?1 AND participant_id = ?2
                    AND revoked_at IS NULL",
                params![Sql(meeting_id), Sql(participant_id)],
                |row| {
                    Ok(SessionBinding {
                        session_id: row.get::<_, Sql<SessionId>>(0)?.into_inner(),
                        meeting_id,
                        participant_id,
                        approved_at: row
                            .get::<_, Option<Sql<UtcTimestamp>>>(1)?
                            .map(Sql::into_inner),
                    })
                },
            )
            .optional()
            .map_err(DbError::from)?;
        Ok(row)
    }

    fn count_participants(&self, meeting_id: MeetingId) -> DomainResult<i64> {
        let count: i64 = self
            .tx
            .query_row(
                "SELECT count(*) FROM participants WHERE meeting_id = ?1",
                params![Sql(meeting_id)],
                |row| row.get(0),
            )
            .map_err(DbError::from)?;
        Ok(count)
    }

    fn find_note(
        &self,
        meeting_id: MeetingId,
        participant_id: ParticipantId,
    ) -> DomainResult<Option<NoteRow>> {
        let row = self
            .tx
            .query_row(
                "SELECT id, content FROM notes WHERE meeting_id = ?1 AND participant_id = ?2",
                params![Sql(meeting_id), Sql(participant_id)],
                |row| Ok((row.get::<_, Sql<NoteId>>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(DbError::from)?;

        Ok(row.map(|(id, content)| NoteRow {
            id: id.into_inner(),
            content,
        }))
    }

    fn latest_note_version(&self, note_id: NoteId) -> DomainResult<i64> {
        let latest: i64 = self
            .tx
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM note_versions WHERE note_id = ?1",
                params![Sql(note_id)],
                |row| row.get(0),
            )
            .map_err(DbError::from)?;
        Ok(latest)
    }

    fn find_remote_submission(
        &self,
        meeting_id: MeetingId,
        participant_id: ParticipantId,
        submission_id: SubmissionId,
    ) -> DomainResult<Option<SubmissionRecord>> {
        // `idx_remote_submissions_identity` covers this exactly. Ordered so a
        // caller reads the newest row first; in practice the unique constraint
        // makes several rows for one `submission_id` possible only when the
        // content differed, which is a modified artefact either way.
        let record = self
            .tx
            .query_row(
                "SELECT participant_id, content_hash, note_version
                   FROM remote_submissions
                  WHERE meeting_id = ?1 AND participant_id = ?2 AND submission_id = ?3
                    AND note_version IS NOT NULL
                  ORDER BY imported_at DESC, id DESC
                  LIMIT 1",
                params![Sql(meeting_id), Sql(participant_id), Sql(submission_id)],
                submission_record,
            )
            .optional()
            .map_err(DbError::from)?;

        Ok(record)
    }

    fn find_foreign_remote_submission(
        &self,
        meeting_id: MeetingId,
        participant_id: ParticipantId,
        submission_id: SubmissionId,
    ) -> DomainResult<Option<SubmissionRecord>> {
        // The cross-participant rule (ADR-0022 decision 6). Scoped to the
        // meeting, which is all the confinement needed: a participant belongs to
        // exactly one meeting, and the composite foreign key enforces it.
        let record = self
            .tx
            .query_row(
                "SELECT participant_id, content_hash, note_version
                   FROM remote_submissions
                  WHERE meeting_id = ?1 AND submission_id = ?2 AND participant_id <> ?3
                    AND note_version IS NOT NULL
                  ORDER BY imported_at ASC, id ASC
                  LIMIT 1",
                params![Sql(meeting_id), Sql(submission_id), Sql(participant_id)],
                submission_record,
            )
            .optional()
            .map_err(DbError::from)?;

        Ok(record)
    }

    fn insert_meeting(&self, proof: &Authorized, meeting: &NewMeeting) -> DomainResult<()> {
        // The id comes from the proof, and `join_token_hash` stays NULL: a join
        // token is minted when the LAN server starts, not when the meeting is
        // created (PRD section 8.1).
        let configuration = &meeting.configuration;
        self.tx
            .execute(
                "INSERT INTO meetings
                   (id, title, topic, date, start_time, end_time, timezone,
                    location, description, status, join_token_hash,
                    created_at, updated_at, locked_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL, ?11, ?11, NULL)",
                params![
                    Sql(proof.meeting_id()),
                    configuration.title,
                    configuration.topic,
                    Sql(configuration.date),
                    Sql(configuration.start_time),
                    Sql(configuration.end_time),
                    configuration.timezone.name(),
                    configuration.location,
                    configuration.description,
                    meeting.status.as_str(),
                    Sql(meeting.at),
                ],
            )
            .map_err(DbError::from)?;
        Ok(())
    }

    fn update_meeting_configuration(
        &self,
        proof: &Authorized,
        configuration: &MeetingConfiguration,
        at: UtcTimestamp,
    ) -> DomainResult<()> {
        // `status` and `locked_at` are untouched: the lifecycle moves only
        // through `set_meeting_status`.
        self.tx
            .execute(
                "UPDATE meetings
                    SET title = ?1, topic = ?2, date = ?3, start_time = ?4, end_time = ?5,
                        timezone = ?6, location = ?7, description = ?8, updated_at = ?9
                  WHERE id = ?10",
                params![
                    configuration.title,
                    configuration.topic,
                    Sql(configuration.date),
                    Sql(configuration.start_time),
                    Sql(configuration.end_time),
                    configuration.timezone.name(),
                    configuration.location,
                    configuration.description,
                    Sql(at),
                    Sql(proof.meeting_id()),
                ],
            )
            .map_err(DbError::from)?;
        Ok(())
    }

    fn insert_participant(
        &self,
        proof: &Authorized,
        participant: &NewParticipant,
    ) -> DomainResult<()> {
        let details = &participant.details;
        self.tx
            .execute(
                "INSERT INTO participants
                   (id, meeting_id, name, department, position, meeting_role, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    Sql(participant.id),
                    Sql(proof.meeting_id()),
                    details.name,
                    details.department,
                    details.position,
                    details.meeting_role,
                    Sql(participant.at),
                ],
            )
            .map_err(DbError::from)?;
        Ok(())
    }

    fn update_participant(
        &self,
        proof: &Authorized,
        participant_id: ParticipantId,
        details: &ParticipantDetails,
    ) -> DomainResult<()> {
        // Scoped to the authorized meeting, so a participant id from another
        // roster cannot be edited through this proof.
        self.tx
            .execute(
                "UPDATE participants
                    SET name = ?1, department = ?2, position = ?3, meeting_role = ?4
                  WHERE id = ?5 AND meeting_id = ?6",
                params![
                    details.name,
                    details.department,
                    details.position,
                    details.meeting_role,
                    Sql(participant_id),
                    Sql(proof.meeting_id()),
                ],
            )
            .map_err(DbError::from)?;
        Ok(())
    }

    fn delete_participant(
        &self,
        proof: &Authorized,
        participant_id: ParticipantId,
    ) -> DomainResult<()> {
        self.tx
            .execute(
                "DELETE FROM participants WHERE id = ?1 AND meeting_id = ?2",
                params![Sql(participant_id), Sql(proof.meeting_id())],
            )
            .map_err(DbError::from)?;
        Ok(())
    }

    fn set_join_token_hash(
        &self,
        proof: &Authorized,
        token_hash: &TokenHash,
        at: UtcTimestamp,
    ) -> DomainResult<()> {
        // Overwriting is rotation: the previous hash matches nothing after this,
        // so the previous URL resolves to no meeting. The column is `UNIQUE`, so
        // the astronomically unlikely collision with another meeting's token is
        // a constraint violation rather than two meetings sharing a URL.
        self.tx
            .execute(
                "UPDATE meetings SET join_token_hash = ?1, updated_at = ?2 WHERE id = ?3",
                params![token_hash.as_str(), Sql(at), Sql(proof.meeting_id())],
            )
            .map_err(DbError::from)?;
        Ok(())
    }

    fn insert_session(&self, proof: &Authorized, session: &NewSession) -> DomainResult<()> {
        // `approved_at` and `revoked_at` are left null: a new session is live
        // and unacknowledged, and it can act (ADR-0016).
        //
        // The partial unique index decides a race here. A second live session
        // for the same identity fails as a constraint violation, which the
        // domain maps back to `IdentityAlreadyClaimed`.
        self.tx
            .execute(
                "INSERT INTO participant_sessions
                   (id, meeting_id, participant_id, session_token_hash, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    Sql(session.id),
                    Sql(proof.meeting_id()),
                    Sql(session.participant_id),
                    session.token_hash.as_str(),
                    Sql(session.at),
                ],
            )
            .map_err(DbError::from)?;
        Ok(())
    }

    fn approve_session(
        &self,
        proof: &Authorized,
        session_id: SessionId,
        at: UtcTimestamp,
    ) -> DomainResult<()> {
        // Only a live session can be acknowledged, and only within the meeting
        // the proof is scoped to.
        self.tx
            .execute(
                "UPDATE participant_sessions
                    SET approved_at = ?1
                  WHERE id = ?2 AND meeting_id = ?3 AND revoked_at IS NULL",
                params![Sql(at), Sql(session_id), Sql(proof.meeting_id())],
            )
            .map_err(DbError::from)?;
        Ok(())
    }

    fn revoke_session(
        &self,
        proof: &Authorized,
        session_id: SessionId,
        at: UtcTimestamp,
    ) -> DomainResult<()> {
        // The row stays; only `revoked_at` is set. History is retained, and the
        // partial unique index stops constraining this row, so the identity
        // becomes claimable again (ADR-0002 rule 5).
        self.tx
            .execute(
                "UPDATE participant_sessions
                    SET revoked_at = ?1
                  WHERE id = ?2 AND meeting_id = ?3 AND revoked_at IS NULL",
                params![Sql(at), Sql(session_id), Sql(proof.meeting_id())],
            )
            .map_err(DbError::from)?;
        Ok(())
    }

    fn set_meeting_status(
        &self,
        proof: &Authorized,
        status: MeetingStatus,
        at: UtcTimestamp,
    ) -> DomainResult<()> {
        // `locked_at` is set exactly when the meeting becomes LOCKED, which is
        // what the schema's `(status = 'LOCKED') = (locked_at IS NOT NULL)`
        // check requires.
        let locked_at = match status {
            MeetingStatus::Locked => Some(Sql(at)),
            _ => None,
        };

        self.tx
            .execute(
                "UPDATE meetings
                    SET status = ?1, updated_at = ?2, locked_at = ?3
                  WHERE id = ?4",
                params![status.as_str(), Sql(at), locked_at, Sql(proof.meeting_id())],
            )
            .map_err(DbError::from)?;
        Ok(())
    }

    fn insert_note(&self, proof: &Authorized, note: &NewNote) -> DomainResult<()> {
        self.tx
            .execute(
                "INSERT INTO notes (id, meeting_id, participant_id, content, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
                params![
                    Sql(note.id),
                    Sql(proof.meeting_id()),
                    Sql(note.participant_id),
                    note.content,
                    Sql(note.at),
                ],
            )
            .map_err(DbError::from)?;
        Ok(())
    }

    fn update_note_content(
        &self,
        proof: &Authorized,
        note_id: NoteId,
        content: &str,
        at: UtcTimestamp,
    ) -> DomainResult<()> {
        // Scoped to the authorized meeting, so a note id from elsewhere cannot
        // be edited through this proof.
        self.tx
            .execute(
                "UPDATE notes SET content = ?1, updated_at = ?2
                  WHERE id = ?3 AND meeting_id = ?4",
                params![content, Sql(at), Sql(note_id), Sql(proof.meeting_id())],
            )
            .map_err(DbError::from)?;
        Ok(())
    }

    fn insert_note_version(
        &self,
        proof: &Authorized,
        version: &NewNoteVersion,
    ) -> DomainResult<()> {
        // `created_by_type` and `created_by` come from the proof, never from a
        // payload: history records who actually acted (ADR-0008).
        self.tx
            .execute(
                "INSERT INTO note_versions
                   (id, note_id, version, content, created_at, created_by_type, created_by)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    Sql(version.id),
                    Sql(version.note_id),
                    version.version,
                    version.content,
                    Sql(version.at),
                    proof.actor_type(),
                    proof.created_by().map(Sql),
                ],
            )
            .map_err(DbError::from)?;
        Ok(())
    }

    fn insert_remote_submission(
        &self,
        proof: &Authorized,
        submission: &NewRemoteSubmission,
    ) -> DomainResult<()> {
        // `meeting_id` comes from the proof, so a ledger row cannot be written
        // against a meeting the actor was not authorized for - the same rule
        // every other write here follows.
        //
        // `raw_payload` is stored exactly as submitted. The column exists to
        // keep external input verbatim for forensics; canonical bytes are for
        // hashing and never reach here (ADR-0022 decision 15).
        self.tx
            .execute(
                "INSERT INTO remote_submissions
                   (id, meeting_id, participant_id, submission_id, content_hash,
                    imported_at, resolution, note_version, raw_payload)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    Sql(submission.id),
                    Sql(proof.meeting_id()),
                    Sql(submission.participant_id),
                    Sql(submission.submission_id),
                    submission.content_hash,
                    Sql(submission.at),
                    submission.resolution.as_str(),
                    submission.note_version,
                    submission.raw_payload,
                ],
            )
            .map_err(DbError::from)?;
        Ok(())
    }

    fn insert_audit(&self, proof: &Authorized, entry: &AuditEntry) -> DomainResult<()> {
        self.tx
            .execute(
                "INSERT INTO audit_logs
                   (id, meeting_id, actor_type, actor_id, action, target_type, target_id,
                    metadata, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    Sql(app_core::id::AuditLogId::new()),
                    Sql(proof.meeting_id()),
                    proof.actor_type(),
                    proof.created_by().map(Sql),
                    entry.action.as_str(),
                    entry.target.target_type(),
                    entry.target.target_id(),
                    entry.metadata.to_string(),
                    Sql(entry.at),
                ],
            )
            .map_err(DbError::from)?;
        Ok(())
    }
}

/// Map one `remote_submissions` row.
///
/// `note_version` is `NOT NULL` for every row the MVP writes - a refusal writes
/// nothing at all (ADR-0022 decision 7) - and both queries filter on that, so
/// the column is read as a plain `i64`.
///
/// A stored id the domain cannot parse means something wrote past the `CHECK`
/// constraint. Surfaced as a conversion failure rather than guessed, exactly as
/// the read model does for a lifecycle status.
fn submission_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<SubmissionRecord> {
    let stored: String = row.get(0)?;
    let participant_id = ParticipantId::parse(&stored).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::other(error.to_string())),
        )
    })?;

    Ok(SubmissionRecord {
        participant_id,
        content_hash: row.get(1)?,
        note_version: row.get(2)?,
    })
}

impl Database for Db {
    /// Open one `BEGIN IMMEDIATE` transaction and hand the domain a view of it.
    ///
    /// `write_with` propagates a `DomainError` from the closure untouched and
    /// drops the transaction without committing, so a refusal and a failure
    /// both roll back everything the closure had written - including any audit
    /// record it had already inserted.
    fn transaction(
        &self,
        f: &mut dyn FnMut(&dyn DomainTx) -> DomainResult<()>,
    ) -> DomainResult<()> {
        self.write_with(|tx| {
            let domain_tx = DbTx::new(tx);
            f(&domain_tx)
        })
    }
}
