//! Read-side queries for a LAN participant.
//!
//! Deliberately **not** `HostQueries` with a filter on top. ADR-0014 named that
//! type for its audience precisely so this one could exist separately: the Host
//! may see everything in their own database, a participant may not, and the
//! surest way to leak something is to reuse a convenient read model that already
//! returns it (architecture rules section 16).
//!
//! # Narrow by construction
//!
//! The safety here is in the **shapes**, not in filtering. There is no struct in
//! this module with a field for a note, an audit entry, a token hash, a session,
//! or another participant's department. A future change that wanted to leak one
//! would have to add a field and a column and a test would have to be rewritten,
//! rather than someone forgetting a `WHERE`.
//!
//! # Before and after the claim
//!
//! Two visibility tiers, decided by what the caller has proved:
//!
//! | Holding | May see |
//! | --- | --- |
//! | a join token | the meeting's public facts; **names and claim status only** |
//! | a session token | the above, plus their own full details and own note |
//!
//! A name is unavoidable: a participant picks their own identity from the list
//! (PRD section 5.2). Department, position and role are not needed to pick a
//! name, so they are withheld until the claim succeeds.
//!
//! Reads run on the read-only pool and never travel through the domain
//! (ADR-0014). Every query has an explicit total ordering (section 26.4).

use app_core::id::{MeetingId, ParticipantId};
use app_core::meeting::MeetingStatus;
use app_core::participant::ParticipantDetails;
use app_core::session::ClaimStatus;
use app_core::time::{MeetingDate, MeetingTime, UtcTimestamp};
use rusqlite::{params, OptionalExtension};

use crate::error::{DbError, DbResult};
use crate::pool::Db;
use crate::sql::Sql;

/// What a participant may know about the meeting they are joining.
///
/// The Host's own view carries more - counts, timestamps, lock times, the
/// description of who did what. None of that is here, because none of it is
/// needed to join.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinableMeeting {
    pub id: MeetingId,
    pub title: String,
    pub topic: Option<String>,
    /// Zoneless, read in `timezone` (PRD 25.2, 25.3).
    pub date: MeetingDate,
    pub start_time: MeetingTime,
    pub end_time: MeetingTime,
    pub timezone: String,
    pub location: Option<String>,
    pub status: MeetingStatus,
}

/// One identity a participant may claim.
///
/// A name and whether it is taken. Nothing else exists on this struct, which is
/// the enforcement: pre-claim metadata cannot leak through a query that has
/// nowhere to put it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimableIdentity {
    pub id: ParticipantId,
    pub name: String,
    pub claim_status: ClaimStatus,
}

/// A participant's own details, after they have claimed the identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnIdentity {
    pub id: ParticipantId,
    pub details: ParticipantDetails,
}

/// A participant's own note.
///
/// Deliberately **not** `HostQueries::note`. Read models are named for their
/// audience (ADR-0014), and the Host's shape carries a note id, a creation
/// timestamp and an author id that a participant has no use for. Reusing it
/// here would be reusing a convenient struct across two audiences, which is
/// how something meant for one ends up in the other's hands.
///
/// `last_author_type` is the one piece of provenance a participant genuinely
/// needs: it is how they can tell that the Host changed their note underneath
/// them. The author's *id* is withheld, because for one's own note the only
/// possible authors are themselves, the Host, or an import - the type alone is
/// unambiguous, and an id would add surface without adding an answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnNote {
    /// GFM-subset Markdown as text (ADR-0007).
    pub content: String,
    /// The newest history version. Dense from 1 (ADR-0019).
    pub version: i64,
    pub updated_at: UtcTimestamp,
    /// `HOST`, `PARTICIPANT` or `REMOTE_IMPORT`, as stored.
    pub last_author_type: String,
}

/// Read-only queries answering what a LAN participant may see.
pub struct ParticipantQueries<'a> {
    db: &'a Db,
}

impl<'a> ParticipantQueries<'a> {
    /// Borrow a database for reading.
    #[must_use]
    pub fn new(db: &'a Db) -> Self {
        Self { db }
    }

    /// The meeting, as a joining participant may see it.
    pub fn joinable_meeting(&self, meeting_id: MeetingId) -> DbResult<Option<JoinableMeeting>> {
        self.db.read(|conn| {
            let row = conn
                .query_row(
                    "SELECT id, title, topic, date, start_time, end_time, timezone,
                            location, status
                       FROM meetings
                      WHERE id = ?1",
                    params![Sql(meeting_id)],
                    |row| {
                        Ok((
                            row.get::<_, Sql<MeetingId>>(0)?.into_inner(),
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, Sql<MeetingDate>>(3)?.into_inner(),
                            row.get::<_, Sql<MeetingTime>>(4)?.into_inner(),
                            row.get::<_, Sql<MeetingTime>>(5)?.into_inner(),
                            row.get::<_, String>(6)?,
                            row.get::<_, Option<String>>(7)?,
                            row.get::<_, String>(8)?,
                        ))
                    },
                )
                .optional()?;

            row.map(
                |(id, title, topic, date, start_time, end_time, timezone, location, status)| {
                    Ok(JoinableMeeting {
                        id,
                        title,
                        topic,
                        date,
                        start_time,
                        end_time,
                        timezone,
                        location,
                        status: parse_status(&status)?,
                    })
                },
            )
            .transpose()
        })
    }

