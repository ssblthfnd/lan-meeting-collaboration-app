//! Concrete, type-safe entity identifiers.
//!
//! Replaces the Phase 1 placeholder `pub type Id = String`. Identifiers are
//! **UUID version 7** (RFC 9562): 48 bits of Unix-millisecond timestamp
//! followed by random bits, so an id is time-ordered as well as unique.
//!
//! That property is load-bearing here. Architecture rules section 26.4 requires
//! every query feeding the UI or an export to have a *total* ordering, and the
//! usual tiebreaker is the id column. With UUIDv7 stored in its canonical
//! lowercase hyphenated form, lexicographic `ORDER BY id` is also chronological
//! order, so the tiebreaker is stable and meaningful rather than arbitrary.
//!
//! # Type safety
//!
//! [`Id`] is generic over a zero-sized [`Entity`] marker, so `MeetingId` and
//! `ParticipantId` are distinct types that cannot be swapped at a call site,
//! yet there is exactly **one** id implementation to maintain rather than one
//! wrapper struct per table.
//!
//! # Representation
//!
//! - **Rust**: [`Id<E>`], a `Copy` newtype over [`uuid::Uuid`].
//! - **SQLite**: `TEXT`, canonical lowercase hyphenated, exactly 36 characters.
//!   The migration additionally enforces the layout and the version-7 nibble
//!   with a `CHECK` constraint, so the invariant does not rest on Rust alone.
//! - **JSON**: the same 36-character string, so `packages/contracts` and the
//!   remote submission schema carry one representation end to end.
//!
//! Parsing is strict: a well-formed UUID of any other version is rejected. An
//! id that arrives in a submission file or from a browser is untrusted input
//! (architecture rules section 10), and "it parsed" must not be mistaken for
//! "it is one of ours".

use core::fmt;
use core::hash::{Hash, Hasher};
use core::marker::PhantomData;
use core::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;
use uuid::Uuid;

/// Marker for a kind of entity that owns identifiers.
///
/// Implementors are zero-sized types used only as a compile-time tag; they are
/// never constructed.
pub trait Entity {
    /// Human-readable name, used in error messages so a rejection can say
    /// which id was wrong (architecture rules section 22).
    const NAME: &'static str;
}

/// Errors produced when parsing an identifier from untrusted input.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum IdError {
    /// The text is not a syntactically valid UUID.
    #[error("invalid {entity} id: expected a 36-character UUID, detected {detected:?}")]
    Malformed {
        entity: &'static str,
        detected: String,
    },

    /// The text is a valid UUID, but not version 7.
    #[error("invalid {entity} id: expected a UUID version 7, detected version {detected_version}")]
    WrongVersion {
        entity: &'static str,
        detected_version: usize,
    },
}

/// A UUIDv7 identifier, tagged with the entity it belongs to.
#[repr(transparent)]
pub struct Id<E: Entity> {
    uuid: Uuid,
    // `fn() -> E` keeps `Id<E>` `Send`, `Sync` and `Copy` regardless of `E`,
    // and keeps `E` purely a compile-time tag.
    entity: PhantomData<fn() -> E>,
}

impl<E: Entity> Id<E> {
    /// Mint a new identifier from the current time.
    ///
    /// Two ids minted within the same millisecond are still distinct; the order
    /// between them is unspecified, so queries must not read more into it than
    /// the total ordering the id itself provides.
    #[must_use]
    pub fn new() -> Self {
        Self {
            uuid: Uuid::now_v7(),
            entity: PhantomData,
        }
    }

    /// Wrap an existing UUID, rejecting anything that is not version 7.
    pub fn from_uuid(uuid: Uuid) -> Result<Self, IdError> {
        let version = uuid.get_version_num();
        if version != 7 {
            return Err(IdError::WrongVersion {
                entity: E::NAME,
                detected_version: version,
            });
        }
        Ok(Self {
            uuid,
            entity: PhantomData,
        })
    }

    /// Parse the canonical 36-character form.
    pub fn parse(text: &str) -> Result<Self, IdError> {
        let uuid = Uuid::try_parse(text).map_err(|_| IdError::Malformed {
            entity: E::NAME,
            detected: text.to_owned(),
        })?;
        Self::from_uuid(uuid)
    }

    /// The underlying UUID.
    #[must_use]
    pub fn as_uuid(&self) -> Uuid {
        self.uuid
    }

    /// The canonical lowercase hyphenated form: exactly what SQLite and JSON
    /// both store.
    #[must_use]
    pub fn to_storage(&self) -> String {
        let mut buf = Uuid::encode_buffer();
        self.uuid.hyphenated().encode_lower(&mut buf).to_owned()
    }
}

