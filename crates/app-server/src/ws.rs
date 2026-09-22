//! The participant notification socket.
//!
//! One route, `GET /ws`, and one direction: **server to client**. There is no
//! inbound application protocol. A participant does not send commands here,
//! does not subscribe to anything and does not write notes over this socket -
//! every mutation stays on HTTP, where the extractors, the domain boundary and
//! the audit trail already are (ADR-0018).
//!
//! That is a security decision before it is a design one. An inbound message
//! would need parsing, authorization and rate limiting on a transport that
//! faces untrusted input; having none means there is nothing to get wrong.
//!
//! # The credential travels in the subprotocol
//!
//! ```text
//! new WebSocket(url, ["lan-meeting.v1", sessionToken])
//! ```
//!
//! The browser WebSocket API offers no way to set a request header, so the
//! `Authorization: Bearer` the HTTP routes use is not available. The remaining
//! choices are a query string, a cookie, a ticket endpoint, or the subprotocol
//! list - and the first three are worse:
//!
//! - **a query string** puts a bearer credential in a URL, where it reaches
//!   access logs, `Referer` headers and browser history;
//! - **a cookie** is sent automatically, which reintroduces the cross-site
//!   request forgery surface ADR-0016 avoided by choosing a bearer token;
//! - **a ticket endpoint** is a second credential system, with its own
//!   lifetime, storage and revocation semantics to get wrong.
//!
//! The subprotocol is a request header (`Sec-WebSocket-Protocol`) that the
//! browser will set on the application's behalf. It is used here purely as
//! **credential transport**; it names no application-level protocol beyond the
//! single version tag, and the server echoes only that tag back, never the
//! credential.
//!
//! The session token is 64 lowercase hexadecimal characters (`app_server::token`),
//! which is a valid RFC 7230 `token` and therefore legal in this header. A test
//! below pins that, because if the credential format ever changed to something
//! containing a comma, a space or a non-ASCII character, this transport would
//! have to change with it rather than silently mangle it.
//!
//! # The actor is fixed at the upgrade
//!
//! Authentication happens **before** the socket exists, in [`WsParticipant`],
//! and it resolves through exactly the same path the HTTP routes use: hash the
//! presented credential, look the hash up, build the actor from the returned
//! row. Nothing in the URL, and nothing a client could send afterwards, can
//! change who the connection is (architecture rules section 14.1 rule 8).
//!
//! Because there is no re-authentication, the actor is immutable for the
//! socket's lifetime, and so is its audience. A session revoked while
//! disconnected simply cannot reconnect: the lookup excludes revoked rows.
//!
//! # A locked meeting does not close sockets
//!
//! Locking broadcasts `meeting.locked` and leaves everybody connected. The UI
//! stops offering to edit; the backend refuses to edit regardless, because
//! every mutation re-reads the status inside its own transaction. A participant
//! who never receives the event, ignores it, or edits the bundle is refused
//! exactly as firmly (architecture rules section 15).
//!
//! Establishing a *new* socket does require an open meeting, because joining a
//! meeting that is finished is not a thing to start doing.

use std::time::Duration;

use app_core::actor::Actor;
use app_core::event::DomainEvent;
use app_core::id::{MeetingId, NoteId, ParticipantId, SessionId};
use app_core::meeting::MeetingStatus;
use app_core::time::UtcTimestamp;
use axum::body::Bytes;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{FromRequestParts, State};
use axum::http::header::SEC_WEBSOCKET_PROTOCOL;
use axum::http::request::Parts;
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::watch;

use crate::error::ApiError;
use crate::state::{participant_queries, presence_store, session_store, LanState};
use crate::token::hash_token;

/// The one subprotocol this server accepts, and the only one it echoes.
///
/// Versioned so that a future frame shape can be introduced without guessing
/// what an old bundle understands.
pub const SUBPROTOCOL: &str = "lan-meeting.v1";

