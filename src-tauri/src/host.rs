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
//! The domain also offers `lock_meeting`, and it is not exposed: locking is
//! irreversible and belongs with the flow that precedes it. Listing the
//! omission is the point - adding the command should be a deliberate act
//! rather than a gap someone fills in passing.
//!
//! Restoring a note version is not here either, and not anywhere: version
//! history is view-only in step 8 (ADR-0019). There is no restore command, no
//! restore operation and no restore audit action to reach for.
//!
//! # Line endings are normalised here, and nothing else is
//!
//! [`HostState::write_participant_note`] converts CRLF to LF before the content
//! reaches the domain. That is a **transport** concern: a Windows WebView can
//! submit CRLF, the domain refuses a carriage return as a control character,
//! and a Host should not be told their note contains an invalid character they
//! cannot see.
//!
//! Nothing else about the content is touched. The note is stored exactly as
//! typed - no reformatting, no re-serialisation, no normalising of spacing or
//! list markers - because a note editor that quietly edits the note is worse
//! than a plain one (ADR-0019).
//!
//! # Events are not published here either
//!
//! A mutation announces itself from inside `Domain`, after the transaction
//! commits (architecture rules section 16). This layer supplies the sink at
//! construction and then has nothing to do with it - which is what stops a
//! command from being the one place an event is forgotten, or emitted twice, or
//! emitted for something that rolled back.

use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;

use app_core::actor::Actor;
use app_core::error::DomainError;
use app_core::event::{EventSink, NoEvents};
use app_core::id::{MeetingId, ParticipantId, SubmissionId};
use app_core::meeting::MeetingStatus;
use app_core::service::{Domain, WriteNote};
use app_core::time::UtcTimestamp;
use app_db::presence::PresenceStore;
use app_db::query::HostQueries;
use app_db::Db;
use app_remote::RemoteFormContext;

use crate::dto::{
    AuditEntryDto, JoinTokenIssuedDto, LanInterfaceDto, MeetingConfigurationInput,
    MeetingCreatedDto, MeetingDetailDto, MeetingSummaryDto, MeetingTransitionedDto,
    MeetingUpdatedDto, NoteDetailDto, NoteOverviewDto, NoteVersionDetailDto, NoteVersionSummaryDto,
    NoteWrittenDto, ParticipantAddedDto, ParticipantDetailsInput, ParticipantPresenceDto,
    ParticipantRemovedDto, ParticipantSummaryDto, ParticipantUpdatedDto, RemoteFormGeneratedDto,
    SessionChangedDto,
};
use crate::error::{HostError, HostErrorKind, HostResult};

/// File name of the Host's database inside the application data directory.
///
/// One file, local to this device, never exposed to the network (PRD 22.7, 22.8).
pub const DATABASE_FILE: &str = "meetings.sqlite3";

/// Folder inside the application data directory for generated remote forms.
///
/// Its own directory rather than beside the database: these are files the Host
/// is expected to open, attach to an email and eventually delete, and the
/// database is not (ADR-0021 decision 10).
pub const REMOTE_FORM_DIRECTORY: &str = "remote-forms";

/// Everything a Host command needs.
pub struct HostState {
    /// Behind an `Arc` so the LAN server shares *this* boundary rather than
    /// constructing its own. One `Domain` for both transports is the point of
    /// ADR-0012: two would be two places for a rule to be enforced differently.
    domain: Arc<Domain<Arc<Db>>>,
    db: Arc<Db>,
}

impl HostState {
    /// Wrap an open database, publishing nothing.
    ///
    /// For tests about what a command writes. The running application uses
    /// [`HostState::with_events`], so a mutation reaches the window and the LAN.
    #[must_use]
    pub fn new(db: Arc<Db>) -> Self {
        Self::with_events(db, Arc::new(NoEvents))
    }

    /// Wrap an open database and publish every committed mutation.
    #[must_use]
    pub fn with_events(db: Arc<Db>, events: Arc<dyn EventSink>) -> Self {
        Self {
            domain: Arc::new(Domain::with_events(Arc::clone(&db), events)),
            db,
        }
    }

