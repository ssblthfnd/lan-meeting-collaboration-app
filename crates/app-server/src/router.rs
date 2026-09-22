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
//! The only body any route accepts is a single identifier. A kilobyte is
//! generous, and it means an untrusted client on the LAN cannot make the Host
//! allocate by sending a large payload (architecture rules section 10).

use axum::extract::DefaultBodyLimit;
use axum::http::{header, HeaderValue};
use axum::routing::{get, post};
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

/// Largest request body any route accepts.
const MAX_BODY_BYTES: usize = 1024;

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
