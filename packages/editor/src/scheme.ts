/**
 * Which link targets may become anchors.
 *
 * Three schemes, from ADR-0007 and PRD section 13.1. Everything else - a
 * script URL, a data URL, a file URL, a relative path - is refused, and the
 * caller renders the link as plain text instead of creating an anchor.
 *
 * # Why this file contains no absolute URL
 *
 * `packages/editor` is bundled into the offline remote form, and
 * `scripts/check-remote-form-offline.mjs` reads the **built bytes** and fails
 * the build on any absolute `http(s)` URL in them. An allowlist written as a
 * list of URL prefixes would therefore break that build, and the guard must not
 * be weakened to accommodate this package (ADR-0009).
 *
 * So the allowlist is a list of **protocol** strings, which is what
 * `URL.protocol` returns: scheme plus colon, no slashes. They are not absolute
 * URLs and the guard has no reason to object to them.
 *
 * # Why parsing rather than matching
 *
 * A prefix comparison is the classic way to get this wrong. `URL` applies the
 * WHATWG parser: it lowercases the scheme, resolves escapes, and rejects a
 * target that is not a valid absolute URL at all. A target that survives it is
 * one a browser would agree with - which is the only opinion that matters when
 * the result becomes an `href`.
 */

/**
 * Permitted values of `URL.protocol`.
 *
 * Scheme and colon, as the URL parser reports them, always lowercase.
 */
const ALLOWED_PROTOCOLS: readonly string[] = ['http:', 'https:', 'mailto:'];

/** The schemes, without the colon, for use in an error message. */
export const ALLOWED_SCHEMES: readonly string[] = ALLOWED_PROTOCOLS.map((protocol) =>
  protocol.slice(0, -1),
);

/**
 * The safe form of a link target, or `null` if it may not become an anchor.
 *
 * `null` for every one of these, deliberately without distinguishing them: a
 * disallowed scheme, a relative path, a protocol-relative target, an empty
 * target, and anything the URL parser refuses. The caller's response is the
 * same in every case - render it as text - so telling them apart would be a
 * distinction with no use.
 *
 * The returned string is the parser's normalised form rather than the input,
 * so what reaches an `href` is what the parser vouched for and not what the
 * author typed.
 */
export function safeLinkTarget(raw: string): string | null {
  const target = raw.trim();
  if (target === '') {
    return null;
  }

  let parsed: URL;
  try {
    // No base: a relative target has no scheme, so it cannot be allowlisted
    // and must not be resolved against whatever page happens to be rendering.
    parsed = new URL(target);
  } catch {
    return null;
  }

  if (!ALLOWED_PROTOCOLS.includes(parsed.protocol)) {
    return null;
  }

  return parsed.href;
}

/** Whether a link target may become an anchor. */
export function isAllowedLinkTarget(raw: string): boolean {
  return safeLinkTarget(raw) !== null;
}
