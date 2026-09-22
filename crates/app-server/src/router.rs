//! Assembling the LAN application.
//!
//! # Security headers
//!
//! This server sets its own, and they are not the Tauri window's. The Tauri CSP
//! governs the Host UI inside the desktop app; this one governs a page served to
//! somebody's phone. Neither weakens the other, and a test asserts both.
//!
//! The policy is the strictest one the bundle can run under: everything from
//! `'self'`, nothing from anywhere else. `connect-src 'self'` is what keeps a
//! compromised or mistaken bundle from talking to anything off the device -
//! there is no cloud, no telemetry and no CDN in this application, and the
//! policy is where that stops being a promise and becomes a rule the browser
//! enforces (PRD section 4, section 24).
//!
//! `connect-src 'self'` covers the notification socket as well. Under CSP
//! Level 3 `'self'` matches a `ws://` URL whose host and port are the page's
//! own, which is the only socket this bundle opens. Adding `ws:` would widen
//! the policy to every host on the network for no gain.
//!
//! # Body limits
//!
//! Almost every body this server accepts is a single identifier. A kilobyte is
//! generous for that, and it means an untrusted client on the LAN cannot make
//! the Host allocate by sending a large payload (architecture rules section
//! 10).
//!
//! A note is the exception, and it gets a **route-scoped** override rather than
//! a raised global. Two separate limits, doing two separate jobs:
//!
//! | Limit | Job |
//! | --- | --- |
//! | 1 KiB, global | nobody makes the Host allocate for an identifier |
//! | 192 KiB, `PUT /api/note` only | nobody makes the Host allocate for a note |
//! | 64 KiB, `app-core::note` | the actual rule about how long a note may be |
//!
//! So a body between 64 and 192 KiB is read and then refused by the domain,
//! with a message naming the limit and the measured size; a body past 192 KiB
//! is refused by the transport with a 413. The domain remains the size
//! authority, and every other route - including the asset fallback - keeps its
//! kilobyte (ADR-0020).

use axum::extract::DefaultBodyLimit;
use axum::http::{header, HeaderValue};
use axum::routing::{get, post, put};
use axum::Router;
use tower_http::set_header::SetResponseHeaderLayer;

use crate::assets;
use crate::routes;
use crate::state::LanState;
use crate::ws;

/// The content security policy served with every LAN response.
///
/// Deliberately no remote origin of any kind. `'unsafe-inline'` is permitted for
/// styles only, which is what the bundler emits; scripts stay strict, so an
/// injected `<script>` cannot run even if participant content ever reached the
/// page.
const CSP: &str = "default-src 'self'; \
                   script-src 'self'; \
                   style-src 'self' 'unsafe-inline'; \
                   img-src 'self' data:; \
                   connect-src 'self'; \
                   font-src 'self'; \
                   object-src 'none'; \
                   base-uri 'self'; \
                   form-action 'self'; \
                   frame-ancestors 'none'";

/// Largest request body most routes accept.
const MAX_BODY_BYTES: usize = 1024;

/// Largest request body `PUT /api/note` reads.
///
/// Three times the 64 KiB a note may contain, which covers JSON escaping of a
/// body made entirely of quotes or backslashes and leaves room for the wrapper.
/// It is a memory guard, not the note rule: content that fits here and not in
/// 64 KiB is refused by `app-core::note`, which can say exactly by how much.
const MAX_NOTE_BODY_BYTES: usize = 192 * 1024;

/// Build the LAN application.
///
/// Returned as a plain `Router` so tests can drive it with `oneshot` without
/// binding a port - the HTTP contract is then tested against the real router
/// rather than a stand-in.
pub fn router(state: LanState) -> Router {
    let api = Router::new()
        .route("/api/join/{token}", get(routes::join))
        .route("/api/join/{token}/claim", post(routes::claim))
        .route("/api/session", get(routes::session))
        // The participant's own note. No path segment and no identifier in
        // the body: the meeting and the participant come from the resolved
        // session, so addressing somebody else's note is inexpressible rather
        // than merely refused (ADR-0020).
        //
        // Registered as two method routes on one path, which axum merges,
        // because the larger body limit belongs to the **write** and not to
        // the path. `GET` reads no body at all, so widening its limit would
        // grant an allowance nothing needs - and an allowance nothing needs is
        // the kind of thing a later handler quietly starts using.
        .route("/api/note", get(routes::read_note))
        // The only route in the application that may read more than a
        // kilobyte. Applied closer to the handler than the global layer below,
        // so it is the one this extractor sees; every other route, method and
        // the asset fallback keep the kilobyte.
        .route(
            "/api/note",
            put(routes::write_note).layer(DefaultBodyLimit::max(MAX_NOTE_BODY_BYTES)),
        )
        // Server-to-client notification only. The credential travels in the
        // subprotocol, never in this path (ADR-0018).
        .route("/ws", get(ws::connect));

    Router::new()
        .merge(api)
        // Every other path is the single-page bundle: `/join/{token}` is a
        // client-side route with no file behind it.
        .fallback(|uri: axum::http::Uri| async move { assets::serve(&uri) })
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(header_layer(header::CONTENT_SECURITY_POLICY, CSP))
        // A browser must not second-guess a declared content type; without this
        // a served asset could be interpreted as something else.
        .layer(header_layer(header::X_CONTENT_TYPE_OPTIONS, "nosniff"))
        // The join URL is an operational secret and it is in the address bar.
        // Without this it would be sent to any origin the page ever links to.
        .layer(header_layer(header::REFERRER_POLICY, "no-referrer"))
        .layer(header_layer(header::X_FRAME_OPTIONS, "DENY"))
        .with_state(state)
}

/// Set a fixed header on every response, overriding any handler value.
fn header_layer(
    name: header::HeaderName,
    value: &'static str,
) -> SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::overriding(name, HeaderValue::from_static(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_policy_allows_no_remote_origin() {
        // The rule that makes "no cloud, no CDN, no telemetry" enforceable by
        // the browser rather than only stated in a document.
        assert!(!CSP.contains("http://"), "{CSP}");
        assert!(!CSP.contains("https://"), "{CSP}");
        assert!(!CSP.contains('*'), "{CSP}");
        assert!(CSP.contains("connect-src 'self'"), "{CSP}");
        assert!(CSP.contains("default-src 'self'"), "{CSP}");
        assert!(CSP.contains("object-src 'none'"), "{CSP}");
        assert!(CSP.contains("frame-ancestors 'none'"), "{CSP}");
    }

    #[test]
    fn scripts_may_not_be_inlined_even_though_styles_may() {
        // A bundler emits inline styles; nothing legitimate here emits an
        // inline script, so the loophole stays closed on the side that matters.
        let script_src = CSP
            .split("script-src")
            .nth(1)
            .expect("script-src")
            .split(';')
            .next()
            .expect("directive");
        assert!(!script_src.contains("unsafe-inline"), "{script_src}");
        assert!(!CSP.contains("unsafe-eval"), "{CSP}");
        assert!(CSP.contains("style-src 'self' 'unsafe-inline'"), "{CSP}");
    }
}
