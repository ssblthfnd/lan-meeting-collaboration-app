/**
 * Shared note editor and renderer.
 *
 * The same note surface exists in all three UI bundles - the Host dashboard,
 * the LAN participant page, and the offline remote form - so that a note
 * written in one produces byte-identical stored content in the others. Keeping
 * it here prevents three divergent editors.
 *
 * Canonical stored format: **GFM-subset Markdown as text**
 * (`docs/adr/0007-note-canonical-format.md`, `docs/adr/0019-note-editing-and-markdown-subset.md`).
 *
 * # What this package is
 *
 * | Module | Answers |
 * | --- | --- |
 * | [`scheme`] | may this link target become an anchor |
 * | [`scan`] | which parts of a note are literal code, and where its link targets are |
 * | [`validate`] | would the backend accept this content |
 * | [`parse`] | what does this note contain |
 * | [`render`] | show it, safely |
 * | [`serialize`] | write it back out, when content was constructed rather than typed |
 *
 * # Constraints on everything added here
 *
 * - **Framework-neutral.** It is bundled into `apps/remote-form`, which has no
 *   UI framework by decision (ADR-0009). Nothing here may import React, and
 *   the rendering surface is DOM nodes that each bundle appends however it
 *   likes.
 * - **No network.** No `fetch`, no socket, no external resource.
 * - **No absolute URL literal**, anywhere in `src`. The remote form's build
 *   guard reads the built bytes and refuses one, and that guard must not be
 *   weakened for this package.
 * - **No HTML sink.** No `innerHTML`, no HTML-string rendering. Text reaches
 *   the DOM through `textContent` and nothing else.
 * - **Raw HTML is refused**, and link schemes are limited to `http`, `https`
 *   and `mailto`.
 * - It must agree with the Rust validator in `crates/app-core/src/note.rs`,
 *   and later with the Rust renderer in `app-export`, on the same subset. The
 *   shared fixtures under `__fixtures__/` are what hold the two together.
 *
 * # What this package is not
 *
 * Not the enforcement. `validate` here is for telling a Host about a problem
 * while they type; whether a note is *stored* is decided in Rust, inside the
 * mutating transaction (architecture rules section 3).
 */

export type { Align, Block, Span } from './parse';
export { parseInline, parseMarkdown } from './parse';

export type { LinkTarget, Segment } from './scan';
export { inlineSegments, isFence, linkTargets, markupSegments } from './scan';

export { ALLOWED_SCHEMES, isAllowedLinkTarget, safeLinkTarget } from './scheme';

export { renderBlocks, renderMarkdown } from './render';

export { normalizeMarkdown, serializeBlocks } from './serialize';

export type { NoteProblem, NoteProblemReason } from './validate';
export {
  byteLength,
  findHtmlConstruct,
  isValidNoteContent,
  MAX_NOTE_BYTES,
  validateNoteContent,
} from './validate';
