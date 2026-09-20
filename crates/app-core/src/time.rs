//! Time, timezone and their canonical storage formats.
//!
//! Implements the rules in PRD section 25 and architecture rules section 26,
//! and the library choice recorded in ADR-0010.
//!
//! Three kinds of time exist in this system and they are deliberately three
//! different Rust types, because conflating them is the bug ADR-0005 was
//! written to prevent:
//!
//! | Kind | Type | Example column |
//! | --- | --- | --- |
//! | An instant, always UTC | [`UtcTimestamp`] | `created_at`, `locked_at` |
//! | A zoneless calendar date/time | [`MeetingDate`], [`MeetingTime`] | `date`, `start_time` |
//! | The zone those are read in | [`MeetingTimeZone`] | `timezone` |
//!
//! A meeting's `date`/`start_time`/`end_time` mean nothing without the
//! meeting's `timezone`, so they are stored zoneless and are only ever resolved
//! to an instant through [`MeetingTimeZone::resolve`]. The OS timezone is never
//! consulted implicitly.
//!
//! # Canonical storage formats
//!
//! Every format below is **fixed width**, which is what makes lexicographic
//! `ORDER BY` on the stored `TEXT` agree with chronological order and makes
//! exports deterministic (architecture rules 19 and 26.4). A variable-width
//! timestamp would silently break that: `...:00Z` and `...:00.5Z` do not
//! compare in time order as strings.
//!
//! | Value | Format | Width |
//! | --- | --- | --- |
//! | [`UtcTimestamp`] | `YYYY-MM-DDTHH:MM:SS.sssZ` | 24 |
//! | [`MeetingDate`] | `YYYY-MM-DD` | 10 |
//! | [`MeetingTime`] | `HH:MM:SS` | 8 |
//!
//! The migration re-states each of these as a `CHECK` constraint, including the
//! trailing `Z`, so "timestamps are UTC" is enforced by the database and not
//! only by this module.

use core::fmt;
use core::str::FromStr;

use jiff::civil;
use jiff::tz::TimeZone;
use jiff::Timestamp;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

/// `strftime` format producing the canonical UTC storage form.
const TIMESTAMP_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.3fZ";
/// Width of [`TIMESTAMP_FORMAT`] output.
pub const TIMESTAMP_WIDTH: usize = 24;
/// Width of the canonical date form.
pub const DATE_WIDTH: usize = 10;
/// Width of the canonical time-of-day form.
pub const TIME_WIDTH: usize = 8;

/// Errors produced when reading time values from storage or untrusted input.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum TimeError {
    /// A timestamp was not in the canonical UTC storage form.
    #[error("invalid timestamp: expected {expected}, detected {detected:?}")]
    Timestamp {
        expected: &'static str,
        detected: String,
    },

    /// A calendar date was not `YYYY-MM-DD`.
    #[error("invalid date: expected YYYY-MM-DD, detected {detected:?}")]
    Date { detected: String },

    /// A time of day was not `HH:MM:SS`.
    #[error("invalid time: expected HH:MM:SS, detected {detected:?}")]
    Time { detected: String },

    /// The timezone is not a known IANA identifier.
    #[error("unknown timezone: expected an IANA identifier such as Asia/Makassar, detected {detected:?}")]
    UnknownTimeZone { detected: String },

    /// A local date and time does not exist, or exists twice, in the meeting's
    /// timezone because of a DST transition.
    #[error("{date} {time} is {problem} in timezone {timezone}")]
    DstBoundary {
        date: String,
        time: String,
        timezone: String,
        problem: &'static str,
    },
}

/// An instant in time, always UTC, stored as RFC 3339 with millisecond
/// precision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UtcTimestamp(Timestamp);

impl UtcTimestamp {
    /// The current instant, truncated to milliseconds so that storage
    /// round-trips exactly.
    #[must_use]
    pub fn now() -> Self {
        Self::truncate(Timestamp::now())
    }

    /// Truncate to the millisecond precision the storage format carries.
    #[must_use]
    pub fn truncate(timestamp: Timestamp) -> Self {
        let millis = timestamp.as_millisecond();
        // `as_millisecond` is within range by construction, so this cannot fail
        // for any timestamp that already exists.
        Self(Timestamp::from_millisecond(millis).unwrap_or(timestamp))
    }

