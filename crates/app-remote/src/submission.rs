//! The remote submission schema, version 1.
//!
//! The Rust half of `packages/contracts/src/submission.v1.ts`. What the offline
//! form produces, and what the Host reads back as **untrusted input**.
//!
//! # Parsing establishes shape, and nothing else
//!
//! [`SubmissionV1::parse`] answers one question: *is this file a submission of a
//! version we know, with identifiers of the right form?* It does not ask whether
//! the meeting exists, whether the participant is on its roster, whether the
//! meeting still accepts writes, whether the note's Markdown is acceptable or
//! whether this artefact has been imported before. Every one of those is a
//! lookup or a domain rule, and they belong to the import pipeline in step 10.
//!
//! Keeping the line there matters: a parser that started resolving identifiers
//! would be a second place where "does this participant exist?" is answered, and
//! the answer that counts is the one read inside the mutating transaction
//! (architecture rules section 15).
//!
//! # Three refusals, told apart
//!
//! ```text
//! not JSON, wrong shape, wrong type   -> Malformed
//! a version this build does not know  -> UnsupportedSchema
//! an id that is not a canonical v7    -> MalformedId
//! ```
//!
//! They are separated because they are different things to do about: a
//! malformed file was damaged or is not a submission, an unsupported version
//! came from a different build, and a bad identifier means the file was edited.
//! Checking the version **before** the identifiers is deliberate - a v2 file
//! should be reported as v2, not as whatever its ids happen to look like to v1.

use app_core::id::{Id, MeetingId, ParticipantId, SubmissionId};
use serde::{Deserialize, Serialize};

use crate::context::SCHEMA_VERSION;
use crate::{RemoteError, RemoteResult};

/// Largest complete submission artefact this application will read.
///
/// Four times the 64 KiB a note may contain, which leaves room for JSON
/// escaping of a note made entirely of quotes or backslashes plus the eight
/// metadata fields around it.
///
/// It is a **memory guard, not the note rule**: content that fits here and not
/// in 64 KiB is refused by `app-core::note`, which can say exactly by how much.
/// The two numbers do two separate jobs, the way the LAN transport's body limit
/// and the domain's note limit already do (ADR-0022 decision 16).
pub const MAX_SUBMISSION_BYTES: usize = 256 * 1024;

/// A parsed remote submission.
///
/// Every field is untrusted. The two identifiers are **candidates** to be
/// resolved by lookup; `participant_name` is displayed to a human for
/// verification and is never a match key (architecture rules sections 10, 21).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SubmissionV1 {
    pub schema_version: u32,
    pub submission_id: SubmissionId,
    pub meeting_id: MeetingId,
    pub participant_id: ParticipantId,
    /// Display and verification only.
    pub participant_name: String,
    /// Advisory context for the Host's preview. Never a precondition.
    pub source_version: i64,
    /// Copied from the form. The Host's clock at generation.
    pub generated_at: String,
    /// A remote machine's clock: untrusted, display-only, and excluded from the
    /// canonical hash so that re-exporting an unchanged note is the same
    /// artefact (ADR-0021 decision 8).
    pub submitted_at: String,
    /// The participant's single note, GFM-subset Markdown (ADR-0003, ADR-0007).
    pub note: String,
}

/// The submission exactly as it appears on disk, before anything is believed.
///
/// Identifiers are `String` here on purpose. Deserializing them straight into
/// [`Id`] would fold "this id is not a v7" into serde's generic shape error, and
/// the Host would be told a file is malformed when the useful sentence names the
/// field that is wrong.
///
/// `deny_unknown_fields` because an unexpected key means the file is not the
/// thing it claims to be. A later schema carries a later `schema_version` and is
/// refused by that check, with a message that says so.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSubmission {
    schema_version: u64,
    submission_id: String,
    meeting_id: String,
    participant_id: String,
    participant_name: String,
    source_version: i64,
    generated_at: String,
    submitted_at: String,
    note: String,
}

