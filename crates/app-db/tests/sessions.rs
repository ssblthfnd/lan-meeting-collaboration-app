//! Join tokens and identity claims, against a real SQLite database.
//!
//! The rules under test are the ones ADR-0002 and ADR-0016 decide: one live
//! session per identity, decided by the database rather than by a check; a claim
//! that is usable the moment it exists; and a credential that is never stored in
//! plain text.
//!
//! Concurrency is exercised with real threads against one database. There are no
//! sleeps: the assertions are about what committed, not about timing.

use app_core::actor::Actor;
use app_core::error::DomainError;
use app_core::id::{MeetingId, ParticipantId, SessionId};
use app_core::meeting::{MeetingConfiguration, MeetingStatus};
use app_core::participant::ParticipantDetails;
use app_core::service::Domain;
use app_core::session::ClaimStatus;
use app_core::time::{MeetingDate, MeetingTime, MeetingTimeZone};
use app_core::token::TokenHash;
use app_db::participant_query::ParticipantQueries;
use app_db::session_store::SessionStore;
use app_db::{Db, Sql};
use rusqlite::params;
use std::sync::Arc;
use tempfile::TempDir;

/// A hash that is shaped like SHA-256 output without being one.
///
/// The transport does the real hashing; these tests only need distinct, valid
/// values, and building them here keeps `app-db` free of a crypto dependency.
fn hash(seed: u8) -> TokenHash {
    let text: String = (0..32).map(|i| format!("{:02x}", seed ^ i)).collect();
    TokenHash::parse(&text).expect("a valid digest shape")
}

struct World {
    db: Arc<Db>,
    domain: Domain<Arc<Db>>,
    _dir: TempDir,
}

impl World {
    fn new() -> Self {
        let dir = TempDir::new().expect("temp dir");
        let db = Arc::new(Db::open(dir.path().join("meeting.sqlite3")).expect("open database"));
        World {
            domain: Domain::new(Arc::clone(&db)),
            db,
            _dir: dir,
        }
    }

    fn queries(&self) -> ParticipantQueries<'_> {
        ParticipantQueries::new(&self.db)
    }

    fn sessions(&self) -> SessionStore<'_> {
        SessionStore::new(&self.db)
    }

    /// An open meeting with a roster, the normal starting point.
    fn open_meeting(&self, names: &[&str]) -> (MeetingId, Vec<ParticipantId>) {
        let meeting_id = self
            .domain
            .create_meeting(
                &Actor::Host,
                MeetingConfiguration {
                    title: "Weekly Coordination".to_owned(),
                    topic: None,
                    date: MeetingDate::new(2026, 9, 20).unwrap(),
                    start_time: MeetingTime::new(9, 0, 0).unwrap(),
                    end_time: MeetingTime::new(10, 30, 0).unwrap(),
                    timezone: MeetingTimeZone::new("Asia/Makassar").unwrap(),
                    location: None,
                    description: None,
                },
            )
            .expect("create meeting")
            .meeting_id;

        let participants = names
            .iter()
            .map(|name| {
                self.domain
                    .add_participant(
                        &Actor::Host,
                        meeting_id,
                        ParticipantDetails {
                            name: (*name).to_owned(),
                            department: Some("Finance".to_owned()),
                            position: Some("Analyst".to_owned()),
                            meeting_role: Some("Note taker".to_owned()),
                        },
                    )
                    .expect("add participant")
                    .participant_id
            })
            .collect();

        self.domain
            .open_meeting(&Actor::Host, meeting_id)
            .expect("open meeting");

        (meeting_id, participants)
    }

    fn claimant(&self, meeting_id: MeetingId, participant_id: ParticipantId) -> Actor {
        let _ = self;
        Actor::Claimant {
            meeting_id,
            participant_id,
        }
    }

    fn count(&self, sql: &str) -> i64 {
        self.db
            .read(|conn| Ok(conn.query_row(sql, [], |row| row.get(0))?))
            .expect("count")
    }

    fn claim_status(&self, meeting_id: MeetingId, participant_id: ParticipantId) -> ClaimStatus {
        self.queries()
            .claimable_identities(meeting_id)
            .expect("identities")
            .into_iter()
            .find(|identity| identity.id == participant_id)
            .expect("identity present")
            .claim_status
    }
}

