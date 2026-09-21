//! The Host error contract.
//!
//! ADR-0012 left this open on purpose: `DomainError` carries no transport
//! vocabulary, and "mapping a domain error onto a Tauri error payload is the
//! transport's job". This module is that job. See ADR-0015.
//!
//! Three properties the UI depends on:
//!
//! 1. **Structured, not stringified.** A `Result<_, HostError>` command rejects
//!    with a JSON object, so the UI branches on data rather than parsing a Rust
//!    `Debug` string that could change with any refactor.
//! 2. **Two levels of precision.** `kind` is the exact refusal; `category` is the
//!    coarse grouping that decides *how* to present it. A view can handle five
//!    categories without enumerating ten kinds, and a view that wants to be
//!    specific still can.
//! 3. **No leaked internals.** A `Conflict` or a `Persistence` failure carries
//!    diagnostic text from SQLite. That text is written to stderr for the Host's
//!    own logs and is *not* serialized: architecture rules section 22 asks for
//!    actionable errors, not for constraint strings in a dialog.
//!
//! There are no HTTP status codes here. This transport has no HTTP.

use app_core::error::DomainError;
use app_core::id::{MeetingId, ParticipantId};
use app_core::meeting::MeetingStatus;
use serde::Serialize;

/// Result of every Host command.
pub type HostResult<T> = Result<T, HostError>;

/// How the UI should treat a refusal, independent of its exact kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCategory {
    /// The actor may not do this. Never worth retrying.
    Authorization,
    /// The thing asked for does not exist.
    NotFound,
    /// The meeting's status forbids it.
    Lifecycle,
    /// The input was refused; `field` names what to correct.
    Validation,
    /// A concurrent change won. Re-reading and retrying may succeed.
    Conflict,
    /// A failure the backend could not interpret.
    Unexpected,
}

/// The precise refusal, with the facts that kind carries.
///
/// Internally tagged, so the serialized form is a discriminated union keyed on
/// `kind` - which is what makes the TypeScript side able to narrow to exactly
/// the fields that are present.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HostErrorKind {
    Unauthorized {
        meeting_id: MeetingId,
    },
    Forbidden {
        actor_type: &'static str,
        action: &'static str,
        target: &'static str,
    },
    MeetingNotFound {
        meeting_id: MeetingId,
    },
    MeetingLocked {
        meeting_id: MeetingId,
        title: String,
    },
    MeetingNotDraft {
        meeting_id: MeetingId,
        detected: MeetingStatus,
    },
    MeetingNotOpen {
        meeting_id: MeetingId,
        detected: MeetingStatus,
    },
    /// A live session already holds the identity (ADR-0002).
    ///
    /// Carries the participant so the Host UI can point at the right row: they
    /// are looking at their own roster, and the useful next action is to revoke
    /// the session holding it.
    IdentityAlreadyClaimed {
        meeting_id: MeetingId,
        participant_id: ParticipantId,
    },
    SessionNotFound {
        meeting_id: MeetingId,
    },
    ParticipantNotFound {
        meeting_id: MeetingId,
        participant_id: ParticipantId,
    },
    InvalidTransition {
        expected: &'static str,
        detected: MeetingStatus,
    },
    Validation {
        field: String,
        expected: String,
        detected: String,
    },
    /// Deliberately field-less: the underlying detail is a SQLite constraint
    /// message, which is a diagnostic and not something to show a user.
    Conflict,
    /// Field-less for the same reason.
    Persistence,
}

impl HostErrorKind {
    /// The category this kind belongs to.
    #[must_use]
    pub fn category(&self) -> ErrorCategory {
        match self {
            HostErrorKind::Unauthorized { .. } | HostErrorKind::Forbidden { .. } => {
                ErrorCategory::Authorization
            }
            HostErrorKind::MeetingNotFound { .. }
            | HostErrorKind::ParticipantNotFound { .. }
            | HostErrorKind::SessionNotFound { .. } => ErrorCategory::NotFound,
            HostErrorKind::MeetingLocked { .. }
            | HostErrorKind::MeetingNotDraft { .. }
            | HostErrorKind::MeetingNotOpen { .. }
            | HostErrorKind::InvalidTransition { .. } => ErrorCategory::Lifecycle,
            HostErrorKind::Validation { .. } => ErrorCategory::Validation,
            // Someone else holds the identity. A conflict rather than a
            // refusal: the Host can resolve it by revoking that session.
            HostErrorKind::IdentityAlreadyClaimed { .. } | HostErrorKind::Conflict => {
                ErrorCategory::Conflict
            }
            HostErrorKind::Persistence => ErrorCategory::Unexpected,
        }
    }
}

/// What a failed Host command rejects with.
///
/// `message` is the backend's own sentence, which names the expected and the
/// detected value wherever that information exists. The UI is expected to show
/// it rather than compose its own text from the fields, so that improving an
/// error message does not mean editing two codebases.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostError {
    pub category: ErrorCategory,
    pub message: String,
    /// Flattened, so `kind` and its fields sit alongside `category` and
    /// `message` in one object rather than nested under a key.
    #[serde(flatten)]
    pub kind: HostErrorKind,
}

