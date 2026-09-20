-- Initial schema.
--
-- Implements the tables in architecture rules section 13, with the refinements
-- accepted in ADR-0003 (one note per participant), ADR-0005 (explicit
-- timezone), ADR-0008 (created_by_type, remote_submissions, derived claim
-- status) and ADR-0011 (storage formats and database-enforced invariants).
--
-- Conventions used throughout, all enforced rather than documented-only:
--
--   id            TEXT, canonical lowercase UUIDv7, 36 chars. The GLOB also
--                 pins the version nibble to '7' and the variant to [89ab], so
--                 a v4 uuid or an arbitrary string cannot be stored as an id.
--                 UUIDv7 is time-ordered, so `ORDER BY ... , id` is a stable
--                 and meaningful total order (section 26.4).
--   timestamps    TEXT, 'YYYY-MM-DDTHH:MM:SS.sssZ', always UTC (section 26.1).
--                 Fixed width, so lexicographic order is chronological order.
--                 The trailing 'Z' is part of the CHECK: a local-time value
--                 cannot be written into a system timestamp column.
--   date / time   TEXT, 'YYYY-MM-DD' and 'HH:MM:SS', deliberately zoneless.
--                 They mean nothing without meetings.timezone (section 26.2).
--
-- PRAGMAs (foreign_keys, WAL, busy_timeout, synchronous) are per-connection and
-- are set in `pool.rs`, not here.

