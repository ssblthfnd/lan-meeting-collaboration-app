//! The LAN server's lifecycle and the join link, from the Host's side.
//!
//! These bind real sockets on loopback. That is deliberate: the thing under
//! test is whether starting and stopping actually take effect, and a mocked
//! listener would prove nothing about a port being free afterwards.
//!
//! Port `0` is used throughout, so the operating system picks a free one and
//! these never collide with a real host, with CI, or with each other.

use std::sync::Arc;

use app_core::actor::Actor;
use lan_meeting_app_lib::dto::{MeetingConfigurationInput, ParticipantDetailsInput};
use lan_meeting_app_lib::error::ErrorCategory;
use lan_meeting_app_lib::{HostState, LanLifecycle};
use tempfile::TempDir;

struct Host {
    state: HostState,
    lan: LanLifecycle,
    _dir: TempDir,
}

impl Host {
    fn new() -> Self {
        let dir = TempDir::new().expect("temp dir");
        let state = HostState::open(dir.path().join("meetings.sqlite3")).expect("open");
        let lan = LanLifecycle::new(state.shared_db(), state.shared_domain());
        Host {
            state,
            lan,
            _dir: dir,
        }
    }

    /// An open meeting with one participant.
    fn open_meeting(&self) -> String {
        let created = self
            .state
            .create_meeting(MeetingConfigurationInput {
                title: "Weekly Coordination".to_owned(),
                topic: None,
                date: "2026-09-20".to_owned(),
                start_time: "09:00:00".to_owned(),
                end_time: "10:30:00".to_owned(),
                timezone: "Asia/Makassar".to_owned(),
                location: None,
                description: None,
            })
            .expect("create");
        let meeting_id = created.meeting_id.to_storage();

        self.state
            .add_participant(
                &meeting_id,
                ParticipantDetailsInput {
                    name: "Budi Santoso".to_owned(),
                    department: None,
                    position: None,
                    meeting_role: None,
                },
            )
            .expect("add");
        self.state.open_meeting(&meeting_id).expect("open");
        meeting_id
    }
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_server_starts_stopped_and_returns_there() {
    let host = Host::new();

    assert!(!host.lan.status().running);
    assert_eq!(host.lan.status().port, None);

    let started = host.lan.start(Some(0)).await.expect("start");
    assert!(started.running);
    // Port 0 means "pick one", so the status must report what was *bound* -
    // the Host UI shows this number to participants.
    let port = started.port.expect("a bound port");
    assert_ne!(port, 0);
    assert_eq!(host.lan.status().port, Some(port));

    let stopped = host.lan.stop().await;
    assert!(!stopped.running);
    assert_eq!(stopped.port, None);
    assert!(!host.lan.status().running);
}

#[tokio::test]
async fn starting_twice_is_not_an_error() {
    // The Host pressed the button twice. The honest answer is the current
    // state, not a failure.
    let host = Host::new();

    let first = host.lan.start(Some(0)).await.expect("start");
    let second = host.lan.start(Some(0)).await.expect("start again");

    assert!(second.running);
    assert_eq!(
        second.port, first.port,
        "the running server is not replaced"
    );

    host.lan.stop().await;
}

#[tokio::test]
async fn stopping_when_stopped_is_not_an_error() {
    let host = Host::new();
    assert!(!host.lan.stop().await.running);
    assert!(!host.lan.stop().await.running);
}

#[tokio::test]
async fn stopping_frees_the_port_for_a_restart() {
    // "Stopped" must mean the socket is closed. If it did not, restarting on
    // the same port would fail for a reason the Host could not see.
    let host = Host::new();

    let started = host.lan.start(Some(0)).await.expect("start");
    let port = started.port.expect("port");
    host.lan.stop().await;

    let restarted = host
        .lan
        .start(Some(port))
        .await
        .expect("restart on the same port");
    assert_eq!(restarted.port, Some(port));
    host.lan.stop().await;
}

#[tokio::test]
async fn a_port_already_in_use_is_reported_with_something_to_do_about_it() {
    let first = Host::new();
    let second = Host::new();

    let started = first.lan.start(Some(0)).await.expect("start");
    let port = started.port.expect("port");

    let error = second
        .lan
        .start(Some(port))
        .await
        .expect_err("the port is taken");

    // Actionable: names the address and both likely causes, because "failed to
    // bind" alone leaves the Host with nothing to try (architecture rules 22).
    let message = error.to_string();
    assert!(message.contains(&port.to_string()), "{message}");
    assert!(message.contains("already be in use"), "{message}");
    assert!(message.contains("Firewall"), "{message}");

    assert!(!second.lan.status().running);
    first.lan.stop().await;
}

#[tokio::test]
async fn concurrent_starts_establish_exactly_one_listener() {
    // The invariant the lifecycle exists to hold: a check followed later by a
    // store is a race, so the check and the reservation are one critical
    // section. Whatever the interleaving, at most one caller may bind, and the
    // one that does is the one whose handle is stored.
    //
    // Port 0 is what makes this a real test: with a fixed port the operating
    // system would reject the second bind and hide the race.
    let host = Arc::new(Host::new());

    const STARTERS: usize = 8;
    let mut handles = Vec::new();
    for _ in 0..STARTERS {
        let host = Arc::clone(&host);
        handles.push(tokio::spawn(async move { host.lan.start(Some(0)).await }));
    }

    let mut bound_ports = Vec::new();
    for handle in handles {
        let status = handle
            .await
            .expect("starter task")
            .expect("port 0 always binds");
        if status.running {
            bound_ports.push(status.port.expect("a running server reports its port"));
        }
    }

    // Every caller that was told "running" was told the same port: there is one
    // server, not several competing for ownership.
    bound_ports.dedup();
    assert!(
        bound_ports.len() <= 1,
        "more than one listener was established: {bound_ports:?}"
    );

    // And the lifecycle agrees with them.
    let settled = host.lan.status();
    assert!(settled.running, "one starter must have won");
    let port = settled.port.expect("port");
    if let Some(reported) = bound_ports.first() {
        assert_eq!(*reported, port);
    }

    // The winner's handle is the one stored, so stopping actually frees it.
    host.lan.stop().await;
    assert!(!host.lan.status().running);
    assert!(
        tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_err(),
        "the single listener must be closed after stopping"
    );
}

#[tokio::test]
async fn a_failed_bind_releases_the_reservation() {
    // A refused start must leave the lifecycle exactly where it was, or the
    // Host could never retry on a corrected port.
    let occupier = Host::new();
    let host = Host::new();

    let taken = occupier
        .lan
        .start(Some(0))
        .await
        .expect("start")
        .port
        .unwrap();

    assert!(host.lan.start(Some(taken)).await.is_err());
    assert!(
        !host.lan.status().running,
        "a failed bind must not look running"
    );

    // The reservation is gone, so a different port still works.
    let recovered = host.lan.start(Some(0)).await.expect("retry on a free port");
    assert!(recovered.running);

    host.lan.stop().await;
    occupier.lan.stop().await;
}

#[tokio::test]
async fn opening_a_meeting_does_not_start_the_server() {
    // ADR-0016: binding a LAN-reachable socket raises the firewall prompt and
    // makes this machine answer on the network. That happens because the Host
    // chose it, never as a side effect of a lifecycle transition.
    let host = Host::new();
    host.open_meeting();
    assert!(!host.lan.status().running);
}

#[tokio::test]
async fn a_running_server_actually_serves_the_join_flow_over_a_socket() {
    // Everything else drives the router directly. This one goes through a real
    // bound port, so it covers what those cannot: that the listener is wired to
    // the router, that the shared database reaches it, and that a participant's
    // browser would get an answer.
    //
    // The request is written by hand rather than with an HTTP client. No client
    // crate belongs in this application - nothing here may reach off the device
    // (PRD section 4) - and a GET is three lines of text.
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let host = Host::new();
    let meeting_id = host.open_meeting();
    let issued = host
        .state
        .issue_join_token(&meeting_id, "127.0.0.1", 0)
        .expect("issue");
    let token = issued
        .join_url
        .rsplit('/')
        .next()
        .expect("token")
        .to_owned();

    let started = host.lan.start(Some(0)).await.expect("start");
    let port = started.port.expect("port");

    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect to the running server");
    stream
        .write_all(
            format!(
                "GET /api/join/{token} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .expect("write request");

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .await
        .expect("read response");

    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(response.contains("Weekly Coordination"), "{response}");
    assert!(response.contains("Budi Santoso"), "{response}");
    // The security headers are present on the wire, not only in a unit test.
    assert!(response.contains("content-security-policy"), "{response}");
    assert!(
        response.contains("referrer-policy: no-referrer"),
        "{response}"
    );
    // Pre-claim, the participant's other details are withheld (ADR-0016).
    assert!(!response.contains("Finance"), "{response}");

    host.lan.stop().await;

    // And once stopped, nothing answers.
    assert!(
        tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_err(),
        "the port must be closed after stopping"
    );
}

// ---------------------------------------------------------------------------
// Join token, URL and QR
// ---------------------------------------------------------------------------

#[test]
fn a_join_link_carries_a_token_that_is_not_what_was_stored() {
    let host = Host::new();
    let meeting_id = host.open_meeting();

    let issued = host
        .state
        .issue_join_token(&meeting_id, "192.168.1.42", 8765)
        .expect("issue");

    assert!(
        issued
            .join_url
            .starts_with("http://192.168.1.42:8765/join/"),
        "{}",
        issued.join_url
    );
    assert!(
        !issued.replaced_previous,
        "nothing to replace the first time"
    );

    let token = issued
        .join_url
        .rsplit('/')
        .next()
        .expect("a token in the url");
    assert_eq!(token.len(), 64);

    // What was stored is the hash, and it is not the token.
    let stored = app_server::hash_token(token);
    assert_ne!(stored.as_str(), token);

    // The QR encodes the same URL the Host can read and copy: one assembly
    // point, so the picture and the text cannot disagree.
    let expected = lan_meeting_app_lib::qr::encode(&issued.join_url).expect("encode");
    assert_eq!(issued.qr, expected);
    assert_eq!(
        issued.qr.modules.len(),
        (issued.qr.size * issued.qr.size) as usize
    );
}

#[test]
fn issuing_again_replaces_the_previous_link() {
    let host = Host::new();
    let meeting_id = host.open_meeting();

    let first = host
        .state
        .issue_join_token(&meeting_id, "192.168.1.42", 8765)
        .expect("issue");
    let second = host
        .state
        .issue_join_token(&meeting_id, "192.168.1.42", 8765)
        .expect("reissue");

    assert!(second.replaced_previous, "the Host must be told");
    assert_ne!(first.join_url, second.join_url);
    assert_ne!(first.qr, second.qr);
}

#[test]
fn a_join_link_can_only_be_issued_for_an_open_meeting() {
    let host = Host::new();

    // Still DRAFT.
    let draft = host
        .state
        .create_meeting(MeetingConfigurationInput {
            title: "Weekly Coordination".to_owned(),
            topic: None,
            date: "2026-09-20".to_owned(),
            start_time: "09:00:00".to_owned(),
            end_time: "10:30:00".to_owned(),
            timezone: "Asia/Makassar".to_owned(),
            location: None,
            description: None,
        })
        .expect("create")
        .meeting_id
        .to_storage();

    let error = host
        .state
        .issue_join_token(&draft, "192.168.1.42", 8765)
        .expect_err("a draft meeting has nobody to join it");
    assert_eq!(error.category, ErrorCategory::Lifecycle);
}

#[test]
fn a_host_address_that_is_not_an_ip_is_refused() {
    let host = Host::new();
    let meeting_id = host.open_meeting();

    for bad in ["", "not-an-address", "example.com", "999.1.1.1"] {
        let error = host
            .state
            .issue_join_token(&meeting_id, bad, 8765)
            .expect_err("should be refused");
        assert_eq!(error.category, ErrorCategory::Validation, "{bad}");
        assert!(error.message.contains("host address"), "{}", error.message);
    }
}

#[test]
fn the_advertised_address_is_chosen_from_the_real_interfaces() {
    // Only the Host knows which network the participants are on, so the list is
    // offered rather than guessed. Loopback is marked, because a participant on
    // another device cannot use it (architecture rules section 4).
    let host = Host::new();
    let interfaces = host.state.lan_interfaces();

    assert!(
        !interfaces.is_empty(),
        "a machine always has at least loopback"
    );
    for interface in &interfaces {
        assert!(interface.address.parse::<std::net::Ipv4Addr>().is_ok());
    }

    // Loopback sorts last: the useful addresses come first.
    if let Some(position) = interfaces.iter().position(|i| i.is_loopback) {
        assert!(interfaces[position..].iter().all(|i| i.is_loopback));
    }
}

// ---------------------------------------------------------------------------
// Host session controls
// ---------------------------------------------------------------------------

#[test]
fn the_host_can_release_a_name_and_acknowledge_a_claim() {
    let host = Host::new();
    let meeting_id = host.open_meeting();
    let participant = host.state.list_participants(&meeting_id).unwrap()[0].id;
    let participant_id = participant.to_storage();

    // Nothing to act on before anyone joins.
    let error = host
        .state
        .revoke_participant_session(&meeting_id, &participant_id)
        .expect_err("no session yet");
    assert_eq!(error.category, ErrorCategory::NotFound);

    // Somebody joins, through the domain as the LAN server would.
    let parsed_meeting = app_core::id::MeetingId::parse(&meeting_id).unwrap();
    let credential = app_server::mint().expect("entropy");
    host.state
        .domain()
        .claim_identity(
            &Actor::Claimant {
                meeting_id: parsed_meeting,
                participant_id: participant,
            },
            parsed_meeting,
            participant,
            &credential.hash,
        )
        .expect("claim");

    // The roster shows them as joined but unacknowledged.
    let roster = host.state.list_participants(&meeting_id).unwrap();
    assert_eq!(roster[0].claim_status, "PENDING");

    // Acknowledging changes the label and nothing else.
    host.state
        .approve_participant_claim(&meeting_id, &participant_id)
        .expect("approve");
    assert_eq!(
        host.state.list_participants(&meeting_id).unwrap()[0].claim_status,
        "CLAIMED"
    );

    // Releasing frees the name.
    host.state
        .revoke_participant_session(&meeting_id, &participant_id)
        .expect("revoke");
    assert_eq!(
        host.state.list_participants(&meeting_id).unwrap()[0].claim_status,
        "REVOKED"
    );
}

#[test]
fn a_session_action_on_an_identity_from_another_meeting_is_refused() {
    let host = Host::new();
    let ours = host.open_meeting();
    let theirs = host.open_meeting();
    let outsider = host.state.list_participants(&theirs).unwrap()[0]
        .id
        .to_storage();

    let error = host
        .state
        .revoke_participant_session(&ours, &outsider)
        .expect_err("not our participant");
    // No live session for that identity *in this meeting*.
    assert_eq!(error.category, ErrorCategory::NotFound);
}

// ---------------------------------------------------------------------------
// The two transports share one boundary
// ---------------------------------------------------------------------------

#[test]
fn both_transports_mutate_through_the_same_domain() {
    // ADR-0012: one mutation boundary. Two would be two places for a rule to be
    // enforced differently. The LAN server is handed *this* `Domain`, so a
    // change committed by one transport is immediately visible to the other.
    let host = Host::new();
    let meeting_id = host.open_meeting();
    let shared = host.state.shared_domain();

    assert!(
        Arc::ptr_eq(&shared, &host.state.shared_domain()),
        "the same boundary must be handed out every time"
    );

    let participant = host.state.list_participants(&meeting_id).unwrap()[0].id;
    let parsed_meeting = app_core::id::MeetingId::parse(&meeting_id).unwrap();
    let credential = app_server::mint().expect("entropy");

    // A claim made the way the LAN server makes it...
    shared
        .claim_identity(
            &Actor::Claimant {
                meeting_id: parsed_meeting,
                participant_id: participant,
            },
            parsed_meeting,
            participant,
            &credential.hash,
        )
        .expect("claim");

    // ...is visible through the Host's own queries at once.
    assert_eq!(
        host.state.list_participants(&meeting_id).unwrap()[0].claim_status,
        "PENDING"
    );
}

#[test]
fn what_is_stored_is_never_what_was_handed_to_the_browser() {
    // The hash and the token are both 64 hex characters, so the type alone
    // cannot tell them apart - what separates them is that hashing is applied,
    // and that the domain is only ever given the result.
    let credential = app_server::mint().expect("entropy");
    assert_ne!(credential.hash.as_str(), credential.token);
    assert_eq!(credential.hash, app_server::hash_token(&credential.token));

    // A credential in any other encoding cannot become a `TokenHash` at all, so
    // a write method cannot be handed one by mistake.
    for not_a_digest in ["not-a-digest", "", &credential.token.to_uppercase()] {
        assert!(app_core::token::TokenHash::parse(not_a_digest).is_err());
    }
}
