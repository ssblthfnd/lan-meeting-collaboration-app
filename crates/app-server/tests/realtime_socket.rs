//! The notification socket, over real TCP.
//!
//! Every test here binds a port, opens actual WebSocket connections and asserts
//! on frames that crossed a socket. Driving the handler directly would test the
//! parts and not the thing: the handshake, the subprotocol negotiation, the
//! upgrade refusal, the close codes and the graceful shutdown are all only real
//! on a real connection.
//!
//! # No sleeps
//!
//! Nothing here waits a fixed interval hoping something happened. Two
//! deterministic mechanisms do the synchronising:
//!
//! - **the Host feed.** `Realtime::subscribe` is exactly what the Host receives
//!   (the Host's audience is every event), so a test awaits on it and knows
//!   when a transition has been published.
//! - **a barrier event.** Acknowledging a claim is repeatable and reaches one
//!   participant, so publishing one and awaiting its arrival proves that
//!   everything published earlier has already been offered to that socket. That
//!   is how "this participant did *not* receive X" is asserted without waiting
//!   to see whether X turns up late.
//!
//! Every await is wrapped in a timeout, so a broken expectation fails in
//! seconds with a message rather than hanging the suite.

use std::sync::Arc;
use std::time::Duration;

use app_core::actor::Actor;
use app_core::authz::{authorize, Operation};
use app_core::event::{DomainEvent, EventSink};
use app_core::id::{MeetingId, ParticipantId};
use app_core::meeting::MeetingConfiguration;
use app_core::participant::ParticipantDetails;
use app_core::service::{Domain, WriteNote};
use app_core::time::{MeetingDate, MeetingTime, MeetingTimeZone, UtcTimestamp};
use app_db::presence::PresenceStore;
use app_db::Db;
use app_server::state::LanState;
use app_server::{Farewell, Realtime, RunningServer, SUBPROTOCOL};
use futures_util::{SinkExt, StreamExt};
use tempfile::TempDir;
use tokio::net::TcpStream;
use tokio::sync::broadcast::Receiver;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::SEC_WEBSOCKET_PROTOCOL;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

/// Long enough that no test sees a keepalive it did not ask for.
const NO_KEEPALIVE: Duration = Duration::from_secs(3600);

/// Every await gets one. A hang is a failure, not a stalled suite.
const PATIENCE: Duration = Duration::from_secs(10);

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// A running LAN server over a throwaway database, with an open meeting.
struct Lan {
    db: Arc<Db>,
    domain: Arc<Domain<Arc<Db>>>,
    realtime: Arc<Realtime>,
    meeting_id: MeetingId,
    participants: Vec<ParticipantId>,
    port: u16,
    server: Option<RunningServer>,
    _dir: TempDir,
}

impl Lan {
    async fn start(names: &[&str]) -> Self {
        Self::start_with_keepalive(names, NO_KEEPALIVE).await
    }

    async fn start_with_keepalive(names: &[&str], keepalive: Duration) -> Self {
        let dir = TempDir::new().expect("temp dir");
        let db = Arc::new(Db::open(dir.path().join("meeting.sqlite3")).expect("open"));

        // One channel behind the mutation boundary and the server both, which
        // is how the assembled application wires it (ADR-0012, ADR-0018).
        let realtime = Arc::new(Realtime::new());
        let events = Arc::clone(&realtime) as Arc<dyn EventSink>;
        let domain = Arc::new(Domain::with_events(Arc::clone(&db), Arc::clone(&events)));

        let meeting_id = domain
            .create_meeting(&Actor::Host, configuration("Weekly Coordination"))
            .expect("create")
            .meeting_id;

        let participants = names
            .iter()
            .map(|name| {
                domain
                    .add_participant(&Actor::Host, meeting_id, details(name))
                    .expect("add")
                    .participant_id
            })
            .collect();

        domain.open_meeting(&Actor::Host, meeting_id).expect("open");

        let state = LanState::with_realtime(
            Arc::clone(&db),
            Arc::clone(&domain),
            events,
            Arc::clone(&realtime),
        )
        .with_keepalive(keepalive);

        let server = app_server::start(state, 0).await.expect("bind");
        let port = server.address().port();

        Lan {
            db,
            domain,
            realtime,
            meeting_id,
            participants,
            port,
            server: Some(server),
            _dir: dir,
        }
    }

    /// A second meeting, open, with one identity, on the same server.
    fn other_meeting(&self, name: &str) -> (MeetingId, ParticipantId) {
        let meeting_id = self
            .domain
            .create_meeting(&Actor::Host, configuration("Budget Review"))
            .expect("create")
            .meeting_id;
        let participant_id = self
            .domain
            .add_participant(&Actor::Host, meeting_id, details(name))
            .expect("add")
            .participant_id;
        self.domain
            .open_meeting(&Actor::Host, meeting_id)
            .expect("open");
        (meeting_id, participant_id)
    }

