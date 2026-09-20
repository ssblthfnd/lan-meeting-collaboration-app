# 0005. Explicit timezone handling

- Status: Accepted
- Date: 2026-09-20

## Context

The original schema stored `date`, `start_time` and `end_time` with no timezone
at all, and system timestamps with no stated zone. Indonesia spans three
offsets (WIB, WITA, WIT), meeting records outlive the machine that created them,
and exports are read elsewhere. Relying on the operating system timezone makes
the meaning of a stored meeting time depend on which machine opens it.

## Decision

Time is handled explicitly.

**Storage.** System timestamps are stored in UTC, as RFC 3339 / ISO 8601:
`created_at`, `updated_at`, `submitted_at`, `imported_at`, `last_seen_at`,
`locked_at`, and audit timestamps.

**Scheduling.** A meeting stores `timezone` as an IANA identifier, for example
`Asia/Makassar`. `date`, `start_time` and `end_time` are interpreted in that
timezone. The OS timezone may be offered as a *default value* when the Host
creates a meeting, but the chosen value is always stored explicitly.

**Display.** The UI shows meeting schedule in the meeting's timezone, not the
reader's.

**Export.** Exports state the meeting timezone explicitly.

**Ordering.** Every query feeding the UI or an export has an explicit total
ordering. Insertion order and implicit rowid order are never relied upon.

**Testing.** Tests cover UTC to meeting-timezone conversion and DST behaviour,
even though the initial target region does not observe DST.

## Consequences

- A meeting record is unambiguous regardless of where it is opened or exported.
- The `meetings` table gains a required `timezone` column.
- A timezone database is needed on the Rust side; the crate choice is part of
  Phase 1, step 1 and will be recorded in ADR-0006 when added.
- DST tests will use a non-Indonesian zone deliberately, because the target
  region would never exercise the code path.