/// How often the server pings an idle socket.
///
/// Protocol-level liveness only. A ping writes nothing to SQLite: presence is
/// recorded on the connection transitions, not on the heartbeat, because an
/// idle meeting of 99 participants must not become a stream of writes against
/// the single writer connection (ADR-0018).
pub const KEEPALIVE: Duration = Duration::from_secs(30);

/* -------------------------------------------------------------------------
 * Close behaviour
 * ------------------------------------------------------------------------- */

/// Why a socket is closing.
///
/// Every reason is a fixed, public string. None of them names a credential, a
/// session, a SQLite error or anything else the participant did not already
/// know: a close frame is delivered to the same untrusted browser an error body
/// is, and the same rule applies (ADR-0015, the LAN half).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Farewell {
    /// The client went away, or the socket failed.
    ClientLeft,
    /// The Host stopped the LAN server.
    ServerStopping,
    /// The Host revoked *this* session. Another socket of the same participant
    /// is unaffected.
    SessionRevoked,
    /// This socket fell far enough behind that events were dropped for it.
    /// Reconnect and re-read current state.
    ResyncRequired,
    /// The client sent an application frame. There is no inbound protocol.
    UnexpectedFrame,
}

impl Farewell {
    /// The WebSocket close code.
    ///
    /// Two are from the private-use range (4000-4999), because no registered
    /// code means "your credential was withdrawn" or "you missed events", and
    /// a client needs to tell those apart: one means forget the token, the
    /// other means reconnect with it.
    #[must_use]
    pub const fn code(self) -> u16 {
        match self {
            Farewell::ClientLeft => 1000,
            Farewell::ServerStopping => 1001,
            Farewell::UnexpectedFrame => 1003,
            Farewell::SessionRevoked => 4401,
            Farewell::ResyncRequired => 4408,
        }
    }

    /// The close reason, as the participant sees it.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Farewell::ClientLeft => "closed",
            Farewell::ServerStopping => "the meeting server is stopping",
            Farewell::SessionRevoked => "this session was ended by the host",
            Farewell::ResyncRequired => "reconnect and reload",
            Farewell::UnexpectedFrame => "this connection does not accept messages",
        }
    }
}

/* -------------------------------------------------------------------------
 * Authentication
 * ------------------------------------------------------------------------- */

/// An authenticated socket's identity, fixed for its lifetime.
///
/// Every field comes from the resolved session row. There is no constructor
/// that takes them from anywhere else, which is what makes "a browser cannot
/// choose who it is" a property of this type rather than a rule in a comment.
#[derive(Debug, Clone, Copy)]
pub struct WsParticipant {
    /// The actor every audience decision for this socket is made against.
    pub actor: Actor,
    pub meeting_id: MeetingId,
    pub participant_id: ParticipantId,
    /// The session this socket authenticated with.
    ///
    /// The reason revocation can be session-specific: the Host ends a session,
    /// and only the socket holding that exact id closes (ADR-0002 rule 5).
    pub session_id: SessionId,
}

impl FromRequestParts<LanState> for WsParticipant {
    type Rejection = ApiError;

