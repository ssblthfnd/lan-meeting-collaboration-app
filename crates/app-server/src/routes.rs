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
//! GET  /api/note                session token -> my note
//! PUT  /api/note                session token -> replace my note
//! ```
//!
//! There is deliberately nothing else. A participant cannot read an audit
//! entry, another roster, another participant's note or their own session row,
//! because no route exists that would return one - and an endpoint that does
//! not exist cannot be got wrong later.
//!
//! # The note routes carry no identifiers
//!
//! `/api/note` takes no path segment and its body has one field. The meeting
//! and the participant come from the resolved session, which makes addressing
//! somebody else's note **inexpressible** rather than merely checked: a check
//! can be forgotten by a future handler, and a parameter that does not exist
//! cannot be. `/api/session` already works this way and says so; these follow
//! it (ADR-0020).

use app_core::id::ParticipantId;
use app_core::service::WriteNote;
use axum::extract::rejection::{BytesRejection, FailedToBufferBody, JsonRejection};
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;

use crate::dto::{
    ClaimRequest, ClaimedView, IdentityView, JoinView, MeetingView, NoteView, SessionView,
    WriteNoteRequest,
};
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

/* -------------------------------------------------------------------------
 * The participant's own note
 * ------------------------------------------------------------------------- */

/// The participant's own note, or `null` if they have not written one.
///
/// Available while the meeting is `LOCKED`. A locked meeting is finished, not
/// secret: the notes are what it was locked to keep, and the participant wrote
/// this one (ADR-0020).
pub async fn read_note(
    State(state): State<LanState>,
    participant: Participant,
) -> ApiResult<Json<Option<NoteView>>> {
    let Participant {
        meeting_id,
        participant_id,
        ..
    } = participant;

    let note = state
        .blocking(move |db| Ok(participant_queries(db).own_note(meeting_id, participant_id)?))
        .await?;

    Ok(Json(note.map(NoteView::from)))
}

/// Create or replace the participant's own note.
///
/// Every decision this handler could get wrong is made somewhere else. It does
/// not check the lock, does not validate the Markdown, does not choose the
/// version and does not decide whether the caller may write: all of that is
/// re-read inside the domain's own transaction, which is the only place it can
/// be done without a race (architecture rules section 15).
///
/// What it does do is the one thing a transport must: establish who is asking.
/// `meeting_id` and `participant_id` come from the resolved session, never from
/// the request. The `participant_id` handed to [`WriteNote`] is a *target*, and
/// `authorize` refuses it unless it equals the actor's own.
pub async fn write_note(
    State(state): State<LanState>,
    participant: Participant,
    body: Result<Json<WriteNoteRequest>, JsonRejection>,
) -> ApiResult<Json<NoteView>> {
    let Json(request) = body.map_err(json_refused)?;

    let Participant {
        actor,
        meeting_id,
        participant_id,
        ..
    } = participant;

    // A transport concern, not a content one: a browser can submit CRLF, the
    // domain refuses a carriage return as a control character, and a
    // participant should not be shown an error about a character they cannot
    // see. Nothing else about the content is touched - the note is stored
    // exactly as typed (ADR-0019).
    let content = request.content.replace("\r\n", "\n").replace('\r', "\n");

    state
        .blocking_domain(move |domain| {
            domain
                .write_note(
                    &actor,
                    WriteNote {
                        meeting_id,
                        participant_id,
                        content,
                    },
                )
                .map(|_| ())
                .map_err(ApiError::from)
        })
        .await?;

    // Read back rather than assembling a response from the request: the
    // database decided the version, and it is authoritative about what the
    // note now says.
    let note = state
        .blocking(move |db| Ok(participant_queries(db).own_note(meeting_id, participant_id)?))
        .await?
        .ok_or_else(ApiError::internal)?;

    Ok(Json(NoteView::from(note)))
}

/// Translate a rejected body into the participant error contract.
///
/// Axum's own rejection is plain text and names types from this crate. One
/// answer is worth telling apart from the rest: the route declining to read a
/// body because it was longer than the limit that route carries. That is a
/// different fact from every other way a body can fail, and it is the only one
/// that earns a 413 (ADR-0020).
///
/// The distinction is the rejection's own, not a guess from the outer type.
/// `BytesRejection` covers *any* failure to buffer the body - a connection that
/// died mid-read is one, and it is not a size problem. Axum has already walked
/// the error's source chain and downcast it to
/// `http_body_util::LengthLimitError`; the result of that walk is exactly the
/// [`FailedToBufferBody::LengthLimitError`] variant, and matching on it is
/// therefore asking the source chain rather than assuming on its behalf.
///
/// | Rejection | Answer |
/// | --- | --- |
/// | length limit exceeded | 413 `payload_too_large` |
/// | body read failed for any other reason | 400 `invalid_request` |
/// | malformed or wrongly shaped JSON | 400 `invalid_request` |
///
/// The second row keeps axum's own status for `UnknownBodyError`, which is a
/// bad request; a participant is told the same thing as for unreadable JSON,
/// because from where they stand it is the same thing - the request did not
/// arrive intact and sending it again is the action either way.
fn json_refused(rejection: JsonRejection) -> ApiError {
    match rejection {
        JsonRejection::BytesRejection(BytesRejection::FailedToBufferBody(
            FailedToBufferBody::LengthLimitError(_),
        )) => ApiError::payload_too_large(),
        _ => ApiError::invalid_request(),
    }
}
