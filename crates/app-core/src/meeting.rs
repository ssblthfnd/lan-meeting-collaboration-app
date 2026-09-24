//! Meeting lifecycle.
//!
//! `DRAFT -> OPEN -> LOCKED`, exactly as PRD section 6 defines it and no
//! further. `EXPORTED` is deliberately not a status: the PRD says export is
//! recorded in the audit log and the meeting stays `LOCKED`.
//!
//! There is no unlock. Nothing in the PRD or the ADRs defines one, and a lock
//! that can be undone is not the guarantee PRD section 19 describes.
//!
//! [`MeetingConfiguration`] is what the Host decides when creating a meeting
//! (PRD section 7). It is mutable only while the meeting is `DRAFT`
//! (ADR-0013), which [`Meeting::ensure_draft`] is the single definition of.

use core::fmt;
use core::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::{DomainError, DomainResult};
use crate::id::MeetingId;
use crate::text;
use crate::time::{MeetingDate, MeetingTime, MeetingTimeZone};

/// Where a meeting is in its lifecycle.
///
/// The string forms are the values stored in `meetings.status` and pinned by a
/// `CHECK` constraint in the migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum MeetingStatus {
    /// The Host is still preparing the meeting.
    Draft,
    /// Participants may join and write notes.
    Open,
    /// No further changes are permitted.
    Locked,
}

impl MeetingStatus {
    /// The persisted discriminator.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            MeetingStatus::Draft => "DRAFT",
            MeetingStatus::Open => "OPEN",
            MeetingStatus::Locked => "LOCKED",
        }
    }

    /// Whether the meeting still accepts mutations.
    ///
    /// This is the single definition of "mutable". Callers ask the meeting,
    /// rather than each deciding for themselves what locked means.
    #[must_use]
    pub fn accepts_mutations(&self) -> bool {
        !matches!(self, MeetingStatus::Locked)
    }
}

impl fmt::Display for MeetingStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for MeetingStatus {
    type Err = DomainError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "DRAFT" => Ok(MeetingStatus::Draft),
            "OPEN" => Ok(MeetingStatus::Open),
            "LOCKED" => Ok(MeetingStatus::Locked),
            other => Err(DomainError::Validation {
                field: "meeting status",
                expected: "DRAFT, OPEN or LOCKED",
                detected: other.to_owned(),
            }),
        }
    }
}

/// What the Host decides about a meeting: its subject, schedule and place.
///
/// Every field in PRD section 7 except the participant list, which is a
/// separate roster (see [`crate::participant`]).
///
/// The schedule is deliberately three values rather than two instants:
/// [`MeetingDate`] and [`MeetingTime`] are zoneless and mean nothing without
/// `timezone`, which is required (ADR-0005, ADR-0010). An OS timezone may be
/// *offered* to the Host as a default, but the stored value is always explicit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeetingConfiguration {
    pub title: String,
    pub topic: Option<String>,
    pub date: MeetingDate,
    pub start_time: MeetingTime,
    pub end_time: MeetingTime,
    pub timezone: MeetingTimeZone,
    pub location: Option<String>,
    pub description: Option<String>,
}

impl MeetingConfiguration {
    /// Validate and normalise, returning the form that is written to SQLite.
    ///
    /// Three things are checked, all of them facts about the input alone:
    ///
    /// 1. The title carries something (the migration demands the same).
    /// 2. The meeting does not end before it starts.
    /// 3. Both ends of the schedule are times that actually exist in the
    ///    meeting's timezone. A meeting placed in a daylight-saving gap is a
    ///    data-entry problem the Host should see, not a value to silently
    ///    shift (ADR-0010).
    ///
    /// Blank optional fields become absent rather than empty strings, so
    /// "no location" has one representation.
    pub fn validated(&self) -> DomainResult<Self> {
        let title = text::required("meeting title", &self.title)?;

        if self.end_time < self.start_time {
            return Err(DomainError::Validation {
                field: "meeting end time",
                expected: "a time at or after the start time",
                detected: format!(
                    "start {}, end {}",
                    self.start_time.to_storage(),
                    self.end_time.to_storage()
                ),
            });
        }

        self.ensure_exists("meeting start time", self.start_time)?;
        self.ensure_exists("meeting end time", self.end_time)?;

        Ok(Self {
            title,
            topic: text::optional(self.topic.as_deref()),
            date: self.date,
            start_time: self.start_time,
            end_time: self.end_time,
            timezone: self.timezone.clone(),
            location: text::optional(self.location.as_deref()),
            description: text::optional(self.description.as_deref()),
        })
    }

