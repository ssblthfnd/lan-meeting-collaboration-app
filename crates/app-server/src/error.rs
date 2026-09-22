//! The participant error contract.
//!
//! Shares a *vocabulary* with the Host's error contract (ADR-0015) and shares no
//! *types* with it, for the reason ADR-0014 gives: the Host and a participant are
//! different audiences, and a type reused across both is how something meant for
//! one ends up in the other's hands.
//!
//! Three differences from `HostError`, each deliberate:
//!
//! | | Host | Participant |
//! | --- | --- | --- |
//! | HTTP status | absent - Tauri has no HTTP | present; this transport *is* HTTP |
//! | Message | the domain's own, naming ids | written for a participant, naming nothing |
//! | Identifiers | included | **never** |
//!
//! # Why no identifiers
//!
//! A refusal that echoes a meeting or participant id tells whoever provoked it
//! that the id is real. Someone holding only a join URL should learn nothing
//! from a rejection beyond the fact that it was rejected - so an unknown join
//! token and an unknown route return the same 404, and a refused claim does not
//! repeat back the id it refused.

use app_core::error::DomainError;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

pub type ApiResult<T> = Result<T, ApiError>;

/// What a failed LAN request returns.
///
/// `kind` and `category` use the same words as the Host contract, so a reader of
/// both sees one vocabulary; the values a participant may be told are a strict
/// subset of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApiError {
    pub kind: &'static str,
    pub category: &'static str,
    /// Written for a participant. Never the domain's own wording, which names
    /// internal ids and is aimed at the Host.
    pub message: &'static str,
    #[serde(skip)]
    status: StatusCode,
}

impl ApiError {
    fn new(
        status: StatusCode,
        kind: &'static str,
        category: &'static str,
        message: &'static str,
    ) -> Self {
        Self {
            kind,
            category,
            message,
            status,
        }
    }

    /// The HTTP status this refusal carries.
    #[must_use]
    pub fn status(&self) -> StatusCode {
        self.status
    }

    /// No meeting answers to this URL.
    ///
    /// Returned for an unknown join token, a malformed one, and a token whose
    /// meeting has been deleted - and it is byte-identical to the response for
    /// an unknown route. A participant cannot use this endpoint to discover
    /// whether a token they hold was ever real.
    #[must_use]
    pub fn not_found() -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "not_found",
            "This link is not valid. Ask the meeting host for a new one.",
        )
    }

    /// The meeting exists but is not accepting participants.
    #[must_use]
    pub fn meeting_not_open() -> Self {
        Self::new(
            StatusCode::CONFLICT,
            "meeting_not_open",
            "lifecycle",
            "This meeting is not open for joining right now.",
        )
    }

    /// Someone else is already using this identity (ADR-0002).
    #[must_use]
    pub fn identity_already_claimed() -> Self {
        Self::new(
            StatusCode::CONFLICT,
            "identity_already_claimed",
            "conflict",
            "That name is already in use on another device. \
             Pick another, or ask the host to release it.",
        )
    }

    /// The credential presented is absent, malformed or no longer live.
    ///
    /// One answer for all three. A revoked session and a token that never
    /// existed are the same fact to whoever is holding it.
    #[must_use]
    pub fn unauthenticated() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
            "authorization",
            "You are not signed in to this meeting. Open the join link again.",
        )
    }

    /// The request was understood and refused.
    #[must_use]
    pub fn forbidden() -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "authorization",
            "You are not allowed to do that.",
        )
    }

    /// The request body or a path parameter was not the right shape.
    #[must_use]
    pub fn invalid_request() -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "validation",
            "That request was not understood.",
        )
    }

    /// The request body was larger than the route will read.
    ///
    /// Distinct from a validation refusal on purpose. A note over 64 KiB is
    /// refused by the domain with a message naming the limit and the measured
    /// size; this is the transport declining to read a body at all, which is a
    /// different fact and a different status code (ADR-0020).
    #[must_use]
    pub fn payload_too_large() -> Self {
        Self::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload_too_large",
            "validation",
            "That note is too large to send. Shorten it and try again.",
        )
    }

    /// Something failed that the participant cannot act on.
    #[must_use]
    pub fn internal() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "unexpected",
            "Something went wrong on the host's computer. Please try again.",
        )
    }
}

impl From<DomainError> for ApiError {
    /// Map a domain refusal onto what a participant may be told.
    ///
    /// Several domain variants collapse into one answer here. That is the point:
    /// the domain distinguishes them because the Host needs them distinguished,
    /// and a participant does not need - or get - the same resolution.
    fn from(error: DomainError) -> Self {
        match error {
            DomainError::IdentityAlreadyClaimed { .. } => Self::identity_already_claimed(),

            DomainError::MeetingNotOpen { .. }
            | DomainError::MeetingLocked { .. }
            | DomainError::MeetingNotDraft { .. }
            | DomainError::InvalidTransition { .. } => Self::meeting_not_open(),

            // A participant asking about a meeting or identity that is not
            // there learns only that this link does not work - the same answer
            // as a bad token, so neither can be used to probe for the other.
            DomainError::MeetingNotFound { .. }
            | DomainError::ParticipantNotFound { .. }
            | DomainError::SessionNotFound { .. } => Self::not_found(),

            DomainError::Unauthorized { .. } | DomainError::Forbidden { .. } => Self::forbidden(),

            DomainError::Validation { .. } => Self::invalid_request(),

            // The diagnostic stays on the Host machine, exactly as it does for
            // the Host's own error contract (ADR-0015).
            DomainError::Conflict { detail } => {
                diagnostic("a concurrent change won a race", &detail);
                Self::identity_already_claimed()
            }
            DomainError::Persistence { operation, detail } => {
                diagnostic(operation, &detail);
                Self::internal()
            }
        }
    }
}

