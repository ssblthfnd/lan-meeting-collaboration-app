//! Schema and migration tests.
//!
//! These exercise the guarantees the *database* makes, not the ones the
//! application intends to make. Each test tries to write something the schema
//! is supposed to refuse, and fails if SQLite accepts it. A rule that is only
//! enforced in Rust is a rule a future code path can skip, which is exactly
//! what architecture rules section 15 warns about.

use app_core::id::{
    AuditLogId, MeetingId, NoteId, NoteLinkId, NoteVersionId, ParticipantId, RemoteSubmissionId,
    SessionId, SubmissionId,
};
use app_core::time::{MeetingDate, MeetingTime, MeetingTimeZone, UtcTimestamp};
use app_db::{Db, DbResult, Sql};
use rusqlite::params;
use tempfile::TempDir;

/// A fresh migrated database in a temporary directory.
struct Fixture {
    db: Db,
    // Held so the directory outlives the database.
    _dir: TempDir,
}

fn fixture() -> Fixture {
    let dir = TempDir::new().expect("temp dir");
    let db = Db::open(dir.path().join("meeting.sqlite3")).expect("open database");
    Fixture { db, _dir: dir }
}

/// A 64-character lowercase hex string, standing in for a SHA-256 digest.
fn digest(seed: u8) -> String {
    (0..32).map(|i| format!("{:02x}", seed ^ i)).collect()
}

fn insert_meeting(db: &Db, id: MeetingId, timezone: &str) -> DbResult<()> {
    let now = UtcTimestamp::now();
    db.write(|tx| {
        tx.execute(
            "INSERT INTO meetings
               (id, title, topic, date, start_time, end_time, timezone,
                location, description, status, join_token_hash,
                created_at, updated_at, locked_at)
             VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?6, NULL, NULL, 'OPEN', NULL, ?7, ?7, NULL)",
            params![
                Sql(id),
                "Weekly Coordination",
                Sql(MeetingDate::new(2026, 9, 20).unwrap()),
                Sql(MeetingTime::new(9, 0, 0).unwrap()),
                Sql(MeetingTime::new(10, 30, 0).unwrap()),
                timezone,
                Sql(now),
            ],
        )?;
        Ok(())
    })
}

fn insert_participant(
    db: &Db,
    meeting_id: MeetingId,
    id: ParticipantId,
    name: &str,
) -> DbResult<()> {
    db.write(|tx| {
        tx.execute(
            "INSERT INTO participants
               (id, meeting_id, name, department, position, meeting_role, created_at)
             VALUES (?1, ?2, ?3, NULL, NULL, NULL, ?4)",
            params![Sql(id), Sql(meeting_id), name, Sql(UtcTimestamp::now())],
        )?;
        Ok(())
    })
}

fn insert_note(
    db: &Db,
    id: NoteId,
    meeting_id: MeetingId,
    participant_id: ParticipantId,
) -> DbResult<()> {
    let now = UtcTimestamp::now();
    db.write(|tx| {
        tx.execute(
            "INSERT INTO notes (id, meeting_id, participant_id, content, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            params![
                Sql(id),
                Sql(meeting_id),
                Sql(participant_id),
                "## Agenda\n\nDiscussed the budget.",
                Sql(now),
            ],
        )?;
        Ok(())
    })
}

/// A meeting with one participant, the starting point for most tests.
fn seeded() -> (Fixture, MeetingId, ParticipantId) {
    let f = fixture();
    let meeting_id = MeetingId::new();
    let participant_id = ParticipantId::new();
    insert_meeting(&f.db, meeting_id, "Asia/Makassar").unwrap();
    insert_participant(&f.db, meeting_id, participant_id, "Budi Santoso").unwrap();
    (f, meeting_id, participant_id)
}

// ---------------------------------------------------------------------------
// Migration
// ---------------------------------------------------------------------------

#[test]
fn migration_creates_every_table_on_a_fresh_database() {
    let f = fixture();

    let mut tables =
        f.db.read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT name FROM sqlite_master
                 WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
                   AND name NOT LIKE 'refinery_%'
                 ORDER BY name",
            )?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .unwrap();
    tables.sort();

    assert_eq!(
        tables,
        vec![
            "audit_logs",
            "meetings",
            "note_links",
            "note_versions",
            "notes",
            "participant_sessions",
            "participants",
            "remote_submissions",
        ]
    );
}