    /// The canonical `YYYY-MM-DDTHH:MM:SS.sssZ` form written to SQLite.
    #[must_use]
    pub fn to_storage(&self) -> String {
        self.0.strftime(TIMESTAMP_FORMAT).to_string()
    }

    /// Read the canonical storage form.
    ///
    /// Accepts any RFC 3339 instant so that a timestamp arriving from a remote
    /// submission is not rejected merely for using a different precision, but
    /// normalises it to millisecond precision on the way in.
    pub fn parse_storage(text: &str) -> Result<Self, TimeError> {
        let timestamp = Timestamp::from_str(text).map_err(|_| TimeError::Timestamp {
            expected: "an RFC 3339 UTC instant, for example 2026-09-20T07:30:00.000Z",
            detected: text.to_owned(),
        })?;
        Ok(Self::truncate(timestamp))
    }

    /// The underlying jiff timestamp.
    #[must_use]
    pub fn as_jiff(&self) -> Timestamp {
        self.0
    }
}

impl fmt::Display for UtcTimestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_storage())
    }
}

impl Serialize for UtcTimestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_storage())
    }
}

impl<'de> Deserialize<'de> for UtcTimestamp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse_storage(&text).map_err(serde::de::Error::custom)
    }
}

/// A calendar date with no timezone, read in the meeting's timezone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MeetingDate(civil::Date);

impl MeetingDate {
    /// Build from year, month and day.
    pub fn new(year: i16, month: i8, day: i8) -> Result<Self, TimeError> {
        civil::Date::new(year, month, day)
            .map(Self)
            .map_err(|_| TimeError::Date {
                detected: format!("{year:04}-{month:02}-{day:02}"),
            })
    }

    /// The canonical `YYYY-MM-DD` form.
    #[must_use]
    pub fn to_storage(&self) -> String {
        self.0.strftime("%Y-%m-%d").to_string()
    }

    /// Read the canonical storage form.
    pub fn parse_storage(text: &str) -> Result<Self, TimeError> {
        civil::Date::from_str(text)
            .map(Self)
            .map_err(|_| TimeError::Date {
                detected: text.to_owned(),
            })
    }
}

impl fmt::Display for MeetingDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_storage())
    }
}

/// A time of day with no timezone, read in the meeting's timezone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MeetingTime(civil::Time);

impl MeetingTime {
    /// Build from hour, minute and second.
    pub fn new(hour: i8, minute: i8, second: i8) -> Result<Self, TimeError> {
        civil::Time::new(hour, minute, second, 0)
            .map(Self)
            .map_err(|_| TimeError::Time {
                detected: format!("{hour:02}:{minute:02}:{second:02}"),
            })
    }

    /// The canonical `HH:MM:SS` form.
    #[must_use]
    pub fn to_storage(&self) -> String {
        self.0.strftime("%H:%M:%S").to_string()
    }

    /// Read the canonical storage form.
    pub fn parse_storage(text: &str) -> Result<Self, TimeError> {
        civil::Time::from_str(text)
            .map(Self)
            .map_err(|_| TimeError::Time {
                detected: text.to_owned(),
            })
    }
}

impl fmt::Display for MeetingTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_storage())
    }
}

/// A meeting's timezone, as an IANA identifier.
///
/// Stored explicitly on every meeting. The OS timezone may be *offered* as a
/// default when the Host creates a meeting, but is never read implicitly to
/// decide what a stored schedule means (PRD 25.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeetingTimeZone {
    name: String,
    tz: TimeZone,
}

impl MeetingTimeZone {
    /// Look up an IANA identifier in the bundled timezone database.
    ///
    /// The database is compiled into the binary, so this works on a Host
    /// machine with no internet and on Windows, which ships no system tzdb.
    pub fn new(name: &str) -> Result<Self, TimeError> {
        let tz = TimeZone::get(name).map_err(|_| TimeError::UnknownTimeZone {
            detected: name.to_owned(),
        })?;
        Ok(Self {
            name: name.to_owned(),
            tz,
        })
    }

