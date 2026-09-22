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

use app_core::id::{AuditLogId, MeetingId, NoteId, ParticipantId};
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

/// A participant's note, as the Host reads it.
///
/// `version` is the number of the newest `note_versions` row, which is also
/// the count of versions because the sequence is dense from 1. It is carried
/// so the Host UI can show "version N" beside the note without a second query,
/// and so a `note.changed` event naming a version can be compared against what
/// is on screen.
///
/// `last_author_type` and `last_author_id` come from that newest history row
/// rather than from the note: a note has no author column, because with the
/// Host, the participant and a future import all able to write it, the useful
/// question is who wrote *this* content (ADR-0008).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteDetail {
    pub id: NoteId,
    pub participant_id: ParticipantId,
    /// GFM-subset Markdown as text (ADR-0007).
    pub content: String,
    pub version: i64,
    pub created_at: UtcTimestamp,
    pub updated_at: UtcTimestamp,
    /// `HOST`, `PARTICIPANT` or `REMOTE_IMPORT`, as stored.
    pub last_author_type: String,
    /// `None` exactly when `last_author_type` is `HOST` (ADR-0008).
    pub last_author_id: Option<ParticipantId>,
}

/// One row of note history, without its body.
///
/// Deliberately no `content`. A history list is metadata, and shipping every
/// historical body to render a list of dates would move a note's entire past
/// across the boundary every time somebody opened the tab. The body is fetched
/// for the one version being previewed, through
/// [`HostQueries::note_version`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteVersionSummary {
    pub version: i64,
    pub created_at: UtcTimestamp,
    pub created_by_type: String,
    /// `None` exactly when `created_by_type` is `HOST`.
    pub created_by: Option<ParticipantId>,
    /// Size of that version's content in bytes, so a list can show how much
    /// changed without carrying the change.
    pub byte_length: i64,
}

/// One historical version, with its body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteVersionDetail {
    pub version: i64,
    pub content: String,
    pub created_at: UtcTimestamp,
    pub created_by_type: String,
    pub created_by: Option<ParticipantId>,
}