    /// Refuse a local time that a daylight-saving transition skipped or
    /// repeated.
    ///
    /// The refusal carries the timezone and the problem, because "invalid time"
    /// on its own would leave the Host with no way to see why an ordinary
    /// looking time was rejected (architecture rules section 22).
    fn ensure_exists(&self, field: &'static str, time: MeetingTime) -> DomainResult<()> {
        self.timezone
            .resolve(self.date, time)
            .map(|_| ())
            .map_err(|error| DomainError::Validation {
                field,
                expected: "a local time that exists in the meeting's timezone",
                detected: error.to_string(),
            })
    }
}

/// The meeting state a mutation is judged against.
///
/// Always loaded from the database **inside** the mutating transaction. A
/// `Meeting` value obtained any other way is a snapshot of the past and must
/// not be used to decide whether a mutation may proceed (architecture rules
/// section 15).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Meeting {
    pub id: MeetingId,
    pub title: String,
    pub status: MeetingStatus,
    /// Whether a join token is currently issued for this meeting.
    ///
    /// A boolean, deliberately not the hash. The domain needs to know only
    /// whether issuing a new token would displace an existing one, so that the
    /// audit record can say so; it has no use for the value, and a credential
    /// hash that is never loaded is a credential hash that cannot be logged,
    /// compared or leaked by accident.
    pub has_join_token: bool,
}

impl Meeting {
    /// Refuse if the meeting no longer accepts mutations.
    ///
    /// The error names the meeting by title as well as id, because "this
    /// meeting is locked" is only actionable if the Host can tell which one
    /// (architecture rules section 22).
    pub fn ensure_mutable(&self) -> DomainResult<()> {
        if self.status.accepts_mutations() {
            Ok(())
        } else {
            Err(DomainError::MeetingLocked {
                meeting_id: self.id,
                title: self.title.clone(),
            })
        }
    }

    /// Refuse unless the meeting is still being prepared.
    ///
    /// This is the single definition of "settled": what a meeting *is*, and who
    /// is in it, are decided while it is `DRAFT` and fixed once it opens
    /// (ADR-0013). Notes are the opposite case and stay writable through `OPEN`,
    /// which is why this is a separate check from [`Meeting::ensure_mutable`]
    /// rather than a stricter version of it.
    pub fn ensure_draft(&self) -> DomainResult<()> {
        match self.status {
            MeetingStatus::Draft => Ok(()),
            detected => Err(DomainError::MeetingNotDraft {
                meeting_id: self.id,
                detected,
            }),
        }
    }

    /// Refuse unless participants may take part.
    ///
    /// The mirror of [`Meeting::ensure_draft`]. Preparation belongs to `DRAFT`
    /// and participation belongs to `OPEN`: a meeting still being prepared has
    /// no one to join it, and a locked one is finished (PRD sections 6 and 19).
    ///
    /// Every join and every claim re-reads this inside its own transaction, so
    /// a meeting that locks mid-session stops accepting participants at once,
    /// with no server restart and no cached status anywhere
    /// (architecture rules section 15).
    pub fn ensure_open(&self) -> DomainResult<()> {
        match self.status {
            MeetingStatus::Open => Ok(()),
            detected => Err(DomainError::MeetingNotOpen {
                meeting_id: self.id,
                detected,
            }),
        }
    }

    /// Refuse unless there is a settled meeting to produce a document about.
    ///
    /// The mirror of [`Meeting::ensure_open`] with a wider acceptance: export
    /// is permitted from `OPEN` **and** `LOCKED` (PRD section 6 explicitly
    /// does not require an `EXPORTED` database status, and nothing in the
    /// PRD or architecture rules restricts export to `LOCKED` alone), and
    /// refused only from `DRAFT`, whose roster and configuration are not yet
    /// settled (ADR-0013). Reuses [`DomainError::MeetingNotOpen`] rather than
    /// adding a variant - `OPEN` is one of the two statuses this accepts, so
    /// the message remains accurate even though it does not separately name
    /// `LOCKED` (Step 12 design freeze, E-3).
    pub fn ensure_exportable(&self) -> DomainResult<()> {
        match self.status {
            MeetingStatus::Open | MeetingStatus::Locked => Ok(()),
            detected => Err(DomainError::MeetingNotOpen {
                meeting_id: self.id,
                detected,
            }),
        }
    }

