/**
 * The note content rules, as the editor applies them.
 *
 * **This is not the enforcement.** `crates/app-core/src/note.rs` runs the same
 * rules inside the mutating transaction, and that is the copy that decides
 * whether a note is stored (architecture rules section 3). This copy exists so
 * a Host is told about a problem while they are typing rather than after they
 * press save.
 *
 * The two are kept in step by `packages/editor/__fixtures__/notes.json`, which
 * both test suites read. A rule that only one of them enforces is a bug in
 * whichever one is missing it, and the fixture is where that shows up.
 */

import { linkTargets, markupSegments } from './scan';
import { ALLOWED_SCHEMES, isAllowedLinkTarget } from './scheme';

/**
 * Largest note content, in UTF-8 bytes.
 *
 * Bytes rather than characters, because that is what SQLite stores and what
 * an export has to carry. A note of 32769 two-byte characters is over the
 * limit even though it is well under 65536 characters, and the fixtures pin
 * that case.
 */
export const MAX_NOTE_BYTES = 64 * 1024;

/** Why content was refused. Shared vocabulary with the Rust validator. */
export type NoteProblemReason =
  | 'empty'
  | 'too_large'
  | 'raw_html'
  | 'link_scheme'
  | 'control_character';

/** A refusal, phrased the way the architecture rules require. */
export interface NoteProblem {
  readonly reason: NoteProblemReason;
  /** What was expected. */
  readonly expected: string;
  /** What was found. Never the whole note, and never a credential. */
  readonly detected: string;
}

/** The size of `content` in UTF-8 bytes. */
export function byteLength(content: string): number {
  return new TextEncoder().encode(content).length;
}

/**
 * Check content against every rule, returning the first problem.
 *
 * Ordered cheapest and most fundamental first, so an empty note is reported as
 * empty rather than as whatever else a scanner would notice about it.
 */
export function validateNoteContent(content: string): NoteProblem | null {
  if (content.trim() === '') {
    return {
      reason: 'empty',
      expected: 'non-empty content',
      detected: content === '' ? 'an empty string' : 'only whitespace',
    };
  }

  const bytes = byteLength(content);
  if (bytes > MAX_NOTE_BYTES) {
    return {
      reason: 'too_large',
      expected: `at most ${MAX_NOTE_BYTES} bytes`,
      detected: `${bytes} bytes`,
    };
  }

  const control = findControlCharacter(content);
  if (control !== null) {
    return {
      reason: 'control_character',
      expected: 'no control characters other than tab and newline',
      detected: control,
    };
  }

  for (const segment of markupSegments(content)) {
    const html = findHtmlConstruct(segment);
    if (html !== null) {
      return {
        reason: 'raw_html',
        expected: 'Markdown without raw HTML',
        detected: html,
      };
    }

    for (const link of linkTargets(segment)) {
      if (!isAllowedLinkTarget(link.target)) {
        return {
          reason: 'link_scheme',
          expected: `a link target using ${ALLOWED_SCHEMES.join(', ')}`,
          detected: describeTarget(link.target),
        };
      }
    }
  }

  return null;
}

/** Convenience for callers that only need a yes or no. */
export function isValidNoteContent(content: string): boolean {
  return validateNoteContent(content) === null;
}

/**
 * The first prohibited control character, described, or `null`.
 *
 * Tab and newline are content. Everything else below U+0020, and U+007F, is
 * not: a carriage return, an escape or a null byte in stored Markdown is
 * either a mistake or an attempt at something. Carriage returns in particular
 * are normalised away by the transport before content reaches validation, so
 * one arriving here means it came from somewhere that skipped that step.
 */
function findControlCharacter(content: string): string | null {
  for (const character of content) {
    const code = character.codePointAt(0) ?? 0;
    const allowed = character === '\n' || character === '\t';
    if (!allowed && (code < 0x20 || code === 0x7f)) {
      return `U+${code.toString(16).toUpperCase().padStart(4, '0')}`;
    }
  }
  return null;
}

/**
 * The first recognizable raw HTML construct in a markup segment, or `null`.
 *
 * The rule that matters here is the one it does **not** catch. `a < b` and
 * `2 < 3` are arithmetic, and a validator that refused them would be refusing
 * ordinary notes. So a bare `<` is text; what is refused is a construct a
 * browser would recognise:
 *
 * | Construct | Example |
 * | --- | --- |
 * | comment | `<!-- note -->` |
 * | declaration | `<!DOCTYPE html>`, `<![CDATA[ ]]>` |
 * | processing instruction | `<?xml ?>` |
 * | tag, open or closing, that is actually closed | `<script>`, `</div>`, `<br/>` |
 *
 * A tag needs its `>` on the same segment, which is what keeps `a<b was here`
 * out of it: a name followed by prose and no bracket is not a tag.
 */
export function findHtmlConstruct(segment: string): string | null {
  for (let index = 0; index < segment.length; index += 1) {
    if (segment[index] !== '<') {
      continue;
    }

    const rest = segment.slice(index);
    if (rest.startsWith('<!--')) {
      return '<!--';
    }
    if (rest.startsWith('<!')) {
      return '<!';
    }
    if (rest.startsWith('<?')) {
      return '<?';
    }

    const tag = /^<\/?[A-Za-z][A-Za-z0-9-]*[^<>]*>/.exec(rest);
    if (tag !== null) {
      const matched = tag[0];
      return matched.length > 24 ? `${matched.slice(0, 24)}…` : matched;
    }
  }

  return null;
}

/**
 * Describe a refused link target without quoting an unbounded string back.
 *
 * The scheme is the actionable part: it is what the author has to change, and
 * it is short. A whole target could be long, and repeating hostile input into
 * an error message is a habit worth not having.
 */
function describeTarget(target: string): string {
  const trimmed = target.trim();
  if (trimmed === '') {
    return 'an empty link target';
  }
  const scheme = /^([A-Za-z][A-Za-z0-9+.-]*):/.exec(trimmed);
  return scheme?.[1] === undefined
    ? 'a link target with no scheme'
    : `the scheme ${scheme[1].toLowerCase()}`;
}
