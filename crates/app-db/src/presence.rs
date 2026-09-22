//! Who is currently connected, and when they were last seen.
//!
//! Presence has two halves, and keeping them apart is the whole design:
//!
//! | Half | Lives in | Answers |
//! | --- | --- | --- |
//! | current connectivity | an in-memory registry in the LAN transport | is a socket open *right now* |
//! | last observed activity | `participant_sessions.last_seen_at` | when was one last open |
//!
//! Only the second half is this module's. The first cannot be persisted
//! honestly: a Host process that is killed leaves no chance to write "everybody
//! disconnected", so a `connected` column would outlive the truth and would
//! have to be distrusted on every read. A column that must be distrusted is
//! worse than no column (ADR-0018).
//!
//! # `last_seen_at` means exactly one thing
//!
//! **The last moment a socket for this session was observed to open or close.**
//! It is not a heartbeat clock and not a liveness guarantee: it is written on
//! the `0 -> 1` and `1 -> 0` connection transitions only, so a participant who
//! has been connected and quiet for an hour still shows the timestamp of the
//! moment they connected. Detecting that a socket died without a close frame
//! takes as long as the transport's keepalive takes to notice, so the value can
//! lag reality by that interval.
//!
//! Writing it per keepalive was the alternative, and it is rejected: it would
//! turn an idle meeting of 99 participants into a steady stream of SQLite
//! writes against the single writer connection, to make a column marginally
//! fresher than the event stream that already carries the same news.
//!
//! # Not a domain mutation
//!
//! [`PresenceStore::touch_session`] writes a row, and it deliberately does not
//! go through `app_core::service::Domain`:
//!
//! - **Nothing is authorized.** The session id comes from an already
//!   authenticated connection, built from the session row itself. There is no
//!   actor decision left to make, and no parameter through which a caller could
//!   name someone else's session.
//! - **Nothing is audited.** The audit log records what people did to the
//!   meeting (architecture rules section 17). A socket opening is not an action
//!   somebody took against the meeting, and putting it in the log would bury
//!   the entries that are (ADR-0018).
//! - **The meeting lock does not apply.** A locked meeting still has people
//!   looking at it, and observing that is not a mutation of the meeting.
//!
//! What it must not become is a back door. It can set exactly one column, on
//! exactly one row, to a timestamp it is given - and it refuses a revoked
//! session, because a revoked session is not connected to anything.

use app_core::id::{MeetingId, ParticipantId, SessionId};
use app_core::time::UtcTimestamp;
use rusqlite::params;

use crate::error::DbResult;
use crate::pool::Db;
use crate::sql::Sql;

/// One participant's presence, as the database can attest to it.
///
/// `session_id` is `None` for an identity nobody currently holds. `last_seen_at`
/// is `None` for a live session that has never opened a socket - claimed over
/// HTTP, never connected.
///
/// There is deliberately no `connected` field. Whether a socket is open is the
/// transport's to say, and a struct read out of SQLite has no way to know it;
/// a field here would be a value that is wrong whenever the process restarted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParticipantPresence {
    pub participant_id: ParticipantId,
    /// The live session holding this identity, if one does.
    pub session_id: Option<SessionId>,
    /// Last observed connection activity for that session (see the module
    /// documentation for what this does and does not promise).
    pub last_seen_at: Option<UtcTimestamp>,
}

/// Reading and recording connection activity.
pub struct PresenceStore<'a> {
    db: &'a Db,
}

impl<'a> PresenceStore<'a> {
    /// Borrow a database.
    #[must_use]
    pub fn new(db: &'a Db) -> Self {
        Self { db }
    }

    /// Record that a session's socket was observed opening or closing.
    ///
    /// Returns whether a row was updated. `false` means the session is revoked
    /// or gone, which a caller may use to stop treating it as live - but it is
    /// not the authorization check: that happened when the connection was
    /// established, and happens again on every mutation.
    ///
    /// `revoked_at IS NULL` is part of the statement rather than a check a
    /// caller could forget, for the same reason it is part of
    /// [`crate::session_store::SessionStore::resolve_session`].
    pub fn touch_session(&self, session_id: SessionId, at: UtcTimestamp) -> DbResult<bool> {
        self.db.write(|tx| {
            let changed = tx.execute(
                "UPDATE participant_sessions
                    SET last_seen_at = ?2
                  WHERE id = ?1
                    AND revoked_at IS NULL",
                params![Sql(session_id), Sql(at)],
            )?;
            Ok(changed > 0)
        })
    }

    /// Every identity in the meeting, with the live session holding it.
    ///
    /// One row per participant, including those nobody has claimed, so the
    /// caller can render a whole roster without a second query and without
    /// inventing rows for the gaps.
    ///
    /// Explicit total ordering, by name then id, matching the roster queries it
    /// is displayed beside (architecture rules section 26.4).
    pub fn roster_presence(&self, meeting_id: MeetingId) -> DbResult<Vec<ParticipantPresence>> {
        self.db.read(|conn| {
            // The correlated sub-selects are bounded by the partial unique
            // index `idx_sessions_one_live_per_participant`: at most one live
            // session per identity, so each returns at most one row (ADR-0002).
            let mut stmt = conn.prepare(
                "SELECT p.id,
                        (SELECT s.id FROM participant_sessions s
                          WHERE s.meeting_id = p.meeting_id
                            AND s.participant_id = p.id
                            AND s.revoked_at IS NULL),
                        (SELECT s.last_seen_at FROM participant_sessions s
                          WHERE s.meeting_id = p.meeting_id
                            AND s.participant_id = p.id
                            AND s.revoked_at IS NULL)
                   FROM participants p
                  WHERE p.meeting_id = ?1
                  ORDER BY p.name ASC, p.id ASC",
            )?;
            let rows = stmt
                .query_map(params![Sql(meeting_id)], |row| {
                    Ok(ParticipantPresence {
                        participant_id: row.get::<_, Sql<ParticipantId>>(0)?.into_inner(),
                        session_id: row
                            .get::<_, Option<Sql<SessionId>>>(1)?
                            .map(Sql::into_inner),
                        last_seen_at: row
                            .get::<_, Option<Sql<UtcTimestamp>>>(2)?
                            .map(Sql::into_inner),
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }
}
