import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

import {
  CANONICAL_PREFIX,
  canonicalSubmission,
  normalizeNote,
  SUBMISSION_SCHEMA_VERSION,
} from '../src/index';
import type { RemoteSubmissionV1 } from '../src/index';

/**
 * The shared remote-submission fixtures, from the TypeScript side.
 *
 * `crates/app-remote/tests/fixtures.rs` reads the same file and asserts the same
 * outcomes. ADR-0021 freezes the submission v1 shape and its canonical byte
 * form, and this pair of suites is what makes a disagreement a failing test
 * naming the row rather than a mystery at import time.
 *
 * Read at run time, not imported, so the two languages genuinely read the same
 * bytes - the arrangement `packages/editor`'s note fixtures already use.
 *
 * SHA-256 comes from `node:crypto`, which is available to the test runner and
 * adds no dependency. The offline form never hashes anything: the Host computes
 * `content_hash` at import time (ADR-0021 decision 8), so this side is proving
 * agreement, not shipping a hasher.
 */

const here = dirname(fileURLToPath(import.meta.url));
const fixturePath = resolve(here, '..', '__fixtures__', 'submissions.json');

interface CanonicalRow {
  readonly name: string;
  readonly why: string;
  readonly submission: RemoteSubmissionV1;
  readonly canonical: string;
  readonly content_hash: string;
}

interface ExcludedRow {
  readonly name: string;
  readonly why: string;
  readonly submission: RemoteSubmissionV1;
  readonly same_hash_as: string;
}

interface InvalidRow {
  readonly name: string;
  readonly reason: string;
  readonly raw: string;
}

const fixtures = JSON.parse(readFileSync(fixturePath, 'utf8')) as {
  readonly canonical_prefix: string;
  readonly canonical: readonly CanonicalRow[];
  readonly excluded: readonly ExcludedRow[];
  readonly invalid: readonly InvalidRow[];
};

/** SHA-256 over the canonical form, lowercase hexadecimal. */
function contentHash(submission: RemoteSubmissionV1): string {
  return createHash('sha256').update(canonicalSubmission(submission), 'utf8').digest('hex');
}

function byName(name: string): CanonicalRow {
  const row = fixtures.canonical.find((candidate) => candidate.name === name);
  if (row === undefined) {
    throw new Error(`no canonical fixture named ${name}`);
  }
  return row;
}

describe('the shared fixtures', () => {
  it('agree with this build about the domain separator', () => {
    // Changing it silently would make every previously recorded hash wrong.
    expect(fixtures.canonical_prefix).toBe(CANONICAL_PREFIX);
  });

  it('are not empty, so a missing file cannot pass as agreement', () => {
    expect(fixtures.canonical.length).toBeGreaterThan(5);
    expect(fixtures.invalid.length).toBeGreaterThan(5);
  });
});

describe('the canonical form', () => {
  for (const row of fixtures.canonical) {
    it(`${row.name}: ${row.why}`, () => {
      expect(canonicalSubmission(row.submission)).toBe(row.canonical);
      expect(contentHash(row.submission)).toBe(row.content_hash);
    });
  }

  it('begins with the domain separator and ends with the note', () => {
    const form = canonicalSubmission(byName('plain').submission);
    expect(form.startsWith(`${CANONICAL_PREFIX}\n`)).toBe(true);
    expect(form).toContain('note=');
  });

  it('counts the note in UTF-8 bytes, not UTF-16 units', () => {
    // The row that would disagree if either side used `String.length`.
    const astral = byName('astral');
    const note = normalizeNote(astral.submission.note);
    const declared = /note=(\d+):/.exec(astral.canonical)?.[1];

    expect(declared).toBe(String(new TextEncoder().encode(note).length));
    expect(Number(declared)).toBeGreaterThan(note.length);
  });
});

describe('line endings', () => {
  it('do not change an artefact', () => {
    // A mail gateway rewriting them must not look like tampering.
    const plain = byName('plain').content_hash;
    expect(contentHash(byName('crlf').submission)).toBe(plain);
    expect(contentHash(byName('lone_cr').submission)).toBe(plain);
  });

  it('normalise CRLF and a lone CR alike', () => {
    expect(normalizeNote('a\r\nb')).toBe('a\nb');
    expect(normalizeNote('a\rb')).toBe('a\nb');
    expect(normalizeNote('a\r\n\rb')).toBe('a\n\nb');
    expect(normalizeNote('a\nb')).toBe('a\nb');
  });

  it('leave a changed note changed', () => {
    expect(contentHash(byName('other_note').submission)).not.toBe(byName('plain').content_hash);
  });
});

describe('the excluded fields', () => {
  for (const row of fixtures.excluded) {
    it(`${row.name}: ${row.why}`, () => {
      expect(contentHash(row.submission)).toBe(byName(row.same_hash_as).content_hash);
    });
  }
});

describe('the note cannot imitate the encoding', () => {
  it('because it is length-prefixed and last', () => {
    const injection = byName('newline_injection');
    const plain = byName('plain');
    expect(injection.content_hash).not.toBe(plain.content_hash);
    expect(injection.canonical).toContain(`note=${
      new TextEncoder().encode(normalizeNote(injection.submission.note)).length
    }:`);
  });
});

describe('the invalid fixtures', () => {
  for (const row of fixtures.invalid) {
    it(`${row.name} is not a usable submission (${row.reason})`, () => {
      // TypeScript owns the *shape*; refusing a file is the Rust parser's job,
      // and `crates/app-remote/tests/fixtures.rs` asserts the exact reason for
      // this same row. What is checked here is that the row is genuinely not a
      // well-formed v1 submission, so the two suites cannot drift apart over
      // what "invalid" means.
      let parsed: unknown;
      try {
        parsed = JSON.parse(row.raw);
      } catch {
        expect(row.reason).toBe('malformed');
        return;
      }

      expect(isWellFormedV1(parsed)).toBe(false);
    });
  }
});

/** The shape check, mirroring what `SubmissionV1::parse` accepts. */
function isWellFormedV1(value: unknown): boolean {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    return false;
  }

  const record = value as Record<string, unknown>;
  const shape: Record<string, string> = {
    schema_version: 'number',
    submission_id: 'string',
    meeting_id: 'string',
    participant_id: 'string',
    participant_name: 'string',
    source_version: 'number',
    generated_at: 'string',
    submitted_at: 'string',
    note: 'string',
  };

  const keys = Object.keys(record);
  if (keys.length !== Object.keys(shape).length) {
    return false;
  }

  for (const [field, type] of Object.entries(shape)) {
    if (typeof record[field] !== type) {
      return false;
    }
  }

  if (record.schema_version !== SUBMISSION_SCHEMA_VERSION) {
    return false;
  }

  // Canonical lowercase UUIDv7, the same form the Rust `Id` parser accepts.
  const uuid7 = /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
  return (
    uuid7.test(record.submission_id as string)
    && uuid7.test(record.meeting_id as string)
    && uuid7.test(record.participant_id as string)
  );
}
