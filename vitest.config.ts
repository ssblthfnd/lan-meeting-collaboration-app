import { defineConfig } from 'vitest/config';

/**
 * TypeScript tests.
 *
 * Scoped to `packages/editor` on purpose. That package is the one piece of
 * TypeScript in this repository carrying logic rather than wiring: a Markdown
 * parser, a scheme allowlist and a renderer whose safety property is the whole
 * point. The UI bundles are thin over typed commands that Rust already tests
 * against a real database, so a component test suite would mostly assert that
 * React renders.
 *
 * Tests live in `packages/editor/tests`, deliberately not beside the sources.
 * `packages/editor/src` is scanned by a boundary guard for absolute URL
 * literals, because that directory is bundled into the offline remote form -
 * and the fixtures have to contain absolute URLs to test the link allowlist.
 * Keeping them apart means the guard needs no exclusion list, which is how an
 * exclusion list stays out of the offline guard too (ADR-0009).
 *
 * `happy-dom` rather than `node`, because the renderer's guarantee is about
 * DOM nodes - no HTML string, no `innerHTML`, `textContent` for every
 * character of author text. Asserting that against a hand-written stand-in
 * would be asserting it against the stand-in.
 */
export default defineConfig({
  test: {
    include: ['packages/editor/tests/**/*.test.ts'],
    environment: 'happy-dom',
    // The fixtures are shared with the Rust suite; a change that breaks one
    // language should not be reported as a flake in the other.
    restoreMocks: true,
  },
});
