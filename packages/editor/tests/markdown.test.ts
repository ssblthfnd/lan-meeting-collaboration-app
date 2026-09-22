/**
 * The subset, construct by construct.
 *
 * The shared fixtures pin the cases both languages must agree on; this file
 * covers the parser and serialiser in more detail than a cross-language table
 * should have to carry.
 */

import { describe, expect, it } from 'vitest';

import { parseMarkdown, type Block } from '../src/parse';
import { renderMarkdown } from '../src/render';
import { normalizeMarkdown, serializeBlocks } from '../src/serialize';
import { byteLength, MAX_NOTE_BYTES, validateNoteContent } from '../src/validate';
import { plainTextOf } from './support';

function kinds(markdown: string): string[] {
  return parseMarkdown(markdown).map((block) => block.kind);
}

function text(markdown: string): string {
  return plainTextOf(renderMarkdown(markdown, document));
}

describe('supported constructs', () => {
  it('parses each block kind', () => {
    expect(kinds('# h')).toEqual(['heading']);
    expect(kinds('para')).toEqual(['paragraph']);
    expect(kinds('- a')).toEqual(['list']);
    expect(kinds('1. a')).toEqual(['list']);
    expect(kinds('| a |\n| --- |\n| b |')).toEqual(['table']);
    expect(kinds('```\nx\n```')).toEqual(['code']);
  });

  it('records heading levels one through six', () => {
    for (let level = 1; level <= 6; level += 1) {
      const block = parseMarkdown(`${'#'.repeat(level)} title`)[0];
      expect(block?.kind).toBe('heading');
      expect((block as Extract<Block, { kind: 'heading' }>).level).toBe(level);
    }
  });

  it('tells ordered from unordered lists', () => {
    const unordered = parseMarkdown('- a\n- b')[0] as Extract<Block, { kind: 'list' }>;
    expect(unordered.ordered).toBe(false);
    expect(unordered.items.length).toBe(2);

    const ordered = parseMarkdown('1. a\n2. b')[0] as Extract<Block, { kind: 'list' }>;
    expect(ordered.ordered).toBe(true);
    expect(ordered.items.length).toBe(2);
  });

  it('reads table alignment from the delimiter row', () => {
    const table = parseMarkdown('| a | b | c | d |\n| :- | :-: | -: | - |\n| 1 | 2 | 3 | 4 |')[0];
    expect((table as Extract<Block, { kind: 'table' }>).align).toEqual([
      'left',
      'center',
      'right',
      null,
    ]);
  });

  it('keeps an escaped pipe inside a table cell', () => {
    const table = parseMarkdown('| a \\| b |\n| --- |\n| c |')[0] as Extract<
      Block,
      { kind: 'table' }
    >;
    expect(table.header.length).toBe(1);
    expect(text('| a \\| b |\n| --- |\n| c |')).toBe('a | b\nc');
  });

  it('keeps a fenced block literal, including its markup', () => {
    expect(text('```\n- not a list\n# not a heading\n```')).toBe('- not a list\n# not a heading');
  });

  it('renders emphasis and strong as elements, not characters', () => {
    const host = document.createElement('div');
    host.appendChild(renderMarkdown('*a* and **b**', document));
    expect(host.querySelector('em')?.textContent).toBe('a');
    expect(host.querySelector('strong')?.textContent).toBe('b');
  });

  it('gives code spans precedence over emphasis and links', () => {
    expect(text('`*a* [b](https://example.invalid/c)`')).toBe(
      '*a* [b](https://example.invalid/c)',
    );
  });
});

describe('unsupported constructs stay as text', () => {
  it.each([
    ['blockquote', '> quoted', '> quoted'],
    ['strikethrough', '~~gone~~', '~~gone~~'],
    ['footnote reference', 'claim[^1]', 'claim[^1]'],
    ['task list marker', '- [ ] todo', '[ ] todo'],
    ['setext underline', 'Title\n---', 'Title'],
    ['seven hashes', '####### x', '####### x'],
  ])('%s', (_name, markdown, expected) => {
    expect(text(markdown)).toContain(expected);
  });

  it('is never refused merely for being unsupported', () => {
    for (const markdown of ['> quoted', '~~gone~~', 'claim[^1]', '- [ ] todo']) {
      expect(validateNoteContent(markdown)).toBeNull();
    }
  });
});

