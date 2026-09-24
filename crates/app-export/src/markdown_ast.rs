//! A bounded parser for the canonical GFM-subset note format.
//!
//! This is **not** a general Markdown/CommonMark parser and introduces **no**
//! new Markdown dialect. It parses exactly the subset ADR-0007 already
//! allowlists (paragraph, ATX heading 1-6, unordered/ordered list - flat, one
//! level - GFM pipe table, link, emphasis, strong, inline code, fenced code,
//! hard line break) and mirrors `packages/editor/src/parse.ts` construct for
//! construct, so the plain-text rendering built on top of this AST agrees with
//! the same oracle the TypeScript side is already tested against
//! (`packages/editor/__fixtures__/notes.json`'s `text` field, via
//! `packages/editor/tests/support.ts::plainTextOf`).
//!
//! Everything outside the subset - a blockquote, a task-list marker, a
//! strikethrough, a footnote reference, a setext underline, an autolink - is
//! not an error here either: it renders as plain text, exactly as the
//! TypeScript renderer already treats it. Input reaching this parser has
//! already passed `app_core::note::validate` (no raw HTML, no disallowed link
//! scheme, no control character), so this parser never refuses anything; it
//! has no error path.

/// Column alignment of a pipe table. Carried for completeness; no renderer in
/// this crate currently reads it (plain-text output does not distinguish
/// alignment, matching `plainTextOf`'s DOM-based oracle).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
    None,
}

/// An inline run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Span {
    Text(String),
    Code(String),
    Emphasis(Vec<Span>),
    Strong(Vec<Span>),
    /// The link text only; the target is deliberately not carried further
    /// than parsing, since no renderer in this crate ever emits it (E-4).
    Link(Vec<Span>),
    Break,
}

/// A block-level element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Paragraph(Vec<Span>),
    Heading {
        level: u8,
        spans: Vec<Span>,
    },
    List {
        items: Vec<Vec<Span>>,
    },
    Table {
        header: Vec<Vec<Span>>,
        rows: Vec<Vec<Vec<Span>>>,
    },
    Code {
        text: String,
    },
}

const MAX_HEADING_LEVEL: u8 = 6;
const MAX_TABLE_ROWS: usize = 1000;
const MAX_TABLE_COLUMNS: usize = 32;

/// Parse note content into blocks.
#[must_use]
pub fn parse_markdown(content: &str) -> Vec<Block> {
    let lines: Vec<&str> = content.split('\n').collect();
    let mut blocks = Vec::new();
    let mut index = 0usize;

    while index < lines.len() {
        let line = lines[index];

        if line.trim().is_empty() {
            index += 1;
            continue;
        }

        if let Some(fence) = opening_fence(line) {
            let mut body: Vec<&str> = Vec::new();
            index += 1;
            while index < lines.len() && opening_fence(lines[index]).is_none() {
                body.push(lines[index]);
                index += 1;
            }
            // Step past the closing fence when there is one. An unclosed
            // fence runs to the end of the note, matching parse.ts.
            if index < lines.len() {
                index += 1;
            }
            let _ = fence; // language is not needed by any renderer here.
            blocks.push(Block::Code {
                text: body.join("\n"),
            });
            continue;
        }

        if let Some((level, text)) = heading_line(line) {
            blocks.push(Block::Heading {
                level,
                spans: parse_inline(&strip_trailing_hashes(text)),
            });
            index += 1;
            continue;
        }

        if let Some((block, next)) = parse_table(&lines, index) {
            blocks.push(block);
            index = next;
            continue;
        }

        if let Some((block, next)) = parse_list(&lines, index) {
            blocks.push(block);
            index = next;
            continue;
        }

        let (spans, next) = take_paragraph(&lines, index);
        blocks.push(Block::Paragraph(spans));
        index = next;
    }

    blocks
}

/// The fence's info string, or `None` if the line does not open one.
fn opening_fence(line: &str) -> Option<&str> {
    let trimmed = strip_up_to_three_leading_spaces(line);
    trimmed.strip_prefix("```").map(str::trim)
}

/// Whether a line starts a block a paragraph cannot absorb.
fn starts_block(line: &str) -> bool {
    line.trim().is_empty()
        || opening_fence(line).is_some()
        || heading_line(line).is_some()
        || list_marker(line).is_some()
}

