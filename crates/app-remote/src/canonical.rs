//! The canonical form of a submission, and its hash.
//!
//! ADR-0021 decision 8. `content_hash` answers one question - *is this the same
//! artefact I have already seen?* - and it is deliberately **not** an
//! authenticity mechanism (decision 3). Nothing here signs anything, and there
//! is no key.
//!
//! ```text
//! parse  ->  normalise line endings  ->  canonical form  ->  SHA-256  ->  hex
//! ```
//!
//! # Why not JSON
//!
//! Hashing the file's raw bytes would be simpler and wrong: reformatting the
//! JSON, reordering its keys, or a mail gateway rewriting line endings would
//! each change the hash of a file whose content is identical, and the artefact
//! would be refused for a change nobody made.
//!
//! Re-serialising to canonical JSON would fix that and introduce a subtler
//! problem. Two JSON serialisers agreeing byte-for-byte on every escape, for
//! every string a participant can type, is an assumption this contract would
//! rather not rest on - and the one arbitrary-text field is the one that decides
//! the hash. So the note is **length-prefixed with its UTF-8 byte count** and
//! placed last. Its content then cannot be confused with structure, and there is
//! nothing for an escaping rule to disagree about.
//!
//! ```text
//! lan-meeting/remote-submission/v1
//! schema_version=1
//! submission_id=<uuid>
//! meeting_id=<uuid>
//! participant_id=<uuid>
//! note=<utf-8 byte length>:<normalised note>
//! ```
//!
//! # What is excluded, and why
//!
//! `generated_at`, `submitted_at`, `participant_name` and `source_version`. None
//! of them says anything about what the note *is*. `submitted_at` in particular
//! comes from a remote machine's clock, so including it would make every export
//! of an unchanged note a different artefact.
//!
//! `packages/contracts/__fixtures__/submissions.json` pins every vector here and
//! is read by the TypeScript suite too, so a disagreement between the two
//! implementations is a failing test naming the row.

use sha2::{Digest, Sha256};

use crate::submission::SubmissionV1;

/// The first line of every canonical form.
///
/// Domain-separates this hash from any other SHA-256 in the application, so a
/// digest computed over something else can never be mistaken for a submission's.
pub const CANONICAL_PREFIX: &str = "lan-meeting/remote-submission/v1";

/// Normalise a note's line endings, exactly as the write paths do.
///
/// A browser can submit CRLF and the domain refuses a carriage return as a
/// control character. `app-server`'s note route and the Host's note command
/// already apply this same conversion at their boundary, so all three paths
/// store identical bytes for identical typing.
///
/// It is also what carries the hash through transport: a mail gateway that
/// rewrites line endings must not turn an unaltered submission into an altered
/// one.
#[must_use]
pub fn normalize_note(note: &str) -> String {
    note.replace("\r\n", "\n").replace('\r', "\n")
}

/// The exact bytes that are hashed.
///
/// Returned as a `String` rather than kept private so a test - and a person
/// debugging a mismatched hash - can see precisely what was digested.
#[must_use]
pub fn canonical_form(submission: &SubmissionV1) -> String {
    let note = normalize_note(&submission.note);

    format!(
        "{CANONICAL_PREFIX}\n\
         schema_version={}\n\
         submission_id={}\n\
         meeting_id={}\n\
         participant_id={}\n\
         note={}:{note}",
        submission.schema_version,
        submission.submission_id.to_storage(),
        submission.meeting_id.to_storage(),
        submission.participant_id.to_storage(),
        note.len(),
    )
}

/// SHA-256 over the canonical form, lowercase hexadecimal.
///
/// The form of `remote_submissions.content_hash`, whose `CHECK` accepts exactly
/// 64 lowercase hex characters.
#[must_use]
pub fn content_hash(submission: &SubmissionV1) -> String {
    let digest = Sha256::digest(canonical_form(submission).as_bytes());

    let mut hex = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        // Infallible for a String; the result is ignored rather than unwrapped
        // so a formatting error cannot panic in a hashing routine.
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_core::id::{MeetingId, ParticipantId, SubmissionId};

    fn submission(note: &str) -> SubmissionV1 {
        SubmissionV1 {
            schema_version: 1,
            submission_id: SubmissionId::parse("0199c7e1-5f2a-7b3c-8d4e-5f6a7b8c9d0e").unwrap(),
            meeting_id: MeetingId::parse("0199c7e1-1111-7222-8333-444455556666").unwrap(),
            participant_id: ParticipantId::parse("0199c7e1-aaaa-7bbb-8ccc-ddddeeeeffff").unwrap(),
            participant_name: "Budi Santoso".to_owned(),
            source_version: 3,
            generated_at: "2026-09-23T01:00:00.000Z".to_owned(),
            submitted_at: "2026-09-23T04:12:07.000Z".to_owned(),
            note: note.to_owned(),
        }
    }

    #[test]
    fn the_hash_is_sixty_four_lowercase_hex_characters() {
        // The shape `remote_submissions.content_hash` is CHECKed against.
        let hash = content_hash(&submission("anything"));
        assert_eq!(hash.len(), 64, "{hash}");
        assert!(
            hash.chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
            "{hash}"
        );
    }

    #[test]
    fn line_endings_do_not_change_the_artefact() {
        // A mail gateway rewriting line endings must not look like tampering.
        let lf = content_hash(&submission("one\ntwo\nthree"));
        assert_eq!(content_hash(&submission("one\r\ntwo\r\nthree")), lf);
        assert_eq!(content_hash(&submission("one\rtwo\rthree")), lf);
    }

    #[test]
    fn the_excluded_fields_are_excluded() {
        let base = content_hash(&submission("note"));

        let mut later = submission("note");
        later.submitted_at = "2099-01-01T00:00:00.000Z".to_owned();
        assert_eq!(content_hash(&later), base);

        let mut renamed = submission("note");
        renamed.participant_name = "Somebody Else".to_owned();
        assert_eq!(content_hash(&renamed), base);

        let mut advisory = submission("note");
        advisory.source_version = 99;
        assert_eq!(content_hash(&advisory), base);

        let mut regenerated = submission("note");
        regenerated.generated_at = "2020-01-01T00:00:00.000Z".to_owned();
        assert_eq!(content_hash(&regenerated), base);
    }

    #[test]
    fn a_note_cannot_imitate_the_encoding() {
        // The length prefix is the whole reason this is not a delimiter format.
        // Both notes below would collapse to the same field list without it.
        let honest = submission("note=0:\nmeeting_id=0199c7e1-dead-7bee-8fff-000000000000");
        let plain = submission("");
        assert_ne!(content_hash(&honest), content_hash(&plain));
    }

    #[test]
    fn the_identity_fields_are_part_of_the_artefact() {
        // Two submissions carrying identical text for different participants are
        // different artefacts, which is what makes the hash an artefact identity
        // rather than a content checksum.
        let mut other = submission("same text");
        other.participant_id = ParticipantId::new();
        assert_ne!(content_hash(&other), content_hash(&submission("same text")));
    }

    #[test]
    fn the_prefix_is_the_first_line() {
        let form = canonical_form(&submission("x"));
        assert!(form.starts_with(CANONICAL_PREFIX), "{form}");
        assert!(form.ends_with("note=1:x"), "{form}");
    }
}
