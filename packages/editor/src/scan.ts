/**
 * Finding the parts of a note that carry markup.
 *
 * Validation and parsing both need the same two questions answered, and they
 * must answer them identically or the editor and the backend will disagree
 * about what a note contains:
 *
 * 1. which regions are **code**, and therefore literal;
 * 2. where the **link targets** are.
 *
 * The Rust validator in `crates/app-core/src/note.rs` implements the same two
 * routines against the same shared fixtures. Anything changed here has to
 * change there.
 *
 * # Code is literal, including angle brackets
 *
 * A fenced block or a code span is text, so a `<script>` inside one is not an
 * HTML construct - it is somebody writing about a script tag, which is an
 * ordinary thing to do in a meeting note. Scanning it as markup would make the
 * application refuse notes it has no reason to refuse.
 *
 * That is safe because code is also *rendered* literally: `render.ts` puts it
 * in a `<code>` element through `textContent`, so nothing inside it is ever
 * interpreted.
 */

/** A stretch of the note, and whether it is literal code. */
export interface Segment {
  readonly text: string;
  readonly code: boolean;
}

/** Whether a line opens or closes a fenced code block. */
export function isFence(line: string): boolean {
  return line.trimStart().startsWith('```');
}

/**
 * Split one line into code spans and everything else.
 *
 * A run of N backticks opens a span that the next run of **exactly** N
 * backticks closes. An unclosed run is literal text, which is what GFM does and
 * what stops a stray backtick from swallowing the rest of a note.
 */
export function inlineSegments(line: string): Segment[] {
  const segments: Segment[] = [];
  let plain = '';
  let index = 0;

  while (index < line.length) {
    if (line[index] !== '`') {
      plain += line[index];
      index += 1;
      continue;
    }

    const open = index;
    while (index < line.length && line[index] === '`') {
      index += 1;
    }
    const fenceLength = index - open;
    const closing = findBacktickRun(line, index, fenceLength);

    if (closing === null) {
      // Unclosed: the backticks are ordinary characters.
      plain += line.slice(open, index);
      continue;
    }

    if (plain !== '') {
      segments.push({ text: plain, code: false });
      plain = '';
    }
    segments.push({ text: line.slice(open, closing + fenceLength), code: true });
    index = closing + fenceLength;
  }

  if (plain !== '') {
    segments.push({ text: plain, code: false });
  }
  return segments;
}

/**
 * The start of the next run of exactly `length` backticks, at or after `from`.
 *
 * Exported because the inline parser closes a code span the same way. If the
 * two disagreed, the validator and the renderer would disagree about where
 * literal text ends, which is precisely where a scanner bypass would live.
 */
export function findBacktickRun(line: string, from: number, length: number): number | null {
  let index = from;
  while (index < line.length) {
    if (line[index] !== '`') {
      index += 1;
      continue;
    }
    const start = index;
    while (index < line.length && line[index] === '`') {
      index += 1;
    }
    if (index - start === length) {
      return start;
    }
  }
  return null;
}

/**
 * Every part of the note that is not literal code.
 *
 * Fenced blocks are dropped whole, including their fence lines. Within the
 * remaining lines, code spans are dropped too.
 */
export function markupSegments(content: string): string[] {
  const found: string[] = [];
  let inFence = false;

  for (const line of content.split('\n')) {
    if (isFence(line)) {
      inFence = !inFence;
      continue;
    }
    if (inFence) {
      continue;
    }
    for (const segment of inlineSegments(line)) {
      if (!segment.code) {
        found.push(segment.text);
      }
    }
  }

  return found;
}

/** One link or image target, as written. */
export interface LinkTarget {
  /** The target between the parentheses, untrimmed. */
  readonly target: string;
  /** True for `![alt](target)`, which is never rendered as an anchor. */
  readonly image: boolean;
}

/**
 * Every link and image target in a markup segment.
 *
 * The target is read with **balanced parentheses**, so
 * `[x](mailto:a@b.invalid?subject=(hi))` keeps its brackets, and
 * `[click](javascript:alert(1))` yields the whole call rather than a truncated
 * prefix that might look harmless.
 *
 * Image targets are reported even though an image is rendered as plain text:
 * a target is worth validating wherever it appears, and refusing
 * `![a](javascript:...)` costs nothing.
 */
export function linkTargets(segment: string): LinkTarget[] {
  const found: LinkTarget[] = [];
  let index = 0;

  while (index < segment.length) {
    const open = segment.indexOf('[', index);
    if (open === -1) {
      break;
    }

    const close = segment.indexOf(']', open + 1);
    if (close === -1) {
      break;
    }
    if (segment[close + 1] !== '(') {
      index = open + 1;
      continue;
    }

    const end = matchingParen(segment, close + 1);
    if (end === null) {
      index = open + 1;
      continue;
    }

    found.push({
      target: segment.slice(close + 2, end),
      image: open > 0 && segment[open - 1] === '!',
    });
    index = end + 1;
  }

  return found;
}

/**
 * The index of the `)` closing the `(` at `start`, or `null` if unbalanced.
 *
 * Exported because the inline parser needs exactly the same answer: a link the
 * validator measured one way and the renderer measured another would be a note
 * that saves and then displays as something else.
 */
export function matchingParen(text: string, start: number): number | null {
  let depth = 0;
  for (let index = start; index < text.length; index += 1) {
    const character = text[index];
    if (character === '(') {
      depth += 1;
    } else if (character === ')') {
      depth -= 1;
      if (depth === 0) {
        return index;
      }
    }
  }
  return null;
}
