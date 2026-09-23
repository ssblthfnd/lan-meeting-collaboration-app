import { defineConfig } from 'vitest/config';

/**
 * TypeScript tests.
 *
 * Scoped to the two packages that carry logic rather than wiring:
 *
 * | Package | What is under test |
 * | --- | --- |
 * | `packages/editor` | a Markdown parser, a scheme allowlist, a renderer whose safety property is the whole point |
 * | `packages/contracts` | the canonical byte form of a remote submission (step 9) |
 * | `apps/remote-form` | the offline form: what it reads, what it refuses, what it exports (step 9) |
 *
 * The two React bundles stay out. They are thin over typed commands that Rust
 * already tests against a real database, so a component suite would mostly
 * assert that React renders.
 *
 * `apps/remote-form` is the exception among the bundles, and for the reason
 * ADR-0009 gives: it has no framework, it runs from `file://` with no backend
 * behind it, and nothing it does is re-checked by a server. Its behaviour -
 * reading its own baked-in context, refusing a note the backend would refuse,
 * and producing a submission with the identity the Host minted - is only
 * asserted here.
 *
 * `packages/contracts` joined this list in step 9 for one reason: ADR-0021
 * freezes a canonical serialisation that **two languages** have to agree on
 * byte for byte, and `__fixtures__/submissions.json` is read at run time by
 * this suite and by `crates/app-remote/tests/fixtures.rs`. Agreement that is
 * only asserted on one side is not agreement.
 *
 * Tests live in each package's `tests/` directory, deliberately not beside the
 * sources. `packages/editor/src` is scanned by a boundary guard for absolute
 * URL literals, because that directory is bundled into the offline remote form
 * - and the fixtures have to contain absolute URLs to test the link allowlist.
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
    include: [
      'packages/editor/tests/**/*.test.ts',
      'packages/contracts/tests/**/*.test.ts',
      'apps/remote-form/tests/**/*.test.ts',
    ],
    environment: 'happy-dom',
    // The fixtures are shared with the Rust suite; a change that breaks one
    // language should not be reported as a flake in the other.
    restoreMocks: true,
  },
});