// ---------------------------------------------------------------------------
// Join tokens
// ---------------------------------------------------------------------------

#[test]
fn only_the_hash_of_a_join_token_is_stored() {
    let w = World::new();
    let (meeting_id, _) = w.open_meeting(&["Budi Santoso"]);
    let token_hash = hash(1);

    w.domain
        .issue_join_token(&Actor::Host, meeting_id, &token_hash)
        .expect("issue");

    let stored: Option<String> =
        w.db.read(|conn| {
            Ok(conn.query_row(
                "SELECT join_token_hash FROM meetings WHERE id = ?1",
                params![Sql(meeting_id)],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(stored.as_deref(), Some(token_hash.as_str()));

    // And the audit record says a token was issued without recording it.
    let metadata: String =
        w.db.read(|conn| {
            Ok(conn.query_row(
                "SELECT metadata FROM audit_logs WHERE action = 'meeting.join_token_issued'",
                [],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert!(
        !metadata.contains(token_hash.as_str()),
        "a credential must not be written to the audit trail: {metadata}"
    );
    assert!(metadata.contains("replaced_previous"), "{metadata}");
}

#[test]
fn issuing_a_new_token_invalidates_the_previous_one() {
    let w = World::new();
    let (meeting_id, _) = w.open_meeting(&["Budi Santoso"]);

    let first = hash(1);
    let issued = w
        .domain
        .issue_join_token(&Actor::Host, meeting_id, &first)
        .expect("issue");
    assert!(
        !issued.replaced_previous,
        "nothing to replace the first time"
    );
    assert!(w.sessions().resolve_join_token(&first).unwrap().is_some());

    let second = hash(2);
    let reissued = w
        .domain
        .issue_join_token(&Actor::Host, meeting_id, &second)
        .expect("reissue");
    assert!(reissued.replaced_previous);

    // The old URL now resolves to nothing at all - rotation is the overwrite.
    assert_eq!(w.sessions().resolve_join_token(&first).unwrap(), None);
    let target = w.sessions().resolve_join_token(&second).unwrap().unwrap();
    assert_eq!(target.meeting_id, meeting_id);
}

#[test]
fn a_join_token_can_only_be_issued_for_an_open_meeting() {
    let w = World::new();

    // DRAFT: participants cannot join a meeting still being prepared.
    let draft = w
        .domain
        .create_meeting(
            &Actor::Host,
            MeetingConfiguration {
                title: "Weekly Coordination".to_owned(),
                topic: None,
                date: MeetingDate::new(2026, 9, 20).unwrap(),
                start_time: MeetingTime::new(9, 0, 0).unwrap(),
                end_time: MeetingTime::new(10, 30, 0).unwrap(),
                timezone: MeetingTimeZone::new("Asia/Makassar").unwrap(),
                location: None,
                description: None,
            },
        )
        .unwrap()
        .meeting_id;

    let err = w
        .domain
        .issue_join_token(&Actor::Host, draft, &hash(1))
        .unwrap_err();
    assert_eq!(
        err,
        DomainError::MeetingNotOpen {
            meeting_id: draft,
            detected: MeetingStatus::Draft,
        }
    );

    // LOCKED: the meeting is finished.
    let (locked, _) = w.open_meeting(&["Budi Santoso"]);
    w.domain.lock_meeting(&Actor::Host, locked).unwrap();
    let err = w
        .domain
        .issue_join_token(&Actor::Host, locked, &hash(2))
        .unwrap_err();
    assert!(matches!(err, DomainError::MeetingLocked { .. }), "{err:?}");
}

#[test]
fn locking_a_meeting_leaves_the_hash_but_the_status_closes_the_door() {
    // The mechanism ADR-0016 chose: the lifecycle check is what closes a join
    // URL, not clearing the column. Two mechanisms could disagree; one cannot.
    let w = World::new();
    let (meeting_id, _) = w.open_meeting(&["Budi Santoso"]);
    let token = hash(1);
    w.domain
        .issue_join_token(&Actor::Host, meeting_id, &token)
        .unwrap();

    w.domain.lock_meeting(&Actor::Host, meeting_id).unwrap();

    // The token still resolves to the meeting...
    let target = w.sessions().resolve_join_token(&token).unwrap().unwrap();
    assert_eq!(target.meeting_id, meeting_id);
    // ...and the status is what a caller must refuse on.
    assert_eq!(target.status, "LOCKED");
}

// ---------------------------------------------------------------------------
// Claiming: first-claim-wins
// ---------------------------------------------------------------------------

#[test]
fn a_claim_creates_one_live_unacknowledged_session() {
    let w = World::new();
    let (meeting_id, participants) = w.open_meeting(&["Budi Santoso"]);
    let budi = participants[0];
    let token = hash(9);

    let claimed = w
        .domain
        .claim_identity(&w.claimant(meeting_id, budi), meeting_id, budi, &token)
        .expect("claim");

    assert_eq!(claimed.participant_id, budi);
    assert_eq!(w.count("SELECT count(*) FROM participant_sessions"), 1);

    // Unacknowledged, and live.
    let (approved_at, revoked_at): (Option<String>, Option<String>) =
        w.db.read(|conn| {
            Ok(conn.query_row(
                "SELECT approved_at, revoked_at FROM participant_sessions",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?)
        })
        .unwrap();
    assert_eq!(approved_at, None, "a new claim is not acknowledged");
    assert_eq!(revoked_at, None, "a new claim is live");

    // Derived status says PENDING, which means joined and working.
    assert_eq!(w.claim_status(meeting_id, budi), ClaimStatus::Pending);
    assert!(ClaimStatus::Pending.is_held());

    // Audited as a participant action.
    let (action, actor_type, actor_id, target_type): (String, String, Option<String>, String) =
        w.db.read(|conn| {
            Ok(conn.query_row(
                "SELECT action, actor_type, actor_id, target_type
                   FROM audit_logs WHERE action = 'participant.claimed'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?)
        })
        .unwrap();
    assert_eq!(action, "participant.claimed");
    // A claimant is recorded as the participant it is - no fourth actor type,
    // and so no migration on the CHECK constraints (ADR-0016).
    assert_eq!(actor_type, "PARTICIPANT");
    assert_eq!(actor_id.as_deref(), Some(budi.to_storage().as_str()));
    assert_eq!(target_type, "session");
}

#[test]
fn only_the_hash_of_a_session_token_is_stored() {
    let w = World::new();
    let (meeting_id, participants) = w.open_meeting(&["Budi Santoso"]);
    let token = hash(9);

    w.domain
        .claim_identity(
            &w.claimant(meeting_id, participants[0]),
            meeting_id,
            participants[0],
            &token,
        )
        .unwrap();

    let stored: String =
        w.db.read(|conn| {
            Ok(conn.query_row(
                "SELECT session_token_hash FROM participant_sessions",
                [],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(stored, token.as_str());
    assert_eq!(stored.len(), 64);
}

#[test]
fn a_second_claim_on_a_live_identity_is_refused() {
    let w = World::new();
    let (meeting_id, participants) = w.open_meeting(&["Budi Santoso"]);
    let budi = participants[0];

    w.domain
        .claim_identity(&w.claimant(meeting_id, budi), meeting_id, budi, &hash(1))
        .expect("first claim");

    let err = w
        .domain
        .claim_identity(&w.claimant(meeting_id, budi), meeting_id, budi, &hash(2))
        .unwrap_err();

    assert_eq!(
        err,
        DomainError::IdentityAlreadyClaimed {
            meeting_id,
            participant_id: budi,
        }
    );
    assert_eq!(w.count("SELECT count(*) FROM participant_sessions"), 1);
    assert_eq!(
        w.count("SELECT count(*) FROM audit_logs WHERE action = 'participant.claimed'"),
        1,
        "a refused claim must not be audited"
    );
}

#[test]
fn an_acknowledged_identity_is_held_just_as_firmly_as_a_pending_one() {
    // ADR-0016: approval changes the label, not the hold.
    let w = World::new();
    let (meeting_id, participants) = w.open_meeting(&["Budi Santoso"]);
    let budi = participants[0];

    w.domain
        .claim_identity(&w.claimant(meeting_id, budi), meeting_id, budi, &hash(1))
        .unwrap();
    w.domain
        .approve_claim(&Actor::Host, meeting_id, budi)
        .unwrap();
    assert_eq!(w.claim_status(meeting_id, budi), ClaimStatus::Claimed);

    let err = w
        .domain
        .claim_identity(&w.claimant(meeting_id, budi), meeting_id, budi, &hash(2))
        .unwrap_err();
    assert!(
        matches!(err, DomainError::IdentityAlreadyClaimed { .. }),
        "{err:?}"
    );
}

#[test]
fn concurrent_claims_on_one_identity_leave_exactly_one_session() {
    let w = World::new();
    let (meeting_id, participants) = w.open_meeting(&["Budi Santoso"]);
    let budi = participants[0];

    // Enough browsers to overrun the limit if the check were outside the
    // transaction, or cached, or trusted from a caller.
    const CLAIMANTS: usize = 32;
    let mut outcomes = Vec::new();
    let domain = &w.domain;

    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for n in 0..CLAIMANTS {
            handles.push(scope.spawn(move || {
                let actor = Actor::Claimant {
                    meeting_id,
                    participant_id: budi,
                };
                domain.claim_identity(&actor, meeting_id, budi, &hash(n as u8))
            }));
        }
        for handle in handles {
            outcomes.push(handle.join().expect("claimant thread"));
        }
    });

    let mut winners = Vec::new();
    for outcome in outcomes {
        match outcome {
            Ok(claimed) => winners.push(claimed.session_id),
            // The only legitimate refusal in this race, whether it came from the
            // in-transaction read or from the partial unique index.
            Err(DomainError::IdentityAlreadyClaimed { .. }) => {}
            Err(other) => panic!("unexpected error while racing a claim: {other:?}"),
        }
    }

    assert_eq!(winners.len(), 1, "exactly one browser may win");
    assert_eq!(w.count("SELECT count(*) FROM participant_sessions"), 1);
    assert_eq!(
        w.count("SELECT count(*) FROM audit_logs WHERE action = 'participant.claimed'"),
        1
    );
}

#[test]
fn ninety_nine_participants_can_all_claim_at_once() {
    let w = World::new();
    let names: Vec<String> = (1..=99).map(|n| format!("Person {n}")).collect();
    let borrowed: Vec<&str> = names.iter().map(String::as_str).collect();
    let (meeting_id, participants) = w.open_meeting(&borrowed);

    let domain = &w.domain;
    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for (n, participant_id) in participants.iter().copied().enumerate() {
            handles.push(scope.spawn(move || {
                let actor = Actor::Claimant {
                    meeting_id,
                    participant_id,
                };
                domain.claim_identity(&actor, meeting_id, participant_id, &hash(n as u8))
            }));
        }
        for handle in handles {
            handle
                .join()
                .expect("thread")
                .expect("every claim is for a different identity");
        }
    });

    assert_eq!(w.count("SELECT count(*) FROM participant_sessions"), 99);
    // Every identity is held, by exactly one session each.
    for participant_id in participants {
        assert_eq!(
            w.claim_status(meeting_id, participant_id),
            ClaimStatus::Pending
        );
    }
}

// ---------------------------------------------------------------------------
// Claiming: what is refused
// ---------------------------------------------------------------------------

#[test]
fn an_identity_from_another_meeting_cannot_be_claimed() {
    let w = World::new();
    let (ours, _) = w.open_meeting(&["Insider"]);
    let (theirs, their_people) = w.open_meeting(&["Outsider"]);
    let outsider = their_people[0];

    // A claimant scoped to our meeting, naming their participant.
    let err = w
        .domain
        .claim_identity(&w.claimant(ours, outsider), ours, outsider, &hash(1))
        .unwrap_err();
    assert_eq!(
        err,
        DomainError::ParticipantNotFound {
            meeting_id: ours,
            participant_id: outsider,
        }
    );

    // And a claimant scoped elsewhere has no standing here at all.
    let err = w
        .domain
        .claim_identity(&w.claimant(theirs, outsider), ours, outsider, &hash(2))
        .unwrap_err();
    assert_eq!(err, DomainError::Unauthorized { meeting_id: ours });

    assert_eq!(w.count("SELECT count(*) FROM participant_sessions"), 0);
}

#[test]
fn a_claimant_cannot_claim_an_identity_other_than_its_own() {
    let w = World::new();
    let (meeting_id, participants) = w.open_meeting(&["Budi Santoso", "Siti Rahayu"]);
    let (budi, siti) = (participants[0], participants[1]);

    // The actor says Budi; the target says Siti. Refused.
    let err = w
        .domain
        .claim_identity(&w.claimant(meeting_id, budi), meeting_id, siti, &hash(1))
        .unwrap_err();
    assert!(matches!(err, DomainError::Forbidden { .. }), "{err:?}");
    assert_eq!(w.count("SELECT count(*) FROM participant_sessions"), 0);
}

#[test]
fn a_meeting_that_is_not_open_cannot_be_joined() {
    let w = World::new();
    let (meeting_id, participants) = w.open_meeting(&["Budi Santoso"]);
    let budi = participants[0];
    w.domain.lock_meeting(&Actor::Host, meeting_id).unwrap();

    let err = w
        .domain
        .claim_identity(&w.claimant(meeting_id, budi), meeting_id, budi, &hash(1))
        .unwrap_err();
    assert!(matches!(err, DomainError::MeetingLocked { .. }), "{err:?}");
    assert_eq!(w.count("SELECT count(*) FROM participant_sessions"), 0);
}

// ---------------------------------------------------------------------------
// Acknowledge and revoke
// ---------------------------------------------------------------------------

#[test]
fn approval_records_a_timestamp_and_changes_nothing_else() {
    let w = World::new();
    let (meeting_id, participants) = w.open_meeting(&["Budi Santoso"]);
    let budi = participants[0];
    let token = hash(1);

    let claimed = w
        .domain
        .claim_identity(&w.claimant(meeting_id, budi), meeting_id, budi, &token)
        .unwrap();

    // Before: resolvable, unacknowledged.
    let before = w.sessions().resolve_session(&token).unwrap().unwrap();
    assert!(!before.is_acknowledged());

    w.domain
        .approve_claim(&Actor::Host, meeting_id, budi)
        .unwrap();

    // After: the same session, the same identity, now acknowledged.
    let after = w.sessions().resolve_session(&token).unwrap().unwrap();
    assert!(after.is_acknowledged());
    assert_eq!(after.session_id, claimed.session_id);
    assert_eq!(after.participant_id, before.participant_id);
    assert_eq!(after.meeting_id, before.meeting_id);

    assert_eq!(w.claim_status(meeting_id, budi), ClaimStatus::Claimed);
    assert_eq!(
        w.count("SELECT count(*) FROM audit_logs WHERE action = 'participant.claim_approved'"),
        1
    );
}

#[test]
fn revoking_frees_the_identity_and_keeps_the_history() {
    let w = World::new();
    let (meeting_id, participants) = w.open_meeting(&["Budi Santoso"]);
    let budi = participants[0];
    let first_token = hash(1);

    w.domain
        .claim_identity(
            &w.claimant(meeting_id, budi),
            meeting_id,
            budi,
            &first_token,
        )
        .unwrap();
    w.domain
        .revoke_session(&Actor::Host, meeting_id, budi)
        .unwrap();

    // The revoked credential resolves to nothing.
    assert_eq!(w.sessions().resolve_session(&first_token).unwrap(), None);
    assert_eq!(w.claim_status(meeting_id, budi), ClaimStatus::Revoked);

    // The identity can be taken again.
    let second_token = hash(2);
    w.domain
        .claim_identity(
            &w.claimant(meeting_id, budi),
            meeting_id,
            budi,
            &second_token,
        )
        .expect("claimable again after a revoke");
    assert!(w
        .sessions()
        .resolve_session(&second_token)
        .unwrap()
        .is_some());

    // And the first session is still on record: history is not deleted.
    assert_eq!(w.count("SELECT count(*) FROM participant_sessions"), 2);
    assert_eq!(
        w.count("SELECT count(*) FROM participant_sessions WHERE revoked_at IS NOT NULL"),
        1
    );
    assert_eq!(
        w.count("SELECT count(*) FROM audit_logs WHERE action = 'participant.session_revoked'"),
        1
    );
}

#[test]
fn acting_on_a_session_that_is_not_there_is_refused() {
    let w = World::new();
    let (meeting_id, participants) = w.open_meeting(&["Budi Santoso"]);
    let budi = participants[0];

    for outcome in [
        w.domain.approve_claim(&Actor::Host, meeting_id, budi),
        w.domain.revoke_session(&Actor::Host, meeting_id, budi),
    ] {
        assert_eq!(
            outcome.unwrap_err(),
            DomainError::SessionNotFound { meeting_id }
        );
    }

    // And revoking twice refuses the second time, rather than silently
    // succeeding against an already-revoked row.
    w.domain
        .claim_identity(&w.claimant(meeting_id, budi), meeting_id, budi, &hash(1))
        .unwrap();
    w.domain
        .revoke_session(&Actor::Host, meeting_id, budi)
        .unwrap();
    assert!(w
        .domain
        .revoke_session(&Actor::Host, meeting_id, budi)
        .is_err());
}

#[test]
fn a_participant_cannot_revoke_or_approve_anyone() {
    let w = World::new();
    let (meeting_id, participants) = w.open_meeting(&["Budi Santoso", "Siti Rahayu"]);
    let (budi, siti) = (participants[0], participants[1]);

    let claimed = w
        .domain
        .claim_identity(&w.claimant(meeting_id, siti), meeting_id, siti, &hash(1))
        .unwrap();

    let budi_actor = Actor::Participant {
        meeting_id,
        participant_id: budi,
        session_id: SessionId::new(),
    };

    for outcome in [
        w.domain.revoke_session(&budi_actor, meeting_id, siti),
        w.domain.approve_claim(&budi_actor, meeting_id, siti),
        // Not even their own.
        w.domain.revoke_session(&budi_actor, meeting_id, budi),
    ] {
        assert!(
            matches!(outcome, Err(DomainError::Forbidden { .. })),
            "{outcome:?}"
        );
    }

    // Siti's session is untouched.
    assert!(w.sessions().resolve_session(&hash(1)).unwrap().is_some());
    assert_eq!(
        w.sessions()
            .resolve_session(&hash(1))
            .unwrap()
            .unwrap()
            .session_id,
        claimed.session_id
    );
}

// ---------------------------------------------------------------------------
// Credential resolution
// ---------------------------------------------------------------------------

#[test]
fn a_session_resolves_only_from_its_own_credential() {
    let w = World::new();
    let (meeting_id, participants) = w.open_meeting(&["Budi Santoso", "Siti Rahayu"]);
    let (budi, siti) = (participants[0], participants[1]);

    let budi_token = hash(1);
    let siti_token = hash(2);
    w.domain
        .claim_identity(&w.claimant(meeting_id, budi), meeting_id, budi, &budi_token)
        .unwrap();
    w.domain
        .claim_identity(&w.claimant(meeting_id, siti), meeting_id, siti, &siti_token)
        .unwrap();

    // Each credential resolves to its own identity, and only to it. There is no
    // parameter here through which a caller could ask to be someone else.
    let resolved = w.sessions().resolve_session(&budi_token).unwrap().unwrap();
    assert_eq!(resolved.participant_id, budi);
    assert_eq!(resolved.meeting_id, meeting_id);

    assert_eq!(
        w.sessions()
            .resolve_session(&siti_token)
            .unwrap()
            .unwrap()
            .participant_id,
        siti
    );

    // A credential nobody holds resolves to nothing.
    assert_eq!(w.sessions().resolve_session(&hash(200)).unwrap(), None);
}

#[test]
fn participant_facing_queries_withhold_details_until_the_claim_succeeds() {
    // ADR-0016 decision 6: before claiming, a name and whether it is taken.
    // Nothing else, and the shapes are what enforce it.
    let w = World::new();
    let (meeting_id, participants) = w.open_meeting(&["Budi Santoso"]);
    let budi = participants[0];

    let identities = w.queries().claimable_identities(meeting_id).unwrap();
    assert_eq!(identities.len(), 1);
    assert_eq!(identities[0].name, "Budi Santoso");
    // The struct has no field for department, position or role - there is
    // nowhere for them to leak to.

    // After claiming, the participant may see their own details in full.
    let own = w.queries().own_identity(meeting_id, budi).unwrap().unwrap();
    assert_eq!(own.details.department.as_deref(), Some("Finance"));
    assert_eq!(own.details.position.as_deref(), Some("Analyst"));

    // And a meeting view that carries none of the Host's extras.
    let meeting = w.queries().joinable_meeting(meeting_id).unwrap().unwrap();
    assert_eq!(meeting.title, "Weekly Coordination");
    assert_eq!(meeting.timezone, "Asia/Makassar");
    assert_eq!(meeting.status, MeetingStatus::Open);
}

#[test]
fn identities_are_listed_by_name_with_a_total_order() {
    let w = World::new();
    let (meeting_id, _) = w.open_meeting(&["Siti Rahayu", "Ahmad Fauzi", "Budi Santoso"]);

    let names: Vec<String> = w
        .queries()
        .claimable_identities(meeting_id)
        .unwrap()
        .into_iter()
        .map(|identity| identity.name)
        .collect();
    assert_eq!(names, ["Ahmad Fauzi", "Budi Santoso", "Siti Rahayu"]);
}

#[test]
fn a_join_token_that_matches_nothing_resolves_to_nothing() {
    let w = World::new();
    w.open_meeting(&["Budi Santoso"]);
    assert_eq!(w.sessions().resolve_join_token(&hash(250)).unwrap(), None);
}

// ---------------------------------------------------------------------------
// Atomicity
// ---------------------------------------------------------------------------

#[test]
fn a_refused_claim_leaves_no_session_and_no_audit_record() {
    let w = World::new();
    let (meeting_id, participants) = w.open_meeting(&["Budi Santoso"]);
    let budi = participants[0];
    let ghost = ParticipantId::new();

    let before = (
        w.count("SELECT count(*) FROM participant_sessions"),
        w.count("SELECT count(*) FROM audit_logs"),
    );

    let refusals = [
        // Not a participant of this meeting.
        w.domain
            .claim_identity(&w.claimant(meeting_id, ghost), meeting_id, ghost, &hash(1))
            .unwrap_err(),
        // Not this claimant's identity.
        w.domain
            .claim_identity(&w.claimant(meeting_id, ghost), meeting_id, budi, &hash(2))
            .unwrap_err(),
        // No such meeting.
        w.domain
            .claim_identity(
                &w.claimant(MeetingId::new(), budi),
                MeetingId::new(),
                budi,
                &hash(3),
            )
            .unwrap_err(),
    ];

    for err in &refusals {
        assert!(err.is_refusal(), "{err:?}");
    }
    assert_eq!(
        (
            w.count("SELECT count(*) FROM participant_sessions"),
            w.count("SELECT count(*) FROM audit_logs"),
        ),
        before
    );
}
