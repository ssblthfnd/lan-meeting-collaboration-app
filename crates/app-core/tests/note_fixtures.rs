//! The shared note fixtures, from the Rust side.
//!
//! `packages/editor/tests/fixtures.test.ts` reads the same file and asserts the
//! same outcomes. ADR-0007 requires the TypeScript and Rust sides to agree on
//! one Markdown subset, and this pair of suites is what turns that requirement
//! into a failing test rather than a good intention.
//!
//! The file is read at run time rather than embedded, so the two languages
//! genuinely read the same bytes and a change to the fixtures cannot be applied
//! to one side only.

use std::path::{Path, PathBuf};

use app_core::note::{self, NoteProblem};
use serde_json::Value;

/// Locate the fixtures by walking up from this crate's manifest directory.
fn fixture_path() -> PathBuf {
    let mut directory: &Path = Path::new(env!("CARGO_MANIFEST_DIR"));
    loop {
        let candidate = directory.join("packages/editor/__fixtures__/notes.json");
        if candidate.is_file() {
            return candidate;
        }
        directory = directory
            .parent()
            .unwrap_or_else(|| panic!("packages/editor/__fixtures__/notes.json not found"));
    }
}

fn fixtures() -> Value {
    let path = fixture_path();
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).expect("the fixtures are valid JSON")
}

/// One fixture row: its name, its content, and its recorded reason if refused.
struct Row {
    name: String,
    markdown: String,
    reason: Option<String>,
}

/// Expand `markdown` or `markdown_repeat` into content.
///
/// The oversized rows are a repeat directive so that 64 KiB of filler is not
/// committed to the fixture file, where it would be diffed forever.
fn rows(group: &Value) -> Vec<Row> {
    group
        .as_array()
        .expect("a fixture group is an array")
        .iter()
        .map(|entry| {
            let name = entry["name"].as_str().expect("a name").to_owned();
            let markdown = match entry.get("markdown") {
                Some(Value::String(text)) => text.clone(),
                _ => {
                    let repeat = entry
                        .get("markdown_repeat")
                        .unwrap_or_else(|| panic!("{name}: no markdown and no markdown_repeat"));
                    let unit = repeat["unit"].as_str().expect("a unit");
                    let count = repeat["count"].as_u64().expect("a count");
                    unit.repeat(usize::try_from(count).expect("a sane count"))
                }
            };
            Row {
                name,
                markdown,
                reason: entry
                    .get("reason")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            }
        })
        .collect()
}

#[test]
fn the_fixture_file_is_shared_and_non_trivial() {
    let all = fixtures();
    assert!(rows(&all["valid"]).len() > 20);
    assert!(rows(&all["invalid"]).len() > 20);
}

#[test]
fn every_fixture_name_is_unique() {
    let all = fixtures();
    let mut names: Vec<String> = rows(&all["valid"])
        .into_iter()
        .chain(rows(&all["invalid"]))
        .map(|row| row.name)
        .collect();
    let total = names.len();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), total, "a fixture name is used twice");
}

#[test]
fn every_valid_fixture_is_accepted() {
    for row in rows(&fixtures()["valid"]) {
        assert_eq!(
            note::inspect(&row.markdown),
            None,
            "valid fixture `{}` was refused",
            row.name
        );
        assert!(
            note::validate(&row.markdown).is_ok(),
            "valid fixture `{}` was refused",
            row.name
        );
    }
}

#[test]
fn every_invalid_fixture_is_refused_for_its_recorded_reason() {
    for row in rows(&fixtures()["invalid"]) {
        let problem = note::inspect(&row.markdown)
            .unwrap_or_else(|| panic!("invalid fixture `{}` was accepted", row.name));
        let reason = row
            .reason
            .unwrap_or_else(|| panic!("invalid fixture `{}` records no reason", row.name));
        assert_eq!(
            problem.as_str(),
            reason,
            "invalid fixture `{}` was refused for the wrong reason",
            row.name
        );
        assert!(note::validate(&row.markdown).is_err(), "{}", row.name);
    }
}

#[test]
fn every_declared_reason_is_exercised() {
    // A reason nothing tests is a rule nothing proves.
    let all = fixtures();
    let used: Vec<String> = rows(&all["invalid"])
        .into_iter()
        .filter_map(|row| row.reason)
        .collect();

    for reason in all["reasons"].as_array().expect("the reason list") {
        let reason = reason.as_str().expect("a reason name");
        assert!(
            used.iter().any(|entry| entry == reason),
            "no invalid fixture exercises `{reason}`"
        );
    }
}

#[test]
fn every_declared_reason_is_one_the_validator_can_produce() {
    let all = fixtures();
    let known = [
        NoteProblem::Empty,
        NoteProblem::TooLarge,
        NoteProblem::RawHtml,
        NoteProblem::LinkScheme,
        NoteProblem::ControlCharacter,
    ];

    for reason in all["reasons"].as_array().expect("the reason list") {
        let reason = reason.as_str().expect("a reason name");
        assert!(
            known.iter().any(|problem| problem.as_str() == reason),
            "the fixtures declare `{reason}`, which no NoteProblem produces"
        );
    }
    assert_eq!(
        all["reasons"].as_array().expect("the reason list").len(),
        known.len(),
        "a NoteProblem variant has no declared reason, or the reverse"
    );
}

#[test]
fn the_fixtures_carry_the_angle_bracket_regressions() {
    // These are the rows the raw-HTML rule exists for, and the ones most
    // likely to be lost in a well-meaning tightening of the scanner.
    let all = fixtures();
    let valid = rows(&all["valid"]);

    for expected in ["a < b", "2 < 3"] {
        assert!(
            valid.iter().any(|row| row.markdown == expected),
            "the fixtures must keep `{expected}` as an accepted note"
        );
        assert_eq!(note::inspect(expected), None, "{expected}");
    }
}
