/**
 * Markdown text to the subset's document model.
 *
 * The model exists for **rendering and inspection**, never as editor state.
 * The editor's state is the Markdown string itself (ADR-0019): parsing it into
 * a tree and serialising it back on every keystroke would let the application
 * quietly rewrite a Host's formatting, which is the round trip ADR-0007
 * avoided when it chose text storage over a document model.
 *
 * # The subset
 *
 * Supported: paragraph, ATX heading (levels 1-6), unordered list with `-`,
 * ordered list with `1.`, GFM pipe table, link, emphasis, strong, inline code,
 * fenced code, hard line break.
 *
 * Everything else is **text**. A blockquote, a task-list marker, a
 * strikethrough, a footnote reference, a setext underline and a second-level
 * list marker are all rendered exactly as written. They are not errors: a note
 * is somebody's writing, and the right response to syntax this application
 * does not interpret is to show it, not to refuse it.
 *
 * Images are the one unsupported construct with a rule of its own: `![a](url)`
 * is emitted as literal text rather than being allowed to fall through to the
 * link parser, because a link wearing an image's syntax is exactly the sort of
 * surprise a renderer should not produce.
 *
 * # Line breaks follow GFM
 *
 * A single newline inside a paragraph is a **space**. A line ending in two or
 * more spaces is a **hard break**. That is what GFM does, and calling the
 * format a GFM subset while doing something else would make the name a lie.
 */

import { findBacktickRun, matchingParen } from './scan';
import { safeLinkTarget } from './scheme';

/** Column alignment of a pipe table, from its delimiter row. */
export type Align = 'left' | 'center' | 'right' | null;

/** An inline run. */
export type Span =
  | { readonly kind: 'text'; readonly text: string }
  | { readonly kind: 'code'; readonly text: string }
  | { readonly kind: 'emphasis'; readonly spans: readonly Span[] }
  | { readonly kind: 'strong'; readonly spans: readonly Span[] }
  /** `href` has already passed the scheme allowlist. */
  | { readonly kind: 'link'; readonly href: string; readonly spans: readonly Span[] }
  | { readonly kind: 'break' };

/** A block-level element. */
export type Block =
  | { readonly kind: 'paragraph'; readonly spans: readonly Span[] }
  | { readonly kind: 'heading'; readonly level: number; readonly spans: readonly Span[] }
  | {
      readonly kind: 'list';
      readonly ordered: boolean;
      readonly items: readonly (readonly Span[])[];
    }
  | {
      readonly kind: 'table';
      readonly header: readonly (readonly Span[])[];
      readonly align: readonly Align[];
      readonly rows: readonly (readonly (readonly Span[])[])[];
    }
  | { readonly kind: 'code'; readonly language: string | null; readonly text: string };

/** Largest heading level the subset allows. */
const MAX_HEADING_LEVEL = 6;

/**
 * Largest table a note may render.
 *
 * Not a Markdown rule - a bound. Participant content is hostile input even in
 * the Host's own window (PRD 13.3), and a table with a hundred thousand rows
 * would freeze the WebView for no legitimate reason. Content past the bound is
 * dropped from the table rather than the note being refused, because the note
 * itself is still readable.
 */
const MAX_TABLE_ROWS = 1000;
const MAX_TABLE_COLUMNS = 32;

