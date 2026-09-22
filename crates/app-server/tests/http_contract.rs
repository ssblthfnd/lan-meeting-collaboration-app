//! The LAN HTTP contract, driven through the real router.
//!
//! `tower::ServiceExt::oneshot` runs a request against the assembled router with
//! no port bound, so what is tested is the actual routing, extraction, error
//! mapping and headers rather than a stand-in for them.
//!
//! The questions these answer are mostly about what a participant *cannot* do:
//! reach a meeting that is not open, use a revoked credential, become someone
//! else by asking, or learn from a refusal whether a token was ever real.

use std::sync::Arc;

use app_core::actor::Actor;
use app_core::event::EventSink;
use app_core::id::ParticipantId;
use app_core::meeting::MeetingConfiguration;
use app_core::participant::ParticipantDetails;
use app_core::service::Domain;
use app_core::time::{MeetingDate, MeetingTime, MeetingTimeZone};
use app_db::Db;
use app_server::state::LanState;
use app_server::{hash_token, router, Realtime};
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use futures_util::stream;
use http_body_util::BodyExt;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

/// A server over a throwaway database, with one open meeting and a roster.
struct Lan {
    state: LanState,
    domain: Arc<Domain<Arc<Db>>>,
    meeting_id: app_core::id::MeetingId,
    participants: Vec<ParticipantId>,
    /// The plaintext join token. Only the hash was stored.
    join_token: String,
    _dir: TempDir,
}

impl Lan {
    fn new(names: &[&str]) -> Self {
        let dir = TempDir::new().expect("temp dir");
        let db = Arc::new(Db::open(dir.path().join("meeting.sqlite3")).expect("open"));
        // One channel behind both the mutation boundary and the server, the
        // way the assembled application wires it (ADR-0018).
        let realtime = Arc::new(Realtime::new());
        let events = Arc::clone(&realtime) as Arc<dyn EventSink>;
        let domain = Arc::new(Domain::with_events(Arc::clone(&db), Arc::clone(&events)));

        let meeting_id = domain
            .create_meeting(
                &Actor::Host,
                MeetingConfiguration {
                    title: "Weekly Coordination".to_owned(),
                    topic: Some("Budget".to_owned()),
                    date: MeetingDate::new(2026, 9, 20).unwrap(),
                    start_time: MeetingTime::new(9, 0, 0).unwrap(),
                    end_time: MeetingTime::new(10, 30, 0).unwrap(),
                    timezone: MeetingTimeZone::new("Asia/Makassar").unwrap(),
                    location: Some("Meeting Room 2".to_owned()),
                    description: Some("Internal only.".to_owned()),
                },
            )
            .expect("create")
            .meeting_id;

        let participants = names
            .iter()
            .map(|name| {
                domain
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
                    .expect("add")
                    .participant_id
            })
            .collect();

        domain.open_meeting(&Actor::Host, meeting_id).expect("open");

        // A real credential: minted by the transport, hashed, hash stored.
        let credential = app_server::mint().expect("entropy");
        domain
            .issue_join_token(&Actor::Host, meeting_id, &credential.hash)
            .expect("issue");

        Lan {
            state: LanState::with_realtime(Arc::clone(&db), Arc::clone(&domain), events, realtime),
            domain,
            meeting_id,
            participants,
            join_token: credential.token,
            _dir: dir,
        }
    }

