//! Host application state, and the implementation behind every command.
//!
//! # An adapter, not a layer of rules
//!
//! Each method here does the same four things and nothing else:
//!
//! ```text
//! parse the request  ->  mint Actor::Host  ->  call Domain or HostQueries
//!                                          ->  map the result
//! ```
//!
//! There is no SQL here, no authorization decision, no status check, no
//! participant limit, no audit record. Those are `app-core`'s, and they are
//! reachable only through [`Domain`], whose write methods require an
//! authorization proof this crate cannot construct (ADR-0012). Bypassing the
//! domain is not a thing to remember not to do; it does not compile.
//!
//! # Why the Host actor is minted here
//!
//! [`Actor::Host`] is established at the transport boundary, never read from a
//! request (architecture rules section 14.1). For this transport that is
//! trivially correct: the Tauri IPC channel *is* the Host's own window, so
//! anything arriving on it is the Host by construction. A command therefore has
//! no actor parameter - there is nothing a caller could pass that would change
//! who it is. The LAN transport has the hard version of this problem and solves
//! it with session tokens; it is deliberately not this file's concern.
//!
//! # Reads and writes take different paths
//!
//! Mutations go through [`Domain`]. Reads go through [`HostQueries`] on the
//! read-only connection pool (ADR-0014). Both are held by [`HostState`], which
//! is the single Tauri-managed value.
//!
//! # What is deliberately not here
//!
//! The domain also offers `lock_meeting` and `write_note`, and neither is
//! exposed. Locking is irreversible and belongs with the flow that precedes it;
//! notes need the shared editor and the Markdown subset (ADR-0007). Listing the
//! omissions is the point - adding either command should be a deliberate act
//! rather than a gap someone fills in passing.

use std::path::Path;
use std::sync::Arc;

use app_core::actor::Actor;
use app_core::id::MeetingId;
use app_core::service::Domain;
use app_db::query::HostQueries;
use app_db::Db;

use crate::dto::{
    AuditEntryDto, MeetingConfigurationInput, MeetingCreatedDto, MeetingDetailDto,
    MeetingSummaryDto, MeetingTransitionedDto, MeetingUpdatedDto, ParticipantAddedDto,
    ParticipantDetailsInput, ParticipantRemovedDto, ParticipantSummaryDto, ParticipantUpdatedDto,
};
use crate::error::{HostError, HostErrorKind, HostResult};

/// File name of the Host's database inside the application data directory.
///
/// One file, local to this device, never exposed to the network (PRD 22.7, 22.8).
pub const DATABASE_FILE: &str = "meetings.sqlite3";

/// Everything a Host command needs.
pub struct HostState {
    domain: Domain<Arc<Db>>,
    db: Arc<Db>,
}

impl HostState {
    /// Wrap an open database.
    #[must_use]
    pub fn new(db: Arc<Db>) -> Self {
        Self {
            domain: Domain::new(Arc::clone(&db)),
            db,
        }
    }

