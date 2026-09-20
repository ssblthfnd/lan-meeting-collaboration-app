# 0008. Phase 1 schema refinements

- Status: Accepted
- Date: 2026-09-20

## Context

Three gaps surfaced while reviewing the schema in the architecture rules
against the rules the same document states elsewhere. All three affect the
first migration, so they are settled before any table is created.

## Decision

### 1. `note_versions.created_by_type`

Section 18 requires note history to distinguish `HOST`, `PARTICIPANT` and
`REMOTE_IMPORT`, but the table carried only `created_by`. A participant id
alone cannot express "the Host edited this", and inferring the actor from
whether `created_by` is null is an undocumented convention.

`note_versions` gains a required `created_by_type` column holding `HOST`,
`PARTICIPANT` or `REMOTE_IMPORT`. `created_by` stays as the participant id and
is null for Host edits.

### 2. `remote_submissions` table

Section 12 requires the Host to choose reject / replace / new-version for a
possible duplicate, and section 11 requires import to be transactional. Neither
is decidable without a persistent record of what has already been imported. The
architecture rules previously called such a table optional; it is not.

A `remote_submissions` table is required, keyed on
`UNIQUE(meeting_id, participant_id, submission_id, content_hash)`:

- `submission_id` is minted when the remote form is generated and travels back
  inside the submission.
- `content_hash` is computed over the normalised payload.
- Same `submission_id` and same `content_hash` means the identical file was
  re-imported: the import is idempotent and is reported as a duplicate.
- Same `submission_id` with a different `content_hash` means the participant
  sent a correction.
- `raw_payload` retains the external input verbatim for forensics and is never
  read as authority.

This table records idempotency, not audit. Import activity is still written to
`audit_logs`.

### 3. Participant claim status is derived, not stored

The Host UI needs a claim status per participant (ADR-0002). Storing it as a
column would create a second source of truth that drifts whenever a session is
created, expires or is revoked.

Claim status is derived from `participant_sessions`:

```text
UNCLAIMED  -- no session rows for the participant
PENDING    -- a session exists awaiting Host approval
CLAIMED    -- an active session exists (revoked_at IS NULL)
REVOKED    -- every session for the participant has revoked_at set
```

`ParticipantClaimStatus` in `packages/contracts` is therefore a view type
describing a computed value, not a stored column.

## Consequences

- The first migration creates `remote_submissions` alongside the tables listed
  in section 13.
- The derivation query must be expressible as a single query with explicit
  ordering so the Host participant list stays cheap.
- Export and audit can now state truthfully who produced each note version.
- Import gains a well-defined idempotency key, so re-importing a file the Host
  already processed is a detected duplicate rather than a silent second write.
