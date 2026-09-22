/**
 * The properties this package exists to guarantee.
 *
 * Everything here is about what the renderer **cannot** produce. Participant
 * content is hostile input even inside the Host's own window (PRD 13.3), and
 * these are the assertions that say so in code rather than in a comment.
 */

import { describe, expect, it } from 'vitest';

import { renderMarkdown } from '../src/render';
import { isAllowedLinkTarget, safeLinkTarget } from '../src/scheme';
import { findHtmlConstruct, validateNoteContent } from '../src/validate';
import { plainTextOf } from './support';

/** Render and return the element tree, for structural assertions. */
function render(markdown: string): HTMLElement {
  const host = document.createElement('div');
  host.appendChild(renderMarkdown(markdown, document));
  return host;
}

/** Every element in the rendered tree, by tag name. */
function tagsIn(markdown: string): string[] {
  return Array.from(render(markdown).querySelectorAll('*')).map((node) => node.tagName);
}

describe('the renderer cannot produce dangerous elements', () => {
  const forbidden = ['IMG', 'IFRAME', 'OBJECT', 'EMBED', 'SCRIPT', 'STYLE', 'SVG', 'FORM'];

  it.each([
    ['image syntax', '![alt](https://example.invalid/a.png)'],
    ['a script tag written literally', 'text <script>alert(1)</script> text'],
    ['an iframe written literally', '<iframe src="x"></iframe>'],
    ['an svg written literally', '<svg onload="alert(1)"></svg>'],
    ['an img written literally', '<img src=x onerror=alert(1)>'],
    ['a style block', '<style>body{}</style>'],
  ])('%s produces none of them', (_name, markdown) => {
    const tags = tagsIn(markdown);
    for (const tag of forbidden) {
      expect(tags).not.toContain(tag);
    }
  });

  it('only ever creates elements from the allowlist', () => {
    const everything = [
      '# Heading',
      '',
      'A *paragraph* with **strong** and `code` and [a link](https://example.invalid/x).',
      '',
      '- one',
      '- two',
      '',
      '1. first',
      '',
      '| a | b |',
      '| --- | --- |',
      '| c | d |',
      '',
      '```rust',
      'fn main() {}',
      '```',
      '',
      'hard  ',
      'break',
    ].join('\n');

    const allowed = new Set([
      'P',
      'H1',
      'H2',
      'H3',
      'H4',
      'H5',
      'H6',
      'UL',
      'OL',
      'LI',
      'TABLE',
      'THEAD',
      'TBODY',
      'TR',
      'TH',
      'TD',
      'PRE',
      'CODE',
      'EM',
      'STRONG',
      'A',
      'BR',
    ]);

    for (const tag of tagsIn(everything)) {
      expect(allowed).toContain(tag);
    }
  });
});

describe('author text never becomes markup', () => {
  it('renders a literal script tag as text', () => {
    const markdown = 'before <script>alert(1)</script> after';
    expect(plainTextOf(render(markdown))).toContain('<script>alert(1)</script>');
    expect(render(markdown).querySelector('script')).toBeNull();
  });

  it('renders angle brackets in a fenced block as text', () => {
    const markdown = '```\n<script>alert(1)</script>\n```';
    const code = render(markdown).querySelector('pre > code');
    expect(code).not.toBeNull();
    expect(code?.textContent).toBe('<script>alert(1)</script>');
    expect(code?.children.length).toBe(0);
  });

  it('does not let a language tag escape into the class attribute', () => {
    const code = render('```a" onload="alert(1)\nx\n```').querySelector('code');
    expect(code?.className).toBe('');
  });
});

describe('links', () => {
  it.each([
    ['https', 'https://example.invalid/x'],
    ['http', 'http://example.invalid/x'],
    ['mailto', 'mailto:budi@example.invalid'],
  ])('allows %s', (_name, target) => {
    expect(isAllowedLinkTarget(target)).toBe(true);
  });

  it.each([
    ['javascript', 'javascript:alert(1)'],
    ['javascript with mixed case', 'JaVaScRiPt:alert(1)'],
    ['javascript with padding', '  javascript:alert(1)  '],
    ['data', 'data:text/html;base64,PHNjcmlwdD4='],
    ['file', 'file:///etc/passwd'],
    ['vbscript', 'vbscript:msgbox'],
    ['blob', 'blob:abc'],
    ['relative', '../notes.md'],
    ['protocol relative', '//example.invalid/a'],
    ['empty', ''],
    ['whitespace', '   '],
    ['unparseable', 'http://[unbalanced'],
  ])('refuses %s', (_name, target) => {
    expect(safeLinkTarget(target)).toBeNull();
  });

  it('renders a refused target as literal text rather than an anchor', () => {
    // Content stored before a rule existed, or arriving from an import, still
    // has to display - and it must not display as something clickable.
    const tree = render('[click](javascript:alert(1))');
    expect(tree.querySelector('a')).toBeNull();
    expect(plainTextOf(tree)).toBe('[click](javascript:alert(1))');
  });

  it('gives every anchor target and rel together', () => {
    const anchor = render('[x](https://example.invalid/x)').querySelector('a');
    expect(anchor?.getAttribute('target')).toBe('_blank');
    expect(anchor?.getAttribute('rel')).toBe('noopener noreferrer');
  });

  it('sets no attribute on an anchor beyond href, target and rel', () => {
    const anchor = render('[x](https://example.invalid/x)').querySelector('a');
    const names = Array.from(anchor?.attributes ?? []).map((attribute) => attribute.name);
    expect(new Set(names)).toEqual(new Set(['href', 'target', 'rel']));
  });

  it('never turns image syntax into an anchor', () => {
    const tree = render('![alt](https://example.invalid/a.png)');
    expect(tree.querySelector('a')).toBeNull();
    expect(tree.querySelector('img')).toBeNull();
  });
});

describe('the raw HTML rule', () => {
  it.each([
    'a < b',
    '2 < 3',
    '2 < 3 > 1',
    // Bare, at the very end: still not a tag, because nothing closes it.
    'a<b',
    'a<b was measured',
    'x < y and y > z',
  ])(
    'treats %j as ordinary text',
    (text) => {
      expect(findHtmlConstruct(text)).toBeNull();
      expect(validateNoteContent(text)).toBeNull();
    },
  );

  it.each([
    '<script>',
    '</div>',
    '<!-- comment -->',
    '<?xml version="1.0"?>',
    '<!DOCTYPE html>',
    '<br/>',
    '<SCRIPT>',
    '<a href="x">',
  ])('refuses %j', (text) => {
    expect(findHtmlConstruct(text)).not.toBeNull();
    expect(validateNoteContent(`note ${text} note`)?.reason).toBe('raw_html');
  });

  it('does not report a construct inside code', () => {
    expect(validateNoteContent('`<script>`')).toBeNull();
    expect(validateNoteContent('```\n<script>\n```')).toBeNull();
  });
});
