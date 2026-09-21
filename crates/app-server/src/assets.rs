//! The participant UI, embedded in the binary.
//!
//! `apps/lan-ui/dist` is compiled into the executable, so a Host machine serves
//! participants with no files on disk, no unpacking step and nothing to point a
//! web server at (ADR-0006).
//!
//! # Only this bundle
//!
//! The embedded path is `apps/lan-ui/dist` and nothing else. `apps/host-ui` is
//! never served over the network - it is the Host's own window and it talks to
//! Tauri commands that a browser has no business reaching (CLAUDE.md, the three
//! UI bundles). A test asserts this crate does not reference `host-ui` at all.
//!
//! # Single-page fallback
//!
//! `/join/{token}` is a client-side route: there is no `join/{token}.html` to
//! serve, so an unmatched path that is not an asset returns `index.html` and the
//! bundle reads the token from its own URL. Unmatched *asset* paths return 404
//! rather than HTML, so a missing script fails visibly instead of arriving as a
//! page that will not parse.

use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

/// The built LAN participant bundle.
#[derive(RustEmbed)]
#[folder = "../../apps/lan-ui/dist"]
struct Bundle;

/// Whether a real bundle was embedded.
///
/// `build.rs` creates the directory when it is missing so that the crate always
/// compiles on a fresh clone; that makes "did the UI actually get built?" a
/// separate question, and this is how it is asked. The Host is told at startup
/// rather than finding out when a participant sees a blank page.
#[must_use]
pub fn is_bundled() -> bool {
    Bundle::get("index.html").is_some()
}

/// Number of files embedded. Diagnostic only.
#[must_use]
pub fn bundled_file_count() -> usize {
    Bundle::iter().count()
}

/// Serve `index.html`, or a plain explanation when no bundle was built.
pub fn index() -> Response {
    match Bundle::get("index.html") {
        Some(file) => html(file.data.into_owned()),
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            "The participant interface was not built into this application. \
             Run `npm run build:lan` and start the host again.",
        )
            .into_response(),
    }
}

/// Serve one embedded asset, or fall back to the single-page entry point.
pub fn serve(uri: &Uri) -> Response {
    let path = uri.path().trim_start_matches('/');

    if let Some(file) = Bundle::get(path) {
        let mime = file.metadata.mimetype().to_owned();
        return (
            [
                (header::CONTENT_TYPE, mime),
                // Hashed filenames from the bundler, so the content at a given
                // path never changes. Revalidation on a LAN costs nothing, but
                // a phone on a flaky connection benefits from the cache.
                (
                    header::CACHE_CONTROL,
                    "public, max-age=31536000, immutable".to_owned(),
                ),
            ],
            file.data.into_owned(),
        )
            .into_response();
    }

    // An asset that is genuinely missing must fail as a missing asset, not as
    // HTML with a 200 - a bundler path typo would otherwise look like a
    // mysterious parse error in the browser.
    if looks_like_an_asset(path) {
        return (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            "Not found",
        )
            .into_response();
    }

    index()
}

/// An HTML response that is never cached.
///
/// The document is small and the bundle it references is hashed, so caching it
/// would only risk serving a stale shell after an update.
fn html(body: Vec<u8>) -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        body,
    )
        .into_response()
}

/// Whether a path is asking for a file rather than a page.
fn looks_like_an_asset(path: &str) -> bool {
    path.starts_with("assets/")
        || path.rsplit('/').next().is_some_and(|last| {
            last.rsplit_once('.')
                .is_some_and(|(_, extension)| !extension.is_empty() && extension != "html")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_paths_are_told_apart_from_page_routes() {
        // Page routes: the SPA handles them.
        for page in ["", "join/abc123", "join", "index.html"] {
            assert!(!looks_like_an_asset(page), "{page} is a page route");
        }

        // Asset paths: a miss must be a 404.
        for asset in [
            "assets/index-abc.js",
            "assets/style.css",
            "favicon.ico",
            "assets/nested/thing.woff2",
        ] {
            assert!(looks_like_an_asset(asset), "{asset} is an asset");
        }
    }
}
