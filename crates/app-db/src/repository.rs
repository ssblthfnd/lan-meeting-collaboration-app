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
use app_core::id::{MeetingId, NoteId, ParticipantId};
use app_core::meeting::{Meeting, MeetingStatus};
use app_core::port::{Database, DomainTx, NewNote, NewNoteVersion, NoteRow};
use app_core::time::UtcTimestamp;
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
                "SELECT id, title, status FROM meetings WHERE id = ?1",
                params![Sql(meeting_id)],
                |row| {
                    Ok((
                        row.get::<_, Sql<MeetingId>>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(DbError::from)?;

        row.map(|(id, title, status)| {
            Ok(Meeting {
                id: id.into_inner(),
                title,
                // A status the domain does not recognise means the database
                // disagrees with this binary about the lifecycle. Surface it
                // rather than guessing.
                status: status.parse::<MeetingStatus>()?,
            })
        })
        .transpose()
    }

    fn participant_exists(
        &self,
        meeting_id: MeetingId,
        participant_id: ParticipantId,
    ) -> DomainResult<bool> {
        let found: Option<i64> = self
            .tx
            .query_row(
                "SELECT 1 FROM participants WHERE meeting_id = ?1 AND id = ?2",
                params![Sql(meeting_id), Sql(participant_id)],
                |row| row.get(0),
            )
            .optional()
            .map_err(DbError::from)?;
        Ok(found.is_some())
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