    /// Resolve the socket's identity, or refuse the upgrade.
    ///
    /// Refusals happen here, as ordinary HTTP status codes, because there is no
    /// socket yet to close. That is deliberate and it is the better contract: a
    /// browser that is told 401 by the handshake never opens a socket at all,
    /// rather than opening one that dies immediately for a reason it has to
    /// decode from a close code.
    ///
    /// | Condition | Answer |
    /// | --- | --- |
    /// | no `Sec-WebSocket-Protocol` | 401 |
    /// | malformed protocol list | 401 |
    /// | unknown or revoked credential | 401 |
    /// | meeting deleted in between | 404 |
    /// | meeting `DRAFT` or `LOCKED` | 409 |
    async fn from_request_parts(
        parts: &mut Parts,
        state: &LanState,
    ) -> Result<Self, Self::Rejection> {
        let credential = offered_credential(parts).ok_or_else(ApiError::unauthenticated)?;

        // Total by construction: a hostile or malformed credential hashes to
        // something that matches no row, so there is no parse step here that
        // behaves differently for a nearly-valid value.
        let hash = hash_token(&credential);
        let binding = state
            .blocking(move |db| Ok(session_store(db).resolve_session(&hash)?))
            .await?
            // Absent, malformed, or revoked: one answer for all three.
            .ok_or_else(ApiError::unauthenticated)?;

        // The meeting comes from the session row, never from the request. A
        // credential for meeting A therefore cannot open a socket on meeting B:
        // there is no parameter on this path that names a meeting at all.
        let meeting_id = binding.meeting_id;
        let meeting = state
            .blocking(move |db| Ok(participant_queries(db).joinable_meeting(meeting_id)?))
            .await?
            .ok_or_else(ApiError::not_found)?;

        if meeting.status != MeetingStatus::Open {
            return Err(ApiError::meeting_not_open());
        }

        Ok(Self {
            actor: Actor::Participant {
                meeting_id: binding.meeting_id,
                participant_id: binding.participant_id,
                session_id: binding.session_id,
            },
            meeting_id: binding.meeting_id,
            participant_id: binding.participant_id,
            session_id: binding.session_id,
        })
    }
}

/// The credential from `Sec-WebSocket-Protocol`, if the list is the right shape.
///
/// Exactly two entries are accepted, in order: the version tag, then the
/// credential. Anything else is refused rather than searched, so there is no
/// "find the entry that looks like a token" heuristic for a client to play with.
///
/// The header may legitimately arrive split across several header lines, which
/// is why every value is considered rather than only the first.
fn offered_credential(parts: &Parts) -> Option<String> {
    let offered: Vec<&str> = parts
        .headers
        .get_all(SEC_WEBSOCKET_PROTOCOL)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .collect();

    // Matched as a whole rather than scanned. An extra entry, a missing one, an
    // empty one or the pair in the wrong order all fall through to `None`,
    // which leaves the header with exactly one valid interpretation and nothing
    // for a client to probe.
    match offered.as_slice() {
        [tag, credential] if *tag == SUBPROTOCOL && !credential.is_empty() => {
            Some((*credential).to_owned())
        }
        _ => None,
    }
}

/* -------------------------------------------------------------------------
 * Frames
 * ------------------------------------------------------------------------- */

/// What travels down the socket.
///
/// Identifiers, a version, a timestamp. Never note content, never participant
/// details, never a credential or a hash - a client that may read the thing
/// that changed asks the HTTP boundary for it, where the audience rules are
/// applied again (architecture rules section 16).
///
/// Absent fields are omitted rather than sent as `null`, so a frame carries
/// only what its kind actually means.
#[derive(Debug, Clone, Serialize)]
pub struct EventFrame {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub meeting_id: MeetingId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub participant_id: Option<ParticipantId>,
    /// Present on `session.revoked` alone, which is the one event that has to
    /// name a socket.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note_id: Option<NoteId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<i64>,
    pub at: UtcTimestamp,
}

impl From<&DomainEvent> for EventFrame {
    fn from(event: &DomainEvent) -> Self {
        let mut frame = Self {
            kind: event.kind(),
            meeting_id: event.meeting_id(),
            participant_id: event.participant_id(),
            session_id: event.session_id(),
            note_id: None,
            version: None,
            at: event.at(),
        };
        if let DomainEvent::NoteChanged {
            note_id, version, ..
        } = event
        {
            frame.note_id = Some(*note_id);
            frame.version = Some(*version);
        }
        frame
    }
}

/* -------------------------------------------------------------------------
 * The route
 * ------------------------------------------------------------------------- */