    /// Open (creating and migrating if needed) the database at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, app_db::DbError> {
        Ok(Self::new(Arc::new(Db::open(path)?)))
    }

    /// The mutation boundary. The only way anything here changes state.
    #[must_use]
    pub fn domain(&self) -> &Domain<Arc<Db>> {
        &self.domain
    }

    fn queries(&self) -> HostQueries<'_> {
        HostQueries::new(&self.db)
    }

    /* ------------------------------------------------------------------
     * Meetings
     * ------------------------------------------------------------------ */

    /// Create a meeting. It always starts in `DRAFT` (ADR-0013).
    pub fn create_meeting(
        &self,
        configuration: MeetingConfigurationInput,
    ) -> HostResult<MeetingCreatedDto> {
        let configuration = configuration.parse()?;
        let outcome = self.domain.create_meeting(&Actor::Host, configuration)?;
        Ok(outcome.into())
    }

    /// Replace a `DRAFT` meeting's configuration.
    ///
    /// Whether the meeting is still `DRAFT` is re-read inside the domain's own
    /// transaction. Nothing is checked here, so a stale UI cannot talk this
    /// command into a write (architecture rules section 15).
    pub fn update_meeting(
        &self,
        meeting_id: &str,
        configuration: MeetingConfigurationInput,
    ) -> HostResult<MeetingUpdatedDto> {
        let meeting_id = crate::dto::parse_meeting_id(meeting_id)?;
        let configuration = configuration.parse()?;
        let outcome = self
            .domain
            .update_meeting(&Actor::Host, meeting_id, configuration)?;
        Ok(outcome.into())
    }

    /// Move a meeting from `DRAFT` to `OPEN`.
    pub fn open_meeting(&self, meeting_id: &str) -> HostResult<MeetingTransitionedDto> {
        let meeting_id = crate::dto::parse_meeting_id(meeting_id)?;
        let outcome = self.domain.open_meeting(&Actor::Host, meeting_id)?;
        Ok(outcome.into())
    }

    /// Every meeting, for the Host's list.
    pub fn list_meetings(&self) -> HostResult<Vec<MeetingSummaryDto>> {
        let rows = self.queries().meetings().map_err(query_failed)?;
        Ok(rows.into_iter().map(MeetingSummaryDto::from).collect())
    }

    /// One meeting in full.
    pub fn get_meeting(&self, meeting_id: &str) -> HostResult<MeetingDetailDto> {
        let meeting_id = crate::dto::parse_meeting_id(meeting_id)?;
        self.queries()
            .meeting(meeting_id)
            .map_err(query_failed)?
            .map(MeetingDetailDto::from)
            .ok_or_else(|| not_found(meeting_id))
    }

    /* ------------------------------------------------------------------
     * Participants
     * ------------------------------------------------------------------ */

    /// Add a participant to a `DRAFT` meeting's roster.
    ///
    /// The 99-participant limit is the domain's, counted inside the mutating
    /// transaction. Nothing here counts anything.
    pub fn add_participant(
        &self,
        meeting_id: &str,
        details: ParticipantDetailsInput,
    ) -> HostResult<ParticipantAddedDto> {
        let meeting_id = crate::dto::parse_meeting_id(meeting_id)?;
        let outcome =
            self.domain
                .add_participant(&Actor::Host, meeting_id, details.into_details())?;
        Ok(outcome.into())
    }

    /// Replace a participant's details.
    pub fn update_participant(
        &self,
        meeting_id: &str,
        participant_id: &str,
        details: ParticipantDetailsInput,
    ) -> HostResult<ParticipantUpdatedDto> {
        let meeting_id = crate::dto::parse_meeting_id(meeting_id)?;
        let participant_id = crate::dto::parse_participant_id(participant_id)?;
        let outcome = self.domain.update_participant(
            &Actor::Host,
            meeting_id,
            participant_id,
            details.into_details(),
        )?;
        Ok(outcome.into())
    }

    /// Remove a participant from a `DRAFT` meeting's roster.
    pub fn remove_participant(
        &self,
        meeting_id: &str,
        participant_id: &str,
    ) -> HostResult<ParticipantRemovedDto> {
        let meeting_id = crate::dto::parse_meeting_id(meeting_id)?;
        let participant_id = crate::dto::parse_participant_id(participant_id)?;
        let outcome = self
            .domain
            .remove_participant(&Actor::Host, meeting_id, participant_id)?;
        Ok(outcome.into())
    }

    /// A meeting's roster, ordered by name.
    pub fn list_participants(&self, meeting_id: &str) -> HostResult<Vec<ParticipantSummaryDto>> {
        let meeting_id = crate::dto::parse_meeting_id(meeting_id)?;
        let rows = self
            .queries()
            .participants(meeting_id)
            .map_err(query_failed)?;
        Ok(rows.into_iter().map(ParticipantSummaryDto::from).collect())
    }

    /* ------------------------------------------------------------------
     * Audit
     * ------------------------------------------------------------------ */

    /// A meeting's audit trail, oldest first.
    ///
    /// Read-only. There is deliberately no command to add, change or remove an
    /// entry: the log is append-only, written only as part of a domain mutation,
    /// and the database refuses `UPDATE` and `DELETE` (ADR-0011).
    pub fn list_audit_entries(&self, meeting_id: &str) -> HostResult<Vec<AuditEntryDto>> {
        let meeting_id = crate::dto::parse_meeting_id(meeting_id)?;
        let rows = self
            .queries()
            .audit_entries(meeting_id)
            .map_err(query_failed)?;
        Ok(rows.into_iter().map(AuditEntryDto::from).collect())
    }
}

/// A read failed for a reason the Host cannot act on.
///
/// The SQLite text is kept off the UI and written to the Host's own console, the
/// same way the domain's persistence failures are handled.
fn query_failed(error: app_db::DbError) -> HostError {
    eprintln!("[host] a read query failed: {error}");
    HostError::new(
        HostErrorKind::Persistence,
        "The database could not be read.".to_owned(),
    )
}

/// The requested meeting does not exist.
fn not_found(meeting_id: MeetingId) -> HostError {
    HostError::new(
        HostErrorKind::MeetingNotFound { meeting_id },
        format!("meeting not found: {meeting_id}"),
    )
}