    /// The identities on offer, by name.
    ///
    /// Claim status is derived from `participant_sessions` rather than stored
    /// (ADR-0008), so a revoked session shows as claimable again without any
    /// second value having to be kept in step.
    ///
    /// `PENDING` and `CLAIMED` both mean taken: a live session holds the
    /// identity whether or not the Host has acknowledged it (ADR-0016).
    pub fn claimable_identities(&self, meeting_id: MeetingId) -> DbResult<Vec<ClaimableIdentity>> {
        self.db.read(|conn| {
            // One query, explicit total ordering (ADR-0008, section 26.4).
            let mut stmt = conn.prepare(
                "SELECT p.id,
                        p.name,
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
                    let total: i64 = row.get(2)?;
                    let live_approved: i64 = row.get(3)?;
                    let live_pending: i64 = row.get(4)?;
                    Ok(ClaimableIdentity {
                        id: row.get::<_, Sql<ParticipantId>>(0)?.into_inner(),
                        name: row.get(1)?,
                        claim_status: derive_claim_status(total, live_approved, live_pending),
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// The participant's own details.
    ///
    /// Scoped by both ids, which the caller took from a resolved session and not
    /// from a request body. A participant asking for someone else's row cannot
    /// express the question: there is no route that passes a participant id here.
    pub fn own_identity(
        &self,
        meeting_id: MeetingId,
        participant_id: ParticipantId,
    ) -> DbResult<Option<OwnIdentity>> {
        self.db.read(|conn| {
            let row = conn
                .query_row(
                    "SELECT id, name, department, position, meeting_role
                       FROM participants
                      WHERE meeting_id = ?1 AND id = ?2",
                    params![Sql(meeting_id), Sql(participant_id)],
                    |row| {
                        Ok(OwnIdentity {
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
                .optional()?;
            Ok(row)
        })
    }

    /// The participant's own note, if they have written one.
    ///
    /// Scoped by **both** ids, and both came from a resolved session rather
    /// than from a request. A participant asking for somebody else's note
    /// cannot express the question: no route passes a participant id here, and
    /// this signature has nowhere to put one that is not the caller's own
    /// (PRD section 5.2 - a participant may not see another participant's
    /// note).
    ///
    /// The newest history row supplies the version and the authorship. It
    /// always exists for a note that exists: `Domain::write_note` appends one
    /// in the same transaction that creates the note, so a note with no
    /// history would mean something had written past the mutation boundary.
    pub fn own_note(
        &self,
        meeting_id: MeetingId,
        participant_id: ParticipantId,
    ) -> DbResult<Option<OwnNote>> {
        self.db.read(|conn| {
            let row = conn
                .query_row(
                    "SELECT n.content, n.updated_at, v.version, v.created_by_type
                       FROM notes n
                       JOIN note_versions v ON v.note_id = n.id
                      WHERE n.meeting_id = ?1 AND n.participant_id = ?2
                      ORDER BY v.version DESC
                      LIMIT 1",
                    params![Sql(meeting_id), Sql(participant_id)],
                    |row| {
                        Ok(OwnNote {
                            content: row.get(0)?,
                            updated_at: row.get::<_, Sql<UtcTimestamp>>(1)?.into_inner(),
                            version: row.get(2)?,
                            last_author_type: row.get(3)?,
                        })
                    },
                )
                .optional()?;
            Ok(row)
        })
    }
}

/// The derivation ADR-0008 defines, in one place.
///
/// Shared by the Host and participant views so the two cannot disagree about
/// whether an identity is taken.
pub(crate) fn derive_claim_status(
    total: i64,
    live_approved: i64,
    live_pending: i64,
) -> ClaimStatus {
    if total == 0 {
        ClaimStatus::Unclaimed
    } else if live_approved > 0 {
        ClaimStatus::Claimed
    } else if live_pending > 0 {
        ClaimStatus::Pending
    } else {
        ClaimStatus::Revoked
    }
}

/// Read a `meetings.status` column into the domain enum.
pub(crate) fn parse_status(text: &str) -> DbResult<MeetingStatus> {
    text.parse::<MeetingStatus>()
        .map_err(|error| DbError::Malformed {
            what: "meetings.status",
            expected: "DRAFT, OPEN or LOCKED".to_owned(),
            detected: error.to_string(),
        })
}