#[test]
fn migration_is_idempotent_and_records_its_version() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meeting.sqlite3");

    let first = Db::open(&path).unwrap();
    drop(first);

    // Re-opening an already migrated database must not fail or re-apply.
    let second = Db::open(&path).unwrap();
    let applied: i64 = second
        .read(|conn| {
            Ok(
                conn.query_row("SELECT count(*) FROM refinery_schema_history", [], |row| {
                    row.get(0)
                })?,
            )
        })
        .unwrap();

    assert_eq!(applied, 1);
    assert_eq!(app_db::migrations::embedded_versions(), vec![1]);
}

#[test]
fn a_migrated_database_passes_sqlites_own_integrity_check() {
    let f = fixture();
    assert_eq!(f.db.integrity_check().unwrap(), "ok");
}

// ---------------------------------------------------------------------------
// PRAGMAs
// ---------------------------------------------------------------------------

#[test]
fn required_pragmas_are_active_on_writer_and_reader() {
    let f = fixture();

    let writer_fk: i64 =
        f.db.with_writer(|conn| Ok(conn.pragma_query_value(None, "foreign_keys", |r| r.get(0))?))
            .unwrap();
    assert_eq!(writer_fk, 1, "foreign_keys must be ON for the writer");

    let reader_fk: i64 =
        f.db.read(|conn| Ok(conn.pragma_query_value(None, "foreign_keys", |r| r.get(0))?))
            .unwrap();
    assert_eq!(reader_fk, 1, "foreign_keys must be ON for pooled readers");

    let journal: String =
        f.db.with_writer(|conn| Ok(conn.pragma_query_value(None, "journal_mode", |r| r.get(0))?))
            .unwrap();
    assert_eq!(journal.to_lowercase(), "wal");
}

#[test]
fn pooled_connections_are_read_only() {
    // Writes must go through `Db::write`; the read path cannot be used to
    // sneak one in.
    let (f, meeting_id, _) = seeded();

    let result = f.db.read(|conn| {
        conn.execute(
            "DELETE FROM meetings WHERE id = ?1",
            params![Sql(meeting_id)],
        )?;
        Ok(())
    });

    assert!(result.is_err(), "a pooled read connection accepted a write");
}

// ---------------------------------------------------------------------------
// Identifiers
// ---------------------------------------------------------------------------

#[test]
fn uuid_v7_ids_round_trip_through_sqlite_unchanged() {
    let (f, meeting_id, participant_id) = seeded();

    let (read_meeting, read_participant) =
        f.db.read(|conn| {
            Ok(conn.query_row(
                "SELECT meeting_id, id FROM participants WHERE id = ?1",
                params![Sql(participant_id)],
                |row| {
                    Ok((
                        row.get::<_, Sql<MeetingId>>(0)?,
                        row.get::<_, Sql<ParticipantId>>(1)?,
                    ))
                },
            )?)
        })
        .unwrap();

    assert_eq!(read_meeting.into_inner(), meeting_id);
    assert_eq!(read_participant.into_inner(), participant_id);
}