describe('the size limit', () => {
  it('is 64 KiB', () => {
    expect(MAX_NOTE_BYTES).toBe(65536);
  });

  it('accepts exactly the limit and refuses one byte more', () => {
    expect(validateNoteContent('a'.repeat(MAX_NOTE_BYTES))).toBeNull();
    expect(validateNoteContent('a'.repeat(MAX_NOTE_BYTES + 1))?.reason).toBe('too_large');
  });

  it('counts bytes rather than characters', () => {
    // 32768 two-byte characters is 65536 bytes: at the limit, though it is
    // half the character count.
    const twoByte = 'é'.repeat(MAX_NOTE_BYTES / 2);
    expect(byteLength(twoByte)).toBe(MAX_NOTE_BYTES);
    expect(validateNoteContent(twoByte)).toBeNull();
    expect(validateNoteContent(`${twoByte}a`)?.reason).toBe('too_large');
  });

  it('reports both the limit and what was measured', () => {
    const problem = validateNoteContent('a'.repeat(MAX_NOTE_BYTES + 5));
    expect(problem?.expected).toContain('65536');
    expect(problem?.detected).toContain('65541');
  });
});

describe('control characters', () => {
  it('allows tab and newline', () => {
    expect(validateNoteContent('a\tb\nc')).toBeNull();
  });

  it.each([
    ['null', '\u0000'],
    ['bell', '\u0007'],
    ['carriage return', '\r'],
    ['escape', '\u001b'],
    ['delete', '\u007f'],
    ['vertical tab', '\u000b'],
  ])('refuses %s', (_name, character) => {
    const problem = validateNoteContent(`before${character}after`);
    expect(problem?.reason).toBe('control_character');
    expect(problem?.detected).toMatch(/^U\+[0-9A-F]{4}$/);
  });
});

describe('serialisation', () => {
  it('is not applied on an ordinary save path', () => {
    // The stored note is what was typed. `normalizeMarkdown` exists for
    // constructed content, and this asserts it is a separate, explicit step
    // rather than something the parse/render path performs.
    const typed = 'a   spaced    paragraph\n\n\n\nwith gaps';
    expect(text(typed)).toBe('a   spaced    paragraph\nwith gaps');
    expect(normalizeMarkdown(typed)).not.toBe(typed);
  });

  it('ends explicitly serialised content with exactly one newline', () => {
    const output = normalizeMarkdown('a\n\nb');
    expect(output.endsWith('\n')).toBe(true);
    expect(output.endsWith('\n\n')).toBe(false);
  });

  it('serialises an empty document to an empty string', () => {
    expect(serializeBlocks([])).toBe('');
  });

  it('settles after one pass', () => {
    for (const markdown of [
      '# h\n\npara\n\n- a\n- b\n\n1. x\n\n| a | b |\n| :- | -: |\n| c | d |',
      '`` a ` b ``',
      'hard  \nbreak',
      '[x](https://example.invalid/y)',
      '```rust\nfn main() {}\n```',
    ]) {
      const once = normalizeMarkdown(markdown);
      expect(normalizeMarkdown(once), markdown).toBe(once);
    }
  });

  it('keeps a backtick inside a code span intact through a round trip', () => {
    const once = normalizeMarkdown('`` a ` b ``');
    expect(text(once)).toBe('a ` b');
  });
});

describe('malformed input never throws', () => {
  it.each([
    'unclosed `code',
    'unclosed **strong',
    'unclosed [link](',
    '[](',
    '|||',
    '| a |\n| --- ',
    '```',
    '```\nunterminated',
    '#',
    '- ',
    '*',
    '[a](b)(c)',
    '![](',
  ])('%j', (markdown) => {
    expect(() => parseMarkdown(markdown)).not.toThrow();
    expect(() => renderMarkdown(markdown, document)).not.toThrow();
    expect(() => validateNoteContent(markdown)).not.toThrow();
  });
});
