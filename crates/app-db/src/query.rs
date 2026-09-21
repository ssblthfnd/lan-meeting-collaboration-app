//! Read-side queries for the Host.
//!
//! Step 3 established that every **mutation** goes through
//! `app_core::service::Domain` (ADR-0012). Reads are the other half, and they do
//! not go through it: there is nothing for the domain to decide about a read
//! that this layer could get wrong, and routing a list query through a write
//! transaction would take the write lock to answer a question. See ADR-0014.
//!
//! What this module is allowed to be: query mechanics and typed results. What it
//! must not become: a second place where rules live. There is no authorization
//! here, no lock check, no "if the meeting is locked then hide this". A caller
//! that needs a rule applied asks `app-core`.
//!
//! # Audience
//!
//! [`HostQueries`] is named for its audience, not for its tables. The Host may
//! see everything in its own database; a LAN participant may not, and must never
//! receive another participant's note (architecture rules section 16). When the
//! LAN transport arrives it gets its own query type with its own, narrower
//! results rather than reusing these.
//!
//! # Ordering
//!
//! Every query here has an explicit **total** order, with an id as the final
//! tiebreaker (architecture rules section 26.4). Nothing relies on insertion
//! order or `rowid`. Because ids are UUIDv7 that tiebreaker is chronological
//! rather than arbitrary (ADR-0011).
//!
//! # Connections
//!
//! Queries run on [`Db::read`], a pool of connections opened
//! `SQLITE_OPEN_READ_ONLY`. A write attempted through this path fails at the
//! SQLite level rather than quietly bypassing the mutation boundary.

use app_core::id::{AuditLogId, MeetingId, ParticipantId};
use app_core::meeting::MeetingStatus;
use app_core::participant::ParticipantDetails;
use app_core::session::ClaimStatus;
use app_core::time::{MeetingDate, MeetingTime, UtcTimestamp};
use rusqlite::{params, OptionalExtension, Row};
use serde_json::Value;

use crate::error::{DbError, DbResult};
use crate::pool::Db;
use crate::sql::Sql;

/// A meeting as the Host's meeting list shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeetingSummary {
    pub id: MeetingId,
    pub title: String,
    pub topic: Option<String>,
    /// Zoneless. Read in `timezone`, never in the OS timezone (PRD 25.2).
    pub date: MeetingDate,
    pub start_time: MeetingTime,
    pub end_time: MeetingTime,
    /// IANA identifier, carried alongside the schedule it gives meaning to.
    pub timezone: String,
    pub status: MeetingStatus,
    /// Derived by counting the roster, not stored (architecture rules 13).
    pub participant_count: i64,
    pub created_at: UtcTimestamp,
}

/// Everything the Host's meeting detail view shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeetingDetail {
    pub id: MeetingId,
    pub title: String,
    pub topic: Option<String>,
    pub date: MeetingDate,
    pub start_time: MeetingTime,
    pub end_time: MeetingTime,
    pub timezone: String,
    pub location: Option<String>,
    pub description: Option<String>,
    pub status: MeetingStatus,
    pub participant_count: i64,
    pub created_at: UtcTimestamp,
    pub updated_at: UtcTimestamp,
    /// Set exactly when `status` is `LOCKED`, which the schema enforces.
    pub locked_at: Option<UtcTimestamp>,
}

/// One participant of a meeting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParticipantSummary {
    pub id: ParticipantId,
    /// Reused from `app-core` rather than redeclared, so the read model and the
    /// write model cannot drift apart.
    pub details: ParticipantDetails,
    /// Derived from `participant_sessions`, never stored (ADR-0008).
    ///
    /// The Host needs this to see who has joined and to decide whether to
    /// revoke an identity someone is holding by mistake. `PENDING` means a live
    /// session the Host has not acknowledged - it is joined and working, not
    /// waiting for permission (ADR-0016).
    pub claim_status: ClaimStatus,
    pub created_at: UtcTimestamp,
}

