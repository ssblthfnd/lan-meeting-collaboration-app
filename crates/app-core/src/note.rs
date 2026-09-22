//! What a note may contain.
//!
//! **This is the enforcement.** `packages/editor` runs the same rules in the
//! browser so a Host is told about a problem while they type, but that copy is
//! a convenience: content is stored only if the rules below accept it, and they
//! run inside the mutating transaction (architecture rules section 3).
//!
//! The two implementations are held together by
//! `packages/editor/__fixtures__/notes.json`, which both test suites read. A
//! rule only one language enforces shows up there as a failure.
//!
//! # What is checked, and what is deliberately not
//!
//! Checked: the content is not empty, it fits in 64 KiB of UTF-8, it carries no
//! recognizable raw HTML construct, every Markdown link target uses an
//! allowlisted scheme, and it contains no control character other than tab and
//! newline.
//!
//! Not checked: whether the Markdown is *well formed*. A stray asterisk, an
//! unclosed fence or a blockquote this application does not interpret are not
//! errors - they are somebody's writing, and the renderer shows them as text.
//! Refusing them would make the application argue with the person using it over
//! syntax it had no reason to care about.
//!
//! There is no Markdown parser here, and Step 8 deliberately does not add one.
//! A scanner is enough to decide the questions above, and the full renderer is
//! the export step's problem (ADR-0019).
//!
//! # The `<` rule
//!
//! `a < b` and `2 < 3` are arithmetic, and a validator that refused them would
//! be refusing ordinary meeting notes. A bare `<` is text. What is refused is a
//! construct a browser would recognise: a comment, a declaration, a processing
//! instruction, or a tag that is actually closed.

use crate::error::{DomainError, DomainResult};

/// Largest note content, in UTF-8 bytes.
///
/// Bytes rather than characters, because bytes are what SQLite stores and what
/// an export carries. 32769 two-byte characters is over the limit even though
/// it is half that many characters, and the shared fixtures pin exactly that.
pub const MAX_NOTE_BYTES: usize = 64 * 1024;

/// Link schemes that may appear in stored note content.
///
/// PRD section 13.1 and ADR-0007. The same three the editor's allowlist uses,
/// compared without their colon so the check reads the same in both languages.
pub const ALLOWED_LINK_SCHEMES: [&str; 3] = ["http", "https", "mailto"];

/// Why content was refused.
///
/// The discriminators are shared with the fixtures and with the TypeScript
/// validator, so a disagreement between the two is a test failure naming the
/// rule rather than a mystery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteProblem {
    Empty,
    TooLarge,
    RawHtml,
    LinkScheme,
    ControlCharacter,
}

impl NoteProblem {
    /// The name this problem carries in `__fixtures__/notes.json`.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            NoteProblem::Empty => "empty",
            NoteProblem::TooLarge => "too_large",
            NoteProblem::RawHtml => "raw_html",
            NoteProblem::LinkScheme => "link_scheme",
            NoteProblem::ControlCharacter => "control_character",
        }
    }
}

/// Check note content, returning the first problem.
///
/// Ordered cheapest and most fundamental first, so empty content is reported as
/// empty rather than as whatever else a scanner would notice about it.
pub fn inspect(content: &str) -> Option<NoteProblem> {
    if content.trim().is_empty() {
        return Some(NoteProblem::Empty);
    }
    if content.len() > MAX_NOTE_BYTES {
        return Some(NoteProblem::TooLarge);
    }
    if find_control_character(content).is_some() {
        return Some(NoteProblem::ControlCharacter);
    }

    for segment in markup_segments(content) {
        if find_html_construct(&segment).is_some() {
            return Some(NoteProblem::RawHtml);
        }
        for target in link_targets(&segment) {
            if !is_allowed_link_target(&target) {
                return Some(NoteProblem::LinkScheme);
            }
        }
    }

    None
}

