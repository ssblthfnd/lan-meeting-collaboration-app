//! The LAN server's lifecycle, owned by the Host.
//!
//! # Started by a person, not by a state change
//!
//! Opening a meeting does **not** start the server (ADR-0016). Binding a
//! LAN-reachable socket is a visible, consequential act - on Windows it raises
//! the firewall prompt - and it should happen because the Host chose it, not as
//! a side effect of a lifecycle transition.
//!
//! "Tied to the meeting lifecycle" is honoured at the *request* level instead,
//! which is the stronger guarantee: every LAN request re-reads the meeting's
//! status from SQLite, so a meeting that locks while the server runs stops
//! accepting participants immediately, with no restart and no cached state
//! anywhere (architecture rules section 15).
//!
//! # One server, many meetings
//!
//! One process, one port. Which meetings are joinable is decided per request by
//! the join token and the meeting's status, so there is no per-meeting server to
//! start, stop or leak.
//!
//! # Stopping means stopped
//!
//! [`LanLifecycle::stop`] waits for the socket to close before reporting. A
//! "stopped" indicator that did not mean the port was free would make the next
//! start fail for a reason the Host could not see.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use app_core::service::Domain;
use app_db::Db;
use app_server::{RunningServer, ServerError, DEFAULT_PORT};
use serde::Serialize;

/// What the Host UI shows about the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LanServerStatus {
    pub running: bool,
    /// The port actually bound, which is not always the one requested.
    pub port: Option<u16>,
    /// Whether the participant bundle was compiled into this binary.
    ///
    /// Surfaced so a developer running a Rust-only build is told why the join
    /// page is blank, rather than debugging the network.
    pub ui_bundled: bool,
}

impl LanServerStatus {
    fn stopped() -> Self {
        Self {
            running: false,
            port: None,
            ui_bundled: app_server::assets::is_bundled(),
        }
    }
}

/// Where the server is in its lifecycle.
///
/// Three states rather than an `Option`, because binding a socket takes time
/// and "a start is in flight" is a real state that a second caller has to be
/// able to see. Without it, two starts can both find nothing running and both
/// bind (ADR-0016 leaves the server Host-controlled, but says nothing about a
/// Host who double-clicks).
enum Lifecycle {
    Stopped,
    /// A caller has reserved the right to start and is binding. Not serving
    /// yet, so [`LanLifecycle::status`] reports it as stopped.
    Starting,
    Running(RunningServer),
}

/// Owns the running server, if there is one.
///
/// One mutex over the whole lifecycle, so the check and the store are a single
/// critical section. The lock is never held across an `await`: a caller marks
/// the lifecycle `Starting`, releases it, binds, and then takes it again to
/// record the outcome.
pub struct LanLifecycle {
    db: Arc<Db>,
    domain: Arc<Domain<Arc<Db>>>,
    state: Mutex<Lifecycle>,
}

impl LanLifecycle {
    /// Share the Host's database and its existing mutation boundary.
    ///
    /// One `Domain` for both transports, as ADR-0012 requires: a second one
    /// would be a second place for a rule to be enforced differently.
    #[must_use]
    pub fn new(db: Arc<Db>, domain: Arc<Domain<Arc<Db>>>) -> Self {
        Self {
            db,
            domain,
            state: Mutex::new(Lifecycle::Stopped),
        }
    }

    /// The lifecycle, recovering a poisoned lock rather than discarding it.
    ///
    /// Poisoning means a panic happened while the lock was held. Nothing in the
    /// critical sections below can panic - they assign and read one enum - so
    /// this should be unreachable, and if it is reached the protected value is
    /// still a whole, valid `Lifecycle`: every write is a single assignment, so
    /// it cannot be torn.
    ///
    /// Recovering is therefore the honest choice. The alternative, treating a
    /// poisoned lock as "stopped", would report that nothing is listening while
    /// a socket was still bound - which is the one thing the Host must never be
    /// told.
    fn state(&self) -> MutexGuard<'_, Lifecycle> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Current state.
    #[must_use]
    pub fn status(&self) -> LanServerStatus {
        match &*self.state() {
            Lifecycle::Running(server) => running(server),
            // A start in flight is not yet serving, so it is not running.
            // Saying otherwise would give the Host a port nobody answers on.
            Lifecycle::Starting | Lifecycle::Stopped => LanServerStatus::stopped(),
        }
    }

    /// Start serving, or return the status unchanged if it already is.
    ///
    /// Starting an already-running server is not an error: the Host pressed a
    /// button twice, and the honest answer is the current state.
    ///
    /// `running: true` is returned only once the handle is stored, so the Host
    /// is never shown a running server whose handle was dropped.
    pub async fn start(&self, port: Option<u16>) -> Result<LanServerStatus, ServerError> {
        // Reserve the right to start. One caller wins; the others return
        // without binding a second socket.
        {
            let mut state = self.state();
            match &*state {
                Lifecycle::Running(server) => return Ok(running(server)),
                // Another caller is already binding. Report what is true now
                // rather than predicting their success.
                Lifecycle::Starting => return Ok(LanServerStatus::stopped()),
                Lifecycle::Stopped => *state = Lifecycle::Starting,
            }
        }

        let outcome = app_server::start(
            Arc::clone(&self.db),
            Arc::clone(&self.domain),
            port.unwrap_or(DEFAULT_PORT),
        )
        .await;

        let server = match outcome {
            Ok(server) => server,
            Err(error) => {
                // Release the reservation so a corrected port can be tried.
                // The bind error reaches the Host unchanged.
                *self.state() = Lifecycle::Stopped;
                return Err(error);
            }
        };

        // Take the lock again to record the outcome. `stop` may have run while
        // this was binding, in which case the reservation is gone and this
        // server must not resurrect it.
        let orphaned = {
            let mut state = self.state();
            if matches!(*state, Lifecycle::Starting) {
                let status = running(&server);
                *state = Lifecycle::Running(server);
                return Ok(status);
            }
            server
        };

        // Stopped while binding. Honour that: shut down what was just bound,
        // outside the lock, so the port does not stay held.
        orphaned.stop().await;
        Ok(LanServerStatus::stopped())
    }

    /// Stop serving and wait for the socket to close.
    ///
    /// Also clears a reservation left behind by a start that never completed,
    /// so the Host can recover through the UI rather than by restarting.
    pub async fn stop(&self) -> LanServerStatus {
        let running = {
            let mut state = self.state();
            match core::mem::replace(&mut *state, Lifecycle::Stopped) {
                Lifecycle::Running(server) => Some(server),
                Lifecycle::Starting | Lifecycle::Stopped => None,
            }
        };

        if let Some(server) = running {
            server.stop().await;
        }

        LanServerStatus::stopped()
    }
}

/// The status of a server that is actually listening.
fn running(server: &RunningServer) -> LanServerStatus {
    LanServerStatus {
        running: true,
        port: Some(server.address().port()),
        ui_bundled: app_server::assets::is_bundled(),
    }
}
