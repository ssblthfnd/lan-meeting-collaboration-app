/**
 * The document model to DOM nodes.
 *
 * # There is no HTML string anywhere in this file
 *
 * Every node is built with `createElement` and every character of author text
 * is set through `textContent`. Nothing here concatenates markup, and nothing
 * here assigns `innerHTML`.
 *
 * That is a stronger guarantee than sanitising would be. A sanitiser defends a
 * sink; this has no sink. There is no parse-then-clean step to get subtly
 * wrong, no bypass to discover, and no dependency to keep patched - injection
 * is not blocked, it is unrepresentable.
 *
 * It is also why this renderer is safe to use in the **Host's** window. A
 * participant's note is hostile input even there (PRD 13.3), and the Host UI
 * gets the same renderer as everyone else rather than a relaxed one.
 *
 * # The element allowlist
 *
 * `p`, `h1`-`h6`, `ul`, `ol`, `li`, `table`, `thead`, `tbody`, `tr`, `th`,
 * `td`, `pre`, `code`, `em`, `strong`, `a`, `br`. Nothing else is ever
 * created - in particular no `img`, `iframe`, `object`, `embed`, `script`,
 * `style` or SVG, none of which the subset can produce.
 *
 * The only attributes set anywhere are `href`, `target`, `rel`, `class` and a
 * table cell's `style` text-align. No attribute value comes from author text
 * except `href`, which has already been through the scheme allowlist.
 */

import type { Align, Block, Span } from './parse';
import { parseMarkdown } from './parse';

/** Class prefix, so a host stylesheet can target rendered note content. */
const CLASS_PREFIX = 'note-md';

/**
 * Render Markdown into a fragment ready to append.
 *
 * A fragment rather than a string, because a string would have to be assigned
 * somewhere, and the only place to assign markup is the sink this module
 * exists to avoid.
 */
export function renderMarkdown(content: string, doc: Document = document): DocumentFragment {
  return renderBlocks(parseMarkdown(content), doc);
}

/** Render an already-parsed document. */
export function renderBlocks(
  blocks: readonly Block[],
  doc: Document = document,
): DocumentFragment {
  const fragment = doc.createDocumentFragment();
  for (const block of blocks) {
    fragment.appendChild(renderBlock(block, doc));
  }
  return fragment;
}

function renderBlock(block: Block, doc: Document): HTMLElement {
  switch (block.kind) {
    case 'paragraph': {
      const paragraph = doc.createElement('p');
      appendSpans(paragraph, block.spans, doc);
      return paragraph;
    }

    case 'heading': {
      // The level came from counting `#`, and the parser clamps it to 1-6, so
      // the tag name is chosen from a fixed set rather than interpolated from
      // anything an author controls.
      const heading = doc.createElement(headingTag(block.level));
      appendSpans(heading, block.spans, doc);
      return heading;
    }

    case 'list': {
      const list = doc.createElement(block.ordered ? 'ol' : 'ul');
      for (const item of block.items) {
        const entry = doc.createElement('li');
        appendSpans(entry, item, doc);
        list.appendChild(entry);
      }
      return list;
    }

    case 'table':
      return renderTable(block, doc);

    case 'code': {
      const pre = doc.createElement('pre');
      const code = doc.createElement('code');
      // The language is shown as a class, never executed and never used to
      // choose behaviour. Restricted to a conservative character set so it
      // cannot smuggle anything into the class attribute.
      if (block.language !== null && /^[A-Za-z0-9_+-]{1,32}$/.test(block.language)) {
        code.className = `${CLASS_PREFIX}-lang-${block.language.toLowerCase()}`;
      }
      code.textContent = block.text;
      pre.appendChild(code);
      return pre;
    }
  }
}

function renderTable(
  block: Extract<Block, { kind: 'table' }>,
  doc: Document,
): HTMLElement {
  const table = doc.createElement('table');
  table.className = `${CLASS_PREFIX}-table`;

  const head = doc.createElement('thead');
  const headRow = doc.createElement('tr');
  block.header.forEach((cell, column) => {
    const th = doc.createElement('th');
    applyAlign(th, block.align[column] ?? null);
    appendSpans(th, cell, doc);
    headRow.appendChild(th);
  });
  head.appendChild(headRow);
  table.appendChild(head);

  const body = doc.createElement('tbody');
  for (const row of block.rows) {
    const tr = doc.createElement('tr');
    row.forEach((cell, column) => {
      const td = doc.createElement('td');
      applyAlign(td, block.align[column] ?? null);
      appendSpans(td, cell, doc);
      tr.appendChild(td);
    });
    body.appendChild(tr);
  }
  table.appendChild(body);

  return table;
}

/** One of three fixed values, never author text. */
function applyAlign(cell: HTMLElement, align: Align): void {
  if (align !== null) {
    cell.style.textAlign = align;
  }
}

function appendSpans(parent: Node, spans: readonly Span[], doc: Document): void {
  for (const span of spans) {
    parent.appendChild(renderSpan(span, doc));
  }
}

function renderSpan(span: Span, doc: Document): Node {
  switch (span.kind) {
    case 'text':
      return doc.createTextNode(span.text);

    case 'break':
      return doc.createElement('br');

    case 'code': {
      const code = doc.createElement('code');
      code.textContent = span.text;
      return code;
    }

    case 'emphasis': {
      const em = doc.createElement('em');
      appendSpans(em, span.spans, doc);
      return em;
    }

    case 'strong': {
      const strong = doc.createElement('strong');
      appendSpans(strong, span.spans, doc);
      return strong;
    }

    case 'link': {
      const anchor = doc.createElement('a');
      // Already normalised by the scheme allowlist; the parser never emits a
      // link span for a target that failed it.
      anchor.href = span.href;
      anchor.target = '_blank';
      // Without `noopener` the opened page can navigate this one through
      // `window.opener`, and `noreferrer` keeps a join URL out of a third
      // party's logs.
      anchor.rel = 'noopener noreferrer';
      appendSpans(anchor, span.spans, doc);
      return anchor;
    }
  }
}

/** Heading tag for a clamped level. */
function headingTag(level: number): 'h1' | 'h2' | 'h3' | 'h4' | 'h5' | 'h6' {
  switch (level) {
    case 1:
      return 'h1';
    case 2:
      return 'h2';
    case 3:
      return 'h3';
    case 4:
      return 'h4';
    case 5:
      return 'h5';
    default:
      return 'h6';
  }
}