impl SubmissionV1 {
    /// Read a submission file.
    ///
    /// Untrusted input in, shape out. See the module documentation for what this
    /// deliberately does not check.
    pub fn parse(text: &str) -> RemoteResult<Self> {
        // Before anything is parsed. A caller that read from a file has already
        // checked the size; this is the check that also covers pasted text, and
        // the one that catches a file which grew between `metadata` and `read`.
        if text.len() > MAX_SUBMISSION_BYTES {
            return Err(RemoteError::TooLarge {
                limit: MAX_SUBMISSION_BYTES,
                detected: text.len() as u64,
            });
        }

        let raw: RawSubmission =
            serde_json::from_str(text).map_err(|error| RemoteError::Malformed {
                detail: error.to_string(),
            })?;

        // Before the identifiers: a v2 file is reported as v2.
        if raw.schema_version != u64::from(SCHEMA_VERSION) {
            return Err(RemoteError::UnsupportedSchema {
                expected: SCHEMA_VERSION,
                detected: raw.schema_version,
            });
        }

        Ok(Self {
            schema_version: SCHEMA_VERSION,
            submission_id: parse_id("submission_id", &raw.submission_id)?,
            meeting_id: parse_id("meeting_id", &raw.meeting_id)?,
            participant_id: parse_id("participant_id", &raw.participant_id)?,
            participant_name: raw.participant_name,
            source_version: raw.source_version,
            generated_at: raw.generated_at,
            submitted_at: raw.submitted_at,
            note: raw.note,
        })
    }
}

/// Parse one identifier, naming the field it came from when it is wrong.
fn parse_id<E: app_core::id::Entity>(field: &'static str, text: &str) -> RemoteResult<Id<E>> {
    Id::parse(text).map_err(|_| RemoteError::MalformedId {
        field,
        detected: text.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid() -> String {
        serde_json::json!({
            "schema_version": 1,
            "submission_id": "0199c7e1-5f2a-7b3c-8d4e-5f6a7b8c9d0e",
            "meeting_id": "0199c7e1-1111-7222-8333-444455556666",
            "participant_id": "0199c7e1-aaaa-7bbb-8ccc-ddddeeeeffff",
            "participant_name": "Budi Santoso",
            "source_version": 3,
            "generated_at": "2026-09-23T01:00:00.000Z",
            "submitted_at": "2026-09-23T04:12:07.000Z",
            "note": "## What I noted"
        })
        .to_string()
    }

    #[test]
    fn a_well_formed_submission_parses() {
        let parsed = SubmissionV1::parse(&valid()).expect("parses");
        assert_eq!(parsed.schema_version, SCHEMA_VERSION);
        assert_eq!(parsed.participant_name, "Budi Santoso");
        assert_eq!(parsed.source_version, 3);
        assert_eq!(parsed.note, "## What I noted");
    }

    #[test]
    fn the_version_is_checked_before_the_identifiers() {
        // A v2 file whose ids are also wrong must still be reported as v2: the
        // useful next action is "this came from a different build", not "fix
        // this uuid".
        let text = serde_json::json!({
            "schema_version": 2,
            "submission_id": "not-a-uuid",
            "meeting_id": "not-a-uuid",
            "participant_id": "not-a-uuid",
            "participant_name": "Budi Santoso",
            "source_version": 0,
            "generated_at": "2026-09-23T01:00:00.000Z",
            "submitted_at": "2026-09-23T04:12:07.000Z",
            "note": "x"
        })
        .to_string();

        assert_eq!(
            SubmissionV1::parse(&text).unwrap_err(),
            RemoteError::UnsupportedSchema {
                expected: 1,
                detected: 2
            }
        );
    }

    #[test]
    fn a_refusal_names_the_field_that_is_wrong() {
        let text = valid().replace("0199c7e1-1111-7222-8333-444455556666", "not-a-uuid");
        assert_eq!(
            SubmissionV1::parse(&text).unwrap_err(),
            RemoteError::MalformedId {
                field: "meeting_id",
                detected: "not-a-uuid".to_owned()
            }
        );
    }

    #[test]
    fn an_unexpected_field_means_this_is_not_a_v1_submission() {
        let text = valid().replace(
            "\"note\":\"## What I noted\"",
            "\"note\":\"## What I noted\",\"links\":[]",
        );
        assert!(matches!(
            SubmissionV1::parse(&text),
            Err(RemoteError::Malformed { .. })
        ));
    }

    #[test]
    fn nothing_here_resolves_an_identifier() {
        // The ids below are syntactically perfect and name nothing at all.
        // Parsing accepts them, because whether they exist is a lookup and the
        // answer that counts is read inside the import transaction (step 10).
        assert!(SubmissionV1::parse(&valid()).is_ok());
    }
}
