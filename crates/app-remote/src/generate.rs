//! Producing the standalone offline form.
//!
//! One HTML file, made by injecting a payload into the built
//! `apps/remote-form` template, which is compiled into the binary with
//! `rust-embed` (ADR-0021 decision 1, and ADR-0006 which named the crate for
//! exactly this).
//!
//! ```text
//! apps/remote-form/dist/index.html      built by npm, offline-guard checked
//!        |  rust-embed
//!        v
//! generate(context) -> String           one payload injected, then verified
//! ```
//!
//! Nothing here touches the filesystem. `src-tauri` writes the string, because
//! reading and writing files is the shell's job and a crate that parses
//! untrusted input is not the place to grow one.
//!
//! # The JSON island, and why the escaping is load-bearing
//!
//! The payload travels in
//!
//! ```html
//! <script type="application/json" id="submission-context">{...}</script>
//! ```
//!
//! read with `textContent` and `JSON.parse`. A `<script>` element ends at the
//! first `</script`, and **the payload is participant-facing data**: a meeting
//! titled `</script><img onerror=...>` would otherwise close the element and the
//! rest of the payload would become markup, inside a file that runs from
//! `file://` with no server and no CSP to fall back on.
//!
//! So [`escape_for_island`] escapes **every** `<` as `\u003c`. Not `</script`,
//! not a case-insensitive variant of it - every one. A `<` that cannot be
//! written cannot begin a tag, a comment or a processing instruction, and there
//! is no cleverness left for a future reader to get wrong. U+2028 and U+2029 go
//! the same way, so the payload stays safe to lift into a JavaScript string if
//! this ever needs the sentinel fallback.
//!
//! Both escapes are transparent to `JSON.parse`: `\u003c` decodes back to `<`,
//! so the form receives exactly what the Host put in.
//!
//! # Generation verifies its own output
//!
//! [`verify_artifact`] runs before the caller is given the string, so an
//! artefact that fails its own offline check is never written to disk. It is
//! the Rust counterpart of `scripts/check-remote-form-offline.mjs`, which reads
//! the *template*'s built bytes at build time; this reads the *generated* file,
//! which nothing else checks.
//!
//! The network scan deliberately skips the payload. A note may legitimately
//! contain `https://example.com` - links are part of the Markdown subset - and
//! a URL inside a JSON string is inert text that nothing fetches. What the
//! payload is checked for instead is the one property that matters there: that
//! it cannot leave its element.

use rust_embed::RustEmbed;

use crate::context::RemoteFormContext;
use crate::{RemoteError, RemoteResult};

/// The built remote form template.
#[derive(RustEmbed)]
#[folder = "../../apps/remote-form/dist"]
struct Template;

/// The element the payload is injected into.
///
/// Must equal `SUBMISSION_CONTEXT_ELEMENT_ID` in `packages/contracts`.
pub const CONTEXT_ELEMENT_ID: &str = "submission-context";

/// The opening tag, exactly as `apps/remote-form/index.html` writes it and as
/// the single-file build preserves it.
const ISLAND_OPEN: &str = r#"<script type="application/json" id="submission-context">"#;

const ISLAND_CLOSE: &str = "</script>";

/// What must never appear in a generated artefact, outside the payload.
///
/// The same properties `scripts/check-remote-form-offline.mjs` enforces on the
/// template, restated here so the *generated* file is held to them too. The two
/// lists are deliberately separate: that guard reads built bytes at build time
/// and must not be weakened, and this one reads a file that only ever exists at
/// run time.
pub const FORBIDDEN_IN_ARTIFACT: &[&str] = &[
    "fetch(",
    "fetch (",
    "XMLHttpRequest",
    "WebSocket",
    "sendBeacon",
    "EventSource",
    "import(",
    // A credential has no business in this file at all (architecture rules
    // section 20). `RemoteFormContext` has nowhere to put one, so this catches a
    // template that started carrying one rather than a payload that leaked it.
    "Bearer ",
    "session_token",
    "join_token",
    "token_hash",
];

