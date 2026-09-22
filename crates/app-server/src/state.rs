//! What a LAN handler is given, and how blocking work leaves the async runtime.
//!
//! # Why every database call is wrapped
//!
//! `app-db` is synchronous: one writer behind a mutex, plus a pool of reading
//! connections (ADR-0006). Calling that directly from an async handler would
//! park a Tokio worker thread for the duration of a SQLite transaction, and with
//! the default worker count a handful of simultaneous claims would stall the
//! whole server - including the requests that were only reading.
//!
//! So every call goes through [`LanState::blocking`], which moves it onto
//! Tokio's blocking pool. `Domain` is `Sync` (the step-2 concurrency tests run
//! it across real threads), so an `Arc` of it moves into the closure cleanly.
//!
//! # No rules here
//!
//! This type holds handles and nothing else. It makes no authorization decision,
//! checks no meeting status and counts nothing: those belong to `app-core`, and
//! a handler reaches them through [`LanState::domain`].
//!
//! # Two event handles, and why they are not one
//!
//! [`LanState::realtime`] is the LAN's own broadcast channel and connection
//! registry: the sockets subscribe to it. [`LanState::events`] is the sink the
//! whole application publishes through, which in the assembled program is a
//! composite reaching both that channel *and* the Host's window over Tauri IPC
//! (ADR-0018).
//!
//! They are separate because a socket needs to *receive* from one particular
//! channel, while presence needs to *publish* to every audience. Publishing a
//! presence transition into the LAN channel alone would leave the Host - the
//! only audience presence has - never hearing about it.

use std::sync::Arc;
use std::time::Duration;

use app_core::event::EventSink;
use app_core::service::Domain;
use app_db::participant_query::ParticipantQueries;
use app_db::presence::PresenceStore;
use app_db::session_store::SessionStore;
use app_db::Db;
use tokio::sync::watch;

use crate::error::ApiError;
use crate::realtime::Realtime;

/// Everything a LAN handler needs.
///
/// Cloneable, because axum hands one to every request; the clone is a handful
/// of `Arc` bumps and shares the single database underneath.
#[derive(Clone)]
pub struct LanState {
    db: Arc<Db>,
    domain: Arc<Domain<Arc<Db>>>,
    events: Arc<dyn EventSink>,
    realtime: Arc<Realtime>,
    /// Set when the Host stops the server.
    ///
    /// Held as a sender so a clone of this state can signal, and so a socket
    /// can take a receiver at any point - including one that opened after the
    /// signal, which then sees it immediately.
    shutdown: Arc<watch::Sender<bool>>,
    keepalive: Duration,
}

impl LanState {
    /// Wrap an open database, with a realtime channel of its own.
    ///
    /// Builds the mutation boundary here and points it at that same channel, so
    /// a state made this way is internally consistent: a mutation through
    /// [`LanState::domain`] is heard by a socket subscribed through
    /// [`LanState::realtime`]. The assembled application does not use this - it
    /// shares the Host's boundary instead (ADR-0012) - but a test that only
    /// needs a LAN server should not have to wire four handles to get one.
    #[must_use]
    pub fn new(db: Arc<Db>) -> Self {
        let realtime = Arc::new(Realtime::new());
        let events = Arc::clone(&realtime) as Arc<dyn EventSink>;
        let domain = Arc::new(Domain::with_events(Arc::clone(&db), Arc::clone(&events)));
        Self::with_realtime(db, domain, events, realtime)
    }

    /// Share the Host's existing mutation boundary and event sink rather than
    /// building a second one.
    ///
    /// The Tauri process already holds a `Domain` over the same database and a
    /// sink reaching both audiences. One boundary for both transports is the
    /// point of ADR-0012: two would be two places for a rule to be enforced
    /// differently, and two sinks would be two places for an audience decision
    /// to drift.
    #[must_use]
    pub fn with_realtime(
        db: Arc<Db>,
        domain: Arc<Domain<Arc<Db>>>,
        events: Arc<dyn EventSink>,
        realtime: Arc<Realtime>,
    ) -> Self {
        let (shutdown, _) = watch::channel(false);
        Self {
            db,
            domain,
            events,
            realtime,
            shutdown: Arc::new(shutdown),
            keepalive: crate::ws::KEEPALIVE,
        }
    }

