//! The shared remote-submission fixtures, from the Rust side.
//!
//! `packages/contracts/tests/submission.test.ts` reads the same file and
//! asserts the same outcomes. ADR-0021 freezes the submission v1 shape and its
//! canonical byte form; this pair of suites is what turns that into a failing
//! test rather than a good intention.
//!
//! The file is read at run time rather than embedded, so the two languages
//! genuinely read the same bytes and a change to the fixtures cannot be applied
//! to one side only. That is the arrangement `app-core`'s note fixtures already
//! use, for the same reason.

use std::path::{Path, PathBuf};

use app_remote::{canonical_form, content_hash, RemoteError, SubmissionV1, CANONICAL_PREFIX};
use serde_json::Value;

/// Locate the fixtures by walking up from this crate's manifest directory.
fn fixture_path() -> PathBuf {
    let mut directory: &Path = Path::new(env!("CARGO_MANIFEST_DIR"));
    loop {
        let candidate = directory.join("packages/contracts/__fixtures__/submissions.json");
        if candidate.is_file() {
            return candidate;
        }
        directory = directory.parent().unwrap_or_else(|| {
            panic!("packages/contracts/__fixtures__/submissions.json not found")
        });
    }
}

fn fixtures() -> Value {
    let path = fixture_path();
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).expect("the fixtures are valid JSON")
}

fn group<'a>(all: &'a Value, name: &str) -> &'a Vec<Value> {
    all[name]
        .as_array()
        .unwrap_or_else(|| panic!("the fixtures have a `{name}` group"))
}

fn parse_row(row: &Value) -> SubmissionV1 {
    let name = row["name"].as_str().expect("a name");
    let text = serde_json::to_string(&row["submission"]).expect("re-serialises");
    SubmissionV1::parse(&text)
        .unwrap_or_else(|e| panic!("{name}: expected a valid submission: {e}"))
}

#[test]
fn every_canonical_fixture_produces_its_recorded_form_and_hash() {
    let all = fixtures();

    for row in group(&all, "canonical") {
        let name = row["name"].as_str().expect("a name");
        let submission = parse_row(row);

        assert_eq!(
            canonical_form(&submission),
            row["canonical"].as_str().expect("a canonical form"),
            "{name}: the canonical form drifted"
        );
        assert_eq!(
            content_hash(&submission),
            row["content_hash"].as_str().expect("a hash"),
            "{name}: the hash drifted"
        );
    }
}

#[test]
fn the_fixtures_agree_with_this_build_about_the_prefix() {
    // Domain separation is part of the contract, not an implementation detail:
    // changing it silently would make every previously recorded hash wrong.
    let all = fixtures();
    assert_eq!(all["canonical_prefix"].as_str(), Some(CANONICAL_PREFIX));
}

#[test]
fn line_endings_do_not_change_an_artefact() {
    // `crlf` and `lone_cr` carry the same text as `plain` with different line
    // endings. A mail gateway rewriting them must not look like tampering.
    let all = fixtures();
    let rows = group(&all, "canonical");

    let hash_of = |name: &str| {
        let row = rows
            .iter()
            .find(|row| row["name"].as_str() == Some(name))
            .unwrap_or_else(|| panic!("a `{name}` fixture"));
        content_hash(&parse_row(row))
    };

    assert_eq!(hash_of("plain"), hash_of("crlf"));
    assert_eq!(hash_of("plain"), hash_of("lone_cr"));
    assert_ne!(hash_of("plain"), hash_of("other_note"));
}

#[test]
fn a_field_outside_the_hash_does_not_change_the_artefact() {
    let all = fixtures();
    let canonical = group(&all, "canonical");

    for row in group(&all, "excluded") {
        let name = row["name"].as_str().expect("a name");
        let same_as = row["same_hash_as"].as_str().expect("a base row");

        let base = canonical
            .iter()
            .find(|candidate| candidate["name"].as_str() == Some(same_as))
            .unwrap_or_else(|| panic!("{name}: no `{same_as}` fixture"));

        assert_eq!(
            content_hash(&parse_row(row)),
            base["content_hash"].as_str().expect("a hash"),
            "{name}: a field the hash excludes changed it"
        );
    }
}

#[test]
fn every_invalid_fixture_is_refused_for_its_recorded_reason() {
    let all = fixtures();

    for row in group(&all, "invalid") {
        let name = row["name"].as_str().expect("a name");
        let raw = row["raw"].as_str().expect("raw text");
        let expected = row["reason"].as_str().expect("a reason");

        let error = SubmissionV1::parse(raw)
            .err()
            .unwrap_or_else(|| panic!("{name}: expected a refusal, it parsed"));

        let detected = match error {
            RemoteError::Malformed { .. } => "malformed",
            RemoteError::UnsupportedSchema { .. } => "unsupported_schema",
            RemoteError::MalformedId { .. } => "malformed_id",
            other => panic!("{name}: unexpected refusal {other}"),
        };

        assert_eq!(detected, expected, "{name}: refused for the wrong reason");
    }
}

#[test]
fn a_refusal_names_the_expected_and_the_detected_value() {
    // Architecture rules section 22. A Host reading this must be able to act on
    // it without opening the file in an editor.
    let all = fixtures();

    for row in group(&all, "invalid") {
        let raw = row["raw"].as_str().expect("raw text");
        if row["reason"].as_str() != Some("unsupported_schema") {
            continue;
        }

        let message = SubmissionV1::parse(raw).unwrap_err().to_string();
        assert!(message.contains("expected version 1"), "{message}");
        assert!(message.contains("detected version"), "{message}");
    }
}