/// Whether a real template was embedded.
///
/// `build.rs` creates the directory when it is missing so the crate always
/// compiles on a fresh clone; that makes "was the form actually built?" a
/// separate question, and this is how it is asked.
#[must_use]
pub fn is_bundled() -> bool {
    Template::get("index.html").is_some()
}

/// Build one standalone offline HTML file for `context`.
///
/// The returned string is complete: it opens from `file://` with the machine
/// offline, carries no credential, and makes no network request of any kind.
pub fn generate(context: &RemoteFormContext) -> RemoteResult<String> {
    let template = Template::get("index.html").ok_or_else(|| RemoteError::Template {
        detail: "no template was built into this application. \
                 Run `npm run build:remote` and start the host again."
            .to_owned(),
    })?;

    let html =
        String::from_utf8(template.data.into_owned()).map_err(|_| RemoteError::Template {
            detail: "the built template is not valid UTF-8".to_owned(),
        })?;

    let payload = serde_json::to_string(context).map_err(|error| RemoteError::Template {
        detail: format!("the generation context could not be serialised: {error}"),
    })?;

    let artifact = inject(&html, &escape_for_island(&payload))?;
    verify_artifact(&artifact)?;
    Ok(artifact)
}

/// Replace the island's contents, leaving every other byte alone.
fn inject(html: &str, payload: &str) -> RemoteResult<String> {
    let (start, end) = island_bounds(html)?;
    let mut out = String::with_capacity(html.len() + payload.len());
    out.push_str(&html[..start]);
    out.push_str(payload);
    out.push_str(&html[end..]);
    Ok(out)
}

/// Byte offsets of the island's contents: after the opening tag, before `</script>`.
fn island_bounds(html: &str) -> RemoteResult<(usize, usize)> {
    let open = html
        .find(ISLAND_OPEN)
        .ok_or_else(|| RemoteError::Template {
            detail: format!(
                "expected one <script type=\"application/json\" id=\"{CONTEXT_ELEMENT_ID}\"> \
             element, detected none. The template must carry the injection point."
            ),
        })?;

    if html[open + ISLAND_OPEN.len()..].contains(ISLAND_OPEN) {
        return Err(RemoteError::Template {
            detail: format!("expected exactly one {CONTEXT_ELEMENT_ID} element, detected several"),
        });
    }

    let start = open + ISLAND_OPEN.len();
    let close = html[start..]
        .find(ISLAND_CLOSE)
        .ok_or_else(|| RemoteError::Template {
            detail: format!("the {CONTEXT_ELEMENT_ID} element is never closed"),
        })?;

    Ok((start, start + close))
}

