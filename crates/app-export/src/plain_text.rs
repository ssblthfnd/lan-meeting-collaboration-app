//! Blocks/spans to plain text.
//!
//! This is the Rust side of the plain-text contract `packages/editor/tests/
//! support.ts::plainTextOf` already pins on the TypeScript side by walking a
//! rendered DOM: a block-level construct (paragraph, heading, list item, code
//! block, table row) is followed by exactly one line break; a table cell is
//! followed by exactly one space; runs of blank lines collapse to one; the
//! whole result is trimmed. This module produces the same string from the
//! same shared AST instead of a DOM, so TXT and AI Context agree with the
//! oracle `packages/editor/__fixtures__/notes.json`'s `text` field already
//! pins for the TypeScript renderer (E-4).
//!
//! A heading's level and a list's marker/number are never emitted here - a
//! heading contributes only its text, exactly as `plainTextOf` already
//! discards `<h1>`-`<h6>` and `<li>` markup down to text plus one newline.

use crate::markdown_ast::{Block, Span};

/// Render parsed blocks to the plain-text contract.
#[must_use]
pub fn render_plain_text(blocks: &[Block]) -> String {
    let mut out = String::new();
    for block in blocks {
        render_block(block, &mut out);
    }
    collapse_and_trim(&out)
}

fn render_block(block: &Block, out: &mut String) {
    match block {
        Block::Paragraph(spans) | Block::Heading { spans, .. } => {
            render_spans(spans, out);
            out.push('\n');
        }
        Block::List { items } => {
            for item in items {
                render_spans(item, out);
                out.push('\n');
            }
        }
        Block::Table { header, rows } => {
            render_row(header, out);
            for row in rows {
                render_row(row, out);
            }
        }
        Block::Code { text } => {
            out.push_str(text);
            out.push('\n');
        }
    }
}

fn render_row(cells: &[Vec<Span>], out: &mut String) {
    for (index, cell) in cells.iter().enumerate() {
        if index > 0 {
            out.push(' ');
        }
        render_spans(cell, out);
    }
    out.push('\n');
}

fn render_spans(spans: &[Span], out: &mut String) {
    for span in spans {
        render_span(span, out);
    }
}

fn render_span(span: &Span, out: &mut String) {
    match span {
        Span::Text(text) | Span::Code(text) => out.push_str(text),
        Span::Break => out.push('\n'),
        Span::Emphasis(spans) | Span::Strong(spans) | Span::Link(spans) => render_spans(spans, out),
    }
}

/// `.replace(/[ \t]+\n/g, '\n').replace(/\n{2,}/g, '\n').trim()`, matching
/// `plainTextOf` exactly.
fn collapse_and_trim(text: &str) -> String {
    let mut collapsed = String::with_capacity(text.len());
    let mut pending_space_run: Vec<char> = Vec::new();

    for c in text.chars() {
        if c == ' ' || c == '\t' {
            pending_space_run.push(c);
            continue;
        }
        if c == '\n' {
            // Trailing spaces/tabs before a newline are dropped.
            pending_space_run.clear();
            collapsed.push('\n');
            continue;
        }
        collapsed.extend(pending_space_run.drain(..));
        collapsed.push(c);
    }
    collapsed.extend(pending_space_run.drain(..));

    let mut result = String::with_capacity(collapsed.len());
    let mut previous_was_newline = false;
    for c in collapsed.chars() {
        if c == '\n' {
            if previous_was_newline {
                continue;
            }
            previous_was_newline = true;
        } else {
            previous_was_newline = false;
        }
        result.push(c);
    }

    result.trim().to_owned()
}
