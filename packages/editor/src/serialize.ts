/**
 * The document model back to Markdown text.
 *
 * # This is not what an ordinary save does
 *
 * Saving a note stores **exactly what the author typed**. It does not parse
 * and re-serialise, because that round trip would let the application quietly
 * rewrite somebody's formatting - collapsing their spacing, renumbering their
 * list, moving their table pipes - and a note editor that edits the note
 * behind your back is worse than a plain one (ADR-0007, ADR-0019).
 *
 * So this exists for the cases where content is **constructed rather than
 * typed**: assembling Markdown programmatically, and normalising for
 * comparison. Callers ask for it explicitly; nothing calls it on the save
 * path.
 *
 * # Idempotence
 *
 * `serialize(parse(x))` may differ from `x` - that is the whole reason it is
 * not used on save. What it must do is settle: serialising a second time
 * changes nothing further. The tests pin that against the shared fixtures.
 */

import type { Align, Block, Span } from './parse';
import { parseMarkdown } from './parse';

/**
 * Render blocks as Markdown text, ending in exactly one newline.
 *
 * The trailing newline is a property of *explicit* serialisation only, which
 * is why it is safe here and would not be on the save path.
 */
export function serializeBlocks(blocks: readonly Block[]): string {
  const rendered = blocks.map(serializeBlock).filter((text) => text !== '');
  return rendered.length === 0 ? '' : `${rendered.join('\n\n')}\n`;
}

/** Parse and re-serialise, for normalising constructed content. */
export function normalizeMarkdown(content: string): string {
  return serializeBlocks(parseMarkdown(content));
}

function serializeBlock(block: Block): string {
  switch (block.kind) {
    case 'paragraph':
      return serializeSpans(block.spans);

    case 'heading':
      return `${'#'.repeat(block.level)} ${serializeSpans(block.spans)}`;

    case 'list':
      return block.items
        .map((item, position) =>
          block.ordered
            ? `${position + 1}. ${serializeSpans(item)}`
            : `- ${serializeSpans(item)}`,
        )
        .join('\n');

    case 'table': {
      const header = `| ${block.header.map(serializeCell).join(' | ')} |`;
      const delimiter = `| ${block.header
        .map((_, column) => delimiterFor(block.align[column] ?? null))
        .join(' | ')} |`;
      const rows = block.rows.map((row) => `| ${row.map(serializeCell).join(' | ')} |`);
      return [header, delimiter, ...rows].join('\n');
    }

    case 'code':
      return `\`\`\`${block.language ?? ''}\n${block.text}\n\`\`\``;
  }
}

/** A cell's pipes are escaped so a round trip keeps the column count. */
function serializeCell(spans: readonly Span[]): string {
  return serializeSpans(spans).replace(/\|/g, '\\|');
}

function delimiterFor(align: Align): string {
  switch (align) {
    case 'left':
      return ':---';
    case 'center':
      return ':---:';
    case 'right':
      return '---:';
    default:
      return '---';
  }
}

function serializeSpans(spans: readonly Span[]): string {
  return spans.map(serializeSpan).join('');
}

function serializeSpan(span: Span): string {
  switch (span.kind) {
    case 'text':
      return span.text;
    case 'break':
      // Two trailing spaces, which is how the parser recognises a hard break.
      return '  \n';
    case 'code':
      return fenceCode(span.text);
    case 'emphasis':
      return `*${serializeSpans(span.spans)}*`;
    case 'strong':
      return `**${serializeSpans(span.spans)}**`;
    case 'link':
      return `[${serializeSpans(span.spans)}](${span.href})`;
  }
}

/**
 * Wrap a code span in a backtick run longer than any inside it.
 *
 * Without this, content containing a backtick would serialise to something
 * that parses back as two spans, and the round trip would not settle.
 */
function fenceCode(text: string): string {
  const longest = [...text.matchAll(/`+/g)].reduce(
    (longestRun, match) => Math.max(longestRun, match[0].length),
    0,
  );
  const fence = '`'.repeat(longest + 1);
  const padding = text.startsWith('`') || text.endsWith('`') ? ' ' : '';
  return `${fence}${padding}${text}${padding}${fence}`;
}