/// `(level, remaining text)` for an ATX heading line, if it is one.
fn heading_line(line: &str) -> Option<(u8, &str)> {
    let rest = strip_up_to_three_leading_spaces(line);
    let hashes = rest.chars().take_while(|c| *c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let after = &rest[hashes..];
    if !after.starts_with(|c: char| c.is_whitespace()) {
        return None;
    }
    let level =
        u8::try_from(hashes.min(usize::from(MAX_HEADING_LEVEL))).unwrap_or(MAX_HEADING_LEVEL);
    Some((level, after.trim_start()))
}

/// A trailing run of `#` characters is ATX closing syntax, not content,
/// matching parse.ts's `text.replace(/\s+#+\s*$/, '')`.
fn strip_trailing_hashes(text: &str) -> String {
    let trimmed = text.trim_end();
    match find_trailing_hash_run(trimmed) {
        Some(pos) => trimmed[..pos].trim_end().to_owned(),
        None => trimmed.to_owned(),
    }
}

/// Position where a whitespace-preceded trailing run of `#` begins, if any:
/// the exact semantics of the regex `\s+#+\s*$`.
fn find_trailing_hash_run(text: &str) -> Option<usize> {
    let len = text.len();
    let mut chars: Vec<(usize, char)> = text.char_indices().collect();

    // Walk backwards over trailing `#`.
    let mut hash_start = len;
    while let Some(&(idx, c)) = chars.last() {
        if c == '#' {
            hash_start = idx;
            chars.pop();
        } else {
            break;
        }
    }
    if hash_start == len {
        return None; // no trailing hashes at all
    }

    // Require at least one whitespace character immediately before the run.
    let mut ws_start = hash_start;
    while let Some(&(idx, c)) = chars.last() {
        if c.is_whitespace() {
            ws_start = idx;
            chars.pop();
        } else {
            break;
        }
    }
    if ws_start == hash_start {
        None
    } else {
        Some(ws_start)
    }
}

/// Strip at most three leading spaces (GFM's tolerance for ATX/fences).
fn strip_up_to_three_leading_spaces(line: &str) -> &str {
    let mut rest = line;
    for _ in 0..3 {
        if let Some(stripped) = rest.strip_prefix(' ') {
            rest = stripped;
        } else {
            break;
        }
    }
    rest
}

/// The content of a list item, or `None` when the line is not one.
///
/// Indentation is ignored, not counted - a deeper marker is a sibling item at
/// the one flat level this subset has, matching `parse.ts`'s `listMarker`.
fn list_marker(line: &str) -> Option<(bool, &str)> {
    let trimmed = line.trim_start();

    if let Some(rest) = trimmed.strip_prefix('-') {
        if rest.starts_with(|c: char| c.is_whitespace()) {
            return Some((false, rest.trim_start()));
        }
        return None;
    }

    // Ordered: one to nine digits, then `.` or `)`, then whitespace.
    let digit_len = trimmed.chars().take_while(char::is_ascii_digit).count();
    if (1..=9).contains(&digit_len) {
        let after_digits = &trimmed[digit_len..];
        let after_marker = after_digits
            .strip_prefix('.')
            .or_else(|| after_digits.strip_prefix(')'));
        if let Some(rest) = after_marker {
            if rest.starts_with(|c: char| c.is_whitespace()) {
                return Some((true, rest.trim_start()));
            }
        }
    }
    None
}

/// Consecutive lines forming one paragraph, with soft/hard breaks resolved.
fn take_paragraph(lines: &[&str], from: usize) -> (Vec<Span>, usize) {
    let mut spans = Vec::new();
    let mut index = from;

    while index < lines.len() {
        let line = lines[index];
        if index > from && starts_block(line) {
            break;
        }

        let hard = ends_with_two_or_more_spaces(line);
        spans.extend(parse_inline(line.trim()));
        index += 1;

        let more = index < lines.len() && !starts_block(lines[index]);
        if more {
            if hard {
                spans.push(Span::Break);
            } else {
                spans.push(Span::Text(" ".to_owned()));
            }
        }
    }

    (spans, index)
}

fn ends_with_two_or_more_spaces(line: &str) -> bool {
    let trimmed_end = line.trim_end_matches('\t');
    let trailing = trimmed_end.len() - trimmed_end.trim_end_matches(' ').len();
    trailing >= 2
}

/// One flat list, at the one level this subset supports.
fn parse_list(lines: &[&str], from: usize) -> Option<(Block, usize)> {
    let (ordered, first_text) = list_marker(lines[from])?;

    let mut items: Vec<Vec<Span>> = Vec::new();
    let mut current: Vec<String> = vec![first_text.to_owned()];
    let mut index = from + 1;

    while index < lines.len() {
        let line = lines[index];
        let marker = list_marker(line);

        if let Some((item_ordered, text)) = marker {
            if item_ordered == ordered {
                items.push(inline_with_soft_breaks(&current));
                current = vec![text.to_owned()];
                index += 1;
                continue;
            }
            break;
        }
        if line.trim().is_empty() {
            break;
        }
        if starts_block(line) {
            break;
        }
        current.push(line.trim().to_owned());
        index += 1;
    }

    items.push(inline_with_soft_breaks(&current));
    Some((Block::List { items }, index))
}

fn inline_with_soft_breaks(lines: &[String]) -> Vec<Span> {
    let mut spans = Vec::new();
    for (position, line) in lines.iter().enumerate() {
        if position > 0 {
            spans.push(Span::Text(" ".to_owned()));
        }
        spans.extend(parse_inline(line));
    }
    spans
}

/// A GFM pipe table, if `from` is its header and `from + 1` its delimiter row.
fn parse_table(lines: &[&str], from: usize) -> Option<(Block, usize)> {
    let header_line = lines[from];
    let delimiter = *lines.get(from + 1)?;
    if !header_line.contains('|') {
        return None;
    }
    if !is_delimiter_row(delimiter) {
        return None;
    }

    let header_cells: Vec<Vec<Span>> = split_row(header_line)
        .into_iter()
        .map(|cell| parse_inline(&cell))
        .collect();

    let mut rows: Vec<Vec<Vec<Span>>> = Vec::new();
    let mut index = from + 2;
    while index < lines.len() {
        let line = lines[index];
        if line.trim().is_empty() || !line.contains('|') {
            break;
        }
        if rows.len() < MAX_TABLE_ROWS {
            let row: Vec<Vec<Span>> = split_row(line)
                .into_iter()
                .map(|cell| parse_inline(&cell))
                .collect();
            rows.push(row);
        }
        index += 1;
    }

    Some((
        Block::Table {
            header: header_cells.into_iter().take(MAX_TABLE_COLUMNS).collect(),
            rows: rows
                .into_iter()
                .map(|row| row.into_iter().take(MAX_TABLE_COLUMNS).collect())
                .collect(),
        },
        index,
    ))
}

/// Whether `line` is a GFM table delimiter row: `| --- | :--: | ---: |`.
fn is_delimiter_row(line: &str) -> bool {
    let trimmed = strip_up_to_three_leading_spaces(line).trim();
    let inner = trimmed.strip_prefix('|').unwrap_or(trimmed);
    let inner = inner.strip_suffix('|').unwrap_or(inner);
    if inner.trim().is_empty() {
        return false;
    }
    inner.split('|').all(|cell| {
        let cell = cell.trim();
        let cell = cell.strip_prefix(':').unwrap_or(cell);
        let cell = cell.strip_suffix(':').unwrap_or(cell);
        !cell.is_empty() && cell.chars().all(|c| c == '-')
    })
}

/// Split a table row on unescaped pipes, dropping the optional outer pair.
fn split_row(line: &str) -> Vec<String> {
    let trimmed = line.trim();
    let mut cells: Vec<String> = Vec::new();
    let mut cell = String::new();
    let chars: Vec<char> = trimmed.chars().collect();
    let mut index = 0;

    while index < chars.len() {
        let character = chars[index];
        if character == '\\' && chars.get(index + 1) == Some(&'|') {
            cell.push('|');
            index += 2;
            continue;
        }
        if character == '|' {
            cells.push(cell.trim().to_owned());
            cell = String::new();
            index += 1;
            continue;
        }
        cell.push(character);
        index += 1;
    }
    cells.push(cell.trim().to_owned());

    if cells.first().is_some_and(String::is_empty) {
        cells.remove(0);
    }
    if cells.last().is_some_and(String::is_empty) {
        cells.pop();
    }
    cells
}

/// Parse one line of inline Markdown.
///
/// Precedence, highest first: code span, image (literal), link, strong,
/// emphasis - matching `parse.ts::parseInline`.
#[must_use]
pub fn parse_inline(line: &str) -> Vec<Span> {
    let chars: Vec<char> = line.chars().collect();
    let mut spans = Vec::new();
    let mut plain = String::new();
    let mut index = 0usize;

    macro_rules! flush {
        () => {
            if !plain.is_empty() {
                spans.push(Span::Text(std::mem::take(&mut plain)));
            }
        };
    }

    while index < chars.len() {
        let character = chars[index];

        if character == '`' {
            if let Some((text, next)) = read_code_span(&chars, index) {
                flush!();
                spans.push(Span::Code(text));
                index = next;
                continue;
            }
        }

        // `![alt](target)` stays literal, consumed as a unit.
        if character == '!' && chars.get(index + 1) == Some(&'[') {
            if let Some((_, _, next)) = read_bracketed(&chars, index + 1) {
                plain.push_str(&chars[index..next].iter().collect::<String>());
                index = next;
                continue;
            }
        }

        if character == '[' {
            if let Some((label, target, next)) = read_bracketed(&chars, index) {
                if is_allowed_link_target(&target) {
                    flush!();
                    spans.push(Span::Link(parse_inline(&label)));
                } else {
                    plain.push_str(&chars[index..next].iter().collect::<String>());
                }
                index = next;
                continue;
            }
        }

        if character == '*' {
            if let Some((text, next)) = read_delimited(&chars, index, "**") {
                flush!();
                spans.push(Span::Strong(parse_inline(&text)));
                index = next;
                continue;
            }
            if let Some((text, next)) = read_delimited(&chars, index, "*") {
                flush!();
                spans.push(Span::Emphasis(parse_inline(&text)));
                index = next;
                continue;
            }
        }

        plain.push(character);
        index += 1;
    }

    flush!();
    spans
}

/// A conservative allowlist check for a link target, mirroring
/// `packages/editor/src/scheme.ts::safeLinkTarget` and
/// `app_core::note::ALLOWED_LINK_SCHEMES`. Note content has already passed
/// `app_core::note::validate` by the time it reaches this renderer, so this
/// exists only to decide whether a bracketed run *renders as a link at all*
/// (link text vs. literal text) - the target itself is never emitted by any
/// renderer in this crate (E-4).
fn is_allowed_link_target(target: &str) -> bool {
    let trimmed = target.trim();
    if trimmed.is_empty() {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    ["http:", "https:", "mailto:"]
        .iter()
        .any(|scheme| lower.starts_with(scheme))
}

/// A code span starting at `start`, or `None` if the run never closes.
fn read_code_span(chars: &[char], start: usize) -> Option<(String, usize)> {
    let mut index = start;
    while index < chars.len() && chars[index] == '`' {
        index += 1;
    }
    let length = index - start;
    if length == 0 {
        return None;
    }
    let closing = find_backtick_run(chars, index, length)?;
    let text: String = chars[index..closing].iter().collect();
    let stripped = strip_one_surrounding_space(&text);
    Some((stripped, closing + length))
}

/// Position of a backtick run of exactly `length`, starting the search at
/// `from`, or `None` if none exists.
fn find_backtick_run(chars: &[char], from: usize, length: usize) -> Option<usize> {
    let mut index = from;
    while index < chars.len() {
        if chars[index] == '`' {
            let run_start = index;
            let mut run_len = 0usize;
            while index < chars.len() && chars[index] == '`' {
                run_len += 1;
                index += 1;
            }
            if run_len == length {
                return Some(run_start);
            }
            continue;
        }
        index += 1;
    }
    None
}

/// GFM strips exactly one leading and trailing space so `` ` `` can be
/// written inside a span.
fn strip_one_surrounding_space(text: &str) -> String {
    if text.starts_with(' ') && text.ends_with(' ') && text.len() > 1 {
        text[1..text.len() - 1].to_owned()
    } else {
        text.to_owned()
    }
}

/// `[label](target)` starting at the `[` in `start`.
fn read_bracketed(chars: &[char], start: usize) -> Option<(String, String, usize)> {
    let close = find_char(chars, start + 1, ']')?;
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let end = matching_paren(chars, close + 1)?;
    let label: String = chars[start + 1..close].iter().collect();
    let target: String = chars[close + 2..end].iter().collect();
    Some((label, target, end + 1))
}

fn find_char(chars: &[char], from: usize, needle: char) -> Option<usize> {
    (from..chars.len()).find(|&i| chars[i] == needle)
}

/// Index of the `)` matching the `(` at `open`, respecting nesting.
fn matching_paren(chars: &[char], open: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut index = open;
    while index < chars.len() {
        match chars[index] {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

/// A run delimited by `marker` on both sides, non-empty in between.
fn read_delimited(chars: &[char], start: usize, marker: &str) -> Option<(String, usize)> {
    let marker_chars: Vec<char> = marker.chars().collect();
    if !starts_with_at(chars, start, &marker_chars) {
        return None;
    }
    let from = start + marker_chars.len();
    let closing = find_subsequence(chars, from, &marker_chars)?;
    if closing == from {
        return None;
    }
    let text: String = chars[from..closing].iter().collect();
    Some((text, closing + marker_chars.len()))
}

fn starts_with_at(chars: &[char], start: usize, needle: &[char]) -> bool {
    if start + needle.len() > chars.len() {
        return false;
    }
    chars[start..start + needle.len()] == *needle
}

fn find_subsequence(chars: &[char], from: usize, needle: &[char]) -> Option<usize> {
    if needle.is_empty() || from > chars.len() {
        return None;
    }
    (from..=chars.len().saturating_sub(needle.len()))
        .find(|&i| chars[i..i + needle.len()] == *needle)
}
