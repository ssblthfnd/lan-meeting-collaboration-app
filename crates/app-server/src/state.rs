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

use std::sync::Arc;

use app_core::service::Domain;
use app_db::participant_query::ParticipantQueries;
use app_db::session_store::SessionStore;
use app_db::Db;

use crate::error::ApiError;

/// Everything a LAN handler needs.
///
/// Cloneable, because axum hands one to every request; the clone is two `Arc`
/// bumps and shares the single database underneath.
#[derive(Clone)]
pub struct LanState {
    db: Arc<Db>,
    domain: Arc<Domain<Arc<Db>>>,
}

impl LanState {
    /// Wrap an open database.
    #[must_use]
    pub fn new(db: Arc<Db>) -> Self {
        Self {
            domain: Arc::new(Domain::new(Arc::clone(&db))),
            db,
        }
    }

    /// Share the Host's existing mutation boundary rather than building a
    /// second one.
    ///
    /// The Tauri process already holds a `Domain` over the same database. One
    /// boundary for both transports is the point of ADR-0012: two would be two
    /// places for a rule to be enforced differently.
    #[must_use]
    pub fn with_domain(db: Arc<Db>, domain: Arc<Domain<Arc<Db>>>) -> Self {
        Self { db, domain }
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
