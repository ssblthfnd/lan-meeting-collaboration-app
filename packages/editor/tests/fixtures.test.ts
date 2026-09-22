/**
 * The shared fixtures, from the TypeScript side.
 *
 * `crates/app-core/tests/note_fixtures.rs` reads the same file and asserts the
 * same outcomes. That is what ADR-0007's "the TypeScript renderer and the Rust
 * renderer must agree" reduces to in practice: one table of cases, two
 * implementations, and a failure in whichever one drifts.
 */

import { describe, expect, it } from 'vitest';

import { parseMarkdown } from '../src/parse';
import { renderMarkdown } from '../src/render';
import { normalizeMarkdown } from '../src/serialize';
import { validateNoteContent } from '../src/validate';
import { fixtures, plainTextOf, type Fixture } from './support';

describe('shared fixtures', () => {
  it('covers both directions', () => {
    expect(fixtures.valid.length).toBeGreaterThan(20);
    expect(fixtures.invalid.length).toBeGreaterThan(20);
  });

  it('names every case exactly once', () => {
    const names = [...fixtures.valid, ...fixtures.invalid].map((entry) => entry.name);
    expect(new Set(names).size).toBe(names.length);
  });

  it('uses only declared refusal reasons', () => {
    for (const fixture of fixtures.invalid) {
      expect(fixtures.reasons).toContain(fixture.reason);
    }
  });

  describe.each(fixtures.valid)('valid: $name', (fixture: Fixture) => {
    it('is accepted', () => {
      expect(validateNoteContent(fixture.markdown)).toBeNull();
    });

    it('renders to the recorded plain text', (context) => {
      if (fixture.text === undefined) {
        context.skip();
        return;
      }
      expect(plainTextOf(renderMarkdown(fixture.markdown, document))).toBe(fixture.text);
    });

    it('serialises idempotently', () => {
      const once = normalizeMarkdown(fixture.markdown);
      expect(normalizeMarkdown(once)).toBe(once);
    });
  });

  describe.each(fixtures.invalid)('invalid: $name', (fixture: Fixture) => {
    it('is refused for the recorded reason', () => {
      const problem = validateNoteContent(fixture.markdown);
      expect(problem).not.toBeNull();
      expect(problem?.reason).toBe(fixture.reason);
    });

    it('still parses and renders without throwing', () => {
      // Refused content reaches the renderer anyway: a note stored before a
      // rule existed, or one arriving from a future import, has to display
      // rather than break the window displaying it.
      expect(() => parseMarkdown(fixture.markdown)).not.toThrow();
      expect(() => renderMarkdown(fixture.markdown, document)).not.toThrow();
    });
  });
});