/// One audit record.
///
/// Read-only by construction: the table has triggers refusing `UPDATE` and
/// `DELETE` (ADR-0011), and nothing in this crate offers to write one outside
/// the domain's audit step.
#[derive(Debug, Clone, PartialEq)]
pub struct AuditEntryView {
    pub id: AuditLogId,
    pub actor_type: String,
    /// `None` exactly when `actor_type` is `HOST`.
    pub actor_id: Option<ParticipantId>,
    pub action: String,
    pub target_type: String,
    pub target_id: Option<String>,
    /// Parsed from the stored text. The column has a `json_valid` check, so a
    /// row that fails to parse means the database was written to by something
    /// that bypassed it - reported rather than silently shown as empty.
    pub metadata: Option<Value>,
    pub created_at: UtcTimestamp,
}

/// Read-only queries answering what the Host UI displays.
pub struct HostQueries<'a> {
    db: &'a Db,
}

impl<'a> HostQueries<'a> {
    /// Borrow a database for reading.
    #[must_use]
    pub fn new(db: &'a Db) -> Self {
        Self { db }
    }

    /// Every meeting, most recent schedule first.
    ///
    /// Ordered by the meeting's own date and time rather than by creation, since
    /// that is the order the Host thinks in. `id` last makes it total.
    pub fn meetings(&self) -> DbResult<Vec<MeetingSummary>> {
        self.db.read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT m.id, m.title, m.topic, m.date, m.start_time, m.end_time,
                        m.timezone, m.status, m.created_at,
                        (SELECT count(*) FROM participants p WHERE p.meeting_id = m.id)
                   FROM meetings m
                  ORDER BY m.date DESC, m.start_time DESC, m.id DESC",
            )?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(MeetingSummary {
                        id: row.get::<_, Sql<MeetingId>>(0)?.into_inner(),
                        title: row.get(1)?,
                        topic: row.get(2)?,
                        date: row.get::<_, Sql<MeetingDate>>(3)?.into_inner(),
                        start_time: row.get::<_, Sql<MeetingTime>>(4)?.into_inner(),
                        end_time: row.get::<_, Sql<MeetingTime>>(5)?.into_inner(),
                        timezone: row.get(6)?,
                        status: status(row, 7)?,
                        created_at: row.get::<_, Sql<UtcTimestamp>>(8)?.into_inner(),
                        participant_count: row.get(9)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// One meeting in full, or `None` when there is no such meeting.
    pub fn meeting(&self, meeting_id: MeetingId) -> DbResult<Option<MeetingDetail>> {
        self.db.read(|conn| {
            let row = conn
                .query_row(
                    "SELECT m.id, m.title, m.topic, m.date, m.start_time, m.end_time,
                            m.timezone, m.location, m.description, m.status,
                            m.created_at, m.updated_at, m.locked_at,
                            (SELECT count(*) FROM participants p WHERE p.meeting_id = m.id)
                       FROM meetings m
                      WHERE m.id = ?1",
                    params![Sql(meeting_id)],
                    |row| {
                        Ok(MeetingDetail {
                            id: row.get::<_, Sql<MeetingId>>(0)?.into_inner(),
                            title: row.get(1)?,
                            topic: row.get(2)?,
                            date: row.get::<_, Sql<MeetingDate>>(3)?.into_inner(),
                            start_time: row.get::<_, Sql<MeetingTime>>(4)?.into_inner(),
                            end_time: row.get::<_, Sql<MeetingTime>>(5)?.into_inner(),
                            timezone: row.get(6)?,
                            location: row.get(7)?,
                            description: row.get(8)?,
                            status: status(row, 9)?,
                            created_at: row.get::<_, Sql<UtcTimestamp>>(10)?.into_inner(),
                            updated_at: row.get::<_, Sql<UtcTimestamp>>(11)?.into_inner(),
                            locked_at: row
                                .get::<_, Option<Sql<UtcTimestamp>>>(12)?
                                .map(Sql::into_inner),
                            participant_count: row.get(13)?,
                        })
                    },
                )
                .optional()?;
            Ok(row)
        })
    }

    /// A meeting's roster, by name.
    ///
    /// `ORDER BY name, id` is the order the architecture rules give as the
    /// example for exactly this list, and it matches
    /// `idx_participants_meeting_name`.
    pub fn participants(&self, meeting_id: MeetingId) -> DbResult<Vec<ParticipantSummary>> {
        self.db.read(|conn| {
            // The claim-status derivation is the one ADR-0008 defines, shared
            // with the participant-facing query so the two audiences cannot be
            // told different things about whether an identity is taken.
            let mut stmt = conn.prepare(
                "SELECT p.id, p.name, p.department, p.position, p.meeting_role, p.created_at,
                        (SELECT count(*) FROM participant_sessions s
                          WHERE s.participant_id = p.id),
                        (SELECT count(*) FROM participant_sessions s
                          WHERE s.participant_id = p.id
                            AND s.revoked_at IS NULL
                            AND s.approved_at IS NOT NULL),
                        (SELECT count(*) FROM participant_sessions s
                          WHERE s.participant_id = p.id
                            AND s.revoked_at IS NULL
                            AND s.approved_at IS NULL)
                   FROM participants p
                  WHERE p.meeting_id = ?1
                  ORDER BY p.name ASC, p.id ASC",
            )?;
            let rows = stmt
                .query_map(params![Sql(meeting_id)], |row| {
                    Ok(ParticipantSummary {
                        id: row.get::<_, Sql<ParticipantId>>(0)?.into_inner(),
                        details: ParticipantDetails {
                            name: row.get(1)?,
                            department: row.get(2)?,
                            position: row.get(3)?,
                            meeting_role: row.get(4)?,
                        },
                        claim_status: crate::participant_query::derive_claim_status(
                            row.get(6)?,
                            row.get(7)?,
                            row.get(8)?,
                        ),
                        created_at: row.get::<_, Sql<UtcTimestamp>>(5)?.into_inner(),
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// A meeting's audit trail, oldest first.
    ///
    /// Chronological, with `id` as the tiebreaker so that records written in the
    /// same millisecond still have one defined order. Because the stored
    /// timestamp is fixed width, text ordering is time ordering (ADR-0011).
    pub fn audit_entries(&self, meeting_id: MeetingId) -> DbResult<Vec<AuditEntryView>> {
        self.db.read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, actor_type, actor_id, action, target_type, target_id,
                        metadata, created_at
                   FROM audit_logs
                  WHERE meeting_id = ?1
                  ORDER BY created_at ASC, id ASC",
            )?;
            // Two passes: `query_map`'s closure can only fail with a
            // `rusqlite::Error`, and a metadata row that is not valid JSON is a
            // `DbError` of our own. Collecting the raw text first keeps that
            // failure honest instead of dressing it up as a SQLite error.
            let raw = stmt
                .query_map(params![Sql(meeting_id)], |row| {
                    Ok((
                        row.get::<_, Sql<AuditLogId>>(0)?.into_inner(),
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<Sql<ParticipantId>>>(2)?
                            .map(Sql::into_inner),
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, Sql<UtcTimestamp>>(7)?.into_inner(),
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            raw.into_iter()
                .map(
                    |(
                        id,
                        actor_type,
                        actor_id,
                        action,
                        target_type,
                        target_id,
                        metadata,
                        created_at,
                    )| {
                        Ok(AuditEntryView {
                            id,
                            actor_type,
                            actor_id,
                            action,
                            target_type,
                            target_id,
                            metadata: parse_metadata(metadata, id)?,
                            created_at,
                        })
                    },
                )
                .collect()
        })
    }
}

/// Read a `meetings.status` column into the domain enum.
///
/// A value the domain does not recognise means the database disagrees with this
/// binary about the lifecycle. Surfaced rather than guessed, exactly as the
/// mutation adapter does.
fn status(row: &Row<'_>, index: usize) -> rusqlite::Result<MeetingStatus> {
    let text: String = row.get(index)?;
    text.parse::<MeetingStatus>().map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::other(error.to_string())),
        )
    })
}

/// Parse `audit_logs.metadata`, which the schema guarantees is valid JSON.
fn parse_metadata(raw: Option<String>, id: AuditLogId) -> DbResult<Option<Value>> {
    match raw {
        None => Ok(None),
        Some(text) => serde_json::from_str(&text).map(Some).map_err(|error| {
            // Reachable only if something wrote past the `json_valid` CHECK.
            DbError::Malformed {
                what: "audit_logs.metadata",
                expected: format!("valid JSON, as the json_valid CHECK requires (audit row {id})"),
                detected: error.to_string(),
            }
        }),
    }
}