/** Parse a note into blocks. */
export function parseMarkdown(content: string): Block[] {
  const lines = content.split('\n');
  const blocks: Block[] = [];
  let index = 0;

  while (index < lines.length) {
    const line = lines[index] ?? '';

    if (line.trim() === '') {
      index += 1;
      continue;
    }

    const fence = openingFence(line);
    if (fence !== null) {
      const body: string[] = [];
      index += 1;
      while (index < lines.length && openingFence(lines[index] ?? '') === null) {
        body.push(lines[index] ?? '');
        index += 1;
      }
      // Step past the closing fence when there is one. An unclosed fence runs
      // to the end of the note, which is what GFM does.
      if (index < lines.length) {
        index += 1;
      }
      blocks.push({
        kind: 'code',
        language: fence === '' ? null : fence,
        text: body.join('\n'),
      });
      continue;
    }

    const heading = /^ {0,3}(#{1,6})\s+(.*)$/.exec(line);
    if (heading !== null) {
      const hashes = heading[1] ?? '';
      const text = heading[2] ?? '';
      blocks.push({
        kind: 'heading',
        level: Math.min(hashes.length, MAX_HEADING_LEVEL),
        // A trailing run of hashes is ATX closing syntax, not content.
        spans: parseInline(text.replace(/\s+#+\s*$/, '')),
      });
      index += 1;
      continue;
    }

    const table = parseTable(lines, index);
    if (table !== null) {
      blocks.push(table.block);
      index = table.next;
      continue;
    }

    const list = parseList(lines, index);
    if (list !== null) {
      blocks.push(list.block);
      index = list.next;
      continue;
    }

    const paragraph = takeParagraph(lines, index);
    blocks.push({ kind: 'paragraph', spans: paragraph.spans });
    index = paragraph.next;
  }

  return blocks;
}

/** The info string of a fence line, or `null` if the line is not a fence. */
function openingFence(line: string): string | null {
  const match = /^ {0,3}```(.*)$/.exec(line);
  return match === null ? null : (match[1] ?? '').trim();
}

/** Whether a line starts a block that a paragraph cannot absorb. */
function startsBlock(line: string): boolean {
  return (
    line.trim() === '' ||
    openingFence(line) !== null ||
    /^ {0,3}#{1,6}\s/.test(line) ||
    listMarker(line) !== null
  );
}

/**
 * The content of a list item, or `null` when the line is not one.
 *
 * Indentation is ignored rather than counted. Nesting is outside the subset,
 * so a deeper marker becomes a **sibling item at the one level that exists**:
 * the writing survives and stays readable as a list, which is better than
 * folding it into the line above or dropping it.
 */
function listMarker(line: string): { ordered: boolean; text: string } | null {
  const unordered = /^\s*-\s+(.*)$/.exec(line);
  if (unordered !== null) {
    return { ordered: false, text: unordered[1] ?? '' };
  }
  const ordered = /^\s*\d{1,9}[.)]\s+(.*)$/.exec(line);
  if (ordered !== null) {
    return { ordered: true, text: ordered[1] ?? '' };
  }
  return null;
}

/**
 * Consecutive lines forming one paragraph, with their breaks resolved.
 *
 * A line ending in two or more spaces produces a hard break; any other newline
 * becomes a single space.
 */
function takeParagraph(lines: string[], from: number): { spans: Span[]; next: number } {
  const spans: Span[] = [];
  let index = from;

  while (index < lines.length) {
    const line = lines[index] ?? '';
    if (index > from && startsBlock(line)) {
      break;
    }

    const hard = /\s{2,}$/.test(line);
    spans.push(...parseInline(line.trim()));
    index += 1;

    const more = index < lines.length && !startsBlock(lines[index] ?? '');
    if (more) {
      spans.push(hard ? { kind: 'break' } : { kind: 'text', text: ' ' });
    }
  }

  return { spans, next: index };
}

/**
 * One list, at one level.
 *
 * A deeper marker is not a nested list: the line is a continuation of the item
 * above it, so `- outer` followed by `  - inner` is one item reading
 * "outer - inner". Nesting is out of the subset (ADR-0019), and silently
 * flattening is better than silently discarding.
 */
function parseList(lines: string[], from: number): { block: Block; next: number } | null {
  const first = listMarker(lines[from] ?? '');
  if (first === null) {
    return null;
  }

  const ordered = first.ordered;
  const items: Span[][] = [];
  let current: string[] = [first.text];
  let index = from + 1;

  while (index < lines.length) {
    const line = lines[index] ?? '';
    const marker = listMarker(line);

    if (marker !== null && marker.ordered === ordered) {
      items.push(inlineWithSoftBreaks(current));
      current = [marker.text];
      index += 1;
      continue;
    }
    if (line.trim() === '' || (marker !== null && marker.ordered !== ordered)) {
      break;
    }
    if (startsBlock(line) && marker === null) {
      break;
    }
    current.push(line.trim());
    index += 1;
  }

  items.push(inlineWithSoftBreaks(current));
  return { block: { kind: 'list', ordered, items }, next: index };
}

/** Join continuation lines with a space, the way a paragraph does. */
function inlineWithSoftBreaks(lines: string[]): Span[] {
  const spans: Span[] = [];
  lines.forEach((line, position) => {
    if (position > 0) {
      spans.push({ kind: 'text', text: ' ' });
    }
    spans.push(...parseInline(line));
  });
  return spans;
}

/** A GFM pipe table, if `from` is its header and `from + 1` its delimiter. */
function parseTable(lines: string[], from: number): { block: Block; next: number } | null {
  const header = lines[from] ?? '';
  const delimiter = lines[from + 1];
  if (delimiter === undefined || !header.includes('|')) {
    return null;
  }
  if (!/^ {0,3}\|?(\s*:?-+:?\s*\|)*\s*:?-+:?\s*\|?\s*$/.test(delimiter)) {
    return null;
  }

  const align = splitRow(delimiter).map((cell): Align => {
    const left = cell.startsWith(':');
    const right = cell.endsWith(':');
    if (left && right) {
      return 'center';
    }
    if (right) {
      return 'right';
    }
    return left ? 'left' : null;
  });

  const headerCells = splitRow(header).map(parseInline);
  const rows: Span[][][] = [];
  let index = from + 2;

  while (index < lines.length) {
    const line = lines[index] ?? '';
    if (line.trim() === '' || !line.includes('|')) {
      break;
    }
    if (rows.length < MAX_TABLE_ROWS) {
      rows.push(splitRow(line).map(parseInline));
    }
    index += 1;
  }

  return {
    block: {
      kind: 'table',
      header: headerCells.slice(0, MAX_TABLE_COLUMNS),
      align: align.slice(0, MAX_TABLE_COLUMNS),
      rows: rows.map((row) => row.slice(0, MAX_TABLE_COLUMNS)),
    },
    next: index,
  };
}

/** Split a table row on unescaped pipes, dropping the optional outer pair. */
function splitRow(line: string): string[] {
  const cells: string[] = [];
  let cell = '';
  const trimmed = line.trim();

  for (let index = 0; index < trimmed.length; index += 1) {
    const character = trimmed[index];
    if (character === '\\' && trimmed[index + 1] === '|') {
      cell += '|';
      index += 1;
      continue;
    }
    if (character === '|') {
      cells.push(cell.trim());
      cell = '';
      continue;
    }
    cell += character;
  }
  cells.push(cell.trim());

  if (cells.length > 0 && cells[0] === '') {
    cells.shift();
  }
  if (cells.length > 0 && cells[cells.length - 1] === '') {
    cells.pop();
  }
  return cells;
}

/**
 * Parse one line of inline Markdown.
 *
 * Precedence, highest first: code span, image (literal), link, strong,
 * emphasis. Code wins outright, which is why `` `**not strong**` `` keeps its
 * asterisks.
 */
export function parseInline(line: string): Span[] {
  const spans: Span[] = [];
  let plain = '';
  let index = 0;

  const flush = (): void => {
    if (plain !== '') {
      spans.push({ kind: 'text', text: plain });
      plain = '';
    }
  };

  while (index < line.length) {
    const character = line[index] ?? '';

    if (character === '`') {
      const code = readCodeSpan(line, index);
      if (code !== null) {
        flush();
        spans.push({ kind: 'code', text: code.text });
        index = code.next;
        continue;
      }
    }

    // `![alt](target)` stays literal. Consumed as a unit so the `[` below
    // cannot pick it up and turn an image into an anchor.
    if (character === '!' && line[index + 1] === '[') {
      const image = readBracketed(line, index + 1);
      if (image !== null) {
        plain += line.slice(index, image.next);
        index = image.next;
        continue;
      }
    }

    if (character === '[') {
      const bracketed = readBracketed(line, index);
      if (bracketed !== null) {
        const href = safeLinkTarget(bracketed.target);
        if (href === null) {
          // Not allowlisted: show exactly what was written rather than an
          // anchor nobody should click or a silently dropped fragment.
          plain += line.slice(index, bracketed.next);
        } else {
          flush();
          spans.push({ kind: 'link', href, spans: parseInline(bracketed.label) });
        }
        index = bracketed.next;
        continue;
      }
    }

    if (character === '*') {
      const strong = readDelimited(line, index, '**');
      if (strong !== null) {
        flush();
        spans.push({ kind: 'strong', spans: parseInline(strong.text) });
        index = strong.next;
        continue;
      }
      const emphasis = readDelimited(line, index, '*');
      if (emphasis !== null) {
        flush();
        spans.push({ kind: 'emphasis', spans: parseInline(emphasis.text) });
        index = emphasis.next;
        continue;
      }
    }

    plain += character;
    index += 1;
  }

  flush();
  return spans;
}

/** A code span starting at `start`, or `null` if the run never closes. */
function readCodeSpan(line: string, start: number): { text: string; next: number } | null {
  let index = start;
  while (index < line.length && line[index] === '`') {
    index += 1;
  }
  const length = index - start;
  const closing = findBacktickRun(line, index, length);
  if (closing === null) {
    return null;
  }
  // A run longer than the opener would not be the closer; GFM strips one
  // leading and trailing space so `` ` `` can be written inside a span.
  const text = line.slice(index, closing);
  return { text: text.replace(/^ (.*) $/, '$1'), next: closing + length };
}

/** `[label](target)` starting at the `[` in `start`. */
function readBracketed(
  line: string,
  start: number,
): { label: string; target: string; next: number } | null {
  const close = line.indexOf(']', start + 1);
  if (close === -1 || line[close + 1] !== '(') {
    return null;
  }
  const end = matchingParen(line, close + 1);
  if (end === null) {
    return null;
  }
  return {
    label: line.slice(start + 1, close),
    target: line.slice(close + 2, end),
    next: end + 1,
  };
}

/** A run delimited by `marker` on both sides, non-empty in between. */
function readDelimited(
  line: string,
  start: number,
  marker: string,
): { text: string; next: number } | null {
  if (!line.startsWith(marker, start)) {
    return null;
  }
  const from = start + marker.length;
  const closing = line.indexOf(marker, from);
  if (closing === -1 || closing === from) {
    return null;
  }
  return { text: line.slice(from, closing), next: closing + marker.length };
}