    /// The IANA identifier as stored.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Resolve a meeting-local date and time to a UTC instant.
    ///
    /// DST is handled explicitly rather than guessed. A local time that does
    /// not exist (spring forward) or occurs twice (fall back) is reported as
    /// [`TimeError::DstBoundary`] instead of being silently shifted, because a
    /// meeting scheduled into a DST gap is a data-entry problem the Host should
    /// see, not something to paper over.
    pub fn resolve(&self, date: MeetingDate, time: MeetingTime) -> Result<UtcTimestamp, TimeError> {
        let local = civil::DateTime::from_parts(date.0, time.0);
        let ambiguous = self.tz.to_ambiguous_zoned(local);

        let problem = match ambiguous.offset() {
            jiff::tz::AmbiguousOffset::Unambiguous { .. } => None,
            jiff::tz::AmbiguousOffset::Gap { .. } => Some("skipped by a daylight-saving change"),
            jiff::tz::AmbiguousOffset::Fold { .. } => Some("repeated by a daylight-saving change"),
        };

        if let Some(problem) = problem {
            return Err(TimeError::DstBoundary {
                date: date.to_storage(),
                time: time.to_storage(),
                timezone: self.name.clone(),
                problem,
            });
        }

        let zoned = ambiguous.compatible().map_err(|_| TimeError::DstBoundary {
            date: date.to_storage(),
            time: time.to_storage(),
            timezone: self.name.clone(),
            problem: "not resolvable",
        })?;

        Ok(UtcTimestamp::truncate(zoned.timestamp()))
    }

    /// Render a UTC instant as a meeting-local date and time, for UI and
    /// exports (PRD 25.3, 25.4).
    #[must_use]
    pub fn render(&self, at: UtcTimestamp) -> (MeetingDate, MeetingTime) {
        let zoned = at.as_jiff().to_zoned(self.tz.clone());
        let local = zoned.datetime();
        (MeetingDate(local.date()), MeetingTime(local.time()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_storage_is_fixed_width_utc_and_round_trips() {
        let now = UtcTimestamp::now();
        let text = now.to_storage();

        assert_eq!(text.len(), TIMESTAMP_WIDTH);
        assert!(
            text.ends_with('Z'),
            "storage must be explicitly UTC: {text}"
        );
        assert_eq!(UtcTimestamp::parse_storage(&text).unwrap(), now);
    }

    #[test]
    fn fixed_width_keeps_string_order_equal_to_time_order() {
        // The reason the format pads to milliseconds. With a variable-width
        // format, the earlier instant would sort *after* the later one as text.
        let earlier = UtcTimestamp::parse_storage("2026-09-20T07:30:00.500Z").unwrap();
        let later = UtcTimestamp::parse_storage("2026-09-20T07:30:01.000Z").unwrap();

        assert!(earlier < later);
        assert!(earlier.to_storage() < later.to_storage());
    }

    #[test]
    fn a_non_utc_offset_is_normalised_to_utc_on_the_way_in() {
        // 09:30+02:00 is 07:30Z. Storage is always UTC (PRD 25.1).
        let parsed = UtcTimestamp::parse_storage("2026-09-20T09:30:00+02:00").unwrap();
        assert_eq!(parsed.to_storage(), "2026-09-20T07:30:00.000Z");
    }

    #[test]
    fn a_malformed_timestamp_names_expected_and_detected() {
        let err = UtcTimestamp::parse_storage("20 September 2026").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("expected"), "{msg}");
        assert!(msg.contains("20 September 2026"), "{msg}");
    }

    #[test]
    fn date_and_time_storage_are_fixed_width() {
        let date = MeetingDate::new(2026, 9, 20).unwrap();
        let time = MeetingTime::new(9, 5, 0).unwrap();

        assert_eq!(date.to_storage(), "2026-09-20");
        assert_eq!(date.to_storage().len(), DATE_WIDTH);
        assert_eq!(time.to_storage(), "09:05:00");
        assert_eq!(time.to_storage().len(), TIME_WIDTH);
    }

    #[test]
    fn an_unknown_timezone_is_rejected_with_an_actionable_message() {
        let err = MeetingTimeZone::new("Mars/Olympus_Mons").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("IANA"), "{msg}");
        assert!(msg.contains("Mars/Olympus_Mons"), "{msg}");
    }

    #[test]
    fn indonesian_zones_resolve_to_their_fixed_offsets() {
        // The target region: three offsets, no DST.
        let date = MeetingDate::new(2026, 9, 20).unwrap();
        let time = MeetingTime::new(9, 0, 0).unwrap();

        let wib = MeetingTimeZone::new("Asia/Jakarta").unwrap();
        let wita = MeetingTimeZone::new("Asia/Makassar").unwrap();
        let wit = MeetingTimeZone::new("Asia/Jayapura").unwrap();

        assert_eq!(
            wib.resolve(date, time).unwrap().to_storage(),
            "2026-09-20T02:00:00.000Z"
        );
        assert_eq!(
            wita.resolve(date, time).unwrap().to_storage(),
            "2026-09-20T01:00:00.000Z"
        );
        assert_eq!(
            wit.resolve(date, time).unwrap().to_storage(),
            "2026-09-20T00:00:00.000Z"
        );
    }

    #[test]
    fn utc_to_meeting_timezone_round_trips() {
        let tz = MeetingTimeZone::new("Asia/Makassar").unwrap();
        let date = MeetingDate::new(2026, 9, 20).unwrap();
        let time = MeetingTime::new(14, 30, 0).unwrap();

        let instant = tz.resolve(date, time).unwrap();
        let (back_date, back_time) = tz.render(instant);

        assert_eq!(back_date, date);
        assert_eq!(back_time, time);
    }

    #[test]
    fn the_same_instant_reads_differently_in_two_meeting_timezones() {
        // Why the timezone must be stored per meeting rather than taken from
        // whichever machine opens the record (ADR-0005).
        let instant = UtcTimestamp::parse_storage("2026-09-20T02:00:00.000Z").unwrap();

        let jakarta = MeetingTimeZone::new("Asia/Jakarta").unwrap();
        let jayapura = MeetingTimeZone::new("Asia/Jayapura").unwrap();

        assert_eq!(jakarta.render(instant).1.to_storage(), "09:00:00");
        assert_eq!(jayapura.render(instant).1.to_storage(), "11:00:00");
    }

    #[test]
    fn a_time_skipped_by_spring_forward_is_reported_not_guessed() {
        // Deliberately a non-Indonesian zone: the target region never exercises
        // this path (ADR-0005), so the test has to go looking for it.
        // 2026-03-08, America/New_York jumps 02:00 -> 03:00; 02:30 never exists.
        let tz = MeetingTimeZone::new("America/New_York").unwrap();
        let date = MeetingDate::new(2026, 3, 8).unwrap();
        let time = MeetingTime::new(2, 30, 0).unwrap();

        let err = tz.resolve(date, time).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("skipped"), "{msg}");
        assert!(msg.contains("America/New_York"), "{msg}");
    }

