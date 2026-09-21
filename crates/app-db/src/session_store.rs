//! Resolving a presented credential to stored facts.
//!
//! This is the narrowest and most security-critical read in the application:
//! its output becomes an `app_core::Actor`, and an `Actor` is what every
//! authorization decision is made against.
//!
//! # What makes it safe
//!
//! A caller hands in a [`TokenHash`] and gets back facts **from the row**. It
//! cannot pass a participant id, a meeting id or a session id, so there is no
//! parameter through which a browser could influence who it turns out to be
//! (architecture rules section 14.1 rule 8). The only thing a client controls is
//! which credential it presents, and a credential it does not hold hashes to
//! something that matches no row.
//!
//! A revoked session resolves to `None`, identically to one that never existed.
//! Both are "you have no session here", and telling them apart would confirm to
//! someone holding a stale token that it was once real.
//!
//! # Why this is not in `HostQueries` or `ParticipantQueries`
//!
//! Those answer "what should be displayed". This answers "who is asking", which
//! happens before anything is displayed and must not be reachable by a handler
//! that merely wants to render a page. Keeping it in its own type with two
//! methods makes that separation visible.
//!
//! Reads, so no transaction and no domain involvement (ADR-0014). The lookup is
//! an indexed equality match on a `UNIQUE` column: the database finds the row,
//! and nothing in Rust compares secrets.

use app_core::id::{MeetingId, ParticipantId, SessionId};
use app_core::session::SessionBinding;
use app_core::time::UtcTimestamp;
use app_core::token::TokenHash;
use rusqlite::{params, OptionalExtension};

use crate::error::DbResult;
use crate::pool::Db;
use crate::sql::Sql;

/// A meeting a join token resolves to.
///
/// Carries the status so the caller can refuse a meeting that is not open
/// without a second query. The token hash is never returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinTarget {
    pub meeting_id: MeetingId,
    /// `DRAFT`, `OPEN` or `LOCKED`, as stored.
    pub status: String,
}

/// Credential lookups.
pub struct SessionStore<'a> {
    db: &'a Db,
}

impl<'a> SessionStore<'a> {
    /// Borrow a database for credential resolution.
    #[must_use]
    pub fn new(db: &'a Db) -> Self {
        Self { db }
    }

    /// The meeting a join token belongs to, if any.
    ///
    /// Returns the meeting **whatever its status**, so the caller can tell the
    /// difference between "this token is not ours" and "this meeting is not
    /// open" - two facts a participant needs distinguished differently at the
    /// transport boundary.
    pub fn resolve_join_token(&self, token_hash: &TokenHash) -> DbResult<Option<JoinTarget>> {
        self.db.read(|conn| {
            let row = conn
                .query_row(
                    "SELECT id, status FROM meetings WHERE join_token_hash = ?1",
                    params![token_hash.as_str()],
                    |row| {
                        Ok(JoinTarget {
                            meeting_id: row.get::<_, Sql<MeetingId>>(0)?.into_inner(),
                            status: row.get(1)?,
                        })
                    },
                )
                .optional()?;
            Ok(row)
        })
    }

    /// The live session a session token belongs to, if any.
    ///
    /// `revoked_at IS NULL` is part of the query, not a check the caller is
    /// trusted to perform afterwards: a revoked session must not be able to
    /// resolve to an actor at all (ADR-0002 rule 10).
    ///
    /// `approved_at` comes back for display, and for display only. Whether the
    /// Host has acknowledged the claim does not decide whether the session
    /// resolves - acknowledgement is not a gate (ADR-0016).
    pub fn resolve_session(&self, token_hash: &TokenHash) -> DbResult<Option<SessionBinding>> {
        self.db.read(|conn| {
            let row = conn
                .query_row(
                    "SELECT id, meeting_id, participant_id, approved_at
                       FROM participant_sessions
                      WHERE session_token_hash = ?1
                        AND revoked_at IS NULL",
                    params![token_hash.as_str()],
                    |row| {
                        Ok(SessionBinding {
                            session_id: row.get::<_, Sql<SessionId>>(0)?.into_inner(),
                            meeting_id: row.get::<_, Sql<MeetingId>>(1)?.into_inner(),
                            participant_id: row.get::<_, Sql<ParticipantId>>(2)?.into_inner(),
                            approved_at: row
                                .get::<_, Option<Sql<UtcTimestamp>>>(3)?
                                .map(Sql::into_inner),
                        })
                    },
                )
                .optional()?;
            Ok(row)
        })
    }
}