/// Make a JSON payload safe to sit inside a `<script>` element.
///
/// Every `<` becomes `\u003c`, so no tag, comment or processing instruction can
/// begin inside the payload and the element cannot be closed early. U+2028 and
/// U+2029 become their escapes for the same reason one layer up.
///
/// All three are transparent to `JSON.parse`, so the form reads exactly what was
/// put in. `<` cannot appear outside a JSON string, so nothing structural is
/// touched.
#[must_use]
fn escape_for_island(json: &str) -> String {
    json.replace('<', "\\u003c")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

/// Check that a generated artefact is what it claims to be.
///
/// Run by [`generate`] before the string is returned, so a failing artefact is
/// never written. Exposed so a test - and `src-tauri`, before it writes - can
/// ask the same question of a file it is holding.
pub fn verify_artifact(html: &str) -> RemoteResult<()> {
    let (start, end) = island_bounds(html).map_err(|error| RemoteError::Artifact {
        detail: error.to_string(),
    })?;
    let payload = &html[start..end];

    // The property that keeps the payload inside its element. Checked on the
    // artefact rather than trusted from the escaper, because this is the
    // assertion the whole design rests on.
    if payload.contains('<') {
        return Err(RemoteError::Artifact {
            detail: "the embedded context contains an unescaped '<', so it could \
                     close its own script element"
                .to_owned(),
        });
    }
    if payload.contains('\u{2028}') || payload.contains('\u{2029}') {
        return Err(RemoteError::Artifact {
            detail: "the embedded context contains an unescaped line separator".to_owned(),
        });
    }

    // It must still be readable as the thing the form will parse.
    serde_json::from_str::<RemoteFormContext>(payload).map_err(|error| RemoteError::Artifact {
        detail: format!("the embedded context is not a readable generation context: {error}"),
    })?;

    // Everything except the payload: the template, which the npm guard already
    // checked at build time and which injection must not have changed.
    let mut shell = String::with_capacity(html.len() - payload.len());
    shell.push_str(&html[..start]);
    shell.push_str(&html[end..]);

    for forbidden in FORBIDDEN_IN_ARTIFACT {
        if shell.contains(forbidden) {
            return Err(RemoteError::Artifact {
                detail: format!(
                    "expected no `{forbidden}` outside the embedded context, detected one. \
                     The form must open offline with no network access and no credential."
                ),
            });
        }
    }

    for absolute in ["http://", "https://"] {
        if let Some(found) = first_foreign_url(&shell, absolute) {
            return Err(RemoteError::Artifact {
                detail: format!(
                    "expected no absolute URL outside the embedded context, detected {found:?}. \
                     Every resource must be inlined (architecture rules section 6)."
                ),
            });
        }
    }

    Ok(())
}

/// The first absolute URL that is not a W3C namespace.
///
/// `www.w3.org` URIs are required by XML and SVG and are never fetched, which is
/// the one exclusion `scripts/check-remote-form-offline.mjs` also makes.
fn first_foreign_url(shell: &str, scheme: &str) -> Option<String> {
    let mut rest = shell;
    while let Some(at) = rest.find(scheme) {
        let tail = &rest[at + scheme.len()..];
        if !tail.starts_with("www.w3.org/") {
            let end = tail
                .find(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == '<')
                .unwrap_or(tail.len())
                .min(60);
            return Some(format!("{scheme}{}", &tail[..end]));
        }
        rest = tail;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_core::id::{MeetingId, ParticipantId, SubmissionId};

    fn context(title: &str, note: &str) -> RemoteFormContext {
        RemoteFormContext {
            schema_version: crate::SCHEMA_VERSION,
            submission_id: SubmissionId::new(),
            meeting_id: MeetingId::new(),
            meeting_title: title.to_owned(),
            meeting_date: "2026-09-20".to_owned(),
            meeting_timezone: "Asia/Makassar".to_owned(),
            participant_id: ParticipantId::new(),
            participant_name: "Budi Santoso".to_owned(),
            source_version: 0,
            existing_content: note.to_owned(),
            generated_at: "2026-09-23T01:00:00.000Z".to_owned(),
        }
    }

    /// A stand-in template, so these tests do not depend on `npm run build:remote`.
    fn shell() -> String {
        format!("<!doctype html><html><body><div id=\"root\"></div>{ISLAND_OPEN}null{ISLAND_CLOSE}</body></html>")
    }

    fn inject_into_shell(context: &RemoteFormContext) -> String {
        inject_into(&shell(), context)
    }

    /// Inject a real context into `html`, for the tests that vary the template.
    fn inject_into(html: &str, context: &RemoteFormContext) -> String {
        let payload = escape_for_island(&serde_json::to_string(context).unwrap());
        inject(html, &payload).expect("the template has an island")
    }

    #[test]
    fn a_hostile_title_cannot_close_the_script_element() {
        let artifact = inject_into_shell(&context(
            "</script><img src=x onerror=alert(1)>",
            "ordinary note",
        ));

        assert!(!artifact.contains("</script><img"), "{artifact}");
        // The escape is spelled `\u003c`; `>` needs none once the `<` is gone.
        assert!(artifact.contains("\\u003c/script>"), "{artifact}");

        // The property, rather than its spelling: no `<` survives inside the
        // island, so nothing in it can begin a tag.
        let (start, end) = island_bounds(&artifact).unwrap();
        assert!(!artifact[start..end].contains('<'), "{artifact}");

        // Exactly one closing tag: the island's own.
        assert_eq!(artifact.matches(ISLAND_CLOSE).count(), 1, "{artifact}");
        verify_artifact(&artifact).expect("still self-contained");
    }

    #[test]
    fn a_hostile_note_cannot_close_the_script_element_either() {
        // The prefilled note is participant-facing data too, and it is the field
        // most likely to contain markup someone typed on purpose.
        let artifact = inject_into_shell(&context(
            "Weekly Coordination",
            "Compare `a < b`\n\n</SCRIPT ><script>alert(1)</script>\n<!-- comment -->",
        ));

        assert_eq!(artifact.matches("<script").count(), 1, "{artifact}");
        assert_eq!(artifact.matches(ISLAND_CLOSE).count(), 1, "{artifact}");
        assert!(!artifact.contains("<!--"), "{artifact}");
        verify_artifact(&artifact).expect("still self-contained");
    }

    #[test]
    fn line_separators_are_escaped() {
        let artifact = inject_into_shell(&context("Weekly", "one\u{2028}two\u{2029}three"));
        let (start, end) = island_bounds(&artifact).unwrap();
        let payload = &artifact[start..end];

        assert!(!payload.contains('\u{2028}'), "{payload}");
        assert!(payload.contains("\\u2028"), "{payload}");
        assert!(payload.contains("\\u2029"), "{payload}");
    }

    #[test]
    fn the_payload_round_trips_through_the_escaping() {
        // What the form will do: read `textContent`, `JSON.parse`. The escapes
        // must be transparent, or the participant is shown mangled text.
        let original = context("</script> & <b>bold</b>", "a < b\u{2028}and more");
        let artifact = inject_into_shell(&original);
        let (start, end) = island_bounds(&artifact).unwrap();

        let decoded: RemoteFormContext = serde_json::from_str(&artifact[start..end]).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn a_note_may_contain_an_absolute_url() {
        // Links are part of the Markdown subset. A URL inside a JSON string is
        // inert text, and refusing it would refuse an ordinary meeting note.
        let artifact = inject_into_shell(&context(
            "Weekly",
            "See [the brief](https://example.com/brief) and mail <someone@example.com>",
        ));
        verify_artifact(&artifact).expect("a link in a note is not a network call");
    }

    #[test]
    fn an_absolute_url_in_the_template_is_refused() {
        let artifact = shell().replace(
            "<div id=\"root\"></div>",
            "<script src=\"https://cdn.example.com/x.js\"></script>",
        );
        let artifact = inject_into(&artifact, &context("Weekly", "note"));

        let error = verify_artifact(&artifact).unwrap_err();
        assert!(
            matches!(&error, RemoteError::Artifact { detail } if detail.contains("cdn.example.com")),
            "{error}"
        );
    }

    #[test]
    fn a_network_call_in_the_template_is_refused() {
        let artifact = shell().replace("<div id=\"root\"></div>", "<script>fetch('/x')</script>");
        let artifact = inject_into(&artifact, &context("Weekly", "note"));
        assert!(matches!(
            verify_artifact(&artifact),
            Err(RemoteError::Artifact { .. })
        ));
    }

    #[test]
    fn a_credential_in_the_template_is_refused() {
        let artifact = shell().replace(
            "<div id=\"root\"></div>",
            "<script>const t = 'session_token';</script>",
        );
        let artifact = inject_into(&artifact, &context("Weekly", "note"));
        assert!(matches!(
            verify_artifact(&artifact),
            Err(RemoteError::Artifact { .. })
        ));
    }

    #[test]
    fn a_template_without_an_island_is_refused() {
        let error = inject("<html><body></body></html>", "null").unwrap_err();
        assert!(matches!(error, RemoteError::Template { .. }), "{error}");
    }

    #[test]
    fn the_w3c_namespace_is_the_only_absolute_url_allowed() {
        // XML and SVG require it and nothing fetches it. The same single
        // exclusion the npm guard makes, and no other.
        let artifact = shell().replace(
            "<div id=\"root\"></div>",
            "<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>",
        );
        let artifact = inject_into(&artifact, &context("Weekly", "note"));
        verify_artifact(&artifact).expect("the W3C namespace is not a resource");
    }
}
