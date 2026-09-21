//! Shapes crossing the Tauri IPC boundary, and the parsing that guards it.
//!
//! The counterpart of `packages/contracts/src/host.ts`. Field names are
//! `snake_case` on both sides and identifiers, timestamps, dates and times use
//! the canonical storage forms from ADR-0010 and ADR-0011, so one representation
//! travels from SQLite to the UI with no translation layer to get wrong.
//!
//! # Inputs are strings, outputs are typed
//!
//! Requests arrive with `String` fields and are parsed here. That is this
//! layer's job - establishing types at the transport boundary - and it is
//! deliberately *not* validation: whether a title is acceptable, whether a
//! schedule is coherent and whether the roster has room are decisions `app-core`
//! makes (ADR-0012, ADR-0013). Parsing answers only "is this the right shape",
//! and a failure is reported in exactly the same form as a domain refusal so the
//! UI needs one error path.
//!
//! Responses carry the domain's own types where they serialize to the canonical
//! form ([`MeetingId`], [`UtcTimestamp`], [`MeetingStatus`]) and `String` for the
//! zoneless date and time values, which have no serde impl by design - they are
//! meaningless without the timezone beside them.

use app_core::id::{Entity, Id, MeetingId, ParticipantId};
use app_core::meeting::{MeetingConfiguration, MeetingStatus};
use app_core::participant::ParticipantDetails;
use app_core::service::{
    MeetingCreated, MeetingTransitioned, MeetingUpdated, ParticipantAdded, ParticipantRemoved,
    ParticipantUpdated,
};
use app_core::time::{MeetingDate, MeetingTime, MeetingTimeZone, UtcTimestamp};
use app_db::query::{AuditEntryView, MeetingDetail, MeetingSummary, ParticipantSummary};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{HostError, HostResult};

/* -------------------------------------------------------------------------
 * Requests
 * ------------------------------------------------------------------------- */

/// What the Host submits when creating or reconfiguring a meeting (PRD 7).
#[derive(Debug, Clone, Deserialize)]
pub struct MeetingConfigurationInput {
    pub title: String,
    pub topic: Option<String>,
    pub date: String,
    pub start_time: String,
    pub end_time: String,
    pub timezone: String,
    pub location: Option<String>,
    pub description: Option<String>,
}

impl MeetingConfigurationInput {
    /// Parse into the domain type.
    ///
    /// Only shapes are checked. The title is passed through untrimmed and
    /// unjudged: `MeetingConfiguration::validated` decides whether it is
    /// acceptable, and it runs inside the mutating transaction.
    pub fn parse(self) -> HostResult<MeetingConfiguration> {
        Ok(MeetingConfiguration {
            title: self.title,
            topic: self.topic,
            date: parse_date(&self.date)?,
            start_time: parse_time("meeting start time", &self.start_time)?,
            end_time: parse_time("meeting end time", &self.end_time)?,
            timezone: parse_timezone(&self.timezone)?,
            location: self.location,
            description: self.description,
        })
    }
}

/// What the Host submits for a participant (PRD 7).
///
/// Every field is passed to the domain as given; `ParticipantDetails::validated`
/// requires the name and normalises the rest.
#[derive(Debug, Clone, Deserialize)]
pub struct ParticipantDetailsInput {
    pub name: String,
    pub department: Option<String>,
    pub position: Option<String>,
    pub meeting_role: Option<String>,
}

impl ParticipantDetailsInput {
    #[must_use]
    pub fn into_details(self) -> ParticipantDetails {
        ParticipantDetails {
            name: self.name,
            department: self.department,
            position: self.position,
            meeting_role: self.meeting_role,
        }
    }
}

/* -------------------------------------------------------------------------
 * Parsing
 * ------------------------------------------------------------------------- */

/// Parse a canonical identifier, naming the entity in the refusal.
///
/// Strict: a syntactically valid UUID of another version is rejected, because
/// "it parsed" is not "it is one of ours" (ADR-0011).
pub fn parse_id<E: Entity>(field: &str, text: &str) -> HostResult<Id<E>> {
    Id::<E>::parse(text)
        .map_err(|_| HostError::validation(field, "a 36-character UUID version 7", text))
}

/// Parse a meeting id.
pub fn parse_meeting_id(text: &str) -> HostResult<MeetingId> {
    parse_id("meeting id", text)
}

/// Parse a participant id.
pub fn parse_participant_id(text: &str) -> HostResult<ParticipantId> {
    parse_id("participant id", text)
}

fn parse_date(text: &str) -> HostResult<MeetingDate> {
    MeetingDate::parse_storage(text)
        .map_err(|_| HostError::validation("meeting date", "a calendar date as YYYY-MM-DD", text))
}

fn parse_time(field: &str, text: &str) -> HostResult<MeetingTime> {
    MeetingTime::parse_storage(text)
        .map_err(|_| HostError::validation(field, "a time of day as HH:MM:SS", text))
}

