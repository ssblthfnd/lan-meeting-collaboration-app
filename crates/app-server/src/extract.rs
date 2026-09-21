//! Establishing who is asking.
//!
//! This is where architecture rules section 14.1 is actually implemented for the
//! LAN. Two extractors, one per credential, and between them they are the *only*
//! way a handler can obtain an `Actor`:
//!
//! | Extractor | Credential | Produces |
//! | --- | --- | --- |
//! | [`JoinContext`] | join token, from the URL path | the meeting, for a claimant |
//! | [`Participant`] | session token, from `Authorization` | [`Actor::Participant`] |
//!
//! # What makes this safe
//!
//! Neither extractor reads anything from a request body. [`Participant`] takes a
//! bearer token, hashes it, looks the hash up, and builds the actor **entirely
//! from the returned row** - so `meeting_id`, `participant_id` and `session_id`
//! all come from the database, and a browser that sends different values in a
//! payload changes nothing (rule 8).
//!
//! That is also what makes reconnection safe: there is no parameter on this path
//! through which an identity could be selected, so a reconnect cannot become an
//! identity change (rule 10).
//!
//! A revoked session resolves to nothing, because `revoked_at IS NULL` is part
//! of the query rather than a check someone could forget afterwards.

use app_core::actor::Actor;
use app_core::id::MeetingId;
use app_db::session_store::JoinTarget;
use axum::extract::{FromRequestParts, Path};
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;

use crate::error::ApiError;
use crate::state::{session_store, LanState};
use crate::token::hash_token;

/// A request carrying a valid join token.
///
/// Holds the meeting the token resolves to. It deliberately does **not** hold
/// an `Actor`: a join token alone does not name an identity, so the actor is
/// only formed once a handler knows which identity is being claimed, and it is
/// always an [`Actor::Claimant`] scoped to this meeting.
#[derive(Debug, Clone)]
pub struct JoinContext {
    pub meeting_id: MeetingId,
}

impl JoinContext {
    /// The actor for claiming `participant_id` in this meeting.
    ///
    /// The meeting comes from the resolved token; the identity comes from the
    /// request. That split is the honest one: possession of the token is the
    /// authority, and the identity is what is being asked for (ADR-0016).
    #[must_use]
    pub fn claimant(&self, participant_id: app_core::id::ParticipantId) -> Actor {
        Actor::Claimant {
            meeting_id: self.meeting_id,
            participant_id,
        }
    }
}

impl FromRequestParts<LanState> for JoinContext {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &LanState,
    ) -> Result<Self, Self::Rejection> {
        // The token is a path segment, extracted before anything else looks at
        // the request.
        let Path(token) = Path::<String>::from_request_parts(parts, state)
            .await
            .map_err(|_| ApiError::not_found())?;

        // Hashing is total: a hostile or malformed token produces a hash that
        // matches no row, so there is no parse step that behaves differently
        // for a nearly-valid value.
        let hash = hash_token(&token);
        let target: Option<JoinTarget> = state
            .blocking(move |db| Ok(session_store(db).resolve_join_token(&hash)?))
            .await?;

        // An unknown token is a 404 identical to an unknown route: nothing here
        // confirms whether a token was ever real.
        let target = target.ok_or_else(ApiError::not_found)?;

        // The meeting's *current* status decides joinability, re-read on every
        // request. This is why locking a meeting closes the join URL without
        // the stored hash being cleared: the lifecycle check is the mechanism,
        // not a second one that could disagree (architecture rules section 15).
        if target.status != app_core::meeting::MeetingStatus::Open.as_str() {
            return Err(ApiError::meeting_not_open());
        }

        Ok(Self {
            meeting_id: target.meeting_id,
        })
    }
}

/// A request carrying a live session token.
///
/// The `actor` inside is built from stored facts only.
#[derive(Debug, Clone)]
pub struct Participant {
    pub actor: Actor,
    pub meeting_id: MeetingId,
    pub participant_id: app_core::id::ParticipantId,
    /// Whether the Host has acknowledged the claim. Display only: no
    /// authorization decision reads it (ADR-0016).
    pub acknowledged: bool,
}

impl FromRequestParts<LanState> for Participant {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &LanState,
    ) -> Result<Self, Self::Rejection> {
        let token = bearer(parts).ok_or_else(ApiError::unauthenticated)?;

        let hash = hash_token(&token);
        let binding = state
            .blocking(move |db| Ok(session_store(db).resolve_session(&hash)?))
            .await?
            // Absent, malformed, or revoked: one answer for all three.
            .ok_or_else(ApiError::unauthenticated)?;

        // Every field comes from the row. There is no parameter on this path
        // through which a caller could name a different identity.
        Ok(Self {
            actor: Actor::Participant {
                meeting_id: binding.meeting_id,
                participant_id: binding.participant_id,
                session_id: binding.session_id,
            },
            meeting_id: binding.meeting_id,
            participant_id: binding.participant_id,
            acknowledged: binding.is_acknowledged(),
        })
    }
}

/// The token from an `Authorization: Bearer` header.
///
/// A header rather than a cookie: a bearer token is never sent automatically by
/// the browser, so there is no cross-site request forgery surface to reason
/// about on a transport that has no origin isolation to begin with (ADR-0016).
///
/// The scheme match is case-insensitive per RFC 7235; the token itself is not
/// touched, because it is about to be hashed.
fn bearer(parts: &Parts) -> Option<String> {
    let value = parts.headers.get(AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    Some(token.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;

    fn parts_with(header: Option<&str>) -> Parts {
        let mut builder = Request::builder().uri("/api/session");
        if let Some(value) = header {
            builder = builder.header(AUTHORIZATION, value);
        }
        builder.body(()).expect("request").into_parts().0
    }

    #[test]
    fn a_bearer_token_is_read_case_insensitively() {
        for header in ["Bearer abc123", "bearer abc123", "BEARER abc123"] {
            assert_eq!(bearer(&parts_with(Some(header))).as_deref(), Some("abc123"));
        }
    }

    #[test]
    fn anything_that_is_not_a_bearer_token_yields_nothing() {
        for header in [
            None,
            Some(""),
            Some("abc123"),
            Some("Basic dXNlcjpwYXNz"),
            Some("Bearer"),
            Some("Bearer "),
            Some("Bearer    "),
        ] {
            assert_eq!(bearer(&parts_with(header)), None, "{header:?}");
        }
    }

    #[test]
    fn a_claimant_is_scoped_to_the_meeting_the_token_resolved_to() {
        // The identity is requested; the meeting is not. A claimant cannot name
        // a meeting other than the one its join token belongs to.
        let meeting_id = MeetingId::new();
        let participant_id = app_core::id::ParticipantId::new();
        let context = JoinContext { meeting_id };

        assert_eq!(
            context.claimant(participant_id),
            Actor::Claimant {
                meeting_id,
                participant_id,
            }
        );
    }
}
