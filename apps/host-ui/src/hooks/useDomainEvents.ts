/**
 * Reacting to what the backend just committed.
 *
 * The backend announces every committed mutation over Tauri IPC (ADR-0018).
 * This hook subscribes for as long as a component is mounted and hands over the
 * events for one meeting.
 *
 * # An event is a cue, never data
 *
 * The payload carries identifiers, a version and a timestamp - never content.
 * A handler's only correct response is to re-read through a command, which is
 * where the read model decides what this window may hold (ADR-0014). Rendering
 * anything straight from an event would be the frontend holding state it has no
 * right to, which is the thing SQLite being the source of truth rules out.
 *
 * # Filtering is presentation, not security
 *
 * Events arrive for every meeting, because the Host's window shows more than
 * one. Narrowing to the meeting on screen is so a change elsewhere does not
 * make this view reload for nothing. It is not a permission boundary: the Host
 * may read every row of their own database either way.
 */

import { useEffect, useRef } from 'react';
import type { DomainEventKind, HostDomainEvent, MeetingId } from '@lan-meeting/contracts';

import * as hostApi from '../api/hostApi';

/**
 * Call `onEvent` for every event about `meetingId`.
 *
 * `kinds` narrows further, so a component that only cares about presence is not
 * woken by a note. An empty list means every kind.
 *
 * The handler is held in a ref, so a caller may pass a fresh closure on every
 * render - the usual React case - without tearing the subscription down and
 * building it again each time.
 */
export function useDomainEvents(
  meetingId: MeetingId,
  kinds: readonly DomainEventKind[],
  onEvent: (event: HostDomainEvent) => void,
): void {
  const handler = useRef(onEvent);
  handler.current = onEvent;

  // Joined into a primitive so the effect's dependency is a value rather than
  // an array identity that changes on every render.
  const wanted = kinds.join(',');

  useEffect(() => {
    let cancelled = false;
    let unsubscribe: (() => void) | null = null;

    void hostApi
      .onDomainEvent((event) => {
        if (event.meeting_id !== meetingId) {
          return;
        }
        if (wanted !== '' && !wanted.split(',').includes(event.type)) {
          return;
        }
        handler.current(event);
      })
      .then((stop) => {
        // Registering a Tauri listener is asynchronous, so the component may
        // already have unmounted by the time it is ready. Without this, the
        // listener would outlive the view that asked for it.
        if (cancelled) {
          stop();
        } else {
          unsubscribe = stop;
        }
      })
      .catch(() => {
        // The window is closing, or IPC is unavailable. There is nothing a
        // Host can do about it and nothing to show them: the views that use
        // this hook all load their data through a command first, so they are
        // correct on arrival and merely stop updating by themselves.
      });

    return () => {
      cancelled = true;
      unsubscribe?.();
    };
  }, [meetingId, wanted]);
}
