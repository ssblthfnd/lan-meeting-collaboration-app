/**
 * The only module that opens a socket.
 *
 * One connection, one direction: the server sends, this bundle listens. There
 * is nothing to send back - every mutation goes over HTTP through
 * {@link ../api/lanApi}, where the backend's extractors and the domain boundary
 * are (ADR-0018). `send` is never called here, and a future caller wanting to
 * should be made to explain why.
 *
 * # The credential is not in the URL
 *
 * ```ts
 * new WebSocket(url, [LAN_SOCKET_SUBPROTOCOL, token]);
 * ```
 *
 * A browser cannot set a request header on a WebSocket handshake, so the token
 * travels as the second entry of the subprotocol list - which *is* a request
 * header. Putting it in the query string instead would leave a bearer
 * credential in access logs, `Referer` headers and browser history, and a
 * structural test in `src-tauri/tests/boundaries.rs` keeps it out of the URL
 * built below.
 *
 * # Nothing here is authoritative
 *
 * An event says something changed; the caller refetches over HTTP to learn what
 * (architecture rules section 16). That is why a dropped connection costs
 * nothing but a refetch, why there is no replay to reason about, and why a
 * reconnect simply re-reads current state.
 */

import { LAN_SOCKET_CLOSE, LAN_SOCKET_SUBPROTOCOL } from '@lan-meeting/contracts';
import type { LanEvent } from '@lan-meeting/contracts';

import { storedToken } from './session';

/** What a caller is told. */
export interface RealtimeHandlers {
  /** A frame arrived. Treat it as a cue to refetch, not as data. */
  readonly onEvent: (event: LanEvent) => void;
  /**
   * The host ended this session, so the credential is dead.
   *
   * Distinct from an ordinary disconnect, because the responses are opposite:
   * a dropped connection should be retried, and a revoked one must not be.
   */
  readonly onRevoked: () => void;
}

/**
 * How long to wait before the nth reconnection attempt, in milliseconds.
 *
 * Doubling from a quarter of a second to eight seconds. The first retries are
 * quick because the common cause is trivial - a laptop lid, a Wi-Fi roam - and
 * the ceiling exists because the other common cause is the host having closed
 * the meeting, and a phone left on a table should not spend the afternoon
 * knocking on a door nobody is behind.
 *
 * A one-shot `setTimeout` per attempt, never a repeating timer: a timer that
 * keeps firing while a connection is in flight is how a bundle ends up with
 * several sockets it has forgotten about.
 */
function backoff(attempt: number): number {
  const base = 250 * 2 ** Math.min(attempt, 5);
  // A little jitter, so 99 phones that lost the host at the same moment do not
  // all come back in the same millisecond.
  return base + Math.floor(Math.random() * 250);
}

/** `ws://` for a page served over HTTP, `wss://` if that ever changes. */
function socketUrl(): string {
  const scheme = window.location.protocol === 'https:' ? 'wss' : 'ws';
  return `${scheme}://${window.location.host}/ws`;
}

/**
 * Keep a socket open for as long as the caller wants one.
 *
 * Returns a function that closes it and stops reconnecting. Calling it is the
 * only way the loop ends, apart from the host revoking the session.
 *
 * If there is no stored token there is nothing to authenticate with, so this
 * does nothing rather than opening a socket that would be refused.
 */
export function connectRealtime(handlers: RealtimeHandlers): () => void {
  let socket: WebSocket | null = null;
  let retry: number | null = null;
  let attempt = 0;
  let stopped = false;

  function open(): void {
    if (stopped) {
      return;
    }

    const token = storedToken();
    if (token === null) {
      return;
    }

    let next: WebSocket;
    try {
      // The tag first, the credential second. The server accepts exactly this
      // pair and echoes back only the tag.
      next = new WebSocket(socketUrl(), [LAN_SOCKET_SUBPROTOCOL, token]);
    } catch {
      // A browser that refuses to construct the socket at all - a content
      // security policy, a blocked scheme. Retrying is still right: the page
      // itself was served from this origin, so this is more likely transient
      // than permanent.
      schedule();
      return;
    }
    socket = next;

    next.onopen = () => {
      // Reset only once a connection actually succeeded, so a server that
      // accepts and immediately drops does not reset the backoff each time.
      attempt = 0;
    };

    next.onmessage = (message) => {
      if (typeof message.data !== 'string') {
        return;
      }
      let event: LanEvent;
      try {
        event = JSON.parse(message.data) as LanEvent;
      } catch {
        // Not something this server sent. Ignored rather than escalated:
        // there is nothing a participant could do about it.
        return;
      }
      handlers.onEvent(event);
    };

    next.onclose = (closed) => {
      socket = null;
      if (closed.code === LAN_SOCKET_CLOSE.SESSION_REVOKED) {
        // The credential is gone. Reconnecting would be refused at the
        // handshake anyway, and retrying it forever would be worse than
        // telling the caller once.
        stopped = true;
        handlers.onRevoked();
        return;
      }
      // Everything else - including a resync close - is retried. The client
      // re-reads current state when it comes back, which is what it would have
      // done with the events it missed.
      schedule();
    };

    // A socket error is always followed by a close, which is where the
    // reconnect decision lives. Swallowed here so it does not reach the
    // console as an unhandled event.
    next.onerror = () => {};
  }

  function schedule(): void {
    if (stopped || retry !== null) {
      return;
    }
    const delay = backoff(attempt);
    attempt += 1;
    retry = window.setTimeout(() => {
      retry = null;
      open();
    }, delay);
  }

  open();

  return () => {
    stopped = true;
    if (retry !== null) {
      window.clearTimeout(retry);
      retry = null;
    }
    if (socket !== null) {
      // The handler is cleared first: this close is the caller's decision, and
      // it must not look like a disconnection to be retried.
      socket.onclose = null;
      socket.close();
      socket = null;
    }
  };
}
