/**
 * Shared helpers for the editor tests.
 *
 * Two jobs: load the fixtures the Rust suite also reads, and turn a rendered
 * fragment into the plain text those fixtures record.
 */

import { existsSync, readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';

/** One fixture row, after `markdown_repeat` has been expanded. */
export interface Fixture {
  readonly name: string;
  readonly why?: string;
  readonly markdown: string;
  /** Recorded plain text, for rows that pin rendering. */
  readonly text?: string;
  /** Present on refused rows only. */
  readonly reason?: string;
}

interface RawFixture {
  readonly name: string;
  readonly why?: string;
  readonly markdown?: string;
  readonly markdown_repeat?: { readonly unit: string; readonly count: number };
  readonly text?: string;
  readonly reason?: string;
}

interface RawFile {
  readonly reasons: readonly string[];
  readonly valid: readonly RawFixture[];
  readonly invalid: readonly RawFixture[];
}

/**
 * Read and expand the fixtures.
 *
 * Read at runtime rather than imported, so the oversized rows stay a
 * `markdown_repeat` directive in the file instead of 64 KiB of literal text
 * that would have to be committed and diffed.
 */
function load(): { reasons: readonly string[]; valid: Fixture[]; invalid: Fixture[] } {
  const raw = JSON.parse(readFileSync(fixturePath(), 'utf8')) as RawFile;
  return {
    reasons: raw.reasons,
    valid: raw.valid.map(expand),
    invalid: raw.invalid.map(expand),
  };
}

/**
 * Locate the fixtures by walking up from the working directory.
 *
 * Not `import.meta.url`: under the `happy-dom` environment that is resolved
 * against the document's base rather than the file system, so it is not a
 * `file:` URL and cannot be turned into a path. Walking up is independent of
 * where the runner was started from, which is the property that actually
 * matters.
 */
function fixturePath(): string {
  const relative = 'packages/editor/__fixtures__/notes.json';
  let directory = process.cwd();

  for (;;) {
    const candidate = resolve(directory, relative);
    if (existsSync(candidate)) {
      return candidate;
    }
    const parent = dirname(directory);
    if (parent === directory) {
      throw new Error(`could not find ${relative} above ${process.cwd()}`);
    }
    directory = parent;
  }
}

function expand(row: RawFixture): Fixture {
  const markdown =
    row.markdown ??
    (row.markdown_repeat === undefined
      ? ''
      : row.markdown_repeat.unit.repeat(row.markdown_repeat.count));

  const fixture: {
    name: string;
    markdown: string;
    why?: string;
    text?: string;
    reason?: string;
  } = { name: row.name, markdown };

  if (row.why !== undefined) {
    fixture.why = row.why;
  }
  if (row.text !== undefined) {
    fixture.text = row.text;
  }
  if (row.reason !== undefined) {
    fixture.reason = row.reason;
  }
  return fixture;
}

export const fixtures = load();

/** Elements that end a line of plain text. */
const BLOCK_TAGS = new Set(['P', 'H1', 'H2', 'H3', 'H4', 'H5', 'H6', 'LI', 'PRE', 'TR']);

/** Elements separated from their siblings by a space rather than a newline. */
const CELL_TAGS = new Set(['TH', 'TD']);

/**
 * The plain text a rendered fragment carries.
 *
 * Not `textContent`, which would lose every structural boundary: a `<br>`
 * contributes nothing to it, and two paragraphs run together. This walks the
 * tree instead, so the fixtures can record a readable string that a future
 * Rust renderer can be held to as well.
 */
export function plainTextOf(node: Node): string {
  const pieces: string[] = [];
  walk(node, pieces);
  return pieces
    .join('')
    .replace(/[ \t]+\n/g, '\n')
    .replace(/\n{2,}/g, '\n')
    .trim();
}

function walk(node: Node, out: string[]): void {
  for (const child of Array.from(node.childNodes)) {
    if (child.nodeType === 3 /* text */) {
      out.push(child.nodeValue ?? '');
      continue;
    }
    if (child.nodeType !== 1 /* element */) {
      continue;
    }

    const element = child as Element;
    if (element.tagName === 'BR') {
      out.push('\n');
      continue;
    }

    walk(element, out);

    if (BLOCK_TAGS.has(element.tagName)) {
      out.push('\n');
    } else if (CELL_TAGS.has(element.tagName)) {
      out.push(' ');
    }
  }
}