    /// Use a different keepalive interval.
    ///
    /// For tests that need a socket to be pinged inside a test's lifetime. The
    /// production value is [`crate::ws::KEEPALIVE`], and it is deliberately not
    /// configurable by the Host: a knob for it would be a knob for making the
    /// server write to SQLite more often, which is the thing the design avoids.
    #[must_use]
    pub fn with_keepalive(mut self, keepalive: Duration) -> Self {
        self.keepalive = keepalive;
        self
    }

    /// The mutation boundary.
    #[must_use]
    pub fn domain(&self) -> Arc<Domain<Arc<Db>>> {
        Arc::clone(&self.domain)
    }

    /// The database handle, for the read-side query types.
    #[must_use]
    pub fn db(&self) -> Arc<Db> {
        Arc::clone(&self.db)
    }

    /// Where a notification this transport originates is published.
    #[must_use]
    pub fn events(&self) -> Arc<dyn EventSink> {
        Arc::clone(&self.events)
    }

    /// The broadcast channel and connection registry the sockets use.
    #[must_use]
    pub fn realtime(&self) -> Arc<Realtime> {
        Arc::clone(&self.realtime)
    }

    /// How often an idle socket is pinged.
    #[must_use]
    pub fn keepalive(&self) -> Duration {
        self.keepalive
    }

    /// A receiver that fires when the Host stops the server.
    ///
    /// Every socket holds one. Without it, a graceful shutdown would wait for
    /// connections that have no reason to end, and "stopped" would never mean
    /// the port was free.
    #[must_use]
    pub fn shutdown_signal(&self) -> watch::Receiver<bool> {
        self.shutdown.subscribe()
    }

    /// Tell every socket to close.
    ///
    /// `send_replace` rather than `send`, because a server with no sockets open
    /// has no receivers, and that is not a failure to stop.
    pub fn begin_shutdown(&self) {
        let _ = self.shutdown.send_replace(true);
    }

    /// Run blocking database work off the async runtime.
    ///
    /// A panic inside the closure - a poisoned mutex, say - becomes a 500 rather
    /// than taking down the worker: the participant is told the Host's computer
    /// had a problem, and the diagnostic goes to the Host's console.
    pub async fn blocking<T, F>(&self, work: F) -> Result<T, ApiError>
    where
        F: FnOnce(&Db) -> Result<T, ApiError> + Send + 'static,
        T: Send + 'static,
    {
        let db = Arc::clone(&self.db);
        tokio::task::spawn_blocking(move || work(&db))
            .await
            .map_err(|error| {
                eprintln!("[lan] a database task failed to complete: {error}");
                ApiError::internal()
            })?
    }

    /// Run blocking work that needs the mutation boundary as well.
    pub async fn blocking_domain<T, F>(&self, work: F) -> Result<T, ApiError>
    where
        F: FnOnce(&Domain<Arc<Db>>) -> Result<T, ApiError> + Send + 'static,
        T: Send + 'static,
    {
        let domain = Arc::clone(&self.domain);
        tokio::task::spawn_blocking(move || work(&domain))
            .await
            .map_err(|error| {
                eprintln!("[lan] a domain task failed to complete: {error}");
                ApiError::internal()
            })?
    }
}

/// Read-side query helpers, scoped to a borrowed database.
///
/// Free functions rather than methods on [`LanState`], so that the query types
/// are constructed inside the blocking closure where the borrow is valid.
#[must_use]
pub fn participant_queries(db: &Db) -> ParticipantQueries<'_> {
    ParticipantQueries::new(db)
}

/// Credential resolution, scoped to a borrowed database.
#[must_use]
pub fn session_store(db: &Db) -> SessionStore<'_> {
    SessionStore::new(db)
}

/// Connection-activity recording, scoped to a borrowed database.
#[must_use]
pub fn presence_store(db: &Db) -> PresenceStore<'_> {
    PresenceStore::new(db)
}