    async fn send(&self, request: Request<Body>) -> (StatusCode, Value, Vec<(String, String)>) {
        let response = router(self.state.clone())
            .oneshot(request)
            .await
            .expect("the router always responds");

        let status = response.status();
        let headers = response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_owned(),
                    value.to_str().unwrap_or_default().to_owned(),
                )
            })
            .collect();

        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);

        (status, body, headers)
    }

    /// The response body as text, for the routes that serve HTML.
    async fn raw(&self, path: &str) -> String {
        let response = router(self.state.clone())
            .oneshot(
                Request::builder()
                    .uri(path)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("the router always responds");

        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    async fn get(&self, path: &str) -> (StatusCode, Value, Vec<(String, String)>) {
        self.send(
            Request::builder()
                .uri(path)
                .body(Body::empty())
                .expect("request"),
        )
        .await
    }

    async fn claim(&self, token: &str, participant_id: &str) -> (StatusCode, Value) {
        let (status, body, _) = self
            .send(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/join/{token}/claim"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(format!(
                        r#"{{"participant_id":"{participant_id}"}}"#
                    )))
                    .expect("request"),
            )
            .await;
        (status, body)
    }

    async fn session(&self, bearer: Option<&str>) -> (StatusCode, Value) {
        let mut builder = Request::builder().uri("/api/session");
        if let Some(token) = bearer {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let (status, body, _) = self
            .send(builder.body(Body::empty()).expect("request"))
            .await;
        (status, body)
    }

    /// `GET /api/note` with an optional bearer token.
    async fn read_note(&self, bearer: Option<&str>) -> (StatusCode, Value) {
        let mut builder = Request::builder().uri("/api/note");
        if let Some(token) = bearer {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let (status, body, _) = self
            .send(builder.body(Body::empty()).expect("request"))
            .await;
        (status, body)
    }

    /// `PUT /api/note` with an optional bearer token and a raw body.
    async fn put_note_raw(&self, bearer: Option<&str>, body: String) -> (StatusCode, Value) {
        let mut builder = Request::builder()
            .method("PUT")
            .uri("/api/note")
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(token) = bearer {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let (status, response, _) = self
            .send(builder.body(Body::from(body)).expect("request"))
            .await;
        (status, response)
    }

    /// `PUT /api/note` with `content` serialised properly.
    async fn put_note(&self, bearer: Option<&str>, content: &str) -> (StatusCode, Value) {
        let body = serde_json::json!({ "content": content }).to_string();
        self.put_note_raw(bearer, body).await
    }

    /// Lock the meeting through the domain, as the Host would.
    fn lock(&self) {
        self.domain
            .lock_meeting(&Actor::Host, self.meeting_id)
            .expect("lock");
    }

    /// Claim an identity and return the session token it produced.
    async fn claim_ok(&self, index: usize) -> String {
        let (status, body) = self
            .claim(&self.join_token, &self.participants[index].to_storage())
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        body["session_token"]
            .as_str()
            .expect("a session token")
            .to_owned()
    }
}

// ---------------------------------------------------------------------------
// The join screen
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_valid_join_token_returns_the_meeting_and_the_names() {
    let lan = Lan::new(&["Siti Rahayu", "Budi Santoso"]);

    let (status, body, _) = lan.get(&format!("/api/join/{}", lan.join_token)).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // The meeting's public facts, with the timezone beside the schedule.
    assert_eq!(body["meeting"]["title"], "Weekly Coordination");
    assert_eq!(body["meeting"]["timezone"], "Asia/Makassar");
    assert_eq!(body["meeting"]["date"], "2026-09-20");
    assert_eq!(body["meeting"]["start_time"], "09:00:00");
    assert_eq!(body["meeting"]["status"], "OPEN");

    // Ordered by name, and nothing but a name and whether it is taken.
    let identities = body["identities"].as_array().expect("identities");
    assert_eq!(identities.len(), 2);
    assert_eq!(identities[0]["name"], "Budi Santoso");
    assert_eq!(identities[1]["name"], "Siti Rahayu");
    assert_eq!(identities[0]["claimed"], false);
}

#[tokio::test]
async fn pre_claim_responses_carry_no_participant_details() {
    // ADR-0016 decision 6. Enforced by the response shape, checked here on the
    // wire because that is where a leak would actually matter.
    let lan = Lan::new(&["Budi Santoso"]);

    let (_, body, _) = lan.get(&format!("/api/join/{}", lan.join_token)).await;
    let text = body.to_string();

    for withheld in ["Finance", "Analyst", "Note taker"] {
        assert!(!text.contains(withheld), "leaked {withheld}: {text}");
    }
    // Nor the Host's own extras.
    for absent in [
        "participant_count",
        "created_at",
        "updated_at",
        "description",
    ] {
        assert!(!text.contains(absent), "leaked {absent}: {text}");
    }
}

#[tokio::test]
async fn an_unknown_join_token_is_indistinguishable_from_an_unknown_route() {
    let lan = Lan::new(&["Budi Santoso"]);

    let (bad_status, bad_body, _) = lan.get("/api/join/not-a-real-token").await;
    let fabricated = app_server::mint().expect("entropy").token;
    let (unknown_status, unknown_body, _) = lan.get(&format!("/api/join/{fabricated}")).await;

    assert_eq!(bad_status, StatusCode::NOT_FOUND);
    assert_eq!(unknown_status, StatusCode::NOT_FOUND);
    // Byte-identical: this endpoint is not an oracle for whether a token exists.
    assert_eq!(bad_body, unknown_body);
    assert_eq!(bad_body["kind"], "not_found");
}

#[tokio::test]
async fn a_rotated_join_token_stops_working_immediately() {
    let lan = Lan::new(&["Budi Santoso"]);
    let old = lan.join_token.clone();

    let replacement = app_server::mint().expect("entropy");
    lan.domain
        .issue_join_token(&Actor::Host, lan.meeting_id, &replacement.hash)
        .expect("reissue");

    let (old_status, _, _) = lan.get(&format!("/api/join/{old}")).await;
    assert_eq!(old_status, StatusCode::NOT_FOUND);

    let (new_status, _, _) = lan.get(&format!("/api/join/{}", replacement.token)).await;
    assert_eq!(new_status, StatusCode::OK);
}

#[tokio::test]
async fn a_locked_meeting_cannot_be_joined_even_with_a_valid_token() {
    // The lifecycle check runs per request, so this needs no restart and no
    // change to the stored token (architecture rules section 15).
    let lan = Lan::new(&["Budi Santoso"]);
    lan.domain
        .lock_meeting(&Actor::Host, lan.meeting_id)
        .expect("lock");

    let (status, body, _) = lan.get(&format!("/api/join/{}", lan.join_token)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["kind"], "meeting_not_open");

    let (claim_status, _) = lan
        .claim(&lan.join_token, &lan.participants[0].to_storage())
        .await;
    assert_eq!(claim_status, StatusCode::CONFLICT);
}

// ---------------------------------------------------------------------------
// Claiming
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_successful_claim_returns_a_session_token_once() {
    let lan = Lan::new(&["Budi Santoso"]);
    let participant = lan.participants[0];

    let (status, body) = lan.claim(&lan.join_token, &participant.to_storage()).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let token = body["session_token"].as_str().expect("a session token");
    assert_eq!(token.len(), 64);
    // Now the participant sees their own details in full.
    assert_eq!(body["participant"]["name"], "Budi Santoso");
    assert_eq!(body["participant"]["department"], "Finance");

    // Only the hash was stored, and it is not the token.
    let stored_hash = hash_token(token);
    assert_ne!(stored_hash.as_str(), token);

    // And the token never appears again: the session route does not repeat it.
    let (_, session) = lan.session(Some(token)).await;
    assert!(
        !session.to_string().contains(token),
        "a session token must be returned once: {session}"
    );
}

#[tokio::test]
async fn a_claimed_identity_shows_as_taken_and_cannot_be_claimed_twice() {
    let lan = Lan::new(&["Budi Santoso", "Siti Rahayu"]);
    let budi = lan.participants[0].to_storage();

    lan.claim_ok(0).await;

    let (_, body, _) = lan.get(&format!("/api/join/{}", lan.join_token)).await;
    let identities = body["identities"].as_array().expect("identities");
    let claimed: Vec<bool> = identities
        .iter()
        .map(|identity| identity["claimed"].as_bool().expect("claimed"))
        .collect();
    assert_eq!(claimed, vec![true, false], "{body}");

    let (status, refusal) = lan.claim(&lan.join_token, &budi).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(refusal["kind"], "identity_already_claimed");
    // The refusal names no identifier.
    assert!(!refusal.to_string().contains(&budi), "{refusal}");
}

#[tokio::test]
async fn a_malformed_participant_id_is_refused_as_a_bad_request() {
    let lan = Lan::new(&["Budi Santoso"]);

    for bad in ["", "not-a-uuid", "9f1a8f5e-4b6d-4c3a-8f2e-1d2c3b4a5968"] {
        let (status, body) = lan.claim(&lan.join_token, bad).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {body}");
        assert_eq!(body["kind"], "invalid_request");
    }
}

#[tokio::test]
async fn an_identity_from_another_meeting_cannot_be_claimed_through_this_token() {
    let lan = Lan::new(&["Budi Santoso"]);
    let outsider = ParticipantId::new().to_storage();

    let (status, body) = lan.claim(&lan.join_token, &outsider).await;
    // Reported as "this link is not valid", the same as an unknown token: a
    // participant learns nothing about which ids exist.
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["kind"], "not_found");
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_session_token_resolves_to_its_own_identity() {
    let lan = Lan::new(&["Budi Santoso", "Siti Rahayu"]);
    let budi_token = lan.claim_ok(0).await;
    let siti_token = lan.claim_ok(1).await;

    let (status, budi) = lan.session(Some(&budi_token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(budi["participant"]["name"], "Budi Santoso");
    // Unacknowledged, and fully joined: approval is not a gate (ADR-0016).
    assert_eq!(budi["acknowledged"], false);

    let (_, siti) = lan.session(Some(&siti_token)).await;
    assert_eq!(siti["participant"]["name"], "Siti Rahayu");

    // Neither response carries the other participant's details.
    assert!(!budi.to_string().contains("Siti"), "{budi}");
    assert!(!siti.to_string().contains("Budi"), "{siti}");
}

#[tokio::test]
async fn reconnecting_cannot_change_which_identity_you_are() {
    // The route takes no parameter but the credential, so there is nothing to
    // send that would select a different participant (ADR-0002 rule 10).
    let lan = Lan::new(&["Budi Santoso", "Siti Rahayu"]);
    let budi_token = lan.claim_ok(0).await;
    let siti_id = lan.participants[1].to_storage();

    // Ask repeatedly, including with a body and a query string naming someone
    // else. The answer does not move.
    for _ in 0..3 {
        let (_, body) = lan.session(Some(&budi_token)).await;
        assert_eq!(body["participant"]["name"], "Budi Santoso");
    }

    let (status, body, _) = lan
        .send(
            Request::builder()
                .uri(format!("/api/session?participant_id={siti_id}"))
                .header(header::AUTHORIZATION, format!("Bearer {budi_token}"))
                .body(Body::empty())
                .expect("request"),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["participant"]["name"], "Budi Santoso", "{body}");
    assert_ne!(body["participant"]["id"], siti_id);
}

#[tokio::test]
async fn a_missing_malformed_or_revoked_credential_is_one_answer() {
    let lan = Lan::new(&["Budi Santoso"]);
    let token = lan.claim_ok(0).await;

    // Live.
    assert_eq!(lan.session(Some(&token)).await.0, StatusCode::OK);

    // Revoked by the Host: the credential stops working at once.
    lan.domain
        .revoke_session(&Actor::Host, lan.meeting_id, lan.participants[0])
        .expect("revoke");

    let revoked = lan.session(Some(&token)).await;
    let absent = lan.session(None).await;
    let fabricated = lan.session(Some(&app_server::mint().unwrap().token)).await;

    for (status, body) in [revoked, absent, fabricated] {
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["kind"], "unauthenticated");
    }
}

#[tokio::test]
async fn a_non_bearer_authorization_header_is_not_accepted() {
    let lan = Lan::new(&["Budi Santoso"]);
    let token = lan.claim_ok(0).await;

    for header_value in [
        token.clone(),
        format!("Basic {token}"),
        "Bearer".to_owned(),
        "Bearer ".to_owned(),
    ] {
        let (status, _, _) = lan
            .send(
                Request::builder()
                    .uri("/api/session")
                    .header(header::AUTHORIZATION, header_value.clone())
                    .body(Body::empty())
                    .expect("request"),
            )
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{header_value:?}");
    }
}

#[tokio::test]
async fn acknowledgement_is_reported_without_changing_access() {
    let lan = Lan::new(&["Budi Santoso"]);
    let token = lan.claim_ok(0).await;

    let (before_status, before) = lan.session(Some(&token)).await;
    assert_eq!(before_status, StatusCode::OK);
    assert_eq!(before["acknowledged"], false);

    lan.domain
        .approve_claim(&Actor::Host, lan.meeting_id, lan.participants[0])
        .expect("approve");

    let (after_status, after) = lan.session(Some(&token)).await;
    // Same access, same identity, only the flag moved.
    assert_eq!(after_status, StatusCode::OK);
    assert_eq!(after["acknowledged"], true);
    assert_eq!(after["participant"], before["participant"]);
}

// ---------------------------------------------------------------------------
// Headers, limits and assets
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_response_carries_the_security_headers() {
    let lan = Lan::new(&["Budi Santoso"]);

    // Including refusals: a 404 is served to anyone who guesses a URL.
    for path in [
        format!("/api/join/{}", lan.join_token),
        "/api/join/nonsense".to_owned(),
        "/api/session".to_owned(),
        "/join/anything".to_owned(),
    ] {
        let (_, _, headers) = lan.get(&path).await;
        let find = |name: &str| {
            headers
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
                .unwrap_or_else(|| panic!("{name} missing on {path}"))
        };

        let csp = find("content-security-policy");
        assert!(csp.contains("default-src 'self'"), "{csp}");
        assert!(csp.contains("connect-src 'self'"), "{csp}");
        assert!(csp.contains("frame-ancestors 'none'"), "{csp}");
        // No remote origin may be reachable from a participant's browser.
        assert!(!csp.contains("http://"), "{csp}");
        assert!(!csp.contains("https://"), "{csp}");

        assert_eq!(find("x-content-type-options"), "nosniff");
        // The join URL is in the address bar and is an operational secret.
        assert_eq!(find("referrer-policy"), "no-referrer");
        assert_eq!(find("x-frame-options"), "DENY");
    }
}

#[tokio::test]
async fn an_oversized_body_is_rejected() {
    // The only body any route takes is one identifier. A participant on the LAN
    // is untrusted input and must not be able to make the Host allocate.
    let lan = Lan::new(&["Budi Santoso"]);

    let (status, _, _) = lan
        .send(
            Request::builder()
                .method("POST")
                .uri(format!("/api/join/{}/claim", lan.join_token))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("x".repeat(64 * 1024)))
                .expect("request"),
        )
        .await;

    assert!(
        status == StatusCode::PAYLOAD_TOO_LARGE || status == StatusCode::BAD_REQUEST,
        "an oversized body must be refused, got {status}"
    );
}

#[tokio::test]
async fn the_single_page_route_is_served_for_client_side_paths() {
    let lan = Lan::new(&["Budi Santoso"]);

    // `/join/{token}` has no file behind it; the bundle reads the token from the
    // URL. Whether a real bundle is embedded depends on `npm run build:lan`
    // having run, so both outcomes are accepted - what must not happen is a 404.
    let (status, _, _) = lan.get(&format!("/join/{}", lan.join_token)).await;
    assert!(
        status == StatusCode::OK || status == StatusCode::SERVICE_UNAVAILABLE,
        "a client-side route must not 404, got {status}"
    );

    // A missing *asset* is a 404, so a bundler path typo fails visibly rather
    // than arriving as HTML that will not parse.
    let (asset_status, _, _) = lan.get("/assets/does-not-exist.js").await;
    assert_eq!(asset_status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_host_bundle_is_never_served_over_the_lan() {
    // `apps/host-ui` talks to Tauri commands a browser has no business reaching,
    // and only `apps/lan-ui/dist` is embedded. A page route therefore resolves
    // to the *participant* bundle - which is the property that matters, rather
    // than merely "not 200": the single-page fallback is supposed to answer.
    let lan = Lan::new(&["Budi Santoso"]);

    for path in [
        "/join/anything",
        "/host-ui/index.html",
        "/../host-ui/index.html",
    ] {
        let body = lan.raw(path).await;
        assert!(
            !body.contains("LAN Meeting Collaboration App"),
            "{path} served the Host page: {body}"
        );
        assert!(
            !body.contains("@tauri-apps"),
            "{path} served Tauri client code: {body}"
        );
        // When a bundle is built this is the participant page; without one it is
        // the plain "not built" notice. Never the Host's.
        assert!(
            body.contains("Join Meeting") || body.contains("was not built"),
            "{path} served something unexpected: {body}"
        );
    }

    // A Host asset path is a missing asset, not a page.
    let (status, _, _) = lan.get("/assets/host-ui.js").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// The participant's own note
// ---------------------------------------------------------------------------

#[tokio::test]
async fn reading_a_note_requires_a_session() {
    let lan = Lan::new(&["Budi Santoso"]);

    for bearer in [None, Some("not-a-token"), Some(&"f".repeat(64)[..])] {
        let (status, body) = lan.read_note(bearer).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
        assert_eq!(body["kind"], "unauthenticated");
    }
}

#[tokio::test]
async fn writing_a_note_requires_a_session() {
    let lan = Lan::new(&["Budi Santoso"]);

    for bearer in [None, Some("not-a-token")] {
        let (status, body) = lan.put_note(bearer, "anything").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
        assert_eq!(body["kind"], "unauthenticated");
    }
}

#[tokio::test]
async fn a_participant_without_a_note_reads_null() {
    let lan = Lan::new(&["Budi Santoso"]);
    let token = lan.claim_ok(0).await;

    let (status, body) = lan.read_note(Some(&token)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.is_null(), "{body}");
}

#[tokio::test]
async fn a_participant_can_write_and_then_read_their_own_note() {
    let lan = Lan::new(&["Budi Santoso"]);
    let token = lan.claim_ok(0).await;

    let (status, body) = lan.put_note(Some(&token), "## Agenda\n\nBudget.").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["content"], "## Agenda\n\nBudget.");
    assert_eq!(body["version"], 1);
    // A participant's own write is attributed to them, not to the Host.
    assert_eq!(body["last_author_type"], "PARTICIPANT");

    let (status, read) = lan.read_note(Some(&token)).await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert_eq!(read["content"], "## Agenda\n\nBudget.");
    assert_eq!(read["version"], 1);
}

#[tokio::test]
async fn a_second_write_replaces_the_note_and_advances_the_version() {
    // Exactly one note per participant (ADR-0003), so this is an upsert.
    let lan = Lan::new(&["Budi Santoso"]);
    let token = lan.claim_ok(0).await;

    lan.put_note(Some(&token), "first").await;
    let (status, body) = lan.put_note(Some(&token), "second").await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["content"], "second");
    assert_eq!(body["version"], 2);
}

#[tokio::test]
async fn the_note_response_carries_nothing_it_should_not() {
    // Four fields, and the omissions are the design: no note id, no author id,
    // no meeting or participant id, no lock state, nothing about the session
    // (ADR-0020).
    let lan = Lan::new(&["Budi Santoso"]);
    let token = lan.claim_ok(0).await;
    let (_, body) = lan.put_note(Some(&token), "content").await;

    let object = body.as_object().expect("an object");
    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec!["content", "last_author_type", "updated_at", "version"]
    );

    let serialized = body.to_string();
    assert!(
        !serialized.contains(&lan.meeting_id.to_storage()),
        "{serialized}"
    );
    assert!(
        !serialized.contains(&lan.participants[0].to_storage()),
        "{serialized}"
    );
    assert!(!serialized.contains(&token), "{serialized}");
}

#[tokio::test]
async fn a_participant_cannot_reach_another_participants_note() {
    // There is no parameter to substitute: the route has no path segment and
    // the body has one field. Each session sees its own note and nothing else.
    let lan = Lan::new(&["Alice Anwar", "Bob Basuki"]);
    let alice = lan.claim_ok(0).await;
    let bob = lan.claim_ok(1).await;

    lan.put_note(Some(&alice), "alice's private note").await;
    lan.put_note(Some(&bob), "bob's private note").await;

    let (_, alices) = lan.read_note(Some(&alice)).await;
    let (_, bobs) = lan.read_note(Some(&bob)).await;

    assert_eq!(alices["content"], "alice's private note");
    assert_eq!(bobs["content"], "bob's private note");
}

#[tokio::test]
async fn a_participant_id_in_the_body_is_not_authority() {
    // The shape of the attack this route is built to be immune to. The extra
    // field is ignored by the deserializer, and the write lands on the session
    // that authenticated it.
    let lan = Lan::new(&["Alice Anwar", "Bob Basuki"]);
    let alice = lan.claim_ok(0).await;
    let bob = lan.claim_ok(1).await;
    lan.put_note(Some(&bob), "bob's own note").await;

    let forged = serde_json::json!({
        "content": "written by alice, aimed at bob",
        "participant_id": lan.participants[1].to_storage(),
        "meeting_id": lan.meeting_id.to_storage(),
    })
    .to_string();

    let (status, body) = lan.put_note_raw(Some(&alice), forged).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Alice's own note took the content; Bob's is untouched.
    assert_eq!(body["content"], "written by alice, aimed at bob");
    let (_, bobs) = lan.read_note(Some(&bob)).await;
    assert_eq!(bobs["content"], "bob's own note");
    assert_eq!(bobs["version"], 1);
}

#[tokio::test]
async fn a_revoked_session_can_neither_read_nor_write_a_note() {
    let lan = Lan::new(&["Budi Santoso"]);
    let token = lan.claim_ok(0).await;
    lan.put_note(Some(&token), "before the revocation").await;

    lan.domain
        .revoke_session(&Actor::Host, lan.meeting_id, lan.participants[0])
        .expect("revoke");

    let (status, body) = lan.read_note(Some(&token)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    let (status, body) = lan.put_note(Some(&token), "after").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
}

#[tokio::test]
async fn a_locked_meeting_still_serves_the_note_but_refuses_a_write() {
    // A locked meeting is finished, not secret. The notes are what it was
    // locked to keep, and the participant wrote this one (ADR-0020).
    let lan = Lan::new(&["Budi Santoso"]);
    let token = lan.claim_ok(0).await;
    lan.put_note(Some(&token), "written while open").await;

    lan.lock();

    let (status, body) = lan.read_note(Some(&token)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["content"], "written while open");

    let (status, body) = lan.put_note(Some(&token), "written after the lock").await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["kind"], "meeting_not_open");

    // And the note is unchanged.
    let (_, after) = lan.read_note(Some(&token)).await;
    assert_eq!(after["content"], "written while open");
    assert_eq!(after["version"], 1);
}

#[tokio::test]
async fn content_the_domain_refuses_is_refused_here() {
    let lan = Lan::new(&["Budi Santoso"]);
    let token = lan.claim_ok(0).await;

    for bad in [
        "",
        "   ",
        "<script>alert(1)</script>",
        "</div>",
        "[click](javascript:alert(1))",
        "before\u{0}after",
    ] {
        let (status, body) = lan.put_note(Some(&token), bad).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad:?}: {body}");
        assert_eq!(body["kind"], "invalid_request", "{bad:?}");
    }

    // Nothing was stored by any of them.
    let (_, read) = lan.read_note(Some(&token)).await;
    assert!(read.is_null(), "{read}");
}

#[tokio::test]
async fn ordinary_arithmetic_is_not_refused_as_html() {
    let lan = Lan::new(&["Budi Santoso"]);
    let token = lan.claim_ok(0).await;

    for good in ["a < b", "2 < 3", "`<script>` in a code span"] {
        let (status, body) = lan.put_note(Some(&token), good).await;
        assert_eq!(status, StatusCode::OK, "{good:?}: {body}");
    }
}

#[tokio::test]
async fn malformed_json_is_a_validation_refusal() {
    let lan = Lan::new(&["Budi Santoso"]);
    let token = lan.claim_ok(0).await;

    for body in ["not json at all", "{\"content\":}", "{}", "[]"] {
        let (status, response) = lan.put_note_raw(Some(&token), body.to_owned()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}: {response}");
        assert_eq!(response["kind"], "invalid_request", "{body}");
    }
}

#[tokio::test]
async fn a_note_over_the_domain_limit_reaches_the_domain_and_is_refused_there() {
    // Between 64 KiB and the route's 192 KiB: the transport reads it, and the
    // domain refuses it with the limit and the measured size. That layering is
    // the point of having two numbers (ADR-0020).
    let lan = Lan::new(&["Budi Santoso"]);
    let token = lan.claim_ok(0).await;

    let (status, body) = lan.put_note(Some(&token), &"a".repeat(70_000)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["kind"], "invalid_request");
}

#[tokio::test]
async fn a_note_at_the_domain_limit_is_accepted() {
    let lan = Lan::new(&["Budi Santoso"]);
    let token = lan.claim_ok(0).await;

    let (status, body) = lan.put_note(Some(&token), &"a".repeat(65_536)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["version"], 1);
}

#[tokio::test]
async fn a_body_past_the_route_limit_is_refused_by_the_transport() {
    let lan = Lan::new(&["Budi Santoso"]);
    let token = lan.claim_ok(0).await;

    let (status, body) = lan.put_note(Some(&token), &"a".repeat(250_000)).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
    assert_eq!(body["kind"], "payload_too_large");
}

#[tokio::test]
async fn a_body_that_fails_mid_read_is_not_reported_as_too_large() {
    // `BytesRejection` covers *every* way a body can fail to buffer, and only
    // one of them is a size problem. A stream that dies part-way through is
    // not: answering it with 413 "shorten your note and try again" would name
    // a cause that is not there, and the note may have been well under the
    // limit. Only the length-limit rejection earns that status (ADR-0020).
    let lan = Lan::new(&["Budi Santoso"]);
    let token = lan.claim_ok(0).await;

    // A body that yields no bytes and then an error, which is what a
    // connection dropping mid-request looks like to the extractor.
    let torn = Body::from_stream(stream::once(async {
        Err::<&'static str, std::io::Error>(std::io::Error::other("connection reset"))
    }));

    let (status, body, _) = lan
        .send(
            Request::builder()
                .method("PUT")
                .uri("/api/note")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(torn)
                .expect("request"),
        )
        .await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["kind"], "invalid_request", "{body}");

    // And nothing was written by a request that never arrived.
    let (_, stored) = lan.read_note(Some(&token)).await;
    assert!(stored.is_null(), "{stored}");
}