    #[test]
    fn a_time_repeated_by_fall_back_is_reported_not_guessed() {
        // 2026-11-01, America/New_York repeats 01:00-02:00; 01:30 happens twice.
        let tz = MeetingTimeZone::new("America/New_York").unwrap();
        let date = MeetingDate::new(2026, 11, 1).unwrap();
        let time = MeetingTime::new(1, 30, 0).unwrap();

        let err = tz.resolve(date, time).unwrap_err();
        assert!(err.to_string().contains("repeated"), "{err}");
    }

    #[test]
    fn dst_shifts_the_utc_offset_for_the_same_local_time() {
        let tz = MeetingTimeZone::new("America/New_York").unwrap();
        let time = MeetingTime::new(12, 0, 0).unwrap();

        // January: UTC-5. July: UTC-4.
        let winter = tz
            .resolve(MeetingDate::new(2026, 1, 15).unwrap(), time)
            .unwrap();
        let summer = tz
            .resolve(MeetingDate::new(2026, 7, 15).unwrap(), time)
            .unwrap();

        assert_eq!(winter.to_storage(), "2026-01-15T17:00:00.000Z");
        assert_eq!(summer.to_storage(), "2026-07-15T16:00:00.000Z");
    }

    #[test]
    fn timestamps_serialize_as_the_storage_string() {
        let at = UtcTimestamp::parse_storage("2026-09-20T07:30:00.000Z").unwrap();
        let json = serde_json::to_string(&at).unwrap();
        assert_eq!(json, "\"2026-09-20T07:30:00.000Z\"");
        assert_eq!(serde_json::from_str::<UtcTimestamp>(&json).unwrap(), at);
    }
}
