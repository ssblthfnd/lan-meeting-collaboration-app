//! # app-server
//!
//! Local HTTP server for LAN participants.
//!
//! This crate is a **transport**, not a place for business rules. It:
//! - binds a LAN-reachable address chosen by the Host (it never assumes
//!   `127.0.0.1` is reachable by participants - architecture rules section 4)
//! - serves the embedded `apps/lan-ui` bundle and the `/join/{token}` entry
//! - resolves an `app_core::Actor` from a stored credential hash, and never
//!   from a client-supplied participant id (section 14.1)
//! - delegates every mutation to `app-core`
//!
//! # The two credentials
//!
//! | Credential | Where it travels | Establishes |
//! | --- | --- | --- |
//! | join token | the URL path | `Actor::Claimant`, which may only claim |
//! | session token | `Authorization: Bearer` | `Actor::Participant` |
//!
//! Neither is ever stored. `app-core` accepts only a `TokenHash`, so the
//! plaintext cannot reach the database even by mistake (PRD 22.2, 22.17).
//!
//! # What this crate may not do
//!
//! It is prohibited from touching database primitives at all. Every write goes
//! through `Domain`, whose methods require an authorization proof only
//! `app-core` can mint (ADR-0012). Reads go to `ParticipantQueries`, which is a
//! separate, narrower type from the Host's own (ADR-0014) - the Host may see
//! everything in their database and a participant may not.
//!
//! That prohibition is enforced from two directions, and it is worth being
//! precise about what each one does and does not cover:
//!
//! - **The dependency graph.** This crate does not depend on `rusqlite`, so no
//!   SQLite type can be named here.
//! - **A structural guard**, in `src-tauri/tests/boundaries.rs`, which scans
//!   this crate's sources for SQL text and for the connection and transaction
//!   primitives `app-db` exposes.
//!
//! The second exists because the first is not sufficient on its own. `app-db`
//! hands a transaction to a closure, and a transaction's methods can be called
//! on the value without naming its type - so "no SQLite dependency" makes a
//! bypass inconvenient rather than impossible. The guard is what closes that
//! gap, in the spirit of ADR-0011: a rule that lives only in a reviewer's
//! attention is a rule some future code path can skip.
//!
//! # Not here yet
//!
//! WebSocket. Realtime is its own roadmap step, and a notification channel with
//! nothing to notify about would be a guess at what that step needs. When it
//! arrives it will broadcast only *after* a successful SQLite commit, on
//! audience-scoped channels (architecture rules section 16).
//!
//! Status: Phase 1, step 6. Join, identity claim and session resolution.

#![forbid(unsafe_code)]

pub mod assets;
pub mod dto;
pub mod error;
pub mod extract;
pub mod network;
pub mod router;
pub mod routes;
pub mod state;
pub mod token;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use app_core::service::Domain;
use app_db::Db;
use thiserror::Error;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

pub use error::{ApiError, ApiResult};
pub use network::{interfaces, join_url, suggested, Interface};
pub use router::router;
pub use state::LanState;
pub use token::{hash_token, mint, Credential};

/// Port the Host uses unless it says otherwise.
///
/// The value architecture rules section 4 gives as the example. Host-overridable
/// because a fixed port is a thing that collides.
pub const DEFAULT_PORT: u16 = 8765;

pub type ServerResult<T> = Result<T, ServerError>;

/// Errors produced by the LAN transport.
#[derive(Debug, Error)]
pub enum ServerError {
    /// The configured address/port could not be bound.
    ///
    /// The message names the likely causes, because "failed to bind" on its own
    /// leaves the Host with nothing to do about it (architecture rules section
    /// 22). The two real causes on Windows are a port already in use and the
    /// firewall refusing the inbound rule.
    #[error(
        "could not start the meeting server on {addr}: {reason}. \
         The port may already be in use - try another one - or Windows Firewall \
         may be blocking it, in which case allow the app on private networks."
    )]
    Bind { addr: String, reason: String },
}

/// A running LAN server.
///
/// Dropping this does not stop the server; [`RunningServer::stop`] does, and it
/// waits for the socket to close. That distinction matters for the Host UI: a
/// "stopped" indicator must mean the port is actually free, or a restart on the
/// same port fails for reasons the Host cannot see.
pub struct RunningServer {
    address: SocketAddr,
    shutdown: oneshot::Sender<()>,
    finished: tokio::task::JoinHandle<()>,
}

impl RunningServer {
    /// The address actually bound.
    ///
    /// Not the address requested: asking for port 0 yields a real port, and the
    /// Host UI must show what participants should type.
    #[must_use]
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// Stop serving and wait for the socket to close.
    pub async fn stop(self) {
        // The receiver is dropped when the server task ends on its own, so a
        // send failure means it is already stopping.
        let _ = self.shutdown.send(());
        let _ = self.finished.await;
    }
}

/// Bind a LAN address and start serving.
///
/// `0.0.0.0` by default, because the Host's own address on the participants'
/// network is not knowable in advance and a participant cannot reach loopback.
///
/// This is called only when the Host explicitly asks: opening a meeting does not
/// start a server (ADR-0016). Binding a LAN-reachable socket prompts the
/// Windows Firewall, and that should happen when someone chose it.
pub async fn start(
    db: Arc<Db>,
    domain: Arc<Domain<Arc<Db>>>,
    port: u16,
) -> ServerResult<RunningServer> {
    let requested = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port);

    let listener = TcpListener::bind(requested)
        .await
        .map_err(|error| ServerError::Bind {
            addr: requested.to_string(),
            reason: error.to_string(),
        })?;

    let address = listener.local_addr().map_err(|error| ServerError::Bind {
        addr: requested.to_string(),
        reason: error.to_string(),
    })?;

    let app = router(LanState::with_domain(db, domain));
    let (shutdown, shutdown_rx) = oneshot::channel();

    let finished = tokio::spawn(async move {
        let served = axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await;

        if let Err(error) = served {
            eprintln!("[lan] the meeting server stopped unexpectedly: {error}");
        }
    });

    Ok(RunningServer {
        address,
        shutdown,
        finished,
    })
}
