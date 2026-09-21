//! The participant-facing routes.
//!
//! Three of them. Each does the same four things: establish the actor from a
//! credential, ask the domain or a participant query, map the result, return it.
//! No handler decides anything - not whether the meeting is open, not whether an
//! identity is free, not whether a session may act. Those are re-read inside the
//! domain's own transaction, every time (architecture rules section 15).
//!
//! ```text
//! GET  /api/join/{token}        join token    -> meeting + claimable identities
//! POST /api/join/{token}/claim  join token    -> session token, once
//! GET  /api/session             session token -> who am I
//! ```
//!
//! There is deliberately nothing else. A participant cannot read a note, an
//! audit entry, another roster or their own session row, because no route
//! exists that would return one - and an endpoint that does not exist cannot be
//! got wrong later.

use app_core::id::ParticipantId;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;

use crate::dto::{ClaimRequest, ClaimedView, IdentityView, JoinView, MeetingView, SessionView};
use crate::error::{ApiError, ApiResult};
use crate::extract::{JoinContext, Participant};
use crate::state::{participant_queries, LanState};
use crate::token;

/// The meeting behind a join token, and the identities on offer.
///
/// Requires only the join token: this is the screen someone sees *before* they
/// have an identity, so there is nothing to authenticate yet. What it returns is
/// correspondingly minimal - names and whether they are taken, and the meeting's
/// public facts (ADR-0016).
pub async fn join(
    State(state): State<LanState>,
    context: JoinContext,
) -> ApiResult<Json<JoinView>> {
    let meeting_id = context.meeting_id;

    let view = state
        .blocking(move |db| {
            let queries = participant_queries(db);
            // The meeting must still be there; the extractor resolved it a
            // moment ago, so absence here means it went away in between.
            let meeting = queries
                .joinable_meeting(meeting_id)?
                .ok_or_else(ApiError::not_found)?;
            let identities = queries.claimable_identities(meeting_id)?;
            Ok(JoinView {
                meeting: MeetingView::from(meeting),
                identities: identities.into_iter().map(IdentityView::from).collect(),
            })
        })
        .await?;

    Ok(Json(view))
}

/// Bind an identity to a new session.
///
/// The one response in the application that carries a session token, returned
/// once to the browser that earned it.
///
/// The claim itself is the domain's: first-claim-wins is enforced inside the
/// transaction and again by the partial unique index, so two browsers racing for
/// the same name cannot both win (ADR-0002). This handler does not check whether
/// the identity is free - it asks, and reports the answer.
pub async fn claim(
    State(state): State<LanState>,
    context: JoinContext,
    Json(request): Json<ClaimRequest>,
) -> ApiResult<(StatusCode, Json<ClaimedView>)> {
    // A target, not an identity claim. Authority came from the join token.
    let participant_id =
        ParticipantId::parse(&request.participant_id).map_err(|_| ApiError::invalid_request())?;

    let actor = context.claimant(participant_id);
    let meeting_id = context.meeting_id;

    // Minted here, in the transport. The domain receives only the hash and has
    // no way to persist the plaintext even if it wanted to (PRD 22.17).
    let credential = token::mint().map_err(|error| {
        eprintln!("[lan] {error}");
        ApiError::internal()
    })?;
    let hash = credential.hash.clone();

    let domain_state = state.clone();
    domain_state
        .blocking_domain(move |domain| {
            domain
                .claim_identity(&actor, meeting_id, participant_id, &hash)
                .map(|_| ())
                .map_err(ApiError::from)
        })
        .await?;

    // Read back what the participant may now see. A separate read rather than
    // assembling it from the request: the database is authoritative about the
    // identity that was just bound.
    let view = state
        .blocking(move |db| {
            let queries = participant_queries(db);
            let meeting = queries
                .joinable_meeting(meeting_id)?
                .ok_or_else(ApiError::not_found)?;
            let participant = queries
                .own_identity(meeting_id, participant_id)?
                .ok_or_else(ApiError::not_found)?;
            Ok((meeting, participant))
        })
        .await?;

    Ok((
        StatusCode::CREATED,
        Json(ClaimedView {
            session_token: credential.token,
            participant: view.1.into(),
            meeting: MeetingView::from(view.0),
        }),
    ))
}

/// Who the presented session token belongs to.
///
/// The reconnect route. It takes no parameters beyond the credential, which is
/// what makes an identity change impossible on this path: there is nothing to
/// pass that would select a different participant (ADR-0002 rule 10).
pub async fn session(
    State(state): State<LanState>,
    participant: Participant,
) -> ApiResult<Json<SessionView>> {
    let Participant {
        meeting_id,
        participant_id,
        acknowledged,
        ..
    } = participant;

    let view = state
        .blocking(move |db| {
            let queries = participant_queries(db);
            let meeting = queries
                .joinable_meeting(meeting_id)?
                .ok_or_else(ApiError::not_found)?;
            let own = queries
                .own_identity(meeting_id, participant_id)?
                .ok_or_else(ApiError::not_found)?;
            Ok(SessionView {
                participant: own.into(),
                meeting: MeetingView::from(meeting),
                acknowledged,
            })
        })
        .await?;

    Ok(Json(view))
}
