# 0007. Canonical stored note format: GFM-subset Markdown

- Status: Accepted
- Date: 2026-09-20

## Context

PRD section 13 requires notes to support plain text, paragraphs, headings,
ordered and unordered lists, tables and links, with no image attachments.

Whatever format is chosen is stored in `notes.content` and in every
`note_versions` row, is produced identically by three separate UI bundles
(Host, LAN, offline remote form), and is consumed by the export renderers,
which must be deterministic (architecture rules section 19).

Options:

1. **GFM-subset Markdown stored as text.** Covers every required construct.
   Human-readable in the database and in exports. Markdown export becomes
   near-trivial and AI Context nearly free. The remote form stays small, which
   matters because it must be a single offline file. Cost: a plain or lightly
   assisted editing experience, and a strict renderer plus sanitiser is needed
   wherever it is displayed.
2. **Constrained document JSON (ProseMirror/TipTap).** Gives a true WYSIWYG
   editor. Cost: a second versioned schema to maintain, a Rust-side AST walker
   for every export format, and a noticeably larger remote form bundle.

## Decision

Notes are stored as **GFM-subset Markdown text**.

- `notes.content` and `note_versions.content` hold Markdown text.
- The permitted subset is an explicit allowlist: paragraph, heading, ordered
  list, unordered list, table, link, inline emphasis, code.
- Raw HTML inside Markdown is rejected.
- Link schemes are limited to `http`, `https` and `mailto`.
- The editor, allowlist and renderer live in `packages/editor` and are used
  identically by all three UI bundles.
- The TypeScript renderer and the Rust renderer must agree on the same subset;
  shared fixtures are exercised by both test suites.

A WYSIWYG surface can be layered on later without changing storage, which is
not true in the other direction.

## Consequences

- Export determinism is straightforward, since stored content is already text.
- `packages/editor` is bundled into the offline remote form, so it may pull in
  no external resource and make no network call.
- Participant Markdown is hostile input when rendered in the Host WebView:
  sanitise at every render point, not only at import.
- Tables in the GFM subset are plain pipe tables; complex layouts are out of
  scope. Accepted.
- If PDF export lands in Phase 2 (ADR-0004), its renderer consumes the same
  Markdown subset rather than a second document model.