    /// Claim an identity and return the session token the browser would hold.
    fn claim(&self, meeting_id: MeetingId, participant_id: ParticipantId) -> String {
        let credential = app_server::mint().expect("entropy");
        self.domain
            .claim_identity(
                &Actor::Claimant {
                    meeting_id,
                    participant_id,
                },
                meeting_id,
                participant_id,
                &credential.hash,
            )
            .expect("claim");
        credential.token
    }

    /// Claim the `index`th identity of the fixture's own meeting.
    fn claim_nth(&self, index: usize) -> String {
        self.claim(self.meeting_id, self.participants[index])
    }

    /// What the Host receives: every event, unfiltered.
    fn host_feed(&self) -> Receiver<Arc<DomainEvent>> {
        self.realtime.subscribe()
    }

    /// Open a socket with a well-formed subprotocol pair, and wait until it can
    /// receive.
    ///
    /// The second half matters. The handshake response is written *before* the
    /// upgrade task runs, so `connect_async` returning proves only that the
    /// server accepted - not that the socket has subscribed to the channel. An
    /// event published in that window would be delivered to nobody, and a test
    /// waiting for it would wait forever.
    ///
    /// The signal is `sockets_served`, which counts subscriptions and only ever
    /// rises - the live subscriber count moves the other way whenever an
    /// earlier socket happens to be closing, and a wait on it can then miss its
    /// own moment. It is observed by yielding rather than by sleeping, so the
    /// wait cannot finish early and does not depend on a chosen interval.
    async fn connect(&self, token: &str) -> Socket {
        let before = self.realtime.sockets_served();
        let socket = self
            .handshake(Some(&format!("{SUBPROTOCOL}, {token}")))
            .await
            .expect("the handshake should have succeeded");

        let deadline = std::time::Instant::now() + PATIENCE;
        while self.realtime.sockets_served() <= before {
            assert!(
                std::time::Instant::now() < deadline,
                "the socket completed its handshake but never subscribed"
            );
            tokio::task::yield_now().await;
        }

        socket
    }

    /// Attempt a handshake with whatever protocol header is given.
    async fn handshake(&self, protocols: Option<&str>) -> Result<Socket, u16> {
        let mut request = format!("ws://127.0.0.1:{}/ws", self.port)
            .into_client_request()
            .expect("a client request");
        if let Some(value) = protocols {
            request.headers_mut().insert(
                SEC_WEBSOCKET_PROTOCOL,
                HeaderValue::from_str(value).expect("a header value"),
            );
        }

        match tokio_tungstenite::connect_async(request).await {
            Ok((socket, response)) => {
                // The server echoes the version tag and never the credential.
                let echoed = response
                    .headers()
                    .get(SEC_WEBSOCKET_PROTOCOL)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned);
                assert_eq!(echoed.as_deref(), Some(SUBPROTOCOL));
                if let Some(offered) = protocols {
                    let credential = offered.split(", ").nth(1).unwrap_or_default();
                    assert!(
                        !credential.is_empty() && !format!("{echoed:?}").contains(credential),
                        "the handshake echoed the credential back"
                    );
                }
                Ok(socket)
            }
            Err(WsError::Http(response)) => Err(response.status().as_u16()),
            Err(other) => panic!("unexpected handshake failure: {other}"),
        }
    }

    /// Acknowledging a claim is repeatable and reaches exactly one participant,
    /// which makes it a barrier: once a socket has this frame, everything
    /// published before it has already been offered to that socket.
    fn barrier(&self, participant_id: ParticipantId) {
        self.domain
            .approve_claim(&Actor::Host, self.meeting_id, participant_id)
            .expect("approve");
    }

    fn last_seen(&self, participant_id: ParticipantId) -> Option<UtcTimestamp> {
        PresenceStore::new(&self.db)
            .roster_presence(self.meeting_id)
            .expect("read")
            .into_iter()
            .find(|row| row.participant_id == participant_id)
            .expect("the participant is on the roster")
            .last_seen_at
    }

    async fn stop(&mut self) {
        if let Some(server) = self.server.take() {
            server.stop().await;
        }
    }
}

fn configuration(title: &str) -> MeetingConfiguration {
    MeetingConfiguration {
        title: title.to_owned(),
        topic: None,
        date: MeetingDate::new(2026, 9, 20).unwrap(),
        start_time: MeetingTime::new(9, 0, 0).unwrap(),
        end_time: MeetingTime::new(10, 30, 0).unwrap(),
        timezone: MeetingTimeZone::new("Asia/Makassar").unwrap(),
        location: None,
        description: None,
    }
}