impl HostError {
    /// Build from a kind, taking the category from it.
    #[must_use]
    pub fn new(kind: HostErrorKind, message: String) -> Self {
        Self {
            category: kind.category(),
            message,
            kind,
        }
    }

    /// A refusal of input this layer parsed before the domain could see it.
    ///
    /// Used for an identifier or a date that is not even the right shape. The
    /// shape matches [`HostErrorKind::Validation`] exactly, so the UI handles a
    /// malformed id and a domain-refused value through one path.
    #[must_use]
    pub fn validation(field: &str, expected: &str, detected: &str) -> Self {
        let message = format!("invalid {field}: expected {expected}, detected {detected}");
        Self::new(
            HostErrorKind::Validation {
                field: field.to_owned(),
                expected: expected.to_owned(),
                detected: detected.to_owned(),
            },
            message,
        )
    }
}

impl From<DomainError> for HostError {
    fn from(error: DomainError) -> Self {
        // Taken from `Display` before the error is consumed, so the UI shows the
        // domain's own wording rather than a paraphrase maintained here.
        let message = error.to_string();

        let kind = match error {
            DomainError::Unauthorized { meeting_id } => HostErrorKind::Unauthorized { meeting_id },
            DomainError::Forbidden {
                actor_type,
                action,
                target,
            } => HostErrorKind::Forbidden {
                actor_type,
                action,
                target,
            },
            DomainError::MeetingNotFound { meeting_id } => {
                HostErrorKind::MeetingNotFound { meeting_id }
            }
            DomainError::MeetingLocked { meeting_id, title } => {
                HostErrorKind::MeetingLocked { meeting_id, title }
            }
            DomainError::MeetingNotDraft {
                meeting_id,
                detected,
            } => HostErrorKind::MeetingNotDraft {
                meeting_id,
                detected,
            },
            DomainError::MeetingNotOpen {
                meeting_id,
                detected,
            } => HostErrorKind::MeetingNotOpen {
                meeting_id,
                detected,
            },
            DomainError::IdentityAlreadyClaimed {
                meeting_id,
                participant_id,
            } => HostErrorKind::IdentityAlreadyClaimed {
                meeting_id,
                participant_id,
            },
            DomainError::SessionNotFound { meeting_id } => {
                HostErrorKind::SessionNotFound { meeting_id }
            }
            DomainError::ParticipantNotFound {
                meeting_id,
                participant_id,
            } => HostErrorKind::ParticipantNotFound {
                meeting_id,
                participant_id,
            },
            DomainError::InvalidTransition { expected, detected } => {
                HostErrorKind::InvalidTransition { expected, detected }
            }
            DomainError::Validation {
                field,
                expected,
                detected,
            } => HostErrorKind::Validation {
                field: field.to_owned(),
                expected: expected.to_owned(),
                detected,
            },

            // The two that carry diagnostics. The detail stays on the Host
            // machine; the UI is told what happened, not how SQLite phrased it.
            DomainError::Conflict { detail } => {
                diagnostic("a concurrent change won a race", &detail);
                return Self::new(
                    HostErrorKind::Conflict,
                    "Another change was saved first. Reload and try again.".to_owned(),
                );
            }
            DomainError::Persistence { operation, detail } => {
                diagnostic(operation, &detail);
                return Self::new(
                    HostErrorKind::Persistence,
                    format!("The database could not complete {operation}."),
                );
            }
        };

        Self::new(kind, message)
    }
}

