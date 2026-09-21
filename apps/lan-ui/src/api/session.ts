/**
 * Where the session token lives in the browser.
 *
 * `sessionStorage`, not a cookie and not `localStorage`:
 *
 * - **Not a cookie**, because a cookie is sent automatically with every request
 *   and would create a cross-site request forgery surface on a transport that
 *   has no origin isolation to begin with. A bearer token is only ever sent
 *   because this code chose to send it (ADR-0016).
 * - **Not `localStorage`**, because a meeting is a sitting, not an account. The
 *   credential should not outlive the tab on a phone that gets passed around or
 *   a shared machine in a meeting room.
 *
 * The token is the participant's only credential (ADR-0002 rule 6), so it is
 * read and written in exactly one place. Nothing else in the bundle touches
 * storage.
 *
 * Every access is guarded: `sessionStorage` throws in a private window with site
 * data blocked, and a participant should get the join screen rather than a blank
 * page.
 */

/** One key, namespaced, so nothing else in the origin can collide with it. */
const TOKEN_KEY = 'lan-meeting:session-token';

/** The stored token, or `null` when there is none or storage is unavailable. */
export function storedToken(): string | null {
  try {
    const value = sessionStorage.getItem(TOKEN_KEY);
    return value === null || value === '' ? null : value;
  } catch {
    return null;
  }
}

/**
 * Remember the token returned by a successful claim.
 *
 * If storage is unavailable the claim still worked - the participant simply has
 * to claim again after a reload, which is a degraded experience rather than a
 * failure, so this does not throw.
 */
export function rememberToken(token: string): void {
  try {
    sessionStorage.setItem(TOKEN_KEY, token);
  } catch {
    // Deliberately ignored; see above.
  }
}

/**
 * Forget the token.
 *
 * Used when the backend says the session is no longer live - revoked by the
 * Host, or belonging to a meeting that has moved on. Keeping a credential the
 * server has already rejected would mean retrying it on every load.
 */
export function forgetToken(): void {
  try {
    sessionStorage.removeItem(TOKEN_KEY);
  } catch {
    // Deliberately ignored; see above.
  }
}

/**
 * The join token from the current URL.
 *
 * The bundle is served at `/join/{token}`, a client-side route with no file
 * behind it, so the token is read from the address bar. It is an operational
 * secret that reached this browser because someone scanned or was sent it
 * (ADR-0002); nothing here validates it, because only the backend can.
 */
export function joinTokenFromUrl(): string | null {
  const match = /^\/join\/([^/?#]+)/.exec(window.location.pathname);
  const token = match?.[1];
  return token === undefined || token === '' ? null : decodeURIComponent(token);
}