fn details(name: &str) -> ParticipantDetails {
    ParticipantDetails {
        name: name.to_owned(),
        department: None,
        position: None,
        meeting_role: None,
    }
}

/* -------------------------------------------------------------------------
 * Reading frames
 * ------------------------------------------------------------------------- */

/// The next JSON frame, skipping protocol pings.
async fn next_frame(socket: &mut Socket) -> serde_json::Value {
    loop {
        let message = tokio::time::timeout(PATIENCE, socket.next())
            .await
            .expect("timed out waiting for a frame")
            .expect("the socket is still open")
            .expect("a frame rather than a transport error");

        match message {
            Message::Text(text) => {
                return serde_json::from_str(&text).expect("a frame is JSON");
            }
            Message::Ping(_) | Message::Pong(_) => {}
            Message::Close(frame) => panic!("closed while waiting for a frame: {frame:?}"),
            other => panic!("unexpected frame: {other:?}"),
        }
    }
}

fn kind(frame: &serde_json::Value) -> &str {
    frame["type"].as_str().expect("every frame has a type")
}

/// Read until the socket closes, and report the close code.
async fn closed_with(socket: &mut Socket) -> u16 {
    loop {
        let message = tokio::time::timeout(PATIENCE, socket.next())
            .await
            .expect("timed out waiting for a close");

        match message {
            Some(Ok(Message::Close(Some(frame)))) => return frame.code.into(),
            Some(Ok(Message::Close(None))) | None => panic!("closed with no code"),
            Some(Ok(_)) => {}
            Some(Err(error)) => panic!("transport error while closing: {error}"),
        }
    }
}

/// The next event on the Host's feed.
async fn next_host_event(feed: &mut Receiver<Arc<DomainEvent>>) -> Arc<DomainEvent> {
    tokio::time::timeout(PATIENCE, feed.recv())
        .await
        .expect("timed out waiting for a host event")
        .expect("the host feed is open")
}

/// The next event on the Host's feed matching `wanted`.
async fn next_host_event_of(
    feed: &mut Receiver<Arc<DomainEvent>>,
    wanted: &str,
) -> Arc<DomainEvent> {
    loop {
        let event = next_host_event(feed).await;
        if event.kind() == wanted {
            return event;
        }
    }
}

/* -------------------------------------------------------------------------
 * Authentication
 * ------------------------------------------------------------------------- */