/// `GET /ws` - upgrade an authenticated participant to a notification socket.
///
/// [`WsParticipant`] runs first and refuses the handshake if the credential does
/// not resolve, so `on_upgrade` is reached only for a session that was live and
/// a meeting that was open at that moment.
pub async fn connect(
    State(state): State<LanState>,
    participant: WsParticipant,
    upgrade: WebSocketUpgrade,
) -> Response {
    upgrade
        // Only the version tag is offered back. The credential the client sent
        // alongside it is never echoed.
        .protocols([SUBPROTOCOL])
        .on_upgrade(move |socket| serve(socket, state, participant))
}

/// Run one socket until it closes.
async fn serve(socket: WebSocket, state: LanState, who: WsParticipant) {
    let (mut outgoing, mut incoming) = socket.split();
    let mut events = state.realtime().subscribe();
    let mut stopping = state.shutdown_signal();

    // Upgraded while the Host was already stopping the server.
    //
    // This is checked rather than left to the loop below, for two reasons. A
    // `watch` receiver created after the signal has been sent sees no *change*
    // and would wait forever, holding the port the Host was told was free. And
    // a socket that is about to close should not touch the registry at all:
    // announcing a connection and a disconnection for something that never
    // really opened would be two presence events describing nothing.
    let already_stopping = *stopping.borrow_and_update();
    if already_stopping {
        say_goodbye(&mut outgoing, Farewell::ServerStopping).await;
        return;
    }

    let mut keepalive = tokio::time::interval(state.keepalive());
    keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // The first tick of a Tokio interval completes immediately.
    keepalive.tick().await;

    if state.realtime().attach(who.meeting_id, who.participant_id) {
        observe(&state, &who, true).await;
    }

    let farewell = loop {
        tokio::select! {
            // Nothing inbound is an application message. This arm exists so the
            // socket notices the client going away, and so the codec can answer
            // protocol pings.
            frame = incoming.next() => match frame {
                None | Some(Err(_)) => break Farewell::ClientLeft,
                Some(Ok(Message::Close(_))) => break Farewell::ClientLeft,
                Some(Ok(Message::Text(_) | Message::Binary(_))) => {
                    break Farewell::UnexpectedFrame
                }
                Some(Ok(_)) => {}
            },

            published = events.recv() => match published {
                Ok(event) => {
                    // The audience decision, made here and only here. A client
                    // cannot influence any of the three values it is matched
                    // against: all of them came from the session row.
                    if !event.audience().reaches_session(
                        who.meeting_id,
                        who.participant_id,
                        who.session_id,
                    ) {
                        continue;
                    }

                    let frame = EventFrame::from(event.as_ref());
                    let Ok(json) = serde_json::to_string(&frame) else {
                        break Farewell::ClientLeft;
                    };
                    if outgoing.send(Message::Text(json.into())).await.is_err() {
                        break Farewell::ClientLeft;
                    }

                    // Delivered first, so the client learns why before the
                    // socket goes. The audience filter above already limits
                    // this event to the revoked session; matching the id again
                    // is what makes that a fact of this arm rather than a
                    // consequence of another function being right.
                    if event.session_id() == Some(who.session_id)
                        && matches!(event.as_ref(), DomainEvent::SessionRevoked { .. })
                    {
                        break Farewell::SessionRevoked;
                    }
                }
                // Bounded channel, slow reader. Close it; the client reconnects
                // and re-reads current state, which is what it would have done
                // with the dropped events anyway (ADR-0018).
                Err(RecvError::Lagged(_)) => break Farewell::ResyncRequired,
                Err(RecvError::Closed) => break Farewell::ServerStopping,
            },

            // Protocol liveness. Writes nothing to SQLite.
            _ = keepalive.tick() => {
                if outgoing.send(Message::Ping(Bytes::new())).await.is_err() {
                    break Farewell::ClientLeft;
                }
            }

            _ = wait_for_shutdown(&mut stopping) => break Farewell::ServerStopping,
        }
    };

    if state.realtime().detach(who.meeting_id, who.participant_id) {
        observe(&state, &who, false).await;
    }

    say_goodbye(&mut outgoing, farewell).await;
}