/// Whether each participant on the roster has a note yet.
///
/// One row per participant, **including those with no note**, so the Host's
/// list is the roster rather than a subset of it with gaps where the
/// interesting work has not started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteOverview {
    pub participant_id: ParticipantId,
    pub name: String,
    /// `None` when this participant has no note.
    pub note_id: Option<NoteId>,
    /// `None` when this participant has no note.
    pub version: Option<i64>,
    pub updated_at: Option<UtcTimestamp>,
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

    /// One participant's note, if they have one.
    ///
    /// Scoped by both ids. A participant id belonging to another meeting
    /// resolves to nothing rather than to that other meeting's note, which is
    /// the same shape every other scoped read in this crate uses.
    ///
    /// The newest history row supplies the version and the authorship. It
    /// always exists for a note that exists: `Domain::write_note` appends one
    /// in the same transaction that creates the note, so a note with no
    /// history would mean something wrote past the mutation boundary.
    pub fn note(
        &self,
        meeting_id: MeetingId,
        participant_id: ParticipantId,
    ) -> DbResult<Option<NoteDetail>> {
        self.db.read(|conn| {
            let row = conn
                .query_row(
                    "SELECT n.id, n.participant_id, n.content, n.created_at, n.updated_at,
                            v.version, v.created_by_type, v.created_by
                       FROM notes n
                       JOIN note_versions v ON v.note_id = n.id
                      WHERE n.meeting_id = ?1 AND n.participant_id = ?2
                      ORDER BY v.version DESC
                      LIMIT 1",
                    params![Sql(meeting_id), Sql(participant_id)],
                    |row| {
                        Ok(NoteDetail {
                            id: row.get::<_, Sql<NoteId>>(0)?.into_inner(),
                            participant_id: row.get::<_, Sql<ParticipantId>>(1)?.into_inner(),
                            content: row.get(2)?,
                            created_at: row.get::<_, Sql<UtcTimestamp>>(3)?.into_inner(),
                            updated_at: row.get::<_, Sql<UtcTimestamp>>(4)?.into_inner(),
                            version: row.get(5)?,
                            last_author_type: row.get(6)?,
                            last_author_id: row
                                .get::<_, Option<Sql<ParticipantId>>>(7)?
                                .map(Sql::into_inner),
                        })
                    },
                )
                .optional()?;
            Ok(row)
        })
    }

    /// A note's history, newest first, without bodies.
    ///
    /// `ORDER BY version DESC` is total on its own: `UNIQUE(note_id, version)`
    /// makes the number unique per note, and the sequence is dense from 1
    /// (architecture rules section 26.4).
    pub fn note_versions(
        &self,
        meeting_id: MeetingId,
        participant_id: ParticipantId,
    ) -> DbResult<Vec<NoteVersionSummary>> {
        self.db.read(|conn| {
            // `length(v.content)` in SQLite counts characters for TEXT, so the
            // cast to BLOB is what makes this a byte count - the same unit the
            // domain's 64 KiB limit is expressed in.
            let mut stmt = conn.prepare(
                "SELECT v.version, v.created_at, v.created_by_type, v.created_by,
                        length(CAST(v.content AS BLOB))
                   FROM note_versions v
                   JOIN notes n ON n.id = v.note_id
                  WHERE n.meeting_id = ?1 AND n.participant_id = ?2
                  ORDER BY v.version DESC",
            )?;
            let rows = stmt
                .query_map(params![Sql(meeting_id), Sql(participant_id)], |row| {
                    Ok(NoteVersionSummary {
                        version: row.get(0)?,
                        created_at: row.get::<_, Sql<UtcTimestamp>>(1)?.into_inner(),
                        created_by_type: row.get(2)?,
                        created_by: row
                            .get::<_, Option<Sql<ParticipantId>>>(3)?
                            .map(Sql::into_inner),
                        byte_length: row.get(4)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// One historical version, with its body.
    ///
    /// The counterpart to [`HostQueries::note_versions`] carrying no bodies:
    /// the Host previews one version at a time, so one body crosses the
    /// boundary at a time.
    pub fn note_version(
        &self,
        meeting_id: MeetingId,
        participant_id: ParticipantId,
        version: i64,
    ) -> DbResult<Option<NoteVersionDetail>> {
        self.db.read(|conn| {
            let row = conn
                .query_row(
                    "SELECT v.version, v.content, v.created_at, v.created_by_type, v.created_by
                       FROM note_versions v
                       JOIN notes n ON n.id = v.note_id
                      WHERE n.meeting_id = ?1 AND n.participant_id = ?2 AND v.version = ?3",
                    params![Sql(meeting_id), Sql(participant_id), version],
                    |row| {
                        Ok(NoteVersionDetail {
                            version: row.get(0)?,
                            content: row.get(1)?,
                            created_at: row.get::<_, Sql<UtcTimestamp>>(2)?.into_inner(),
                            created_by_type: row.get(3)?,
                            created_by: row
                                .get::<_, Option<Sql<ParticipantId>>>(4)?
                                .map(Sql::into_inner),
                        })
                    },
                )
                .optional()?;
            Ok(row)
        })
    }

    /// Every participant, and whether they have written a note.
    ///
    /// A left join, so a roster of ninety-nine people with two notes returns
    /// ninety-nine rows. The Host's notes list is the roster; showing only the
    /// participants who have already written would hide exactly the people the
    /// Host is looking for.
    ///
    /// Ordered by name then id, matching [`HostQueries::participants`] so the
    /// two lists cannot disagree about order.
    pub fn notes_overview(&self, meeting_id: MeetingId) -> DbResult<Vec<NoteOverview>> {
        self.db.read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT p.id, p.name, n.id, n.updated_at,
                        (SELECT max(v.version) FROM note_versions v WHERE v.note_id = n.id)
                   FROM participants p
                   LEFT JOIN notes n
                     ON n.meeting_id = p.meeting_id AND n.participant_id = p.id
                  WHERE p.meeting_id = ?1
                  ORDER BY p.name ASC, p.id ASC",
            )?;
            let rows = stmt
                .query_map(params![Sql(meeting_id)], |row| {
                    Ok(NoteOverview {
                        participant_id: row.get::<_, Sql<ParticipantId>>(0)?.into_inner(),
                        name: row.get(1)?,
                        note_id: row.get::<_, Option<Sql<NoteId>>>(2)?.map(Sql::into_inner),
                        updated_at: row
                            .get::<_, Option<Sql<UtcTimestamp>>>(3)?
                            .map(Sql::into_inner),
                        version: row.get(4)?,
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