    /// Validate a lifecycle transition against the *current* status.
    ///
    /// Only the two transitions PRD section 6 defines are legal, and each is
    /// legal from exactly one state. Re-opening an open meeting or re-locking a
    /// locked one is rejected rather than treated as a harmless no-op: a
    /// no-op would still write an audit record claiming something happened.
    pub fn ensure_transition(&self, to: MeetingStatus) -> DomainResult<()> {
        let expected = match to {
            MeetingStatus::Open => "DRAFT",
            MeetingStatus::Locked => "OPEN",
            MeetingStatus::Draft => {
                // Nothing returns to DRAFT, and there is no unlock.
                return Err(DomainError::InvalidTransition {
                    expected: "a forward transition to OPEN or LOCKED",
                    detected: self.status,
                });
            }
        };

        // The only two transitions PRD section 6 defines.
        let legal = matches!(
            (self.status, to),
            (MeetingStatus::Draft, MeetingStatus::Open)
                | (MeetingStatus::Open, MeetingStatus::Locked)
        );

        if legal {
            Ok(())
        } else {
            Err(DomainError::InvalidTransition {
                expected,
                detected: self.status,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meeting(status: MeetingStatus) -> Meeting {
        Meeting {
            id: MeetingId::new(),
            title: "Weekly Coordination".to_owned(),
            status,
            has_join_token: false,
        }
    }

    fn configuration(timezone: &str) -> MeetingConfiguration {
        MeetingConfiguration {
            title: "Weekly Coordination".to_owned(),
            topic: Some("Budget".to_owned()),
            date: MeetingDate::new(2026, 9, 20).unwrap(),
            start_time: MeetingTime::new(9, 0, 0).unwrap(),
            end_time: MeetingTime::new(10, 30, 0).unwrap(),
            timezone: MeetingTimeZone::new(timezone).unwrap(),
            location: Some("Meeting Room 2".to_owned()),
            description: None,
        }
    }

    #[test]
    fn status_strings_match_the_persisted_discriminators() {
        assert_eq!(MeetingStatus::Draft.as_str(), "DRAFT");
        assert_eq!(MeetingStatus::Open.as_str(), "OPEN");
        assert_eq!(MeetingStatus::Locked.as_str(), "LOCKED");

        for status in [
            MeetingStatus::Draft,
            MeetingStatus::Open,
            MeetingStatus::Locked,
        ] {
            assert_eq!(status.as_str().parse::<MeetingStatus>().unwrap(), status);
        }
    }

    #[test]
    fn an_unknown_status_string_is_rejected_with_what_was_expected() {
        let err = "EXPORTED".parse::<MeetingStatus>().unwrap_err();
        let message = err.to_string();
        assert!(message.contains("DRAFT, OPEN or LOCKED"), "{message}");
        assert!(message.contains("EXPORTED"), "{message}");
    }

    #[test]
    fn only_a_locked_meeting_refuses_mutations() {
        assert!(meeting(MeetingStatus::Draft).ensure_mutable().is_ok());
        assert!(meeting(MeetingStatus::Open).ensure_mutable().is_ok());

        let locked = meeting(MeetingStatus::Locked);
        let err = locked.ensure_mutable().unwrap_err();
        assert!(matches!(err, DomainError::MeetingLocked { .. }));
        // Actionable: the Host can tell which meeting refused.
        assert!(err.to_string().contains("Weekly Coordination"));
    }

    #[test]
    fn only_a_draft_meeting_is_still_being_prepared() {
        assert!(meeting(MeetingStatus::Draft).ensure_draft().is_ok());

        for settled in [MeetingStatus::Open, MeetingStatus::Locked] {
            let subject = meeting(settled);
            let err = subject.ensure_draft().unwrap_err();
            assert_eq!(
                err,
                DomainError::MeetingNotDraft {
                    meeting_id: subject.id,
                    detected: settled,
                }
            );
            // Actionable: names the expected and the detected status.
            let message = err.to_string();
            assert!(message.contains("DRAFT"), "{message}");
            assert!(message.contains(settled.as_str()), "{message}");
        }
    }

    #[test]
    fn export_is_allowed_from_open_and_locked_but_not_draft() {
        assert!(meeting(MeetingStatus::Open).ensure_exportable().is_ok());
        assert!(meeting(MeetingStatus::Locked).ensure_exportable().is_ok());

        let draft = meeting(MeetingStatus::Draft);
        let err = draft.ensure_exportable().unwrap_err();
        assert_eq!(
            err,
            DomainError::MeetingNotOpen {
                meeting_id: draft.id,
                detected: MeetingStatus::Draft,
            }
        );
    }

    #[test]
    fn a_valid_configuration_is_normalised_rather_than_stored_verbatim() {
        let mut input = configuration("Asia/Makassar");
        input.title = "  Weekly Coordination  ".to_owned();
        input.location = Some("   ".to_owned());
        input.description = Some("".to_owned());

        let normalised = input.validated().unwrap();
        assert_eq!(normalised.title, "Weekly Coordination");
        // Blank optional text is absent, not an empty string.
        assert_eq!(normalised.location, None);
        assert_eq!(normalised.description, None);
        assert_eq!(normalised.topic, Some("Budget".to_owned()));
    }

    #[test]
    fn a_configuration_without_a_title_is_rejected() {
        let mut input = configuration("Asia/Makassar");
        input.title = "   ".to_owned();
        let err = input.validated().unwrap_err();
        assert!(
            matches!(
                err,
                DomainError::Validation {
                    field: "meeting title",
                    ..
                }
            ),
            "{err:?}"
        );
    }

    #[test]
    fn a_meeting_cannot_end_before_it_starts() {
        let mut input = configuration("Asia/Makassar");
        input.start_time = MeetingTime::new(10, 30, 0).unwrap();
        input.end_time = MeetingTime::new(9, 0, 0).unwrap();

        let err = input.validated().unwrap_err();
        let message = err.to_string();
        // Names both values, so the Host can see which way round they are.
        assert!(
            message.contains("10:30:00") && message.contains("09:00:00"),
            "{message}"
        );
    }

    #[test]
    fn a_zero_length_meeting_is_accepted() {
        // Nothing in the PRD forbids it, and the schema's CHECK is `>=`.
        let mut input = configuration("Asia/Makassar");
        input.end_time = input.start_time;
        assert!(input.validated().is_ok());
    }

    #[test]
    fn a_schedule_inside_a_daylight_saving_gap_is_rejected() {
        // 2026-03-08 02:30 does not exist in New York: the clock jumps from
        // 02:00 to 03:00. ADR-0010 requires this to reach the Host as a
        // data-entry problem rather than being silently shifted. Indonesian
        // zones have fixed offsets and would never reach this path.
        let mut input = configuration("America/New_York");
        input.date = MeetingDate::new(2026, 3, 8).unwrap();
        input.start_time = MeetingTime::new(2, 30, 0).unwrap();
        input.end_time = MeetingTime::new(4, 0, 0).unwrap();

        let err = input.validated().unwrap_err();
        let message = err.to_string();
        assert!(message.contains("America/New_York"), "{message}");
        assert!(message.contains("daylight-saving"), "{message}");

        // The same wall-clock schedule is perfectly valid in a fixed-offset
        // zone, which is what makes the timezone load-bearing rather than
        // decorative.
        let mut fixed = input.clone();
        fixed.timezone = MeetingTimeZone::new("Asia/Makassar").unwrap();
        assert!(fixed.validated().is_ok());
    }

    #[test]
    fn only_the_two_forward_transitions_are_legal() {
        assert!(meeting(MeetingStatus::Draft)
            .ensure_transition(MeetingStatus::Open)
            .is_ok());
        assert!(meeting(MeetingStatus::Open)
            .ensure_transition(MeetingStatus::Locked)
            .is_ok());
    }

    #[test]
    fn skipping_draft_to_locked_is_rejected() {
        let err = meeting(MeetingStatus::Draft)
            .ensure_transition(MeetingStatus::Locked)
            .unwrap_err();
        assert_eq!(
            err,
            DomainError::InvalidTransition {
                expected: "OPEN",
                detected: MeetingStatus::Draft,
            }
        );
    }

    #[test]
    fn repeating_a_transition_is_rejected_rather_than_a_silent_no_op() {
        // A no-op would still write an audit record saying the meeting was
        // locked, which would be a lie about what happened.
        assert!(meeting(MeetingStatus::Open)
            .ensure_transition(MeetingStatus::Open)
            .is_err());
        assert!(meeting(MeetingStatus::Locked)
            .ensure_transition(MeetingStatus::Locked)
            .is_err());
    }

    #[test]
    fn there_is_no_unlock_and_no_return_to_draft() {
        for status in [
            MeetingStatus::Draft,
            MeetingStatus::Open,
            MeetingStatus::Locked,
        ] {
            assert!(meeting(status)
                .ensure_transition(MeetingStatus::Draft)
                .is_err());
        }
        // Specifically: LOCKED never goes back to OPEN.
        assert!(meeting(MeetingStatus::Locked)
            .ensure_transition(MeetingStatus::Open)
            .is_err());
    }
}