/// Record a diagnostic the UI will not be shown.
///
/// stderr, not a file and not a service: the Host machine's own console is
/// where this belongs, and nothing here may leave the device (PRD section 24).
fn diagnostic(context: &str, detail: &str) {
    eprintln!("[host] {context}: {detail}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn as_json(error: HostError) -> Value {
        serde_json::to_value(error).expect("HostError must serialize")
    }

    #[test]
    fn every_domain_error_maps_to_its_own_kind_and_category() {
        let meeting_id = MeetingId::new();
        let participant_id = ParticipantId::new();

        let cases: Vec<(DomainError, &str, &str)> = vec![
            (
                DomainError::Unauthorized { meeting_id },
                "unauthorized",
                "authorization",
            ),
            (
                DomainError::Forbidden {
                    actor_type: "PARTICIPANT",
                    action: "add a participant",
                    target: "the participant roster",
                },
                "forbidden",
                "authorization",
            ),
            (
                DomainError::MeetingNotFound { meeting_id },
                "meeting_not_found",
                "not_found",
            ),
            (
                DomainError::MeetingLocked {
                    meeting_id,
                    title: "Weekly Coordination".to_owned(),
                },
                "meeting_locked",
                "lifecycle",
            ),
            (
                DomainError::MeetingNotDraft {
                    meeting_id,
                    detected: MeetingStatus::Open,
                },
                "meeting_not_draft",
                "lifecycle",
            ),
            (
                DomainError::MeetingNotOpen {
                    meeting_id,
                    detected: MeetingStatus::Draft,
                },
                "meeting_not_open",
                "lifecycle",
            ),
            (
                DomainError::IdentityAlreadyClaimed {
                    meeting_id,
                    participant_id,
                },
                "identity_already_claimed",
                "conflict",
            ),
            (
                DomainError::SessionNotFound { meeting_id },
                "session_not_found",
                "not_found",
            ),
            (
                DomainError::ParticipantNotFound {
                    meeting_id,
                    participant_id,
                },
                "participant_not_found",
                "not_found",
            ),
            (
                DomainError::InvalidTransition {
                    expected: "OPEN",
                    detected: MeetingStatus::Draft,
                },
                "invalid_transition",
                "lifecycle",
            ),
            (
                DomainError::Validation {
                    field: "meeting title",
                    expected: "at least one non-whitespace character",
                    detected: "only whitespace".to_owned(),
                },
                "validation",
                "validation",
            ),
            (
                DomainError::Conflict {
                    detail: "UNIQUE constraint failed: note_versions.version".to_owned(),
                },
                "conflict",
                "conflict",
            ),
            (
                DomainError::Persistence {
                    operation: "adding a participant",
                    detail: "disk I/O error".to_owned(),
                },
                "persistence",
                "unexpected",
            ),
        ];

        // Every variant is covered: if `DomainError` gains one, this count fails
        // and the mapping has to be decided rather than defaulted.
        assert_eq!(cases.len(), 13);

        for (domain, kind, category) in cases {
            let json = as_json(HostError::from(domain));
            assert_eq!(json["kind"], kind);
            assert_eq!(json["category"], category);
            assert!(
                json["message"].as_str().is_some_and(|m| !m.is_empty()),
                "{kind} must carry a message"
            );
        }
    }

    #[test]
    fn a_refusal_carries_the_domains_own_actionable_message() {
        let error = HostError::from(DomainError::Validation {
            field: "participant count",
            expected: "at most 99 participants per meeting",
            detected: "the meeting already has 99".to_owned(),
        });

        // Names the expected and the detected value (architecture rules 22).
        assert!(error.message.contains("at most 99"), "{}", error.message);
        assert!(
            error.message.contains("already has 99"),
            "{}",
            error.message
        );

        // And the field is available separately, for attaching to a form input.
        let json = as_json(error);
        assert_eq!(json["field"], "participant count");
    }

    #[test]
    fn sqlite_diagnostics_are_never_serialized() {
        let leak = "UNIQUE constraint failed: participants.id";
        let conflict = as_json(HostError::from(DomainError::Conflict {
            detail: leak.to_owned(),
        }));
        assert!(
            !conflict.to_string().contains("UNIQUE constraint"),
            "leaked: {conflict}"
        );
        // Still actionable: it says what to do about it.
        assert!(conflict["message"]
            .as_str()
            .expect("message")
            .contains("Reload"));

        let persistence = as_json(HostError::from(DomainError::Persistence {
            operation: "creating a meeting",
            detail: "attempt to write a readonly database".to_owned(),
        }));
        assert!(
            !persistence.to_string().contains("readonly database"),
            "leaked: {persistence}"
        );
        assert!(persistence["message"]
            .as_str()
            .expect("message")
            .contains("creating a meeting"));
    }

    #[test]
    fn the_serialized_shape_is_one_flat_object() {
        // The TypeScript contract is a discriminated union on `kind`, which
        // requires the kind's own fields to sit beside `category` and `message`.
        let meeting_id = MeetingId::new();
        let json = as_json(HostError::from(DomainError::MeetingNotDraft {
            meeting_id,
            detected: MeetingStatus::Open,
        }));

        assert_eq!(
            json,
            json!({
                "kind": "meeting_not_draft",
                "category": "lifecycle",
                "message": format!(
                    "meeting {meeting_id} is no longer being prepared: \
                     expected status DRAFT, detected OPEN"
                ),
                "meeting_id": meeting_id.to_storage(),
                "detected": "OPEN",
            })
        );
    }

    #[test]
    fn a_malformed_identifier_is_refused_as_ordinary_validation() {
        // So the UI needs one error path, not a separate one for "the id the
        // frontend sent was not even a uuid".
        let json = as_json(HostError::validation(
            "meeting id",
            "a 36-character UUID version 7",
            "not-a-uuid",
        ));
        assert_eq!(json["kind"], "validation");
        assert_eq!(json["category"], "validation");
        assert_eq!(json["field"], "meeting id");
        assert_eq!(json["detected"], "not-a-uuid");
    }

    #[test]
    fn no_http_status_codes_appear_in_the_contract() {
        // This transport has no HTTP. A status code here would be a category
        // error that the LAN transport would then be tempted to copy.
        let json = as_json(HostError::from(DomainError::MeetingNotFound {
            meeting_id: MeetingId::new(),
        }));
        let object = json.as_object().expect("object");
        for key in ["status", "status_code", "http_status", "code"] {
            assert!(!object.contains_key(key), "{key} must not be present");
        }
    }
}
