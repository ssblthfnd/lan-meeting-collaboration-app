//! Shapes crossing the LAN boundary.
//!
//! The counterpart of `packages/contracts/src/lan.ts`, and deliberately not of
//! `host.ts`: a participant's view is its own contract (ADR-0014).
//!
//! # Everything here is a subset
//!
//! Compare any type below with its Host equivalent and the difference is what a
//! participant is not told. There is no participant count, no audit trail, no
//! created/updated timestamp, no lock time, no token, no session id, and - until
//! the claim succeeds - no department, position or role.
//!
//! That is enforced by the absence of fields rather than by filtering. A future
//! change that wanted to leak one would have to add it here *and* to a query
//! *and* break a test, instead of forgetting a `WHERE`.

use app_core::id::{MeetingId, ParticipantId};
use app_core::meeting::MeetingStatus;
use app_db::participant_query::{ClaimableIdentity, JoinableMeeting, OwnIdentity};
use serde::{Deserialize, Serialize};

/* -------------------------------------------------------------------------
 * Requests
 * ------------------------------------------------------------------------- */

/// Which identity the caller wishes to claim.
///
/// `participant_id` is a **target**, not a statement of who is asking: the
/// authority to claim comes from the join token in the URL, and who wins a race
/// for the identity is decided by the database (ADR-0002, ADR-0016). It is the
/// same distinction `WriteNote.participant_id` already carries.
#[derive(Debug, Clone, Deserialize)]
pub struct ClaimRequest {
    pub participant_id: String,
}

/* -------------------------------------------------------------------------
 * Responses
 * ------------------------------------------------------------------------- */

/// The meeting, as a joining participant may see it.
#[derive(Debug, Clone, Serialize)]
pub struct MeetingView {
    pub id: MeetingId,
    pub title: String,
    pub topic: Option<String>,
    /// Zoneless `YYYY-MM-DD`, meaningless without `timezone` beside it.
    pub date: String,
    pub start_time: String,
    pub end_time: String,
    /// Always sent with the schedule, so a phone in another timezone still
    /// shows the meeting's own hours (PRD 25.3).
    pub timezone: String,
    pub location: Option<String>,
    pub status: MeetingStatus,
}

impl From<JoinableMeeting> for MeetingView {
    fn from(row: JoinableMeeting) -> Self {
        Self {
            id: row.id,
            title: row.title,
            topic: row.topic,
            date: row.date.to_storage(),
            start_time: row.start_time.to_storage(),
            end_time: row.end_time.to_storage(),
            timezone: row.timezone,
            location: row.location,
            status: row.status,
        }
    }
}

/// One identity on the join screen.
///
/// A name and whether it is taken. `claimed` is a boolean rather than the
/// four-state claim status: whether the Host has acknowledged someone else's
/// claim is the Host's business, and a participant only needs to know which
/// names are still free.
#[derive(Debug, Clone, Serialize)]
pub struct IdentityView {
    pub id: ParticipantId,
    pub name: String,
    pub claimed: bool,
}

impl From<ClaimableIdentity> for IdentityView {
    fn from(row: ClaimableIdentity) -> Self {
        Self {
            id: row.id,
            name: row.name,
            // Pending and acknowledged are equally held (ADR-0016).
            claimed: row.claim_status.is_held(),
        }
    }
}

/// What `GET /api/join/{token}` returns.
#[derive(Debug, Clone, Serialize)]
pub struct JoinView {
    pub meeting: MeetingView,
    pub identities: Vec<IdentityView>,
}

/// The participant's own identity, after claiming.
#[derive(Debug, Clone, Serialize)]
pub struct OwnIdentityView {
    pub id: ParticipantId,
    pub name: String,
    pub department: Option<String>,
    pub position: Option<String>,
    pub meeting_role: Option<String>,
}

impl From<OwnIdentity> for OwnIdentityView {
    fn from(row: OwnIdentity) -> Self {
        Self {
            id: row.id,
            name: row.details.name,
            department: row.details.department,
            position: row.details.position,
            meeting_role: row.details.meeting_role,
        }
    }
}

/// What a successful claim returns.
///
/// The **only** response in the application that carries a session token. It is
/// returned once, to the browser that earned it, and is never stored in plain
/// text anywhere (PRD 22.17).
#[derive(Debug, Clone, Serialize)]
pub struct ClaimedView {
    pub session_token: String,
    pub participant: OwnIdentityView,
    pub meeting: MeetingView,
}

/// What `GET /api/session` returns.
///
/// Deliberately carries no session token: reconnecting proves you already have
/// one. It also carries no session id - there is nothing a participant can do
/// with it, and an identifier that serves no purpose is one more thing that can
/// be logged by a browser extension.
#[derive(Debug, Clone, Serialize)]
pub struct SessionView {
    pub participant: OwnIdentityView,
    pub meeting: MeetingView,
    /// Whether the Host has acknowledged the claim.
    ///
    /// Display only. A participant with `acknowledged: false` can do everything
    /// a participant with `true` can (ADR-0016), and the UI must not gate on it.
    pub acknowledged: bool,
}