/// Check note content, as the domain reports a refusal.
///
/// Every message names the expected and the detected value, which is what makes
/// it actionable (architecture rules section 22). None of them quotes the note
/// back: content can be long, and repeating untrusted input into an error is a
/// habit worth not having.
pub fn validate(content: &str) -> DomainResult<()> {
    let Some(problem) = inspect(content) else {
        return Ok(());
    };

    let (expected, detected) = match problem {
        NoteProblem::Empty => (
            "non-empty content",
            if content.is_empty() {
                "an empty string".to_owned()
            } else {
                "only whitespace".to_owned()
            },
        ),
        NoteProblem::TooLarge => (
            "at most 65536 bytes of content",
            format!("{} bytes", content.len()),
        ),
        NoteProblem::ControlCharacter => (
            "no control characters other than tab and newline",
            find_control_character(content)
                .map_or_else(|| "a control character".to_owned(), describe_control),
        ),
        NoteProblem::RawHtml => (
            "Markdown without raw HTML",
            markup_segments(content)
                .iter()
                .find_map(|segment| find_html_construct(segment))
                .unwrap_or_else(|| "an HTML construct".to_owned()),
        ),
        NoteProblem::LinkScheme => (
            "a link target using http, https or mailto",
            markup_segments(content)
                .iter()
                .flat_map(|segment| link_targets(segment))
                .find(|target| !is_allowed_link_target(target))
                .map_or_else(
                    || "a link target".to_owned(),
                    |target| describe_target(&target),
                ),
        ),
    };

    Err(DomainError::Validation {
        field: "note content",
        expected,
        detected,
    })
}

/// Whether a link target may become an anchor.
///
/// Scheme, then the shape the scheme implies. `http` and `https` need an
/// authority, which is what refuses a bare `https:`; `mailto` needs a
/// recipient. This is the Rust half of what `new URL(..)` decides in the
/// browser, and it is written out rather than delegated because `app-core`
/// has no URL parser and Step 8 does not add one.
pub fn is_allowed_link_target(target: &str) -> bool {
    let trimmed = target.trim();
    if trimmed.is_empty() {
        return false;
    }

    let Some((scheme, rest)) = trimmed.split_once(':') else {
        // No scheme: a relative target, which could never render as a link.
        return false;
    };
    if scheme.is_empty()
        || !scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        || !scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    {
        return false;
    }

    let scheme = scheme.to_ascii_lowercase();
    if !ALLOWED_LINK_SCHEMES.contains(&scheme.as_str()) {
        return false;
    }

    match scheme.as_str() {
        // `//host/path`, with a non-empty host and nothing a URL parser would
        // reject outright.
        "http" | "https" => {
            let Some(authority) = rest.strip_prefix("//") else {
                return false;
            };
            // Not trimmed: a space is a forbidden host character, and the URL
            // parser the browser half uses refuses it rather than tidying it.
            let host = authority.split(['/', '?', '#']).next().unwrap_or_default();
            !host.is_empty()
                && !host.contains(char::is_whitespace)
                && host.matches('[').count() == host.matches(']').count()
        }
        // Anything non-empty. A mailbox is not worth a parser here; the
        // renderer's `new URL` is the second opinion.
        _ => !rest.trim().is_empty(),
    }
}

/// The first prohibited control character.
fn find_control_character(content: &str) -> Option<char> {
    content
        .chars()
        .find(|c| *c != '\n' && *c != '\t' && ((*c as u32) < 0x20 || (*c as u32) == 0x7f))
}

fn describe_control(character: char) -> String {
    format!("U+{:04X}", character as u32)
}

/// Describe a refused link target by its scheme, never by quoting it whole.
fn describe_target(target: &str) -> String {
    let trimmed = target.trim();
    if trimmed.is_empty() {
        return "an empty link target".to_owned();
    }
    match trimmed.split_once(':') {
        Some((scheme, _)) if !scheme.is_empty() => {
            format!("the scheme {}", scheme.to_ascii_lowercase())
        }
        _ => "a link target with no scheme".to_owned(),
    }
}

