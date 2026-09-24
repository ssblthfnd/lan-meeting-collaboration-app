//! The TXT renderer against the shared editor fixtures.
//!
//! `packages/editor/tests/fixtures.test.ts` already asserts, on the
//! TypeScript side, that `plainTextOf(renderMarkdown(fixture.markdown))`
//! equals `fixture.text` for every valid fixture that records one. This is
//! the Rust side of that same assertion, against
//! [`app_export::render_txt`]'s underlying block-level renderer
//! ([`app_export::plain_text::render_plain_text`]) fed by
//! [`app_export::markdown_ast::parse_markdown`] - closing the gap noted
//! during Step 12's design inspection, where `crates/app-core/tests/
//! note_fixtures.rs` reads this file but never checks its `text` field.

use app_export::markdown_ast::parse_markdown;
use app_export::plain_text::render_plain_text;
use serde_json::Value;
use std::path::{Path, PathBuf};

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

#[test]
fn every_valid_fixture_with_a_recorded_text_renders_to_it() {
    let all = fixtures();
    let valid = all["valid"].as_array().expect("valid is an array");
    let mut checked = 0usize;

    for entry in valid {
        let name = entry["name"].as_str().expect("a name");
        let Some(expected) = entry.get("text").and_then(Value::as_str) else {
            continue; // Rows with no recorded text (e.g. the size-limit case).
        };
        let markdown = entry["markdown"].as_str().unwrap_or_else(|| {
            panic!("{name}: only `markdown_repeat` rows omit `text`, so this one should not")
        });

        let rendered = render_plain_text(&parse_markdown(markdown));
        assert_eq!(
            rendered, expected,
            "fixture `{name}` did not render to its recorded text"
        );
        checked += 1;
    }

    assert!(
        checked > 20,
        "expected to check more than 20 fixtures, checked {checked}"
    );
}

#[test]
fn rendering_is_deterministic() {
    let all = fixtures();
    for entry in all["valid"].as_array().expect("valid is an array") {
        if let Some(markdown) = entry.get("markdown").and_then(Value::as_str) {
            let once = render_plain_text(&parse_markdown(markdown));
            let twice = render_plain_text(&parse_markdown(markdown));
            assert_eq!(once, twice);
        }
    }
}