impl<E: Entity> Default for Id<E> {
    fn default() -> Self {
        Self::new()
    }
}

// Deriving these would place an unwanted `E: Trait` bound on every impl, so
// they are written out. `E` is only ever a compile-time tag.
impl<E: Entity> Clone for Id<E> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<E: Entity> Copy for Id<E> {}

impl<E: Entity> PartialEq for Id<E> {
    fn eq(&self, other: &Self) -> bool {
        self.uuid == other.uuid
    }
}

impl<E: Entity> Eq for Id<E> {}

impl<E: Entity> PartialOrd for Id<E> {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<E: Entity> Ord for Id<E> {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.uuid.cmp(&other.uuid)
    }
}

impl<E: Entity> Hash for Id<E> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.uuid.hash(state);
    }
}

impl<E: Entity> fmt::Debug for Id<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}({})", E::NAME, self.uuid.hyphenated())
    }
}

impl<E: Entity> fmt::Display for Id<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.uuid.hyphenated(), f)
    }
}

impl<E: Entity> FromStr for Id<E> {
    type Err = IdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl<E: Entity> Serialize for Id<E> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_storage())
    }
}

impl<'de, E: Entity> Deserialize<'de> for Id<E> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// Declare entity markers and their id aliases in one place.
macro_rules! entities {
    ($($(#[$doc:meta])* $marker:ident => $alias:ident, $name:literal;)*) => {
        $(
            $(#[$doc])*
            #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
            pub enum $marker {}

            impl Entity for $marker {
                const NAME: &'static str = $name;
            }

            $(#[$doc])*
            pub type $alias = Id<$marker>;
        )*
    };
}

entities! {
    /// A meeting.
    Meeting => MeetingId, "meeting";
    /// A participant of a meeting.
    Participant => ParticipantId, "participant";
    /// One participant session, created by an identity claim.
    Session => SessionId, "session";
    /// A participant's single note (ADR-0003).
    Note => NoteId, "note";
    /// A link attached to a note.
    NoteLink => NoteLinkId, "note link";
    /// One historical version of a note.
    NoteVersion => NoteVersionId, "note version";
    /// An audit log entry.
    AuditLog => AuditLogId, "audit log";
    /// A processed remote submission record.
    RemoteSubmission => RemoteSubmissionId, "remote submission";
    /// The stable submission identity minted when a remote form is generated
    /// and carried back inside the submission file (ADR-0008).
    Submission => SubmissionId, "submission";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minted_ids_are_version_7_and_round_trip() {
        let id = MeetingId::new();
        assert_eq!(id.as_uuid().get_version_num(), 7);

        let text = id.to_storage();
        assert_eq!(text.len(), 36);
        assert_eq!(text, text.to_lowercase());
        assert_eq!(MeetingId::parse(&text).unwrap(), id);
    }

    #[test]
    fn ids_are_time_ordered_so_order_by_id_is_chronological() {
        // UUIDv7 embeds a millisecond timestamp, so an id minted later must
        // sort later both as a value and as the string SQLite stores.
        let first = NoteId::new();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let second = NoteId::new();

        assert!(first < second);
        assert!(first.to_storage() < second.to_storage());
    }

    #[test]
    fn a_valid_uuid_of_another_version_is_rejected() {
        // Version 4: syntactically fine, but not one of ours.
        let v4 = "9f8b7c6d-5e4f-4a3b-8c9d-0e1f2a3b4c5d";
        let err = ParticipantId::parse(v4).unwrap_err();
        assert_eq!(
            err,
            IdError::WrongVersion {
                entity: "participant",
                detected_version: 4,
            }
        );
        // The message names expected and detected (architecture rules 22).
        assert!(err.to_string().contains("expected a UUID version 7"));
        assert!(err.to_string().contains("detected version 4"));
    }

    #[test]
    fn malformed_text_is_rejected_and_names_what_was_seen() {
        let err = MeetingId::parse("not-an-id").unwrap_err();
        assert_eq!(
            err,
            IdError::Malformed {
                entity: "meeting",
                detected: "not-an-id".to_owned(),
            }
        );
        assert!(err.to_string().contains("not-an-id"));
    }

    #[test]
    fn distinct_entities_keep_distinct_names_in_errors() {
        // One generic type, but the diagnostics still say which id was wrong.
        let bad = "nope";
        assert!(MeetingId::parse(bad)
            .unwrap_err()
            .to_string()
            .contains("meeting"));
        assert!(NoteId::parse(bad).unwrap_err().to_string().contains("note"));
    }
}
