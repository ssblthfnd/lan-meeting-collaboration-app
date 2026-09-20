//! Who is performing a mutation.
//!
//! An [`Actor`] is minted at the transport boundary and never parsed from a
//! request body. `app-server` may only mint [`Actor::Participant`] after
//! resolving a stored session token hash; `src-tauri` mints [`Actor::Host`].
//!
//! See the architecture rules, sections 14 and 14.1.

/// Identifier type placeholder. The concrete representation is decided in
/// Phase 1, step 1 together with the database schema.
pub type Id = String;

/// The authenticated origin of a mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Actor {
    /// The Host operating the desktop application.
    Host,
    /// A LAN participant, resolved from a validated session token.
    Participant {
        meeting_id: Id,
        participant_id: Id,
        session_id: Id,
    },
    /// A remote submission import, confirmed by the Host.
    RemoteImport { meeting_id: Id, participant_id: Id },
}

impl Actor {
    /// Audit `actor_type` discriminator.
    ///
    /// Mirrors the values required by the architecture rules (sections 17, 18).
    pub fn actor_type(&self) -> &'static str {
        match self {
            Actor::Host => "HOST",
            Actor::Participant { .. } => "PARTICIPANT",
            Actor::RemoteImport { .. } => "REMOTE_IMPORT",
        }
    }
}