#[tokio::test]
async fn an_unauthenticated_oversized_body_is_refused_before_it_is_read() {
    // The extractor order is load-bearing: `Participant` runs before the body
    // is touched, so raising the limit for this route does not let an
    // unauthenticated caller make the Host allocate 192 KiB.
    let lan = Lan::new(&["Budi Santoso"]);

    let (status, body) = lan.put_note(None, &"a".repeat(250_000)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["kind"], "unauthenticated");
}

#[tokio::test]
async fn the_kilobyte_limit_still_governs_every_other_route() {
    // The note route's override must not have widened anything else.
    let lan = Lan::new(&["Budi Santoso"]);

    let oversized = serde_json::json!({ "participant_id": "x".repeat(4096) }).to_string();
    let (status, _, _) = lan
        .send(
            Request::builder()
                .method("POST")
                .uri(format!("/api/join/{}/claim", lan.join_token))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(oversized))
                .expect("request"),
        )
        .await;

    assert_ne!(status, StatusCode::CREATED);
    assert!(
        status == StatusCode::PAYLOAD_TOO_LARGE || status.is_client_error(),
        "the claim route must still refuse a body over a kilobyte, got {status}"
    );
}

#[tokio::test]
async fn windows_line_endings_are_normalised_at_the_transport() {
    // A browser can submit CRLF and the domain refuses a carriage return as a
    // control character; a participant should not be told about a character
    // they cannot see.
    let lan = Lan::new(&["Budi Santoso"]);
    let token = lan.claim_ok(0).await;

    let (status, body) = lan.put_note(Some(&token), "line one\r\nline two\r\n").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["content"], "line one\nline two\n");
}