impl From<app_db::DbError> for ApiError {
    fn from(error: app_db::DbError) -> Self {
        diagnostic("a read query failed", &error.to_string());
        Self::internal()
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status;
        (
            status,
            // A refusal must never be cached: a 404 for a token that is later
            // issued, or a 401 before signing in, would otherwise stick.
            [(header::CACHE_CONTROL, "no-store")],
            Json(self),
        )
            .into_response()
    }
}

/// Record a diagnostic the participant will not be shown.
///
/// The Host's own console. Nothing leaves the device (PRD section 24).
fn diagnostic(context: &str, detail: &str) {
    eprintln!("[lan] {context}: {detail}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_core::id::{MeetingId, ParticipantId};
    use app_core::meeting::MeetingStatus;

    fn body_of(error: &ApiError) -> String {
        serde_json::to_string(error).expect("serializes")
    }

    #[test]
    fn a_refusal_never_carries_an_identifier() {
        // The property that stops this endpoint being an oracle. Every domain
        // variant that carries ids must lose them on the way out.
        let meeting_id = MeetingId::new();
        let participant_id = ParticipantId::new();

        let errors = [
            ApiError::from(DomainError::MeetingNotFound { meeting_id }),
            ApiError::from(DomainError::ParticipantNotFound {
                meeting_id,
                participant_id,
            }),
            ApiError::from(DomainError::SessionNotFound { meeting_id }),
            ApiError::from(DomainError::IdentityAlreadyClaimed {
                meeting_id,
                participant_id,
            }),
            ApiError::from(DomainError::MeetingNotOpen {
                meeting_id,
                detected: MeetingStatus::Draft,
            }),
            ApiError::from(DomainError::Unauthorized { meeting_id }),
        ];

        for error in &errors {
            let body = body_of(error);
            assert!(
                !body.contains(&meeting_id.to_storage()),
                "leaked a meeting id: {body}"
            );
            assert!(
                !body.contains(&participant_id.to_storage()),
                "leaked a participant id: {body}"
            );
        }
    }

    #[test]
    fn an_unknown_meeting_is_indistinguishable_from_an_unknown_link() {
        // Someone holding a stale or guessed token learns nothing from the
        // difference between "no such meeting" and "no such token".
        let unknown_link = ApiError::not_found();
        let unknown_meeting = ApiError::from(DomainError::MeetingNotFound {
            meeting_id: MeetingId::new(),
        });

        assert_eq!(unknown_link, unknown_meeting);
        assert_eq!(body_of(&unknown_link), body_of(&unknown_meeting));
        assert_eq!(unknown_link.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn sqlite_diagnostics_never_reach_a_participant() {
        let error = ApiError::from(DomainError::Persistence {
            operation: "claiming an identity",
            detail: "attempt to write a readonly database".to_owned(),
        });
        let body = body_of(&error);
        assert!(!body.contains("readonly"), "leaked: {body}");
        assert!(!body.contains("claiming an identity"), "leaked: {body}");
        assert_eq!(error.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn statuses_match_the_kind_of_refusal() {
        assert_eq!(ApiError::not_found().status(), StatusCode::NOT_FOUND);
        assert_eq!(
            ApiError::unauthenticated().status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(ApiError::forbidden().status(), StatusCode::FORBIDDEN);
        assert_eq!(
            ApiError::invalid_request().status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            ApiError::identity_already_claimed().status(),
            StatusCode::CONFLICT
        );
        assert_eq!(ApiError::meeting_not_open().status(), StatusCode::CONFLICT);
        assert_eq!(
            ApiError::payload_too_large().status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
    }

    #[test]
    fn an_oversized_body_and_an_oversized_note_are_different_answers() {
        // The transport declining to read a body is not the domain refusing a
        // note. A participant who sent 300 KiB and one who sent 70 KiB have
        // different problems, and only the second can be told the exact limit
        // (ADR-0020).
        let transport = ApiError::payload_too_large();
        let domain = ApiError::from(DomainError::Validation {
            field: "note content",
            expected: "at most 65536 bytes of content",
            detected: "70000 bytes".to_owned(),
        });

        assert_ne!(transport, domain);
        assert_eq!(transport.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(domain.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn a_lost_race_and_a_second_attempt_look_the_same() {
        // One is caught by the in-transaction read, the other by the partial
        // unique index. A participant is told the same thing either way.
        let refused = ApiError::from(DomainError::IdentityAlreadyClaimed {
            meeting_id: MeetingId::new(),
            participant_id: ParticipantId::new(),
        });
        let raced = ApiError::from(DomainError::Conflict {
            detail: "UNIQUE constraint failed: idx_sessions_one_live_per_participant".to_owned(),
        });

        assert_eq!(refused, raced);
        assert!(!body_of(&raced).contains("UNIQUE"), "leaked a constraint");
    }
}