    /// Open (creating and migrating if needed) the database at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, app_db::DbError> {
        Ok(Self::new(Arc::new(Db::open(path)?)))
    }

    /// Open the database at `path` and publish every committed mutation.
    pub fn open_with_events(
        path: impl AsRef<Path>,
        events: Arc<dyn EventSink>,
    ) -> Result<Self, app_db::DbError> {
        Ok(Self::with_events(Arc::new(Db::open(path)?), events))
    }

    /// The mutation boundary. The only way anything here changes state.
    #[must_use]
    pub fn domain(&self) -> &Domain<Arc<Db>> {
        &self.domain
    }

    /// The same boundary, shareable with the LAN server.
    #[must_use]
    pub fn shared_domain(&self) -> Arc<Domain<Arc<Db>>> {
        Arc::clone(&self.domain)
    }

    /// The shared database handle, for the LAN server.
    #[must_use]
    pub fn shared_db(&self) -> Arc<Db> {
        Arc::clone(&self.db)
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
     * LAN access
     * ------------------------------------------------------------------ */

    /// Mint a join token and build the URL and QR code for it.
    ///
    /// The token is generated here, hashed, and only the hash is handed to the
    /// domain - which accepts nothing else, so the plaintext cannot be persisted
    /// even by a mistake in this method (PRD 22.2).
    ///
    /// The plaintext is returned to the Host once, for the URL and the QR. It is
    /// not kept, and issuing again produces a different one and invalidates this.
    ///
    /// `address` is chosen by the Host from [`HostState::lan_interfaces`]: only
    /// they know which network the participants are on, and `127.0.0.1` must
    /// never be assumed reachable (architecture rules section 4).
    pub fn issue_join_token(
        &self,
        meeting_id: &str,
        address: &str,
        port: u16,
    ) -> HostResult<JoinTokenIssuedDto> {
        let meeting_id = crate::dto::parse_meeting_id(meeting_id)?;
        let address: IpAddr = address.parse().map_err(|_| {
            HostError::validation(
                "host address",
                "an IPv4 address from the list of local interfaces",
                address,
            )
        })?;

        let credential = app_server::mint().map_err(|error| {
            eprintln!("[host] {error}");
            HostError::new(
                HostErrorKind::Persistence,
                "A secure join token could not be generated.".to_owned(),
            )
        })?;

        let outcome = self
            .domain
            .issue_join_token(&Actor::Host, meeting_id, &credential.hash)?;

        let join_url = app_server::join_url(address, port, &credential.token);
        let qr = crate::qr::encode(&join_url).map_err(|error| {
            eprintln!("[host] {error}");
            HostError::new(
                HostErrorKind::Persistence,
                "The join code could not be drawn.".to_owned(),
            )
        })?;

        Ok(JoinTokenIssuedDto {
            meeting_id: outcome.meeting_id,
            join_url,
            qr,
            replaced_previous: outcome.replaced_previous,
            at: outcome.at,
        })
    }

    /// Every local address the Host could advertise, most useful first.
    #[must_use]
    pub fn lan_interfaces(&self) -> Vec<LanInterfaceDto> {
        app_server::interfaces()
            .into_iter()
            .map(LanInterfaceDto::from)
            .collect()
    }

    /// Acknowledge a participant's claim.
    ///
    /// Records that the Host saw it. It grants nothing - the participant could
    /// already act, and still can (ADR-0016).
    pub fn approve_participant_claim(
        &self,
        meeting_id: &str,
        participant_id: &str,
    ) -> HostResult<SessionChangedDto> {
        let meeting_id = crate::dto::parse_meeting_id(meeting_id)?;
        let participant_id = crate::dto::parse_participant_id(participant_id)?;
        let outcome = self
            .domain
            .approve_claim(&Actor::Host, meeting_id, participant_id)?;
        Ok(outcome.into())
    }

    /// End a participant's session, freeing the identity to be claimed again.
    ///
    /// What makes first-claim-wins operable: an identity taken by the wrong
    /// person, or stranded on a closed laptop, can be released
    /// (ADR-0002 rule 5).
    pub fn revoke_participant_session(
        &self,
        meeting_id: &str,
        participant_id: &str,
    ) -> HostResult<SessionChangedDto> {
        let meeting_id = crate::dto::parse_meeting_id(meeting_id)?;
        let participant_id = crate::dto::parse_participant_id(participant_id)?;
        let outcome = self
            .domain
            .revoke_session(&Actor::Host, meeting_id, participant_id)?;
        Ok(outcome.into())
    }

    /* ------------------------------------------------------------------
     * Notes
     *
     * Writes go through `Domain::write_note`, which validates the content,
     * re-reads the meeting lock inside its own transaction, appends an
     * immutable `note_versions` row and writes the audit record - all before
     * publishing `note.changed`. Nothing about that is repeated here.
     *
     * Reads go through `HostQueries`, on the read-only pool (ADR-0014).
     * ------------------------------------------------------------------ */

    /// One participant's note, or `None` if they have not written one.
    pub fn get_participant_note(
        &self,
        meeting_id: &str,
        participant_id: &str,
    ) -> HostResult<Option<NoteDetailDto>> {
        let meeting_id = crate::dto::parse_meeting_id(meeting_id)?;
        let participant_id = crate::dto::parse_participant_id(participant_id)?;
        Ok(self
            .queries()
            .note(meeting_id, participant_id)
            .map_err(query_failed)?
            .map(NoteDetailDto::from))
    }

    /// Create or replace a participant's note, as the Host.
    ///
    /// The Host may write any participant's note (PRD section 15). The version
    /// number, the history row and the audit record are the domain's, derived
    /// inside the transaction; this method parses, normalises line endings, and
    /// reports what the domain decided.
    ///
    /// Whether the meeting still permits the write is re-read inside that
    /// transaction, so a stale window cannot talk this command into a write
    /// (architecture rules section 15).
    pub fn write_participant_note(
        &self,
        meeting_id: &str,
        participant_id: &str,
        content: &str,
    ) -> HostResult<NoteWrittenDto> {
        let meeting_id = crate::dto::parse_meeting_id(meeting_id)?;
        let participant_id = crate::dto::parse_participant_id(participant_id)?;

        let outcome = self.domain.write_note(
            &Actor::Host,
            WriteNote {
                meeting_id,
                participant_id,
                content: normalize_line_endings(content),
            },
        )?;
        Ok(outcome.into())
    }

    /// A note's history, newest first, without bodies.
    pub fn list_note_versions(
        &self,
        meeting_id: &str,
        participant_id: &str,
    ) -> HostResult<Vec<NoteVersionSummaryDto>> {
        let meeting_id = crate::dto::parse_meeting_id(meeting_id)?;
        let participant_id = crate::dto::parse_participant_id(participant_id)?;
        let rows = self
            .queries()
            .note_versions(meeting_id, participant_id)
            .map_err(query_failed)?;
        Ok(rows.into_iter().map(NoteVersionSummaryDto::from).collect())
    }

    /// One historical version, with its body.
    ///
    /// Read-only in every sense: there is no command that writes a version, and
    /// the database refuses `UPDATE` on `note_versions` regardless (ADR-0011).
    pub fn get_note_version(
        &self,
        meeting_id: &str,
        participant_id: &str,
        version: i64,
    ) -> HostResult<Option<NoteVersionDetailDto>> {
        let meeting_id = crate::dto::parse_meeting_id(meeting_id)?;
        let participant_id = crate::dto::parse_participant_id(participant_id)?;
        Ok(self
            .queries()
            .note_version(meeting_id, participant_id, version)
            .map_err(query_failed)?
            .map(NoteVersionDetailDto::from))
    }

    /// Every participant, and whether they have written a note.
    pub fn list_notes_overview(&self, meeting_id: &str) -> HostResult<Vec<NoteOverviewDto>> {
        let meeting_id = crate::dto::parse_meeting_id(meeting_id)?;
        let rows = self
            .queries()
            .notes_overview(meeting_id)
            .map_err(query_failed)?;
        Ok(rows.into_iter().map(NoteOverviewDto::from).collect())
    }

    /// Who is connected, and when each identity was last seen.
    ///
    /// Two halves from two places, and the split is the point. `last_seen_at`
    /// comes from SQLite; `connected` comes from `connected`, which reads the
    /// LAN server's in-memory registry. Persisting the second would mean a Host
    /// machine that lost power left a database claiming everybody was still
    /// here (ADR-0018).
    ///
    /// When the LAN server is not running, nobody is connected - which is true,
    /// not a fallback: there is no socket to be connected to.
    pub fn list_participant_presence(
        &self,
        meeting_id: &str,
        connected: impl FnOnce(MeetingId) -> Vec<ParticipantId>,
    ) -> HostResult<Vec<ParticipantPresenceDto>> {
        let meeting_id = crate::dto::parse_meeting_id(meeting_id)?;
        let rows = PresenceStore::new(&self.db)
            .roster_presence(meeting_id)
            .map_err(query_failed)?;
        let live = connected(meeting_id);

        Ok(rows
            .into_iter()
            .map(|row| ParticipantPresenceDto {
                connected: live.contains(&row.participant_id),
                participant_id: row.participant_id,
                last_seen_at: row.last_seen_at,
            })
            .collect())
    }

    /* ------------------------------------------------------------------
     * Remote participation
     * ------------------------------------------------------------------ */

    /// Generate a standalone offline remote form for one participant.
    ///
    /// The Host's half of PRD section 10: a file to send to somebody who is not
    /// on the network, which they fill in offline and send back.
    ///
    /// ```text
    /// read meeting + participant + note   (HostQueries, read-only)
    ///   -> mint submission_id             (UUIDv7, here and only here)
    ///     -> app_remote::generate          template + escaped JSON island
    ///       -> verify the artefact         refuse rather than write a bad one
    ///         -> std::fs::write            under `directory`, chosen by the shell
    /// ```
    ///
    /// # Nothing is written to the database
    ///
    /// Generating a form mutates nothing: no note, no version, no audit entry
    /// and no `remote_submissions` row. The ledger row is written when a
    /// submission is **imported**, atomically with the note it produces, which
    /// is step 10. Generation is a read and a file.
    ///
    /// That is also why the lifecycle check below sits here rather than in the
    /// domain. It is not enforcement - there is nothing to enforce, because
    /// nothing is persisted - it is a refusal to hand someone a form that could
    /// never be imported. The rule that counts is re-read inside the import
    /// transaction (architecture rules section 15), and it is the same rule:
    /// `LOCKED` refuses, `DRAFT` and `OPEN` do not (ADR-0021 decision 2).
    ///
    /// # The path is the shell's, not the window's
    ///
    /// `directory` comes from Tauri's application-data directory, resolved by
    /// the command layer. The renderer names a meeting and a participant and
    /// nothing else: a window that could choose a path would be a window that
    /// could write anywhere. Only the file name is derived from data, and it is
    /// slugified to `[A-Za-z0-9-]` before it reaches the filesystem.
    ///
    /// # No credential, and no actor
    ///
    /// The generated file carries no token, hash or URL: `RemoteFormContext` has
    /// nowhere to put one, and `app-remote` verifies the bytes anyway. No
    /// `Actor::RemoteImport` is constructed here or anywhere else yet - an
    /// actor is authority, and generation asks for none (ADR-0021 decisions 3
    /// and 14).
    pub fn generate_remote_form(
        &self,
        meeting_id: &str,
        participant_id: &str,
        directory: &Path,
    ) -> HostResult<RemoteFormGeneratedDto> {
        let meeting_id = crate::dto::parse_meeting_id(meeting_id)?;
        let participant_id = crate::dto::parse_participant_id(participant_id)?;

        let queries = self.queries();

        let meeting = queries
            .meeting(meeting_id)
            .map_err(query_failed)?
            .ok_or_else(|| not_found(meeting_id))?;

        if meeting.status == MeetingStatus::Locked {
            // A form generated now could never be imported, so the honest
            // answer is to refuse rather than to produce a file that will be
            // rejected after somebody has filled it in.
            return Err(HostError::from(DomainError::MeetingLocked {
                meeting_id,
                title: meeting.title.clone(),
            }));
        }

        // Scoped to the meeting, so a participant of another one is absent
        // rather than borrowed.
        let participant = queries
            .participants(meeting_id)
            .map_err(query_failed)?
            .into_iter()
            .find(|row| row.id == participant_id)
            .ok_or_else(|| {
                HostError::from(DomainError::ParticipantNotFound {
                    meeting_id,
                    participant_id,
                })
            })?;

        // Absent is a real answer: a participant who has written nothing gets an
        // empty editor and source version 0 (ADR-0021 decision 4).
        let note = queries
            .note(meeting_id, participant_id)
            .map_err(query_failed)?;

        let submission_id = SubmissionId::new();
        let generated_at = UtcTimestamp::now();

        let context = RemoteFormContext {
            schema_version: app_remote::SCHEMA_VERSION,
            submission_id,
            meeting_id,
            meeting_title: meeting.title.clone(),
            meeting_date: meeting.date.to_storage(),
            meeting_timezone: meeting.timezone.clone(),
            participant_id,
            participant_name: participant.details.name.clone(),
            source_version: note.as_ref().map_or(0, |row| row.version),
            existing_content: note.map(|row| row.content).unwrap_or_default(),
            generated_at: generated_at.to_storage(),
        };

        // Generation verifies its own output, so a file that is not
        // self-contained is never written to disk.
        let html = app_remote::generate(&context)?;

        let file_name =
            remote_form_file_name(&meeting.title, &participant.details.name, submission_id);
        let path = directory.join(&file_name);

        std::fs::create_dir_all(directory).map_err(|error| write_failed(directory, &error))?;
        std::fs::write(&path, html.as_bytes()).map_err(|error| write_failed(&path, &error))?;

        Ok(RemoteFormGeneratedDto {
            meeting_id,
            participant_id,
            submission_id,
            path: path.display().to_string(),
            file_name,
            bytes: html.len() as u64,
            source_version: context.source_version,
            generated_at,
        })
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

/// Convert CRLF and lone CR to LF.
///
/// A transport concern, not a content one. A WebView on Windows can submit
/// CRLF, the domain refuses a carriage return as a control character, and a
/// Host should not be shown a validation error about a character they cannot
/// see and did not type.
///
/// Doing it here also keeps stored content uniform, which matters for export
/// determinism later: a note written on Windows and one written elsewhere must
/// not differ only in invisible bytes (architecture rules section 19).
fn normalize_line_endings(content: &str) -> String {
    content.replace("\r\n", "\n").replace('\r', "\n")
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

/// A file the Host's own machine would not write.
///
/// A full disk, a directory the installation cannot create, a file held open by
/// something else. The path is named because it is the Host's own machine and
/// the useful next action is to look at it; the operating system's message goes
/// to the console rather than into the window.
fn write_failed(path: &Path, error: &std::io::Error) -> HostError {
    eprintln!("[host] writing {}: {error}", path.display());
    HostError::new(
        HostErrorKind::Persistence,
        format!(
            "The remote form could not be written to {}.              Check that the folder exists and there is room on the disk.",
            path.display()
        ),
    )
}

/// A file name for a generated form.
///
/// Convenience only: the application never reads identity from a filename
/// (architecture rules section 21), and the Host reads every identifier from
/// inside the file. Renaming it changes nothing.
///
/// Slugified to `[A-Za-z0-9-]` before it reaches the filesystem, so a meeting
/// titled `../../etc/passwd` cannot become a path. The tail of the submission
/// id is appended because generating a second form must not overwrite the
/// first: older forms stay valid (ADR-0021 decision 13). The *tail* rather than
/// the head, because a UUIDv7 begins with a timestamp and two forms made in the
/// same minute would share it.
fn remote_form_file_name(
    meeting_title: &str,
    participant_name: &str,
    submission_id: SubmissionId,
) -> String {
    fn slug(text: &str) -> String {
        let mut out = String::new();
        let mut pending_dash = false;

        for character in text.chars() {
            if character.is_ascii_alphanumeric() {
                if pending_dash && !out.is_empty() {
                    out.push('-');
                }
                pending_dash = false;
                out.push(character);
            } else {
                pending_dash = true;
            }
            if out.len() >= 40 {
                break;
            }
        }

        out
    }

    let meeting = slug(meeting_title);
    let meeting = if meeting.is_empty() {
        "meeting"
    } else {
        &meeting
    };

    let who = slug(participant_name);
    let who = if who.is_empty() { "participant" } else { &who };

    let id = submission_id.to_storage();
    let tail = &id[id.len() - 12..];

    format!("{meeting}-{who}-{tail}.html")
}

/// The requested meeting does not exist.
fn not_found(meeting_id: MeetingId) -> HostError {
    HostError::new(
        HostErrorKind::MeetingNotFound { meeting_id },
        format!("meeting not found: {meeting_id}"),
    )
}