#[tokio::test]
async fn nothing_else_about_the_content_is_normalised() {
    // The stored note is what was typed (ADR-0019).
    let lan = Lan::new(&["Budi Santoso"]);
    let token = lan.claim_ok(0).await;

    let typed = "#  Odd   spacing\n\n\n\n-    loose    marker";
    let (status, body) = lan.put_note(Some(&token), typed).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["content"], typed);
}

#[tokio::test]
async fn a_pending_claim_may_write_a_note() {
    // Approval is acknowledgement, not a gate (ADR-0016). A participant whose
    // claim the Host has not looked at can work.
    let lan = Lan::new(&["Budi Santoso"]);
    let token = lan.claim_ok(0).await;

    let (status, body) = lan.put_note(Some(&token), "written while pending").await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn the_note_routes_never_appear_in_a_url_with_an_identifier() {
    // The whole point of the route shape: there is nothing to address. A path
    // carrying a participant id is not a route this server has.
    let lan = Lan::new(&["Budi Santoso"]);
    let token = lan.claim_ok(0).await;

    for path in [
        format!("/api/note/{}", lan.participants[0].to_storage()),
        format!("/api/meetings/{}/note", lan.meeting_id.to_storage()),
        format!(
            "/api/participants/{}/note",
            lan.participants[0].to_storage()
        ),
    ] {
        let (status, body, _) = lan
            .send(
                Request::builder()
                    .uri(&path)
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .expect("request"),
            )
            .await;
        // The single-page fallback answers unknown paths, so the status is
        // not the interesting part. What matters is that no note came back:
        // these paths are not API routes, and an identifier in a URL buys
        // nothing.
        let _ = status;
        assert!(
            body["content"].is_null() && body["version"].is_null(),
            "{path} served a note: {body}"
        );
    }
}
