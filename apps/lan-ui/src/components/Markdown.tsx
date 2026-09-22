import { useEffect, useRef } from 'react';
import { renderMarkdown } from '@lan-meeting/editor';

/**
 * Note content, rendered.
 *
 * The **only** place this bundle turns Markdown into anything visible. The work
 * is done by `@lan-meeting/editor`, shared with the Host dashboard and the
 * offline remote form so that one note cannot mean three different things
 * (ADR-0007).
 *
 * # Why a ref rather than returned elements
 *
 * The shared renderer is framework-neutral by decision: the remote form has no
 * React, so the editor cannot be a React component library (ADR-0009). It
 * returns a `DocumentFragment`, and this is the small amount of code that puts
 * one into a React tree.
 *
 * That is also what keeps the safety property intact. The fragment is built
 * with `createElement` and `textContent`; there is no HTML string anywhere, so
 * there is no `dangerouslySetInnerHTML` here and no sanitiser to get wrong.
 *
 * `replaceChildren` clears the previous render, so a draft being edited cannot
 * leave the previous preview stacked underneath it.
 */
export function Markdown({
  content,
  className,
}: {
  readonly content: string;
  readonly className?: string;
}) {
  const host = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const node = host.current;
    if (node === null) {
      return;
    }
    node.replaceChildren(renderMarkdown(content, document));
  }, [content]);

  return <div ref={host} className={className ?? 'note-body'} />;
}