#[test]
fn ids_are_stored_as_canonical_36_character_lowercase_text() {
    let (f, meeting_id, _) = seeded();

    let (stored, length, kind): (String, i64, String) =
        f.db.read(|conn| {
            Ok(conn.query_row(
                "SELECT id, length(id), typeof(id) FROM meetings WHERE id = ?1",
                params![Sql(meeting_id)],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?)
        })
        .unwrap();

    assert_eq!(length, 36);
    assert_eq!(kind, "text");
    assert_eq!(stored, stored.to_lowercase());
    assert_eq!(stored, meeting_id.to_storage());
    // The version nibble, at the start of the third group.
    assert_eq!(&stored[14..15], "7");
}

#[test]
fn the_database_rejects_an_id_that_is_not_a_uuid_v7() {
    let f = fixture();
    let now = UtcTimestamp::now();

    // A syntactically valid UUIDv4. Rust would refuse to build this id, so the
    // test goes around the type system to prove the database refuses it too.
    let insert = |id: &str| -> DbResult<()> {
        f.db.write(|tx| {
            tx.execute(
                "INSERT INTO meetings
                   (id, title, date, start_time, end_time, timezone, status,
                    created_at, updated_at)
                 VALUES (?1, 'T', '2026-09-20', '09:00:00', '10:00:00',
                         'Asia/Makassar', 'OPEN', ?2, ?2)",
                params![id, Sql(now)],
            )?;
            Ok(())
        })
    };

    let v4 = insert("9f8b7c6d-5e4f-4a3b-8c9d-0e1f2a3b4c5d").unwrap_err();
    assert!(v4.is_constraint_violation(), "v4 uuid accepted: {v4}");

    let arbitrary = insert("meeting-1").unwrap_err();
    assert!(arbitrary.is_constraint_violation(), "{arbitrary}");

    let uppercase = insert("0199C7E1-1111-7111-8111-111111111111").unwrap_err();
    assert!(uppercase.is_constraint_violation(), "{uppercase}");
}

// ---------------------------------------------------------------------------
// Time and timezone
// ---------------------------------------------------------------------------

#[test]
fn timestamps_round_trip_consistently() {
    let (f, meeting_id, _) = seeded();

    let (created, raw): (Sql<UtcTimestamp>, String) =
        f.db.read(|conn| {
            Ok(conn.query_row(
                "SELECT created_at, created_at FROM meetings WHERE id = ?1",
                params![Sql(meeting_id)],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?)
        })
        .unwrap();

    assert_eq!(raw.len(), 24, "stored timestamp must be fixed width: {raw}");
    assert!(raw.ends_with('Z'), "stored timestamp must be UTC: {raw}");
    assert_eq!(created.into_inner().to_storage(), raw);
}

#[test]
fn the_database_rejects_a_timestamp_that_is_not_utc() {
    let f = fixture();

    // A local-time value with an offset must not be storable in a column the
    // architecture says is UTC (section 26.1).
    let result = f.db.write(|tx| {
        tx.execute(
            "INSERT INTO meetings
               (id, title, date, start_time, end_time, timezone, status,
                created_at, updated_at)
             VALUES (?1, 'T', '2026-09-20', '09:00:00', '10:00:00',
                     'Asia/Makassar', 'OPEN', '2026-09-20T09:00:00+08:00',
                     '2026-09-20T09:00:00+08:00')",
            params![Sql(MeetingId::new())],
        )?;
        Ok(())
    });

    assert!(
        result
            .as_ref()
            .is_err_and(app_db::DbError::is_constraint_violation),
        "a non-UTC timestamp was accepted: {result:?}"
    );
}

#[test]
fn meeting_iana_timezone_values_persist_exactly() {
    // All three Indonesian zones plus a DST zone, since the meaning of the
    // stored schedule depends on this string being preserved verbatim.
    for zone in [
        "Asia/Jakarta",
        "Asia/Makassar",
        "Asia/Jayapura",
        "America/New_York",
    ] {
        let f = fixture();
        let meeting_id = MeetingId::new();
        insert_meeting(&f.db, meeting_id, zone).unwrap();

        let stored: String =
            f.db.read(|conn| {
                Ok(conn.query_row(
                    "SELECT timezone FROM meetings WHERE id = ?1",
                    params![Sql(meeting_id)],
                    |row| row.get(0),
                )?)
            })
            .unwrap();

        assert_eq!(stored, zone);
        // And it is still resolvable as a real zone after the round trip.
        assert_eq!(MeetingTimeZone::new(&stored).unwrap().name(), zone);
    }
}

#[test]
fn a_meeting_schedule_is_stored_zoneless_and_read_in_its_own_timezone() {
    let f = fixture();
    let meeting_id = MeetingId::new();
    insert_meeting(&f.db, meeting_id, "Asia/Makassar").unwrap();

    let (date, start, timezone): (Sql<MeetingDate>, Sql<MeetingTime>, String) =
        f.db.read(|conn| {
            Ok(conn.query_row(
                "SELECT date, start_time, timezone FROM meetings WHERE id = ?1",
                params![Sql(meeting_id)],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?)
        })
        .unwrap();

    // 09:00 in Asia/Makassar (UTC+8) is 01:00Z. The stored date and time carry
    // no offset of their own; the meeting's timezone supplies it.
    let tz = MeetingTimeZone::new(&timezone).unwrap();
    let instant = tz.resolve(date.into_inner(), start.into_inner()).unwrap();
    assert_eq!(instant.to_storage(), "2026-09-20T01:00:00.000Z");
}

// ---------------------------------------------------------------------------
// Referential integrity
// ---------------------------------------------------------------------------

#[test]
fn foreign_keys_are_enforced() {
    let f = fixture();

    // No such meeting.
    let orphan = f.db.write(|tx| {
        tx.execute(
            "INSERT INTO participants (id, meeting_id, name, created_at)
             VALUES (?1, ?2, 'Ghost', ?3)",
            params![
                Sql(ParticipantId::new()),
                Sql(MeetingId::new()),
                Sql(UtcTimestamp::now())
            ],
        )?;
        Ok(())
    });

    assert!(
        orphan
            .as_ref()
            .is_err_and(app_db::DbError::is_constraint_violation),
        "an orphan participant was accepted: {orphan:?}"
    );
}

#[test]
fn a_note_cannot_borrow_a_participant_from_another_meeting() {
    // The composite foreign key is what makes "participant belongs to meeting"
    // (architecture rules section 9) a database guarantee rather than a
    // validation step someone might forget.
    let f = fixture();

    let meeting_a = MeetingId::new();
    let meeting_b = MeetingId::new();
    insert_meeting(&f.db, meeting_a, "Asia/Makassar").unwrap();
    insert_meeting(&f.db, meeting_b, "Asia/Jakarta").unwrap();

    let participant_in_b = ParticipantId::new();
    insert_participant(&f.db, meeting_b, participant_in_b, "Siti").unwrap();

    // Claim meeting A owns a participant that actually belongs to meeting B.
    let result = insert_note(&f.db, NoteId::new(), meeting_a, participant_in_b);

    assert!(
        result
            .as_ref()
            .is_err_and(app_db::DbError::is_constraint_violation),
        "a cross-meeting note was accepted: {result:?}"
    );
}

#[test]
fn deleting_a_meeting_cascades_to_its_participants_and_notes() {
    let (f, meeting_id, participant_id) = seeded();
    insert_note(&f.db, NoteId::new(), meeting_id, participant_id).unwrap();

    f.db.write(|tx| {
        tx.execute(
            "DELETE FROM meetings WHERE id = ?1",
            params![Sql(meeting_id)],
        )?;
        Ok(())
    })
    .unwrap();

    let remaining: i64 =
        f.db.read(|conn| {
            Ok(conn.query_row(
                "SELECT (SELECT count(*) FROM participants) + (SELECT count(*) FROM notes)",
                [],
                |row| row.get(0),
            )?)
        })
        .unwrap();

    assert_eq!(remaining, 0);
}

// ---------------------------------------------------------------------------
// Note cardinality (ADR-0003)
// ---------------------------------------------------------------------------

#[test]
fn a_participant_may_have_only_one_note_per_meeting() {
    let (f, meeting_id, participant_id) = seeded();

    insert_note(&f.db, NoteId::new(), meeting_id, participant_id).unwrap();
    let second = insert_note(&f.db, NoteId::new(), meeting_id, participant_id);

    assert!(
        second
            .as_ref()
            .is_err_and(app_db::DbError::is_constraint_violation),
        "a second note was accepted: {second:?}"
    );
}

#[test]
fn note_versions_accumulate_for_the_same_note() {
    let (f, meeting_id, participant_id) = seeded();
    let note_id = NoteId::new();
    insert_note(&f.db, note_id, meeting_id, participant_id).unwrap();

    // v1 by the participant, v2 by the Host, v3 by a remote import: the three
    // actor kinds section 18 requires history to distinguish.
    let versions = [
        (1, "PARTICIPANT", Some(participant_id)),
        (2, "HOST", None),
        (3, "REMOTE_IMPORT", Some(participant_id)),
    ];

    for (version, created_by_type, created_by) in versions {
        f.db.write(|tx| {
            tx.execute(
                "INSERT INTO note_versions
                   (id, note_id, version, content, created_at, created_by_type, created_by)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    Sql(NoteVersionId::new()),
                    Sql(note_id),
                    version,
                    format!("content v{version}"),
                    Sql(UtcTimestamp::now()),
                    created_by_type,
                    created_by.map(Sql),
                ],
            )?;
            Ok(())
        })
        .unwrap();
    }

    let ordered: Vec<(i64, String)> =
        f.db.read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT version, created_by_type FROM note_versions
                 WHERE note_id = ?1 ORDER BY version ASC",
            )?;
            let rows = stmt
                .query_map(params![Sql(note_id)], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .unwrap();

    assert_eq!(
        ordered,
        vec![
            (1, "PARTICIPANT".to_owned()),
            (2, "HOST".to_owned()),
            (3, "REMOTE_IMPORT".to_owned()),
        ]
    );

    // The same version number cannot be recorded twice.
    let duplicate = f.db.write(|tx| {
        tx.execute(
            "INSERT INTO note_versions
               (id, note_id, version, content, created_at, created_by_type, created_by)
             VALUES (?1, ?2, 2, 'again', ?3, 'HOST', NULL)",
            params![
                Sql(NoteVersionId::new()),
                Sql(note_id),
                Sql(UtcTimestamp::now())
            ],
        )?;
        Ok(())
    });
    assert!(duplicate
        .as_ref()
        .is_err_and(app_db::DbError::is_constraint_violation));
}

#[test]
fn a_host_note_version_carries_no_participant_and_others_must() {
    let (f, meeting_id, participant_id) = seeded();
    let note_id = NoteId::new();
    insert_note(&f.db, note_id, meeting_id, participant_id).unwrap();

    let insert = |version: i64, kind: &str, by: Option<ParticipantId>| {
        f.db.write(|tx| {
            tx.execute(
                "INSERT INTO note_versions
                   (id, note_id, version, content, created_at, created_by_type, created_by)
                 VALUES (?1, ?2, ?3, 'c', ?4, ?5, ?6)",
                params![
                    Sql(NoteVersionId::new()),
                    Sql(note_id),
                    version,
                    Sql(UtcTimestamp::now()),
                    kind,
                    by.map(Sql),
                ],
            )?;
            Ok(())
        })
    };

    // A Host edit that names a participant is incoherent, and so is a
    // participant edit that names nobody (ADR-0008).
    assert!(insert(1, "HOST", Some(participant_id))
        .as_ref()
        .is_err_and(app_db::DbError::is_constraint_violation));
    assert!(insert(2, "PARTICIPANT", None)
        .as_ref()
        .is_err_and(app_db::DbError::is_constraint_violation));

    assert!(insert(3, "HOST", None).is_ok());
    assert!(insert(4, "PARTICIPANT", Some(participant_id)).is_ok());
}

#[test]
fn note_history_cannot_be_rewritten() {
    let (f, meeting_id, participant_id) = seeded();
    let note_id = NoteId::new();
    insert_note(&f.db, note_id, meeting_id, participant_id).unwrap();

    f.db.write(|tx| {
        tx.execute(
            "INSERT INTO note_versions
               (id, note_id, version, content, created_at, created_by_type, created_by)
             VALUES (?1, ?2, 1, 'original', ?3, 'HOST', NULL)",
            params![
                Sql(NoteVersionId::new()),
                Sql(note_id),
                Sql(UtcTimestamp::now())
            ],
        )?;
        Ok(())
    })
    .unwrap();

    let rewrite = f.db.write(|tx| {
        tx.execute(
            "UPDATE note_versions SET content = 'rewritten' WHERE note_id = ?1",
            params![Sql(note_id)],
        )?;
        Ok(())
    });

    assert!(rewrite.is_err(), "note history was rewritten");
    assert!(rewrite
        .unwrap_err()
        .to_string()
        .contains("UPDATE is not permitted"));
}

// ---------------------------------------------------------------------------
// Links
// ---------------------------------------------------------------------------

#[test]
fn a_note_may_carry_at_most_five_links() {
    let (f, meeting_id, participant_id) = seeded();
    let note_id = NoteId::new();
    insert_note(&f.db, note_id, meeting_id, participant_id).unwrap();

    let add_link = |n: usize| {
        f.db.write(|tx| {
            tx.execute(
                "INSERT INTO note_links (id, note_id, title, description, url, created_at, updated_at)
                 VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?5)",
                params![
                    Sql(NoteLinkId::new()),
                    Sql(note_id),
                    format!("Link {n}"),
                    format!("https://example.invalid/{n}"),
                    Sql(UtcTimestamp::now()),
                ],
            )?;
            Ok(())
        })
    };

    for n in 1..=5 {
        add_link(n).unwrap_or_else(|e| panic!("link {n} should be allowed: {e}"));
    }

    let sixth = add_link(6);
    assert!(sixth.is_err(), "a sixth link was accepted");
    assert!(sixth.unwrap_err().to_string().contains("at most 5 links"));
}

#[test]
fn link_schemes_are_restricted_to_http_https_and_mailto() {
    let (f, meeting_id, participant_id) = seeded();
    let note_id = NoteId::new();
    insert_note(&f.db, note_id, meeting_id, participant_id).unwrap();

    let add = |url: &str| {
        f.db.write(|tx| {
            tx.execute(
                "INSERT INTO note_links (id, note_id, url, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?4)",
                params![
                    Sql(NoteLinkId::new()),
                    Sql(note_id),
                    url,
                    Sql(UtcTimestamp::now())
                ],
            )?;
            Ok(())
        })
    };

    assert!(add("https://example.invalid/a").is_ok());
    assert!(add("http://example.invalid/b").is_ok());
    assert!(add("mailto:someone@example.invalid").is_ok());

    // The scheme allowlist exists because a link can arrive from a submission
    // file, not only from the editor (ADR-0007).
    for hostile in [
        "javascript:alert(1)",
        "data:text/html;base64,PHNjcmlwdD4=",
        "file:///C:/Windows/System32",
    ] {
        let rejected = add(hostile);
        assert!(
            rejected
                .as_ref()
                .is_err_and(app_db::DbError::is_constraint_violation),
            "{hostile} was accepted"
        );
    }
}

// ---------------------------------------------------------------------------
// Sessions and derived claim status (ADR-0002, ADR-0008)
// ---------------------------------------------------------------------------

#[test]
fn only_one_live_session_may_hold_a_participant_identity() {
    let (f, meeting_id, participant_id) = seeded();

    let claim = |token: u8| {
        f.db.write(|tx| {
            tx.execute(
                "INSERT INTO participant_sessions
                   (id, meeting_id, participant_id, session_token_hash, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    Sql(SessionId::new()),
                    Sql(meeting_id),
                    Sql(participant_id),
                    digest(token),
                    Sql(UtcTimestamp::now()),
                ],
            )?;
            Ok(())
        })
    };

    claim(1).unwrap();

    // First-claim-wins: a second browser cannot take a live identity.
    let second = claim(2);
    assert!(
        second
            .as_ref()
            .is_err_and(app_db::DbError::is_constraint_violation),
        "a second live session was accepted: {second:?}"
    );

    // After a revoke the identity is claimable again, and the revoked row stays.
    f.db.write(|tx| {
        tx.execute(
            "UPDATE participant_sessions SET revoked_at = ?1 WHERE participant_id = ?2",
            params![Sql(UtcTimestamp::now()), Sql(participant_id)],
        )?;
        Ok(())
    })
    .unwrap();

    claim(3).unwrap();

    let total: i64 =
        f.db.read(|conn| {
            Ok(
                conn.query_row("SELECT count(*) FROM participant_sessions", [], |r| {
                    r.get(0)
                })?,
            )
        })
        .unwrap();
    assert_eq!(total, 2, "session history must be retained");
}

#[test]
fn claim_status_is_derived_from_sessions_and_is_not_a_column() {
    let (f, meeting_id, participant_id) = seeded();

    // There is no claim status column to read; it must be computed (ADR-0008).
    let columns: Vec<String> =
        f.db.read(|conn| {
            let mut stmt = conn.prepare("SELECT name FROM pragma_table_info('participants')")?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .unwrap();
    assert!(
        !columns
            .iter()
            .any(|c| c.contains("claim") || c.contains("status")),
        "participants must not store claim status: {columns:?}"
    );

    // The derivation, as one query with explicit ordering (section 26.4).
    const DERIVE: &str = "
        SELECT CASE
                 WHEN s.total = 0 THEN 'UNCLAIMED'
                 WHEN s.live_approved > 0 THEN 'CLAIMED'
                 WHEN s.live_pending > 0 THEN 'PENDING'
                 ELSE 'REVOKED'
               END
        FROM participants p
        JOIN (SELECT
                  count(*) AS total,
                  sum(CASE WHEN revoked_at IS NULL AND approved_at IS NOT NULL
                           THEN 1 ELSE 0 END) AS live_approved,
                  sum(CASE WHEN revoked_at IS NULL AND approved_at IS NULL
                           THEN 1 ELSE 0 END) AS live_pending
              FROM participant_sessions
              WHERE participant_id = ?1) s
        WHERE p.id = ?1
        ORDER BY p.name, p.id";

    let status = |f: &Fixture| -> String {
        f.db.read(
            |conn| Ok(conn.query_row(DERIVE, params![Sql(participant_id)], |row| row.get(0))?),
        )
        .unwrap()
    };

    assert_eq!(status(&f), "UNCLAIMED");

    let session_id = SessionId::new();
    f.db.write(|tx| {
        tx.execute(
            "INSERT INTO participant_sessions
               (id, meeting_id, participant_id, session_token_hash, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                Sql(session_id),
                Sql(meeting_id),
                Sql(participant_id),
                digest(7),
                Sql(UtcTimestamp::now()),
            ],
        )?;
        Ok(())
    })
    .unwrap();
    assert_eq!(status(&f), "PENDING");

    f.db.write(|tx| {
        tx.execute(
            "UPDATE participant_sessions SET approved_at = ?1 WHERE id = ?2",
            params![Sql(UtcTimestamp::now()), Sql(session_id)],
        )?;
        Ok(())
    })
    .unwrap();
    assert_eq!(status(&f), "CLAIMED");

    f.db.write(|tx| {
        tx.execute(
            "UPDATE participant_sessions SET revoked_at = ?1 WHERE id = ?2",
            params![Sql(UtcTimestamp::now()), Sql(session_id)],
        )?;
        Ok(())
    })
    .unwrap();
    assert_eq!(status(&f), "REVOKED");
}

#[test]
fn a_session_token_is_stored_only_as_a_64_character_hash() {
    let (f, meeting_id, participant_id) = seeded();

    let store = |token: &str| {
        f.db.write(|tx| {
            tx.execute(
                "INSERT INTO participant_sessions
                   (id, meeting_id, participant_id, session_token_hash, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    Sql(SessionId::new()),
                    Sql(meeting_id),
                    Sql(participant_id),
                    token,
                    Sql(UtcTimestamp::now()),
                ],
            )?;
            Ok(())
        })
    };

    // A raw token is not 64 hex characters, so it cannot be stored by accident.
    let raw = store("this-looks-like-a-plaintext-session-token");
    assert!(raw
        .as_ref()
        .is_err_and(app_db::DbError::is_constraint_violation));
}

// ---------------------------------------------------------------------------
// Remote submissions (ADR-0008)
// ---------------------------------------------------------------------------

#[test]
fn re_importing_an_identical_submission_is_a_detected_duplicate() {
    let (f, meeting_id, participant_id) = seeded();
    let submission_id = SubmissionId::new();
    let content_hash = digest(9);

    let record = |resolution: &str, note_version: Option<i64>, hash: &str| {
        f.db.write(|tx| {
            tx.execute(
                "INSERT INTO remote_submissions
                   (id, meeting_id, participant_id, submission_id, content_hash,
                    imported_at, resolution, note_version, raw_payload)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    Sql(RemoteSubmissionId::new()),
                    Sql(meeting_id),
                    Sql(participant_id),
                    Sql(submission_id),
                    hash,
                    Sql(UtcTimestamp::now()),
                    resolution,
                    note_version,
                    r#"{"schema_version":1}"#,
                ],
            )?;
            Ok(())
        })
    };

    record("IMPORTED", Some(1), &content_hash).unwrap();

    // The identical file again: same submission_id, same content_hash.
    let again = record("IMPORTED", Some(2), &content_hash);
    assert!(
        again
            .as_ref()
            .is_err_and(app_db::DbError::is_constraint_violation),
        "the same submission was written twice: {again:?}"
    );

    // A correction from the same participant: same submission_id, new hash.
    record("REPLACED", Some(2), &digest(10)).unwrap();

    let count: i64 =
        f.db.read(|conn| {
            Ok(conn.query_row(
                "SELECT count(*) FROM remote_submissions WHERE submission_id = ?1",
                params![Sql(submission_id)],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(
        count, 2,
        "a correction must be distinguishable from a duplicate"
    );
}

#[test]
fn a_rejected_submission_records_no_note_version() {
    let (f, meeting_id, participant_id) = seeded();

    let record = |resolution: &str, note_version: Option<i64>| {
        f.db.write(|tx| {
            tx.execute(
                "INSERT INTO remote_submissions
                   (id, meeting_id, participant_id, submission_id, content_hash,
                    imported_at, resolution, note_version, raw_payload)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'not even json')",
                params![
                    Sql(RemoteSubmissionId::new()),
                    Sql(meeting_id),
                    Sql(participant_id),
                    Sql(SubmissionId::new()),
                    digest(11),
                    Sql(UtcTimestamp::now()),
                    resolution,
                    note_version,
                ],
            )?;
            Ok(())
        })
    };

    // A rejected submission that claims to have produced a version, and an
    // imported one that claims it did not, are both incoherent.
    assert!(record("REJECTED_INVALID", Some(1))
        .as_ref()
        .is_err_and(app_db::DbError::is_constraint_violation));
    assert!(record("IMPORTED", None)
        .as_ref()
        .is_err_and(app_db::DbError::is_constraint_violation));

    // A malformed payload is still retained verbatim for forensics.
    assert!(record("REJECTED_INVALID", None).is_ok());
}

// ---------------------------------------------------------------------------
// Audit (section 17)
// ---------------------------------------------------------------------------

#[test]
fn audit_rows_cannot_be_updated_or_deleted() {
    let (f, meeting_id, _) = seeded();
    let audit_id = AuditLogId::new();

    f.db.write(|tx| {
        tx.execute(
            "INSERT INTO audit_logs
               (id, meeting_id, actor_type, actor_id, action, target_type, target_id,
                metadata, created_at)
             VALUES (?1, ?2, 'HOST', NULL, 'meeting.created', 'meeting', ?3, ?4, ?5)",
            params![
                Sql(audit_id),
                Sql(meeting_id),
                Sql(meeting_id),
                r#"{"title":"Weekly Coordination"}"#,
                Sql(UtcTimestamp::now()),
            ],
        )?;
        Ok(())
    })
    .unwrap();

    let updated = f.db.write(|tx| {
        tx.execute(
            "UPDATE audit_logs SET action = 'tampered' WHERE id = ?1",
            params![Sql(audit_id)],
        )?;
        Ok(())
    });
    assert!(updated.is_err());
    assert!(updated.unwrap_err().to_string().contains("append-only"));

    let deleted = f.db.write(|tx| {
        tx.execute(
            "DELETE FROM audit_logs WHERE id = ?1",
            params![Sql(audit_id)],
        )?;
        Ok(())
    });
    assert!(deleted.is_err());
    assert!(deleted.unwrap_err().to_string().contains("append-only"));
}

#[test]
fn audit_metadata_must_be_valid_json_when_present() {
    let (f, meeting_id, _) = seeded();

    let write = |metadata: Option<&str>| {
        f.db.write(|tx| {
            tx.execute(
                "INSERT INTO audit_logs
                   (id, meeting_id, actor_type, action, target_type, metadata, created_at)
                 VALUES (?1, ?2, 'HOST', 'meeting.locked', 'meeting', ?3, ?4)",
                params![
                    Sql(AuditLogId::new()),
                    Sql(meeting_id),
                    metadata,
                    Sql(UtcTimestamp::now()),
                ],
            )?;
            Ok(())
        })
    };

    assert!(write(Some(r#"{"reason":"end of meeting"}"#)).is_ok());
    assert!(write(None).is_ok());
    assert!(write(Some("{not json"))
        .as_ref()
        .is_err_and(app_db::DbError::is_constraint_violation));
}

// ---------------------------------------------------------------------------
// Transactions
// ---------------------------------------------------------------------------

#[test]
fn a_failed_write_leaves_nothing_behind() {
    // Import must be all-or-nothing (architecture rules section 11).
    let (f, meeting_id, participant_id) = seeded();

    let result: DbResult<()> = f.db.write(|tx| {
        tx.execute(
            "INSERT INTO notes (id, meeting_id, participant_id, content, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'first', ?4, ?4)",
            params![
                Sql(NoteId::new()),
                Sql(meeting_id),
                Sql(participant_id),
                Sql(UtcTimestamp::now())
            ],
        )?;
        // Same participant again: rejected by the ADR-0003 constraint, which
        // must roll back the insert above too.
        tx.execute(
            "INSERT INTO notes (id, meeting_id, participant_id, content, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'second', ?4, ?4)",
            params![
                Sql(NoteId::new()),
                Sql(meeting_id),
                Sql(participant_id),
                Sql(UtcTimestamp::now())
            ],
        )?;
        Ok(())
    });

    assert!(result.is_err());

    let notes: i64 =
        f.db.read(|conn| Ok(conn.query_row("SELECT count(*) FROM notes", [], |r| r.get(0))?))
            .unwrap();
    assert_eq!(notes, 0, "a partial write survived a failed transaction");
}

#[test]
fn a_locked_meeting_must_carry_a_lock_time() {
    let f = fixture();
    let meeting_id = MeetingId::new();
    insert_meeting(&f.db, meeting_id, "Asia/Makassar").unwrap();

    // LOCKED without locked_at would make "is it locked?" answerable two ways.
    let inconsistent = f.db.write(|tx| {
        tx.execute(
            "UPDATE meetings SET status = 'LOCKED' WHERE id = ?1",
            params![Sql(meeting_id)],
        )?;
        Ok(())
    });
    assert!(inconsistent
        .as_ref()
        .is_err_and(app_db::DbError::is_constraint_violation));

    let consistent = f.db.write(|tx| {
        tx.execute(
            "UPDATE meetings SET status = 'LOCKED', locked_at = ?1, updated_at = ?1
             WHERE id = ?2",
            params![Sql(UtcTimestamp::now()), Sql(meeting_id)],
        )?;
        Ok(())
    });
    assert!(consistent.is_ok());
}
