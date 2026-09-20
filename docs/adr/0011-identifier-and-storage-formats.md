# 0011. Identifier type and canonical storage formats

- Status: Accepted
- Date: 2026-09-20

Completes ADR-0008 for the first migration. Depends on ADR-0010 for the
date-time library.

## Context

The skeleton carried `pub type Id = String`, with a note that the real type
arrives with the schema. Section 13 of the architecture rules lists columns but
not their types, formats or constraints, and "id" as a bare string would let any
value at all into a primary key.

Three things had to be settled together, because each constrains the others: how
an identifier is represented, how a timestamp is represented, and how much of
that the database itself refuses to violate.

## Decision

### 1. Identifiers are UUIDv7, in one generic type

`Id<E>` is a `Copy` newtype over `uuid::Uuid`, generic over a zero-sized entity
marker, with aliases `MeetingId`, `ParticipantId`, `NoteId` and so on.

One generic type rather than nine wrapper structs: `MeetingId` and
`ParticipantId` are still distinct types that cannot be swapped at a call site,
but there is a single implementation of parsing, formatting, ordering and serde
to maintain and to get right.

Version 7 specifically, because it is time-ordered. Section 26.4 requires a
*total* ordering on every query feeding the UI or an export, and the tiebreaker
is normally the id. With UUIDv7 that tiebreaker is chronological rather than
arbitrary, so `ORDER BY name, id` and `ORDER BY created_at, id` both mean
something.

Parsing is strict: a syntactically valid UUID of any other version is rejected.
Ids arrive in submission files and from browsers, which are untrusted input
(section 10), and "it parsed" is not "it is one of ours".

### 2. Canonical storage formats, fixed width

| Value | SQLite type | Format | Width |
| --- | --- | --- | --- |
| Identifier | `TEXT` | canonical lowercase hyphenated UUID | 36 |
| System timestamp | `TEXT` | `YYYY-MM-DDTHH:MM:SS.sssZ` | 24 |
| Meeting date | `TEXT` | `YYYY-MM-DD` | 10 |
| Meeting time | `TEXT` | `HH:MM:SS` | 8 |

`TEXT` rather than a 16-byte `BLOB` for ids. A blob is smaller and sorts the
same way, but the canonical string is already the JSON representation in
`packages/contracts` and in the submission schema, so `TEXT` means one
representation end to end with no conversion asymmetry between what the
database holds and what a submission carries. It is also legible when reading
`raw_payload` or an audit row during an investigation, and it is checkable: a
`CHECK` constraint can validate the layout and the version nibble of a string,
and can validate nothing at all about a blob beyond its length. At 99
participants per meeting, the size difference is not a consideration.

**Fixed width is the load-bearing part of the timestamp format.** Millisecond
digits are always printed, even when zero. With a variable-width format,
`2026-01-01T00:00:00.500Z` sorts *after* `2026-01-01T00:00:01Z` as text, because
`.` is below `Z` in ASCII — so `ORDER BY created_at` would silently stop being
chronological. The trailing `Z` is part of the `CHECK`, so a local time with an
offset cannot be written into a column the architecture says is UTC.

### 3. The database enforces the invariants, not just the application

Section 15 says a disabled button is not enforcement. The same reasoning applies
one layer down: a rule that lives only in Rust is a rule some future code path
can skip. The migration therefore enforces:

- ids match a UUIDv7 `GLOB`, including the version nibble and variant, so a v4
  uuid or an arbitrary string cannot occupy a primary key;
- timestamps match the fixed-width UTC pattern, `Z` included;
- `notes` has `UNIQUE(meeting_id, participant_id)` (ADR-0003);
- `notes`, `participant_sessions` and `remote_submissions` reference
  `participants(meeting_id, id)` as a **composite** foreign key, so "this
  participant belongs to this meeting" (section 9) is structural rather than a
  validation step someone might omit;
- a partial unique index over `(meeting_id, participant_id) WHERE revoked_at IS
  NULL` enforces first-claim-wins (ADR-0002) — a race between two browsers is
  decided by the database, not by a check-then-insert;
- `note_versions` has `UNIQUE(note_id, version)`, `CHECK((created_by_type =
  'HOST') = (created_by IS NULL))`, and a trigger refusing `UPDATE`, so history
  is never rewritten (section 18.1);
- `audit_logs` has triggers refusing `UPDATE` and `DELETE` (section 17);
- `note_links.url` is restricted to `http`, `https` and `mailto` (ADR-0007), and
  a trigger caps a note at five links (PRD section 14);
- `remote_submissions` has the ADR-0008 idempotency key and a `CHECK` that a
  resolution which wrote a note has a `note_version` and one that rejected has
  none.

### 4. Two columns not in the section 13 list

**`participant_sessions.approved_at`.** ADR-0008 requires `PENDING` to be
derivable from `participant_sessions`, but the section 13 column list has no way
to distinguish "a session exists, awaiting Host approval" from "a session is
live". `approved_at` supplies exactly that, and nothing more. The derivation is
a single query with explicit ordering, as ADR-0008 requires.

**`participants` gains `UNIQUE(meeting_id, id)`.** Redundant against the primary
key on its own; its purpose is to be the target of the composite foreign keys
above.

### 5. Deletion and retention

`audit_logs.meeting_id` is `ON DELETE RESTRICT`, and the table refuses `DELETE`
outright. An audited meeting therefore cannot be deleted. This is deliberate:
the PRD gives the Host no meeting-deletion flow, and an append-only audit trail
that disappears with its subject is not append-only. Everything else cascades
from `meetings`, so deleting a participant removes that participant's note,
versions and sessions — a deliberate destructive act on the parent row, not an
edit of history.

## Consequences

- `Id = String` is gone. Every id crossing a boundary is validated at the type
  level in Rust and again by a `CHECK` in SQLite.
- Lexicographic ordering of the stored text equals chronological ordering for
  both ids and timestamps, which is what section 26.4 needs and what makes
  exports deterministic (section 19).
- Invalid data fails at write time with a constraint violation rather than
  becoming a rendering bug later. `DbError::is_constraint_violation` lets a
  caller treat an expected violation, such as a duplicate submission, as a
  result rather than a failure.
- The `CHECK` patterns duplicate the formats defined in `app_core::time` and
  `app_core::id`. That duplication is intentional — two independent enforcement
  points — and is covered by tests that write bad values directly through SQL,
  bypassing the Rust types, to prove the database refuses them on its own.
- Changing a storage format later means a migration, not just a Rust edit.
- Because an audited meeting cannot be deleted, any future requirement to remove
  a meeting needs an explicit archival design and a superseding ADR.