#[tokio::test(flavor = "multi_thread")]
async fn a_live_session_may_open_a_socket() {
    let mut lan = Lan::start(&["Budi Santoso"]).await;
    let token = lan.claim_nth(0);

    let mut socket = lan.connect(&token).await;
    lan.barrier(lan.participants[0]);
    assert_eq!(kind(&next_frame(&mut socket).await), "claim.approved");

    lan.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_handshake_without_a_credential_is_refused() {
    let mut lan = Lan::start(&["Budi Santoso"]).await;

    assert_eq!(lan.handshake(None).await.err(), Some(401));
    assert_eq!(lan.handshake(Some(SUBPROTOCOL)).await.err(), Some(401));

    lan.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_malformed_or_unknown_credential_is_refused() {
    let mut lan = Lan::start(&["Budi Santoso"]).await;
    let real = lan.claim_nth(0);

    for offered in [
        // Not a credential at all.
        format!("{SUBPROTOCOL}, not-a-token"),
        // A well-formed credential nobody holds.
        format!("{SUBPROTOCOL}, {}", "f".repeat(64)),
        // The right credential, in the wrong position.
        format!("{real}, {SUBPROTOCOL}"),
        // The right credential under a protocol tag this server does not speak.
        format!("lan-meeting.v0, {real}"),
        // An extra entry: exactly two, or nothing.
        format!("{SUBPROTOCOL}, {real}, extra"),
    ] {
        assert_eq!(
            lan.handshake(Some(&offered)).await.err(),
            Some(401),
            "accepted `{offered}`"
        );
    }

    lan.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revoked_session_may_not_open_a_socket() {
    // The reconnect rule: no authorization survives between sockets, so a
    // credential withdrawn while disconnected is simply not a credential.
    let mut lan = Lan::start(&["Budi Santoso"]).await;
    let token = lan.claim_nth(0);

    lan.domain
        .revoke_session(&Actor::Host, lan.meeting_id, lan.participants[0])
        .expect("revoke");

    assert_eq!(
        lan.handshake(Some(&format!("{SUBPROTOCOL}, {token}")))
            .await
            .err(),
        Some(401)
    );

    lan.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pending_claim_may_open_a_socket() {
    // Approval is acknowledgement, not a gate (ADR-0016). A participant whose
    // claim the Host has not looked at is fully connected.
    let mut lan = Lan::start(&["Budi Santoso"]).await;
    let token = lan.claim_nth(0);
    let mut feed = lan.host_feed();

    let _socket = lan.connect(&token).await;

    let event = next_host_event_of(&mut feed, "presence.changed").await;
    assert_eq!(event.participant_id(), Some(lan.participants[0]));

    lan.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_socket_may_not_open_on_a_meeting_that_is_not_open() {
    let mut lan = Lan::start(&["Budi Santoso"]).await;
    let token = lan.claim_nth(0);

    lan.domain
        .lock_meeting(&Actor::Host, lan.meeting_id)
        .expect("lock");

    assert_eq!(
        lan.handshake(Some(&format!("{SUBPROTOCOL}, {token}")))
            .await
            .err(),
        Some(409)
    );

    lan.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_draft_meeting_is_refused_even_though_it_cannot_normally_hold_a_session() {
    // The lifecycle makes this state unreachable: a session exists only after a
    // claim, a claim needs an OPEN meeting, and nothing moves a meeting back to
    // DRAFT. The transport checks anyway, and this forces the state behind the
    // domain's back to prove the check is not relying on that invariant.
    let mut lan = Lan::start(&["Budi Santoso"]).await;
    let token = lan.claim_nth(0);

    // The lifecycle has no way back, which is why the seeding below is needed.
    assert!(lan
        .domain
        .open_meeting(&Actor::Host, lan.meeting_id)
        .is_err());

    lan.db
        .write(|tx| {
            tx.execute("UPDATE meetings SET status = 'DRAFT'", [])?;
            Ok(())
        })
        .expect("force the meeting back to DRAFT");

    assert_eq!(
        lan.handshake(Some(&format!("{SUBPROTOCOL}, {token}")))
            .await
            .err(),
        Some(409)
    );

    lan.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_credential_for_one_meeting_cannot_reach_another() {
    // There is no meeting in the URL, so this is not a filtering question: the
    // meeting comes from the session row and nowhere else. What the socket
    // receives is decided by that meeting, whatever else is happening on the
    // same server.
    let mut lan = Lan::start(&["Budi Santoso"]).await;
    let (other_meeting, other_participant) = lan.other_meeting("Siti Rahayu");

    let token = lan.claim_nth(0);
    let mut socket = lan.connect(&token).await;

    // A private event in the other meeting, then a meeting-wide one. Both are
    // the kinds this socket would receive if the meeting came from anywhere
    // but its own session row.
    drop(lan.claim(other_meeting, other_participant));
    lan.domain
        .approve_claim(&Actor::Host, other_meeting, other_participant)
        .expect("approve in the other meeting");
    lan.domain
        .lock_meeting(&Actor::Host, other_meeting)
        .expect("lock the other meeting");

    // The barrier reaches this socket. Anything published before it that this
    // socket was entitled to would have arrived first.
    lan.barrier(lan.participants[0]);
    assert_eq!(kind(&next_frame(&mut socket).await), "claim.approved");

    lan.stop().await;
}

/* -------------------------------------------------------------------------
 * Audience isolation
 * ------------------------------------------------------------------------- */

#[tokio::test(flavor = "multi_thread")]
async fn the_host_receives_a_participants_private_event() {
    let mut lan = Lan::start(&["Budi Santoso"]).await;
    let mut feed = lan.host_feed();

    lan.domain
        .write_note(
            &Actor::Host,
            WriteNote {
                meeting_id: lan.meeting_id,
                participant_id: lan.participants[0],
                content: "Budget discussed.".to_owned(),
            },
        )
        .expect("write");

    let event = next_host_event_of(&mut feed, "note.changed").await;
    assert!(event.audience().reaches_host());
    assert_eq!(event.participant_id(), Some(lan.participants[0]));

    lan.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_participant_receives_their_own_private_event_and_meeting_wide_ones() {
    let mut lan = Lan::start(&["Budi Santoso"]).await;
    let token = lan.claim_nth(0);
    let mut socket = lan.connect(&token).await;

    lan.barrier(lan.participants[0]);
    assert_eq!(kind(&next_frame(&mut socket).await), "claim.approved");

    lan.domain
        .lock_meeting(&Actor::Host, lan.meeting_id)
        .expect("lock");
    assert_eq!(kind(&next_frame(&mut socket).await), "meeting.locked");

    lan.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn neither_participant_receives_the_others_private_event() {
    // The property the audience model exists for, asserted in both directions.
    let mut lan = Lan::start(&["Alice Anwar", "Bob Basuki"]).await;
    let alice = lan.participants[0];
    let bob = lan.participants[1];

    let mut alices = lan.connect(&lan.claim(lan.meeting_id, alice)).await;
    let mut bobs = lan.connect(&lan.claim(lan.meeting_id, bob)).await;

    // A note belonging to each, in turn.
    for (owner, content) in [(alice, "Alice's note."), (bob, "Bob's note.")] {
        lan.domain
            .write_note(
                &Actor::Host,
                WriteNote {
                    meeting_id: lan.meeting_id,
                    participant_id: owner,
                    content: content.to_owned(),
                },
            )
            .expect("write");
    }

    // Alice's socket sees her own note change, then her barrier - never Bob's.
    lan.barrier(alice);
    let first = next_frame(&mut alices).await;
    assert_eq!(kind(&first), "note.changed");
    assert_eq!(
        first["participant_id"].as_str().unwrap(),
        alice.to_storage()
    );
    assert_eq!(kind(&next_frame(&mut alices).await), "claim.approved");

    // And Bob's sees his own, then his.
    lan.barrier(bob);
    let first = next_frame(&mut bobs).await;
    assert_eq!(kind(&first), "note.changed");
    assert_eq!(first["participant_id"].as_str().unwrap(), bob.to_storage());
    assert_eq!(kind(&next_frame(&mut bobs).await), "claim.approved");

    lan.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_only_event_reaches_no_participant() {
    // Roster changes and presence are the Host's operational view. A socket
    // must see the barrier and nothing before it.
    let mut lan = Lan::start(&["Budi Santoso"]).await;
    let token = lan.claim_nth(0);
    let mut socket = lan.connect(&token).await;

    // Host-only: this participant's own presence, and a second participant
    // joining the roster of a draft meeting elsewhere.
    let (other_meeting, _) = lan.other_meeting("Siti Rahayu");
    lan.domain
        .issue_join_token(
            &Actor::Host,
            other_meeting,
            &app_server::mint().expect("entropy").hash,
        )
        .expect("issue");

    lan.barrier(lan.participants[0]);
    assert_eq!(kind(&next_frame(&mut socket).await), "claim.approved");

    lan.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn revocation_closes_one_session_and_leaves_the_other_open() {
    // A laptop and a phone. Revoking is session-specific, so the second socket
    // keeps working and the participant stays present (ADR-0002 rule 5).
    let mut lan = Lan::start(&["Budi Santoso"]).await;
    let participant_id = lan.participants[0];

    let laptop_token = lan.claim(lan.meeting_id, participant_id);
    let mut laptop = lan.connect(&laptop_token).await;

    // The identity is held, so a second socket reuses the same credential -
    // which is what a second tab on the same device does.
    let mut phone = lan.connect(&laptop_token).await;

    lan.domain
        .revoke_session(&Actor::Host, lan.meeting_id, participant_id)
        .expect("revoke");

    // Both sockets authenticated with the *same* session, so both are told.
    for socket in [&mut laptop, &mut phone] {
        let frame = next_frame(socket).await;
        assert_eq!(kind(&frame), "session.revoked");
        assert!(frame["session_id"].is_string(), "{frame}");
        assert_eq!(
            closed_with(socket).await,
            Farewell::SessionRevoked.code(),
            "a revoked socket closes with 4401"
        );
    }

    lan.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revocation_naming_another_session_is_ignored() {
    // The audience that the `ParticipantSession` variant exists for, at socket
    // level. First-claim-wins means one participant holds at most one *live*
    // session at a time (ADR-0002), so two simultaneous sessions for one person
    // cannot be produced through the domain - but a revocation for a session
    // this socket does not hold is still reachable, for instance when a
    // previous session is revoked while its socket is still winding down and
    // the identity has already been claimed again.
    //
    // Published straight onto the channel, because the point is what the
    // socket's filter does with an event, not how the event came to exist.
    let mut lan = Lan::start(&["Budi Santoso"]).await;
    let participant_id = lan.participants[0];
    let token = lan.claim_nth(0);

    let mut socket = lan.connect(&token).await;

    lan.realtime.publish(&DomainEvent::SessionRevoked {
        meeting_id: lan.meeting_id,
        participant_id,
        // Same meeting, same participant, a different session.
        session_id: app_core::id::SessionId::new(),
        at: UtcTimestamp::now(),
    });

    // The socket neither received it nor closed: the barrier is the next thing
    // it sees, and it is still there to see it.
    lan.barrier(participant_id);
    assert_eq!(kind(&next_frame(&mut socket).await), "claim.approved");
    assert!(lan.realtime.is_connected(lan.meeting_id, participant_id));

    lan.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn revoking_one_participant_does_not_disturb_another() {
    let mut lan = Lan::start(&["Alice Anwar", "Bob Basuki"]).await;
    let alice = lan.participants[0];
    let bob = lan.participants[1];

    let mut alices = lan.connect(&lan.claim(lan.meeting_id, alice)).await;
    let mut bobs = lan.connect(&lan.claim(lan.meeting_id, bob)).await;

    lan.domain
        .revoke_session(&Actor::Host, lan.meeting_id, alice)
        .expect("revoke alice");

    assert_eq!(kind(&next_frame(&mut alices).await), "session.revoked");
    assert_eq!(closed_with(&mut alices).await, 4401);

    // Bob's socket never saw it, and still works.
    lan.barrier(bob);
    assert_eq!(kind(&next_frame(&mut bobs).await), "claim.approved");

    lan.stop().await;
}

/* -------------------------------------------------------------------------
 * Presence
 * ------------------------------------------------------------------------- */

#[tokio::test(flavor = "multi_thread")]
async fn presence_transitions_only_on_the_first_and_last_socket() {
    let mut lan = Lan::start(&["Budi Santoso"]).await;
    let participant_id = lan.participants[0];
    let token = lan.claim_nth(0);
    let mut feed = lan.host_feed();

    assert_eq!(lan.last_seen(participant_id), None);

    // 0 -> 1: connected, and a timestamp is recorded. Waiting for the event is
    // also what proves the socket has finished attaching - the handshake
    // returns before `on_upgrade` runs, so "connected" is not true merely
    // because `connect` returned.
    let first = lan.connect(&token).await;
    let event = next_host_event_of(&mut feed, "presence.changed").await;
    assert!(matches!(
        event.as_ref(),
        DomainEvent::PresenceChanged {
            connected: true,
            ..
        }
    ));
    let connected_at = lan.last_seen(participant_id).expect("a timestamp");

    // 1 -> 2: no second transition.
    //
    // A socket cannot receive anything until it has attached, so once `second`
    // has the barrier frame its attach has certainly happened - and any
    // presence event it had published would already be sitting ahead of that
    // barrier on the host feed. Finding the barrier there instead is the proof.
    let mut second = lan.connect(&token).await;
    lan.barrier(participant_id);
    assert_eq!(kind(&next_frame(&mut second).await), "claim.approved");
    assert_eq!(next_host_event(&mut feed).await.kind(), "claim.approved");
    assert!(lan.realtime.is_connected(lan.meeting_id, participant_id));

    // 2 -> 1 and 1 -> 0. Both sockets are known to be attached, so between
    // them they must produce exactly one transition however the two server
    // tasks interleave.
    drop(second);
    assert!(
        lan.realtime.is_connected(lan.meeting_id, participant_id),
        "one socket left is still connected, whatever the other task is doing"
    );
    drop(first);

    let event = next_host_event_of(&mut feed, "presence.changed").await;
    assert!(
        matches!(
            event.as_ref(),
            DomainEvent::PresenceChanged {
                connected: false,
                ..
            }
        ),
        "closing one of two sockets must not announce a disconnect: {event:?}"
    );

    // Reaching zero means both tasks have detached, so nothing is still in
    // flight - and a second transition would now be ahead of this barrier.
    lan.barrier(participant_id);
    assert_eq!(
        next_host_event(&mut feed).await.kind(),
        "claim.approved",
        "exactly one disconnect, not one per socket"
    );

    let disconnected_at = lan.last_seen(participant_id).expect("a timestamp");
    assert!(
        disconnected_at >= connected_at,
        "{disconnected_at} is before {connected_at}"
    );
    assert!(!lan.realtime.is_connected(lan.meeting_id, participant_id));
    assert_eq!(lan.realtime.open_socket_count(), 0);

    lan.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_keepalive_does_not_write_to_sqlite() {
    // Liveness is a protocol ping. Recording it would turn an idle meeting of
    // 99 participants into a stream of writes against the one writer
    // connection, to keep a column marginally fresher than the event stream
    // that already carries the same news (ADR-0018).
    let mut lan = Lan::start_with_keepalive(&["Budi Santoso"], Duration::from_millis(20)).await;
    let participant_id = lan.participants[0];
    let token = lan.claim_nth(0);

    // Subscribed before connecting, so the connect transition is on the feed
    // rather than possibly ahead of it.
    let mut feed = lan.host_feed();
    let mut socket = lan.connect(&token).await;
    assert_eq!(
        next_host_event_of(&mut feed, "presence.changed")
            .await
            .kind(),
        "presence.changed"
    );

    lan.barrier(participant_id);
    assert_eq!(kind(&next_frame(&mut socket).await), "claim.approved");
    assert_eq!(next_host_event(&mut feed).await.kind(), "claim.approved");
    let after_connect = lan.last_seen(participant_id).expect("a timestamp");

    // Five server pings, each answered by the client's codec.
    let mut pings = 0;
    while pings < 5 {
        let message = tokio::time::timeout(PATIENCE, socket.next())
            .await
            .expect("timed out waiting for a ping")
            .expect("open")
            .expect("a frame");
        if matches!(message, Message::Ping(_)) {
            pings += 1;
            // Flushing is what sends the queued pong.
            socket.flush().await.expect("flush");
        }
    }

    assert_eq!(
        lan.last_seen(participant_id),
        Some(after_connect),
        "a keepalive must not move last_seen_at"
    );
    // And no presence event was published for any of them: a transition would
    // sit ahead of this barrier on the feed.
    lan.barrier(participant_id);
    assert_eq!(next_host_event(&mut feed).await.kind(), "claim.approved");

    lan.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn being_connected_grants_no_authority() {
    // Presence is an observation. It is not consulted by `authorize`, and a
    // connected participant is refused exactly what a disconnected one is.
    let mut lan = Lan::start(&["Budi Santoso"]).await;
    let participant_id = lan.participants[0];
    let token = lan.claim_nth(0);

    let mut feed = lan.host_feed();
    let _socket = lan.connect(&token).await;
    next_host_event_of(&mut feed, "presence.changed").await;
    assert!(lan.realtime.is_connected(lan.meeting_id, participant_id));

    let actor = Actor::Participant {
        meeting_id: lan.meeting_id,
        participant_id,
        session_id: app_core::id::SessionId::new(),
    };

    // Connected, and still not allowed to lock the meeting or to touch
    // somebody else's note.
    assert!(authorize(&actor, lan.meeting_id, Operation::LockMeeting).is_err());
    assert!(lan
        .domain
        .write_note(
            &actor,
            WriteNote {
                meeting_id: lan.meeting_id,
                participant_id: ParticipantId::new(),
                content: "Not mine.".to_owned(),
            },
        )
        .is_err());

    lan.stop().await;
}

/* -------------------------------------------------------------------------
 * Lock
 * ------------------------------------------------------------------------- */

#[tokio::test(flavor = "multi_thread")]
async fn locking_notifies_without_disconnecting_and_refuses_regardless() {
    let mut lan = Lan::start(&["Alice Anwar", "Bob Basuki"]).await;
    let alice = lan.participants[0];
    let bob = lan.participants[1];

    // Subscribed first, so the two connect transitions are on the feed.
    // Waiting for both is what makes "still connected" below a fact rather
    // than a guess: the handshake returns before the socket has attached.
    let mut feed = lan.host_feed();
    let mut alices = lan.connect(&lan.claim(lan.meeting_id, alice)).await;
    // Bob connects too, and deliberately never reads his socket: he is the
    // participant who "missed the event".
    let _bobs = lan.connect(&lan.claim(lan.meeting_id, bob)).await;
    next_host_event_of(&mut feed, "presence.changed").await;
    next_host_event_of(&mut feed, "presence.changed").await;

    lan.domain
        .lock_meeting(&Actor::Host, lan.meeting_id)
        .expect("lock");

    // Alice is told, and stays connected.
    assert_eq!(kind(&next_frame(&mut alices).await), "meeting.locked");
    assert!(lan.realtime.is_connected(lan.meeting_id, alice));
    assert!(lan.realtime.is_connected(lan.meeting_id, bob));

    // Both are refused, the one who read the event and the one who did not.
    for participant_id in [alice, bob] {
        let refused = lan.domain.write_note(
            &Actor::Host,
            WriteNote {
                meeting_id: lan.meeting_id,
                participant_id,
                content: "Too late.".to_owned(),
            },
        );
        assert!(refused.is_err(), "a locked meeting must refuse every write");
    }

    lan.stop().await;
}

/* -------------------------------------------------------------------------
 * Reconnect
 * ------------------------------------------------------------------------- */

#[tokio::test(flavor = "multi_thread")]
async fn a_reconnect_authenticates_again_and_rebuilds_its_audience() {
    let mut lan = Lan::start(&["Budi Santoso"]).await;
    let participant_id = lan.participants[0];
    let token = lan.claim_nth(0);
    let mut feed = lan.host_feed();

    let first = lan.connect(&token).await;
    next_host_event_of(&mut feed, "presence.changed").await;
    drop(first);
    next_host_event_of(&mut feed, "presence.changed").await;
    assert!(!lan.realtime.is_connected(lan.meeting_id, participant_id));

    // A new socket, authenticated from scratch, with a fresh audience.
    let mut second = lan.connect(&token).await;
    lan.barrier(participant_id);
    assert_eq!(kind(&next_frame(&mut second).await), "claim.approved");
    assert!(lan.realtime.is_connected(lan.meeting_id, participant_id));

    // Revoked between sockets: no authorization survives the gap.
    drop(second);
    next_host_event_of(&mut feed, "presence.changed").await;
    lan.domain
        .revoke_session(&Actor::Host, lan.meeting_id, participant_id)
        .expect("revoke");

    assert_eq!(
        lan.handshake(Some(&format!("{SUBPROTOCOL}, {token}")))
            .await
            .err(),
        Some(401)
    );

    lan.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_can_refetch_current_state_over_http_after_connecting() {
    // The whole contract in one test: the socket says something changed, HTTP
    // says what it is. The frame carries no content at all.
    let mut lan = Lan::start(&["Budi Santoso"]).await;
    let participant_id = lan.participants[0];
    let token = lan.claim_nth(0);

    let mut socket = lan.connect(&token).await;
    lan.barrier(participant_id);
    let frame = next_frame(&mut socket).await;
    assert_eq!(kind(&frame), "claim.approved");
    assert!(frame.get("acknowledged").is_none(), "{frame}");

    let response = reqwest_free_get(lan.port, "/api/session", &token).await;
    assert!(
        response.contains("\"acknowledged\":true"),
        "the refetch is where the value lives: {response}"
    );

    lan.stop().await;
}

/// A minimal HTTP GET, so the refetch half of the contract is exercised without
/// an HTTP client dependency this application has no other use for.
async fn reqwest_free_get(port: u16, path: &str, token: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect");
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\n\
         Accept: application/json\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .expect("send the request");

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .await
        .expect("read the response");
    response
}

/* -------------------------------------------------------------------------
 * Backpressure and shutdown
 * ------------------------------------------------------------------------- */

#[tokio::test(flavor = "multi_thread")]
async fn a_socket_that_falls_far_enough_behind_is_told_to_resync() {
    // The bounded-channel policy. The client is not punished: it reconnects and
    // re-reads current state, which is what it would have done with the dropped
    // events anyway (ADR-0018).
    let mut lan = Lan::start(&["Budi Santoso"]).await;
    let participant_id = lan.participants[0];
    let token = lan.claim_nth(0);

    let mut socket = lan.connect(&token).await;

    // Far more events than the channel holds, published while nothing is
    // reading the socket. Published straight onto the channel rather than
    // through the domain: the subject here is the transport's backpressure
    // policy, and a database transaction per event would test SQLite's
    // throughput instead.
    let flood = DomainEvent::ClaimApproved {
        meeting_id: lan.meeting_id,
        participant_id,
        at: UtcTimestamp::now(),
    };
    for _ in 0..(app_server::realtime::EVENT_QUEUE * 64) {
        lan.realtime.publish(&flood);
    }

    let code = loop {
        let message = tokio::time::timeout(PATIENCE, socket.next())
            .await
            .expect("timed out waiting for the resync close")
            .expect("open")
            .expect("a frame");
        if let Message::Close(Some(frame)) = message {
            break u16::from(frame.code);
        }
    };
    assert_eq!(code, Farewell::ResyncRequired.code());

    lan.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_application_frame_from_a_client_closes_the_socket() {
    // There is no inbound protocol. Refusing rather than ignoring keeps the
    // parsing and authorization surface at zero.
    let mut lan = Lan::start(&["Budi Santoso"]).await;
    let token = lan.claim_nth(0);

    let mut socket = lan.connect(&token).await;
    socket
        .send(Message::Text("{\"hello\":true}".into()))
        .await
        .expect("send");

    assert_eq!(
        closed_with(&mut socket).await,
        Farewell::UnexpectedFrame.code()
    );

    lan.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stopping_the_server_closes_every_socket_and_frees_the_port() {
    // Without the shutdown signal a graceful stop would wait for sockets that
    // have no reason to end, and "stopped" would never mean the port was free.
    let mut lan = Lan::start(&["Alice Anwar", "Bob Basuki"]).await;
    let mut alices = lan
        .connect(&lan.claim(lan.meeting_id, lan.participants[0]))
        .await;
    let mut bobs = lan
        .connect(&lan.claim(lan.meeting_id, lan.participants[1]))
        .await;

    tokio::time::timeout(PATIENCE, lan.stop())
        .await
        .expect("stopping must not wait on an open socket");

    for socket in [&mut alices, &mut bobs] {
        assert_eq!(
            closed_with(socket).await,
            Farewell::ServerStopping.code(),
            "a socket must be told why it is closing"
        );
    }

    // The registry empties as the sockets go.
    assert_eq!(lan.realtime.open_socket_count(), 0);
}