/// Resolve an IANA identifier against the bundled timezone database.
///
/// The only input parsed here that consults data rather than a format. It is
/// still shape-checking: an unknown zone cannot be turned into a
/// `MeetingTimeZone` at all, so the domain could not be handed one.
fn parse_timezone(text: &str) -> HostResult<MeetingTimeZone> {
    MeetingTimeZone::new(text).map_err(|_| {
        HostError::validation(
            "meeting timezone",
            "an IANA timezone identifier, for example Asia/Makassar",
            text,
        )
    })
}

/* -------------------------------------------------------------------------
 * Responses: mutation outcomes
 *
 * Small on purpose. A mutation reports what it did; the UI then re-reads the
 * affected query, because the database is authoritative and a locally
 * assembled "probably now looks like this" is not.
 * ------------------------------------------------------------------------- */

#[derive(Debug, Clone, Serialize)]
pub struct MeetingCreatedDto {
    pub meeting_id: MeetingId,
    pub status: MeetingStatus,
    pub at: UtcTimestamp,
}

impl From<MeetingCreated> for MeetingCreatedDto {
    fn from(outcome: MeetingCreated) -> Self {
        Self {
            meeting_id: outcome.meeting_id,
            status: outcome.status,
            at: outcome.at,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MeetingUpdatedDto {
    pub meeting_id: MeetingId,
    pub at: UtcTimestamp,
}

impl From<MeetingUpdated> for MeetingUpdatedDto {
    fn from(outcome: MeetingUpdated) -> Self {
        Self {
            meeting_id: outcome.meeting_id,
            at: outcome.at,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MeetingTransitionedDto {
    pub meeting_id: MeetingId,
    pub from: MeetingStatus,
    pub to: MeetingStatus,
    pub at: UtcTimestamp,
}

impl From<MeetingTransitioned> for MeetingTransitionedDto {
    fn from(outcome: MeetingTransitioned) -> Self {
        Self {
            meeting_id: outcome.meeting_id,
            from: outcome.from,
            to: outcome.to,
            at: outcome.at,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ParticipantAddedDto {
    pub participant_id: ParticipantId,
    pub roster_size: i64,
    pub at: UtcTimestamp,
}

impl From<ParticipantAdded> for ParticipantAddedDto {
    fn from(outcome: ParticipantAdded) -> Self {
        Self {
            participant_id: outcome.participant_id,
            roster_size: outcome.roster_size,
            at: outcome.at,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ParticipantUpdatedDto {
    pub participant_id: ParticipantId,
    pub at: UtcTimestamp,
}

impl From<ParticipantUpdated> for ParticipantUpdatedDto {
    fn from(outcome: ParticipantUpdated) -> Self {
        Self {
            participant_id: outcome.participant_id,
            at: outcome.at,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ParticipantRemovedDto {
    pub participant_id: ParticipantId,
    pub roster_size: i64,
    pub at: UtcTimestamp,
}

impl From<ParticipantRemoved> for ParticipantRemovedDto {
    fn from(outcome: ParticipantRemoved) -> Self {
        Self {
            participant_id: outcome.participant_id,
            roster_size: outcome.roster_size,
            at: outcome.at,
        }
    }
}

/* -------------------------------------------------------------------------
 * Responses: read models
 * ------------------------------------------------------------------------- */

/// A meeting in the Host's list.
#[derive(Debug, Clone, Serialize)]
pub struct MeetingSummaryDto {
    pub id: MeetingId,
    pub title: String,
    pub topic: Option<String>,
    /// Zoneless `YYYY-MM-DD`, read in `timezone`.
    pub date: String,
    pub start_time: String,
    pub end_time: String,
    pub timezone: String,
    pub status: MeetingStatus,
    pub participant_count: i64,
    pub created_at: UtcTimestamp,
}

impl From<MeetingSummary> for MeetingSummaryDto {
    fn from(row: MeetingSummary) -> Self {
        Self {
            id: row.id,
            title: row.title,
            topic: row.topic,
            date: row.date.to_storage(),
            start_time: row.start_time.to_storage(),
            end_time: row.end_time.to_storage(),
            timezone: row.timezone,
            status: row.status,
            participant_count: row.participant_count,
            created_at: row.created_at,
        }
    }
}

/// A meeting in full.
#[derive(Debug, Clone, Serialize)]
pub struct MeetingDetailDto {
    pub id: MeetingId,
    pub title: String,
    pub topic: Option<String>,
    pub date: String,
    pub start_time: String,
    pub end_time: String,
    pub timezone: String,
    pub location: Option<String>,
    pub description: Option<String>,
    pub status: MeetingStatus,
    pub participant_count: i64,
    pub created_at: UtcTimestamp,
    pub updated_at: UtcTimestamp,
    pub locked_at: Option<UtcTimestamp>,
}

impl From<MeetingDetail> for MeetingDetailDto {
    fn from(row: MeetingDetail) -> Self {
        Self {
            id: row.id,
            title: row.title,
            topic: row.topic,
            date: row.date.to_storage(),
            start_time: row.start_time.to_storage(),
            end_time: row.end_time.to_storage(),
            timezone: row.timezone,
            location: row.location,
            description: row.description,
            status: row.status,
            participant_count: row.participant_count,
            created_at: row.created_at,
            updated_at: row.updated_at,
            locked_at: row.locked_at,
        }
    }
}

/// One participant of a meeting.
#[derive(Debug, Clone, Serialize)]
pub struct ParticipantSummaryDto {
    pub id: ParticipantId,
    pub name: String,
    pub department: Option<String>,
    pub position: Option<String>,
    pub meeting_role: Option<String>,
    pub created_at: UtcTimestamp,
}

impl From<ParticipantSummary> for ParticipantSummaryDto {
    fn from(row: ParticipantSummary) -> Self {
        Self {
            id: row.id,
            name: row.details.name,
            department: row.details.department,
            position: row.details.position,
            meeting_role: row.details.meeting_role,
            created_at: row.created_at,
        }
    }
}

/// One audit record. Read-only; no command writes or changes one.
#[derive(Debug, Clone, Serialize)]
pub struct AuditEntryDto {
    pub id: String,
    pub actor_type: String,
    pub actor_id: Option<ParticipantId>,
    pub action: String,
    pub target_type: String,
    pub target_id: Option<String>,
    pub metadata: Option<Value>,
    pub created_at: UtcTimestamp,
}

impl From<AuditEntryView> for AuditEntryDto {
    fn from(row: AuditEntryView) -> Self {
        Self {
            id: row.id.to_storage(),
            actor_type: row.actor_type,
            actor_id: row.actor_id,
            action: row.action,
            target_type: row.target_type,
            target_id: row.target_id,
            metadata: row.metadata,
            created_at: row.created_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configuration_input() -> MeetingConfigurationInput {
        MeetingConfigurationInput {
            title: "Weekly Coordination".to_owned(),
            topic: Some("Budget".to_owned()),
            date: "2026-09-20".to_owned(),
            start_time: "09:00:00".to_owned(),
            end_time: "10:30:00".to_owned(),
            timezone: "Asia/Makassar".to_owned(),
            location: None,
            description: None,
        }
    }

    #[test]
    fn a_well_formed_configuration_parses_into_domain_types() {
        let parsed = configuration_input().parse().expect("should parse");
        assert_eq!(parsed.date.to_storage(), "2026-09-20");
        assert_eq!(parsed.start_time.to_storage(), "09:00:00");
        assert_eq!(parsed.timezone.name(), "Asia/Makassar");
    }

    #[test]
    fn parsing_does_not_judge_content() {
        // A blank title is a *domain* refusal, not a parse failure: this layer
        // must not pre-empt a rule, or the rule would live in two places.
        let mut input = configuration_input();
        input.title = "   ".to_owned();
        let parsed = input
            .parse()
            .expect("parsing must not refuse a blank title");
        assert_eq!(parsed.title, "   ");
        // And the domain does refuse it.
        assert!(parsed.validated().is_err());
    }

    #[test]
    fn a_malformed_date_time_or_timezone_is_refused_with_the_field_named() {
        let cases = [
            ("20-09-2026", "09:00:00", "Asia/Makassar", "meeting date"),
            ("2026-09-20", "9am", "Asia/Makassar", "meeting start time"),
            ("2026-09-20", "09:00:00", "Mars/Olympus", "meeting timezone"),
        ];

        for (date, start, timezone, field) in cases {
            let mut input = configuration_input();
            input.date = date.to_owned();
            input.start_time = start.to_owned();
            input.timezone = timezone.to_owned();

            let error = input.parse().expect_err("should be refused");
            let json = serde_json::to_value(&error).expect("serializes");
            assert_eq!(json["kind"], "validation");
            assert_eq!(json["field"], field);
        }
    }

    #[test]
    fn an_end_time_before_the_start_parses_and_is_left_to_the_domain() {
        let mut input = configuration_input();
        input.start_time = "11:00:00".to_owned();
        input.end_time = "10:00:00".to_owned();

        let parsed = input.parse().expect("both are valid times");
        assert!(
            parsed.validated().is_err(),
            "the domain must be the one to refuse it"
        );
    }

    #[test]
    fn identifiers_are_parsed_strictly() {
        let real = MeetingId::new();
        assert_eq!(parse_meeting_id(&real.to_storage()).unwrap(), real);

        for bad in [
            "",
            "not-a-uuid",
            // Syntactically a UUID, but version 4.
            "9f1a8f5e-4b6d-4c3a-8f2e-1d2c3b4a5968",
        ] {
            let error = parse_meeting_id(bad).expect_err("should be refused");
            assert!(error.message.contains("meeting id"), "{error:?}");
        }
    }

    #[test]
    fn a_request_deserializes_from_snake_case_json() {
        // The same casing on both sides of the boundary: no translation step to
        // get wrong, and the field names match the SQLite columns.
        let json = serde_json::json!({
            "title": "Weekly Coordination",
            "topic": null,
            "date": "2026-09-20",
            "start_time": "09:00:00",
            "end_time": "10:30:00",
            "timezone": "Asia/Makassar",
            "location": null,
            "description": null,
        });
        let input: MeetingConfigurationInput =
            serde_json::from_value(json).expect("should deserialize");
        assert!(input.parse().is_ok());
    }
}