/* -------------------------------------------------------------------------
 * The scanner
 *
 * Mirrors `packages/editor/src/scan.ts`. Anything changed here changes there.
 * ------------------------------------------------------------------------- */

/// Whether a line opens or closes a fenced code block.
fn is_fence(line: &str) -> bool {
    line.trim_start().starts_with("```")
}

/// Every part of the content that is not literal code.
///
/// Fenced blocks are dropped whole, including their fence lines; within the
/// remaining lines, code spans are dropped too. Code is literal, so a
/// `<script>` inside a fence is somebody writing *about* a script tag - an
/// ordinary thing to do in a meeting note - and it renders as text.
fn markup_segments(content: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut in_fence = false;

    for line in content.split('\n') {
        if is_fence(line) {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        found.extend(non_code_spans(line));
    }

    found
}

/// The parts of one line that are outside inline code spans.
///
/// A run of N backticks opens a span that the next run of **exactly** N
/// backticks closes. An unclosed run is literal text, which is what GFM does
/// and what stops a stray backtick swallowing the rest of a note.
fn non_code_spans(line: &str) -> Vec<String> {
    let characters: Vec<char> = line.chars().collect();
    let mut found = Vec::new();
    let mut plain = String::new();
    let mut index = 0;

    while index < characters.len() {
        if characters[index] != '`' {
            plain.push(characters[index]);
            index += 1;
            continue;
        }

        let open = index;
        while index < characters.len() && characters[index] == '`' {
            index += 1;
        }
        let run = index - open;

        match find_backtick_run(&characters, index, run) {
            None => {
                plain.extend(&characters[open..index]);
            }
            Some(closing) => {
                if !plain.is_empty() {
                    found.push(std::mem::take(&mut plain));
                }
                index = closing + run;
            }
        }
    }

    if !plain.is_empty() {
        found.push(plain);
    }
    found
}

/// The start of the next run of exactly `length` backticks, at or after `from`.
fn find_backtick_run(characters: &[char], from: usize, length: usize) -> Option<usize> {
    let mut index = from;
    while index < characters.len() {
        if characters[index] != '`' {
            index += 1;
            continue;
        }
        let start = index;
        while index < characters.len() && characters[index] == '`' {
            index += 1;
        }
        if index - start == length {
            return Some(start);
        }
    }
    None
}

/// The first recognizable raw HTML construct in a markup segment.
///
/// | Construct | Example |
/// | --- | --- |
/// | comment | `<!-- note -->` |
/// | declaration | `<!DOCTYPE html>`, `<![CDATA[ ]]>` |
/// | processing instruction | `<?xml ?>` |
/// | tag, open or closing, that is actually closed | `<script>`, `</div>`, `<br/>` |
///
/// A tag needs its `>` on the same segment, which is what keeps `a<b was here`
/// out of it: a name followed by prose and no bracket is not a tag.
fn find_html_construct(segment: &str) -> Option<String> {
    let characters: Vec<char> = segment.chars().collect();

    for start in 0..characters.len() {
        if characters[start] != '<' {
            continue;
        }

        let rest = &characters[start..];
        if rest.starts_with(&['<', '!', '-', '-']) {
            return Some("<!--".to_owned());
        }
        if rest.starts_with(&['<', '!']) {
            return Some("<!".to_owned());
        }
        if rest.starts_with(&['<', '?']) {
            return Some("<?".to_owned());
        }

        let mut cursor = 1;
        if rest.get(cursor) == Some(&'/') {
            cursor += 1;
        }
        if !rest.get(cursor).is_some_and(char::is_ascii_alphabetic) {
            continue;
        }
        cursor += 1;
        while rest
            .get(cursor)
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '-')
        {
            cursor += 1;
        }
        // Everything up to the closing bracket, which must be on this segment
        // and must not be interrupted by another `<`.
        while let Some(character) = rest.get(cursor) {
            match character {
                '>' => {
                    let matched: String = rest[..=cursor].iter().collect();
                    return Some(if matched.chars().count() > 24 {
                        format!("{}…", matched.chars().take(24).collect::<String>())
                    } else {
                        matched
                    });
                }
                '<' => break,
                _ => cursor += 1,
            }
        }
    }

    None
}