/// Resolve once the Host has stopped the server.
///
/// `wait_for` rather than `changed`, so a signal that arrived between the check
/// at the top of [`serve`] and this loop is still seen rather than waited for
/// forever. The borrow it yields is deliberately consumed inside this function:
/// a `watch::Ref` is not `Send`, and holding one across the socket's `await`
/// would make the whole connection future unspawnable.
async fn wait_for_shutdown(stopping: &mut watch::Receiver<bool>) {
    // An error means the sender is gone, which happens only when the server's
    // state is dropped. The socket should end either way.
    let _ = stopping.wait_for(|stopping| *stopping).await.is_err();
}

/// Send the close frame and shut the socket down.
///
/// Both failures are discarded, and there is nothing else to do with them: the
/// only reason a close frame does not arrive is that the client has already
/// gone, which is what the frame was about to say.
async fn say_goodbye<S>(outgoing: &mut S, farewell: Farewell)
where
    S: SinkExt<Message> + Unpin,
{
    let _ = outgoing
        .send(Message::Close(Some(CloseFrame {
            code: farewell.code(),
            reason: farewell.reason().into(),
        })))
        .await;
    let _ = outgoing.close().await;
}

/// Record and announce a presence transition.
///
/// Called only on `0 -> 1` and `1 -> 0`, so a participant with two tabs
/// produces one connect and one disconnect, not four. The durable timestamp is
/// written first and the event follows it, the same order every mutation uses.
///
/// A failed write is logged on the Host's console and does not stop the
/// announcement: the in-memory registry is what "connected right now" means,
/// and the column is a record of when that was last observed.
async fn observe(state: &LanState, who: &WsParticipant, connected: bool) {
    let session_id = who.session_id;
    let at = UtcTimestamp::now();

    let recorded = state
        .blocking(move |db| Ok(presence_store(db).touch_session(session_id, at)?))
        .await;
    if recorded.is_err() {
        eprintln!("[lan] a presence timestamp could not be recorded");
    }

    state.events().publish(&DomainEvent::PresenceChanged {
        meeting_id: who.meeting_id,
        participant_id: who.participant_id,
        connected,
        at,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;

    fn parts_with(protocols: &[&str]) -> Parts {
        let mut builder = Request::builder().uri("/ws");
        for value in protocols {
            builder = builder.header(SEC_WEBSOCKET_PROTOCOL, *value);
        }
        builder.body(()).expect("request").into_parts().0
    }

    #[test]
    fn a_session_token_is_legal_in_the_subprotocol_header() {
        // The prerequisite for this whole transport. A `Sec-WebSocket-Protocol`
        // entry is an RFC 7230 `token`: no comma, no whitespace, no quoting, no
        // non-ASCII. The credential is 64 lowercase hexadecimal characters, so
        // it qualifies - and if that ever changed, this transport would have to
        // change with it rather than mangle the credential silently.
        for _ in 0..16 {
            let credential = crate::token::mint().expect("entropy").token;
            assert_eq!(credential.len(), 64);
            assert!(
                credential
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                "{credential} is not lowercase hexadecimal"
            );
            assert!(!credential.contains(','), "{credential}");
            assert!(!credential.contains(' '), "{credential}");
            assert!(credential.is_ascii(), "{credential}");
        }
    }

    #[test]
    fn the_credential_is_read_from_the_second_entry() {
        let token = "a".repeat(64);
        assert_eq!(
            offered_credential(&parts_with(&[&format!("{SUBPROTOCOL}, {token}")])).as_deref(),
            Some(token.as_str())
        );
        // Split across two header lines, which is equally legal.
        assert_eq!(
            offered_credential(&parts_with(&[SUBPROTOCOL, &token])).as_deref(),
            Some(token.as_str())
        );
    }

    #[test]
    fn anything_that_is_not_the_expected_pair_yields_nothing() {
        let token = "a".repeat(64);
        for offered in [
            vec![],
            vec![SUBPROTOCOL.to_owned()],
            vec![token.clone()],
            // The version tag must come first: no searching for the entry that
            // looks like a credential.
            vec![format!("{token}, {SUBPROTOCOL}")],
            vec![format!("lan-meeting.v0, {token}")],
            vec![format!("{SUBPROTOCOL}, {token}, extra")],
            vec![format!("{SUBPROTOCOL},,{token}")],
            vec![String::new()],
        ] {
            let borrowed: Vec<&str> = offered.iter().map(String::as_str).collect();
            assert_eq!(
                offered_credential(&parts_with(&borrowed)),
                None,
                "{offered:?}"
            );
        }
    }

    #[test]
    fn a_close_reason_never_explains_more_than_it_should() {
        // A close frame reaches the same untrusted browser an error body does.
        for farewell in [
            Farewell::ClientLeft,
            Farewell::ServerStopping,
            Farewell::SessionRevoked,
            Farewell::ResyncRequired,
            Farewell::UnexpectedFrame,
        ] {
            let reason = farewell.reason();
            assert!(reason.len() <= 123, "close reasons are capped at 123 bytes");
            for forbidden in ["sql", "token", "hash", "session_id", "participant_id"] {
                assert!(!reason.to_lowercase().contains(forbidden), "{reason}");
            }
        }
    }

    #[test]
    fn close_codes_are_distinct_and_tell_a_client_what_to_do() {
        use std::collections::HashSet;

        let codes: HashSet<u16> = [
            Farewell::ClientLeft,
            Farewell::ServerStopping,
            Farewell::SessionRevoked,
            Farewell::ResyncRequired,
            Farewell::UnexpectedFrame,
        ]
        .into_iter()
        .map(Farewell::code)
        .collect();
        assert_eq!(codes.len(), 5);

        // Forget the credential.
        assert_eq!(Farewell::SessionRevoked.code(), 4401);
        // Keep it and reconnect.
        assert_eq!(Farewell::ResyncRequired.code(), 4408);
    }

    #[test]
    fn a_frame_carries_identifiers_and_a_version_and_nothing_else() {
        let meeting_id = MeetingId::new();
        let participant_id = ParticipantId::new();
        let note_id = NoteId::new();

        let frame = EventFrame::from(&DomainEvent::NoteChanged {
            meeting_id,
            participant_id,
            note_id,
            version: 7,
            at: UtcTimestamp::now(),
        });
        let json = serde_json::to_string(&frame).expect("serializes");

        assert!(json.contains("\"type\":\"note.changed\""), "{json}");
        assert!(json.contains("\"version\":7"), "{json}");
        assert!(json.contains(&note_id.to_storage()), "{json}");
        // No session id on an event that does not target a socket.
        assert!(!json.contains("session_id"), "{json}");
    }

    #[test]
    fn only_the_revocation_frame_names_a_session() {
        let meeting_id = MeetingId::new();
        let participant_id = ParticipantId::new();
        let session_id = SessionId::new();
        let at = UtcTimestamp::now();

        let revoked = EventFrame::from(&DomainEvent::SessionRevoked {
            meeting_id,
            participant_id,
            session_id,
            at,
        });
        assert_eq!(revoked.session_id, Some(session_id));

        let locked = EventFrame::from(&DomainEvent::MeetingLocked { meeting_id, at });
        assert_eq!(locked.session_id, None);
        assert_eq!(locked.participant_id, None);

        let json = serde_json::to_string(&locked).expect("serializes");
        assert!(!json.contains("participant_id"), "{json}");
        assert!(!json.contains("note_id"), "{json}");
    }
}
