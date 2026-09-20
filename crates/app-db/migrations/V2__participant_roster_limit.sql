-- Participant roster limit: at most 99 per meeting (PRD section 7).
--
-- The domain counts the roster inside the mutating transaction and refuses the
-- addition that would exceed the limit. This is the second enforcement point,
-- for the reason ADR-0011 gives: a rule that lives only in Rust is a rule some
-- future code path can skip. The five-link cap is enforced the same way, and for
-- the same reason.
--
-- A CHECK constraint cannot count sibling rows, so this is a trigger.
--
-- Separate migration rather than an edit to V1: refinery records a checksum for
-- an applied migration and refuses a file that changed afterwards. A correction
-- is always a new V{n}.

CREATE TRIGGER participants_max_per_meeting_insert
BEFORE INSERT ON participants
WHEN (SELECT count(*) FROM participants WHERE meeting_id = NEW.meeting_id) >= 99
BEGIN
    SELECT RAISE(ABORT, 'participants: a meeting may have at most 99 participants');
END;

-- Moving a participant between meetings is not a flow the application offers,
-- but the limit must hold against it too: otherwise the cap could be exceeded
-- by an UPDATE instead of an INSERT.
CREATE TRIGGER participants_max_per_meeting_update
BEFORE UPDATE OF meeting_id ON participants
WHEN NEW.meeting_id <> OLD.meeting_id
 AND (SELECT count(*) FROM participants WHERE meeting_id = NEW.meeting_id) >= 99
BEGIN
    SELECT RAISE(ABORT, 'participants: a meeting may have at most 99 participants');
END;