/// Every link and image target in a markup segment, as written.
///
/// Targets are read with **balanced parentheses**, so
/// `[click](javascript:alert(1))` yields the whole call rather than a prefix
/// that might look harmless. Image targets are included: an image renders as
/// plain text, but refusing `![a](javascript:..)` costs nothing.
fn link_targets(segment: &str) -> Vec<String> {
    let characters: Vec<char> = segment.chars().collect();
    let mut found = Vec::new();
    let mut index = 0;

    while index < characters.len() {
        let Some(open) = (index..characters.len()).find(|i| characters[*i] == '[') else {
            break;
        };
        let Some(close) = (open + 1..characters.len()).find(|i| characters[*i] == ']') else {
            break;
        };
        if characters.get(close + 1) != Some(&'(') {
            index = open + 1;
            continue;
        }
        let Some(end) = matching_paren(&characters, close + 1) else {
            index = open + 1;
            continue;
        };

        found.push(characters[close + 2..end].iter().collect());
        index = end + 1;
    }

    found
}

/// The index of the `)` closing the `(` at `start`.
fn matching_paren(characters: &[char], start: usize) -> Option<usize> {
    let mut depth = 0_i32;
    for (offset, character) in characters.iter().enumerate().skip(start) {
        match character {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(offset);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_comparisons_are_not_html() {
        // The rule that matters: a validator refusing `a < b` would be
        // refusing ordinary meeting notes.
        for text in [
            "a < b",
            "2 < 3",
            "2 < 3 > 1",
            // Bare, with nothing after it: a name at the very end of the
            // content is still not a tag, because there is no closing bracket.
            "a<b",
            "a<b was measured",
            "x < y and y > z",
        ] {
            assert_eq!(find_html_construct(text), None, "{text}");
            assert_eq!(inspect(text), None, "{text}");
        }
    }

    #[test]
    fn recognizable_constructs_are_refused() {
        for text in [
            "<script>",
            "</div>",
            "<!-- comment -->",
            "<?xml version=\"1.0\"?>",
            "<!DOCTYPE html>",
            "<br/>",
            "<SCRIPT>",
            "<a href=\"x\">",
            "<![CDATA[x]]>",
        ] {
            assert!(find_html_construct(text).is_some(), "{text}");
            assert_eq!(
                inspect(&format!("note {text} note")),
                Some(NoteProblem::RawHtml),
                "{text}"
            );
        }
    }

    #[test]
    fn code_is_literal_and_is_not_scanned() {
        // Writing about a script tag is an ordinary thing to do in a note, and
        // the renderer shows a fence as text, so there is nothing to refuse.
        assert_eq!(inspect("`<script>`"), None);
        assert_eq!(inspect("```\n<script>alert(1)</script>\n```"), None);
        assert_eq!(inspect("`[a](javascript:alert(1))`"), None);
    }

    #[test]
    fn an_unclosed_backtick_does_not_swallow_the_note() {
        assert_eq!(inspect("a ` b <script>"), Some(NoteProblem::RawHtml));
    }

    #[test]
    fn allowed_schemes_pass_and_others_do_not() {
        for target in [
            "http://example.invalid/x",
            "https://example.invalid/x",
            "HTTPS://example.invalid/x",
            "mailto:budi@example.invalid",
        ] {
            assert!(is_allowed_link_target(target), "{target}");
        }

        for target in [
            "javascript:alert(1)",
            "JaVaScRiPt:alert(1)",
            "  javascript:alert(1)  ",
            "data:text/html;base64,PHNjcmlwdD4=",
            "file:///etc/passwd",
            "vbscript:msgbox",
            "blob:abc",
            "../notes.md",
            "//example.invalid/a",
            "",
            "   ",
            "https:",
            "https://",
            "http://[unbalanced",
            "https:// spaced.invalid",
            "mailto:",
        ] {
            assert!(!is_allowed_link_target(target), "{target}");
        }
    }

    #[test]
    fn a_target_is_read_with_balanced_parentheses() {
        // A prefix would read `javascript:alert(1` and might look harmless;
        // the whole call is what the renderer would have used.
        let targets = link_targets("[click](javascript:alert(1))");
        assert_eq!(targets, vec!["javascript:alert(1)".to_owned()]);
    }

    #[test]
    fn an_image_target_is_validated_even_though_it_renders_as_text() {
        assert_eq!(
            inspect("![a](javascript:alert(1))"),
            Some(NoteProblem::LinkScheme)
        );
        assert_eq!(inspect("![a](https://example.invalid/a.png)"), None);
    }

    #[test]
    fn the_size_limit_counts_bytes() {
        assert_eq!(MAX_NOTE_BYTES, 65536);
        assert_eq!(inspect(&"a".repeat(MAX_NOTE_BYTES)), None);
        assert_eq!(
            inspect(&"a".repeat(MAX_NOTE_BYTES + 1)),
            Some(NoteProblem::TooLarge)
        );

        // Two-byte characters reach the limit at half the character count.
        let two_byte = "é".repeat(MAX_NOTE_BYTES / 2);
        assert_eq!(two_byte.len(), MAX_NOTE_BYTES);
        assert_eq!(inspect(&two_byte), None);
        assert_eq!(
            inspect(&format!("{two_byte}a")),
            Some(NoteProblem::TooLarge)
        );
    }

    #[test]
    fn tab_and_newline_are_content_and_the_rest_are_not() {
        assert_eq!(inspect("a\tb\nc"), None);
        for character in ['\u{0}', '\u{7}', '\r', '\u{1b}', '\u{7f}', '\u{b}'] {
            assert_eq!(
                inspect(&format!("before{character}after")),
                Some(NoteProblem::ControlCharacter),
                "{character:?}"
            );
        }
    }

    #[test]
    fn a_refusal_names_the_expected_and_the_detected_value() {
        let error = validate(&"a".repeat(MAX_NOTE_BYTES + 5)).unwrap_err();
        let DomainError::Validation {
            expected, detected, ..
        } = &error
        else {
            panic!("{error:?}");
        };
        assert!(expected.contains("65536"), "{expected}");
        assert!(detected.contains("65541"), "{detected}");
    }

    #[test]
    fn a_refusal_never_quotes_the_whole_note_back() {
        let secret = "Budget overrun of 40 percent, do not circulate";
        let error = validate(&format!("{secret} [x](javascript:alert(1))")).unwrap_err();
        let message = error.to_string();
        assert!(!message.contains(secret), "{message}");
        assert!(message.contains("javascript"), "{message}");
    }

    #[test]
    fn ordinary_notes_pass() {
        assert_eq!(
            inspect("## Agenda\n\n- Budget\n- Venue\n\nSee [notes](https://example.invalid/n)."),
            None
        );
    }

    #[test]
    fn problem_names_match_the_shared_fixture_vocabulary() {
        assert_eq!(NoteProblem::Empty.as_str(), "empty");
        assert_eq!(NoteProblem::TooLarge.as_str(), "too_large");
        assert_eq!(NoteProblem::RawHtml.as_str(), "raw_html");
        assert_eq!(NoteProblem::LinkScheme.as_str(), "link_scheme");
        assert_eq!(NoteProblem::ControlCharacter.as_str(), "control_character");
    }
}
