/**
 * Shared note editor and renderer.
 *
 * The same note-writing surface must exist in all three UI bundles - the Host
 * dashboard, the LAN participant page, and the offline remote form - so that a
 * note written in one produces byte-identical stored content in the others.
 * Keeping it here prevents three divergent editors.
 *
 * Canonical stored format: **GFM-subset Markdown as text**
 * (see `docs/adr/0007-note-canonical-format.md`, Accepted).
 *
 * This package will hold:
 * - the allowlist of permitted Markdown constructs (paragraph, heading,
 *   ordered list, unordered list, table, link, inline emphasis, code)
 * - the editor component shared by the three bundles
 * - the renderer and sanitiser used at every render point, including the Host
 *   UI, where participant content is still hostile input
 *
 * Constraints that apply to everything added here:
 * - it is bundled into the offline remote form, so no external CDN resource and
 *   no network call (architecture rules 6, 20)
 * - raw HTML inside Markdown is rejected
 * - link schemes are limited to `http`, `https` and `mailto`
 * - it must agree with the Rust renderer in `app-export` on the same subset,
 *   guarded by shared fixtures
 *
 * Implementation arrives with Phase 1 steps 6 and 7.
 */

export {};
