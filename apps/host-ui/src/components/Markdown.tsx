import { useEffect, useRef } from 'react';
import { renderMarkdown } from '@lan-meeting/editor';

/**
 * Note content, rendered.
 *
 * The **only** place the Host UI turns Markdown into anything visible. The
 * work is done by `@lan-meeting/editor`, which is shared with the participant
 * page and the offline remote form so that one note cannot mean three
 * different things (ADR-0007).
 *
 * # Why this component holds a ref instead of returning elements
 *
 * The shared renderer is framework-neutral by decision: the remote form has no
 * React, so the editor cannot be a React component library (ADR-0009). It
 * returns a `DocumentFragment`, and this is the ten lines that put one into a
 * React tree.
 *
 * That is also what keeps the safety property intact. The fragment is built
 * with `createElement` and `textContent`; there is no HTML string anywhere, so
 * there is no `dangerouslySetInnerHTML` here and no sanitiser to get wrong.
 * Participant note content is hostile input even inside the Host's own window
 * (PRD 13.3), and it gets the same renderer everyone else gets rather than a
 * relaxed one.
 *
 * `replaceChildren` clears the previous render, so switching participants
 * cannot leave one person's note stacked under another's.
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