-- ---------------------------------------------------------------------------
-- meetings
-- ---------------------------------------------------------------------------
CREATE TABLE meetings (
    id              TEXT NOT NULL PRIMARY KEY
                    CHECK (id GLOB '[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-7[0-9a-f][0-9a-f][0-9a-f]-[89ab][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]'),

    title           TEXT NOT NULL CHECK (length(trim(title)) > 0),
    topic           TEXT,

    -- Read in `timezone`, never in the OS timezone (section 26.2).
    date            TEXT NOT NULL
                    CHECK (date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]'),
    start_time      TEXT NOT NULL
                    CHECK (start_time GLOB '[0-9][0-9]:[0-9][0-9]:[0-9][0-9]'),
    end_time        TEXT NOT NULL
                    CHECK (end_time GLOB '[0-9][0-9]:[0-9][0-9]:[0-9][0-9]'),

    -- IANA identifier, e.g. 'Asia/Makassar'. Required: a meeting whose
    -- timezone is unknown has an ambiguous schedule (ADR-0005).
    timezone        TEXT NOT NULL CHECK (length(trim(timezone)) > 0),

    location        TEXT,
    description     TEXT,

    status          TEXT NOT NULL CHECK (status IN ('DRAFT', 'OPEN', 'LOCKED')),

    -- Only ever a hash; the join token itself is never stored (PRD 22.2).
    join_token_hash TEXT UNIQUE
                    CHECK (join_token_hash IS NULL
                           OR (length(join_token_hash) = 64
                               AND NOT join_token_hash GLOB '*[^0-9a-f]*')),

    created_at      TEXT NOT NULL
                    CHECK (created_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
    updated_at      TEXT NOT NULL
                    CHECK (updated_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
    locked_at       TEXT
                    CHECK (locked_at IS NULL
                           OR locked_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),

    CHECK (end_time >= start_time),
    CHECK (updated_at >= created_at),
    -- A locked meeting has a lock time and an unlocked one does not. Without
    -- this, "is it locked?" would have two answers that could disagree.
    CHECK ((status = 'LOCKED') = (locked_at IS NOT NULL))
);

-- ---------------------------------------------------------------------------
-- participants
-- ---------------------------------------------------------------------------
CREATE TABLE participants (
    id           TEXT NOT NULL PRIMARY KEY
                 CHECK (id GLOB '[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-7[0-9a-f][0-9a-f][0-9a-f]-[89ab][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]'),

    meeting_id   TEXT NOT NULL
                 REFERENCES meetings (id) ON DELETE CASCADE ON UPDATE RESTRICT,

    name         TEXT NOT NULL CHECK (length(trim(name)) > 0),
    department   TEXT,
    position     TEXT,
    meeting_role TEXT,

    created_at   TEXT NOT NULL
                 CHECK (created_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),

    -- Redundant given the primary key, but it gives child tables a composite
    -- foreign key target. That is what lets the database prove "this
    -- participant belongs to this meeting" (architecture rules section 9)
    -- instead of trusting the writer to have checked.
    UNIQUE (meeting_id, id)
);

-- Host participant list: ordered by name with id as the tiebreaker (26.4).
CREATE INDEX idx_participants_meeting_name ON participants (meeting_id, name, id);

-- ---------------------------------------------------------------------------
-- participant_sessions
--
-- Claim status is NOT a column here. It is derived from these rows (ADR-0008):
--
--   UNCLAIMED  no rows for the participant
--   PENDING    a row with approved_at IS NULL AND revoked_at IS NULL
--   CLAIMED    a row with approved_at IS NOT NULL AND revoked_at IS NULL
--   REVOKED    rows exist and every one has revoked_at set
--
-- `approved_at` is not in the section 13 column list, but PENDING cannot be
-- distinguished from CLAIMED without it, and ADR-0008 requires that
-- distinction to be derivable from this table. See ADR-0011.
-- ---------------------------------------------------------------------------
CREATE TABLE participant_sessions (
    id                 TEXT NOT NULL PRIMARY KEY
                       CHECK (id GLOB '[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-7[0-9a-f][0-9a-f][0-9a-f]-[89ab][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]'),

    meeting_id         TEXT NOT NULL,
    participant_id     TEXT NOT NULL,

    -- Only the hash is stored (PRD 5.2.1 item 9, 22.17).
    session_token_hash TEXT NOT NULL UNIQUE
                       CHECK (length(session_token_hash) = 64
                              AND NOT session_token_hash GLOB '*[^0-9a-f]*'),

    created_at         TEXT NOT NULL
                       CHECK (created_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
    approved_at        TEXT
                       CHECK (approved_at IS NULL
                              OR approved_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
    last_seen_at       TEXT
                       CHECK (last_seen_at IS NULL
                              OR last_seen_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
    revoked_at         TEXT
                       CHECK (revoked_at IS NULL
                              OR revoked_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),

    CHECK (approved_at IS NULL OR approved_at >= created_at),
    CHECK (revoked_at IS NULL OR revoked_at >= created_at),

    FOREIGN KEY (meeting_id, participant_id)
        REFERENCES participants (meeting_id, id)
        ON DELETE CASCADE ON UPDATE RESTRICT
);

-- First-claim-wins, enforced by the database (ADR-0002, section 14.1 rule 2).
-- A partial index: any number of revoked sessions may exist in history, but at
-- most one may be live at a time. A race between two browsers claiming the same
-- identity is decided here, not by application-level checking.
CREATE UNIQUE INDEX idx_sessions_one_live_per_participant
    ON participant_sessions (meeting_id, participant_id)
    WHERE revoked_at IS NULL;

CREATE INDEX idx_sessions_participant
    ON participant_sessions (meeting_id, participant_id, created_at, id);

-- ---------------------------------------------------------------------------
-- notes
-- ---------------------------------------------------------------------------
CREATE TABLE notes (
    id             TEXT NOT NULL PRIMARY KEY
                   CHECK (id GLOB '[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-7[0-9a-f][0-9a-f][0-9a-f]-[89ab][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]'),

    meeting_id     TEXT NOT NULL,
    participant_id TEXT NOT NULL,

    -- GFM-subset Markdown as text (ADR-0007, section 13.1).
    content        TEXT NOT NULL,

    created_at     TEXT NOT NULL
                   CHECK (created_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
    updated_at     TEXT NOT NULL
                   CHECK (updated_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),

    CHECK (updated_at >= created_at),

    -- ADR-0003. This is the primary enforcement of note cardinality, not a
    -- convention: remote import is an upsert precisely because this constraint
    -- makes a second note impossible.
    UNIQUE (meeting_id, participant_id),

    FOREIGN KEY (meeting_id, participant_id)
        REFERENCES participants (meeting_id, id)
        ON DELETE CASCADE ON UPDATE RESTRICT
);

CREATE INDEX idx_notes_meeting ON notes (meeting_id, participant_id);

-- ---------------------------------------------------------------------------
-- note_links
-- ---------------------------------------------------------------------------
CREATE TABLE note_links (
    id          TEXT NOT NULL PRIMARY KEY
                CHECK (id GLOB '[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-7[0-9a-f][0-9a-f][0-9a-f]-[89ab][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]'),

    note_id     TEXT NOT NULL
                REFERENCES notes (id) ON DELETE CASCADE ON UPDATE RESTRICT,

    title       TEXT,
    description TEXT,

    -- Link scheme allowlist (section 13.1, ADR-0007). Enforced here as well as
    -- in the editor, because a link can also arrive from a submission file.
    url         TEXT NOT NULL
                CHECK (url LIKE 'http://%'
                       OR url LIKE 'https://%'
                       OR url LIKE 'mailto:%'),

    created_at  TEXT NOT NULL
                CHECK (created_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
    updated_at  TEXT NOT NULL
                CHECK (updated_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),

    CHECK (updated_at >= created_at)
);

CREATE INDEX idx_note_links_note ON note_links (note_id, created_at, id);

-- Maximum 5 links per note (PRD section 14). A CHECK constraint cannot count
-- sibling rows, so this is a trigger. The limit is a hard invariant, and the
-- PRD requires the backend to enforce it; enforcing it in the database means a
-- direct write or a future code path cannot quietly exceed it.
CREATE TRIGGER note_links_max_per_note_insert
BEFORE INSERT ON note_links
WHEN (SELECT count(*) FROM note_links WHERE note_id = NEW.note_id) >= 5
BEGIN
    SELECT RAISE(ABORT, 'note_links: a note may have at most 5 links');
END;

CREATE TRIGGER note_links_max_per_note_update
BEFORE UPDATE OF note_id ON note_links
WHEN NEW.note_id <> OLD.note_id
 AND (SELECT count(*) FROM note_links WHERE note_id = NEW.note_id) >= 5
BEGIN
    SELECT RAISE(ABORT, 'note_links: a note may have at most 5 links');
END;

-- ---------------------------------------------------------------------------
-- note_versions
-- ---------------------------------------------------------------------------
CREATE TABLE note_versions (
    id              TEXT NOT NULL PRIMARY KEY
                    CHECK (id GLOB '[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-7[0-9a-f][0-9a-f][0-9a-f]-[89ab][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]'),

    note_id         TEXT NOT NULL
                    REFERENCES notes (id) ON DELETE CASCADE ON UPDATE RESTRICT,

    version         INTEGER NOT NULL CHECK (version >= 1),
    content         TEXT NOT NULL,

    created_at      TEXT NOT NULL
                    CHECK (created_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),

    -- ADR-0008: `created_by` alone cannot say "the Host edited this".
    created_by_type TEXT NOT NULL
                    CHECK (created_by_type IN ('HOST', 'PARTICIPANT', 'REMOTE_IMPORT')),
    created_by      TEXT
                    REFERENCES participants (id) ON DELETE CASCADE ON UPDATE RESTRICT,

    -- Host edits carry no participant; participant and import edits must.
    CHECK ((created_by_type = 'HOST') = (created_by IS NULL)),

    -- Version numbers are dense and unique per note, so history is an
    -- unambiguous sequence and `ORDER BY version` is total (section 26.4).
    UNIQUE (note_id, version)
);

CREATE INDEX idx_note_versions_note ON note_versions (note_id, version);

-- History is never rewritten (section 18.1). Rows may still disappear when the
-- note itself is deleted by cascade, which is a deliberate destructive act on
-- the parent, not an edit of history.
CREATE TRIGGER note_versions_immutable
BEFORE UPDATE ON note_versions
BEGIN
    SELECT RAISE(ABORT, 'note_versions is history: UPDATE is not permitted');
END;

-- ---------------------------------------------------------------------------
-- audit_logs
-- ---------------------------------------------------------------------------
CREATE TABLE audit_logs (
    id          TEXT NOT NULL PRIMARY KEY
                CHECK (id GLOB '[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-7[0-9a-f][0-9a-f][0-9a-f]-[89ab][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]'),

    -- RESTRICT, not CASCADE: an audited meeting cannot be deleted out from
    -- under its audit trail. MVP has no meeting-deletion flow, so nothing
    -- needs this to cascade (see ADR-0011).
    meeting_id  TEXT NOT NULL
                REFERENCES meetings (id) ON DELETE RESTRICT ON UPDATE RESTRICT,

    actor_type  TEXT NOT NULL
                CHECK (actor_type IN ('HOST', 'PARTICIPANT', 'REMOTE_IMPORT')),

    -- Deliberately not a foreign key. The audit record of "participant removed"
    -- must survive the participant it describes.
    actor_id    TEXT,

    action      TEXT NOT NULL CHECK (length(trim(action)) > 0),
    target_type TEXT NOT NULL CHECK (length(trim(target_type)) > 0),
    target_id   TEXT,

    metadata    TEXT CHECK (metadata IS NULL OR json_valid(metadata)),

    created_at  TEXT NOT NULL
                CHECK (created_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),

    CHECK ((actor_type = 'HOST') = (actor_id IS NULL))
);

CREATE INDEX idx_audit_logs_meeting_time ON audit_logs (meeting_id, created_at, id);

-- Append-only (section 17, PRD 22.11). Not "the UI does not offer it": the
-- database refuses.
CREATE TRIGGER audit_logs_no_update
BEFORE UPDATE ON audit_logs
BEGIN
    SELECT RAISE(ABORT, 'audit_logs is append-only: UPDATE is not permitted');
END;

CREATE TRIGGER audit_logs_no_delete
BEFORE DELETE ON audit_logs
BEGIN
    SELECT RAISE(ABORT, 'audit_logs is append-only: DELETE is not permitted');
END;

-- ---------------------------------------------------------------------------
-- remote_submissions
--
-- Idempotency ledger, not audit (ADR-0008). Import activity is still written
-- to audit_logs.
-- ---------------------------------------------------------------------------
CREATE TABLE remote_submissions (
    id             TEXT NOT NULL PRIMARY KEY
                   CHECK (id GLOB '[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-7[0-9a-f][0-9a-f][0-9a-f]-[89ab][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]'),

    meeting_id     TEXT NOT NULL,
    participant_id TEXT NOT NULL,

    -- Minted when the remote form is generated; travels back in the file.
    submission_id  TEXT NOT NULL
                   CHECK (submission_id GLOB '[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-7[0-9a-f][0-9a-f][0-9a-f]-[89ab][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]'),

    -- SHA-256 over the normalised payload, lowercase hex.
    content_hash   TEXT NOT NULL
                   CHECK (length(content_hash) = 64
                          AND NOT content_hash GLOB '*[^0-9a-f]*'),

    imported_at    TEXT NOT NULL
                   CHECK (imported_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),

    resolution     TEXT NOT NULL
                   CHECK (resolution IN ('IMPORTED', 'REPLACED',
                                         'REJECTED_DUPLICATE', 'REJECTED_INVALID')),

    -- The note version this submission produced, or NULL when it produced none.
    note_version   INTEGER CHECK (note_version IS NULL OR note_version >= 1),

    -- The external input, verbatim, for forensics. Never read as authority
    -- (section 13, section 10), and deliberately not required to be valid JSON:
    -- a rejected malformed payload is exactly what is worth keeping.
    raw_payload    TEXT NOT NULL,

    -- A submission that was written produced a version; one that was rejected
    -- did not.
    CHECK ((resolution IN ('IMPORTED', 'REPLACED')) = (note_version IS NOT NULL)),

    -- ADR-0008 idempotency key. Same submission_id + same content_hash means
    -- the identical file was re-imported; same submission_id with a different
    -- hash is a correction from the same participant.
    UNIQUE (meeting_id, participant_id, submission_id, content_hash),

    FOREIGN KEY (meeting_id, participant_id)
        REFERENCES participants (meeting_id, id)
        ON DELETE CASCADE ON UPDATE RESTRICT
);

CREATE INDEX idx_remote_submissions_meeting_time
    ON remote_submissions (meeting_id, imported_at, id);

CREATE INDEX idx_remote_submissions_identity
    ON remote_submissions (meeting_id, participant_id, submission_id);
