# 0018. Realtime events, audiences and presence

- Status: Accepted
- Date: 2026-09-22

Implements architecture rules section 16. Depends on ADR-0012 for the mutation
boundary, ADR-0014 for the split read path, and ADR-0002/ADR-0016 for the
session model that authentication here reuses.

## Context

Until now a participant's browser learned about a change only by asking. The
Host locked a meeting and every open page carried on offering things that would
be refused; a participant joined and the Host's roster stayed stale until they
reloaded. The architecture rules have always said what the fix looks like -
"HTTP mutation, SQLite transaction, success, WebSocket broadcast", and never the
reverse - but that leaves ten decisions to make, and they constrain each other.

This ADR settles them together.

## Decision 1: events are published by the domain, after the commit

`Domain` holds an `EventSink` and publishes on the path where the transaction
returned `Ok`, never before:

```text
transport -> Domain::<operation>
               -> BEGIN IMMEDIATE ... COMMIT
             -> EventSink::publish
```

Two properties follow, and both are asserted by tests in
`crates/app-db/tests/events.rs`:

- **An event only ever describes something durable.** A refusal, a validation
  failure and a rollback emit nothing, because the publish call sits after the
  `?` on the transaction.
- **SQLite does not depend on delivery.** `EventSink::publish` returns `()`.
  There is no error for a caller to propagate, so a disconnected client, a full
  channel or a closed window cannot undo a committed write.

Publishing from the transports instead was rejected: there are two of them, and
"the one place a rule is enforced" (ADR-0012) would have become "the one place a
rule is enforced and the two places it is announced".

## Decision 2: the audience is decided in Rust, per event variant

`DomainEvent::audience` is an exhaustive match producing one of four values:

| Audience | Reaches |
| --- | --- |
| `HostOnly(meeting)` | the Host only |
| `Participant(meeting, participant)` | every live socket of one identity, and the Host |
| `ParticipantSession(meeting, participant, session)` | exactly one socket, and the Host |
| `Meeting(meeting)` | every participant of the meeting, and the Host |

Adding a variant does not compile until its audience is chosen, and a test
pairs every variant with its expected audience so that the whole policy is one
readable table.

The current assignment: roster changes, the lifecycle before the lock, join
tokens, claims and presence are `HostOnly`; `meeting.locked` is `Meeting`;
`claim.approved` and `note.changed` are `Participant`; `session.revoked` is
`ParticipantSession`.

The Host's audience is *everything*, deliberately. They may already read every
row of their own database (ADR-0014), so withholding a notification about a row
they can read would hide nothing and only make their roster stale.

Frontend filtering was never a candidate. UI hiding is not a security
mechanism, and a participant must never *receive* another participant's event,
not merely decline to display it.

## Decision 3: payloads carry identifiers, never content

A frame carries the kind, the meeting, the relevant participant, a note id and
version where one exists, and a timestamp. It never carries note Markdown,
participant details, a credential, a token hash, or audit metadata.

A client that may read the thing that changed refetches it over HTTP or through
a command, where the read models decide what that audience may hold. This is
what keeps the socket from becoming a second, weaker copy of the API - and it
is why a dropped event costs a refetch and nothing else.

`session_id` appears on exactly one variant, `session.revoked`, because that is
the only event whose job is to name a socket.

## Decision 4: the credential travels in the subprotocol

The browser WebSocket API cannot set a request header, so the
`Authorization: Bearer` the HTTP routes use is unavailable. The client offers
two subprotocol entries and the server accepts exactly that pair:

```ts
new WebSocket(url, ['lan-meeting.v1', sessionToken]);
```

The server echoes back `lan-meeting.v1` and never the credential.

The subprotocol list is used purely as **credential transport**; it names no
application-level protocol beyond the version tag. The alternatives were each
worse:

- **a query string** puts a bearer credential in a URL, where it reaches access
  logs, `Referer` headers and browser history;
- **a cookie** is sent automatically, reintroducing the cross-site request
  forgery surface ADR-0016 avoided by choosing a bearer token;
- **a ticket endpoint** is a second credential system, with its own lifetime,
  storage and revocation semantics to get wrong.

This works because the session token is 64 lowercase hexadecimal characters,
which is a valid RFC 7230 `token` and so legal in a header field of this shape.
A unit test pins that, so a future change to the credential format has to
confront this transport rather than silently mangle it.

Resolution is the existing path, unchanged: hash the presented credential,
`SessionStore::resolve_session` (which excludes `revoked_at IS NOT NULL`), then
build `Actor::Participant` from the returned row. A browser-supplied
`participant_id`, `meeting_id` or `session_id` is never authority, and on this
route there is nowhere to put one - the URL is `/ws` and nothing else.

## Decision 5: refusals happen at the handshake

Authentication runs before the upgrade, so a failure is an ordinary HTTP status
rather than a socket that dies immediately:

| Condition | Answer |
| --- | --- |
| no subprotocol header | 401 |
| malformed protocol list | 401 |
| unknown or revoked credential | 401 |
| the meeting has gone | 404 |
| the meeting is `DRAFT` or `LOCKED` | 409 |

Once a socket exists, it closes with a code instead:

| Reason | Code |
| --- | --- |
| client left | 1000 |
| server stopping | 1001 |
| the client sent an application frame | 1003 |
| this session was revoked | 4401 |
| this socket fell behind; resync | 4408 |

4401 and 4408 are in the private-use range because no registered code
distinguishes "your credential was withdrawn" from "you missed events", and the
two call for opposite responses: forget the token, or reconnect with it.

No close reason names a credential, a session, a meeting or a SQLite error. A
close frame reaches the same untrusted browser an error body does.

## Decision 6: the actor is immutable, and nothing survives a reconnect

A socket's identity is fixed at the upgrade and cannot change. There is no
re-authentication message, no subscription message and no inbound application
protocol at all: the connection is **server to client only**. Protocol-level
ping and pong are used for liveness; a `Text` or `Binary` frame from a client
closes the socket with 1003.

That eliminates a parsing and authorization surface on the transport that faces
untrusted input, and it costs nothing - every mutation already has an HTTP
route with extractors, the domain boundary and an audit trail behind it.

Reconnecting is therefore a fresh authentication, a fresh actor and a fresh
audience, followed by the client refetching current state. There is **no event
replay**: a socket that has just opened is about to re-read everything anyway,
so replaying what it missed would tell it what it is already about to learn. A
session revoked while disconnected simply fails the handshake.

## Decision 7: revocation targets a session, not a person

The Host ends *a session*. `session.revoked` carries the `session_id` from the
row the transaction acted on, and only the socket that authenticated with that
exact id closes.

A participant with a laptop and a phone, holding two sessions, keeps the other
one. Presence stays `connected` while any socket of theirs remains. Closing
every socket a participant happens to have open would make revocation a blunter
instrument than the domain's own model, which has always been per-session
(ADR-0002 rule 5).

The `session_id` used for this comes from the authenticated connection state,
never from a client frame - which, given decision 6, is not a thing that exists.

## Decision 8: the Host is not a WebSocket client

```text
Domain
 └── CompositeSink
      ├── Realtime        -> participant WebSockets, when the server runs
      └── TauriEventSink  -> the Host window, over IPC, always
```

Giving the Host UI a socket to loopback was rejected:

- the Host window is in the **same process** as the database, so a network
  round trip to reach itself is pure cost;
- it would only work while the LAN server is running, and the Host may prepare
  a whole meeting - roster, configuration, join token - without ever starting
  it (ADR-0016);
- it would put the Host's own view on the transport that exists to face
  untrusted input, and tie a local UI concern to a firewall prompt.

One sink fans out to both, so neither audience can be told about something the
other was not.

## Decision 9: presence is in memory, and `last_seen_at` is an observation

Connectivity lives in an in-memory registry counting open sockets per identity.
It is **not** a column, and that is deliberate: a Host machine that loses power
gets no chance to write "everybody disconnected", so a `connected` column would
outlive the truth and have to be distrusted on every read. A column that must be
distrusted is worse than no column. After a restart the registry is empty, which
is true.

Transitions are emitted on `0 -> 1` and `1 -> 0` only, so a second tab cannot
make a roster flicker.

`participant_sessions.last_seen_at` is the durable half and means exactly:
**the last moment a socket for that session was observed to open or close.** It
is not a heartbeat. A participant connected and quiet for an hour still shows
the moment they connected, and a socket that dies without a close frame is
noticed only when the keepalive notices it - so the value can lag reality by up
to one keepalive interval.

Writing it per ping was rejected: it would turn an idle meeting of 99
participants into a steady stream of writes against the single writer
connection, to keep a column marginally fresher than the event stream that
already carries the same news.

Presence is **not** audited. The audit log records what people did to the
meeting (architecture rules section 17), and a socket opening is not that;
burying the real entries under connection noise would make the log unreadable.

Presence grants **no authority**. Nothing in `authorize` consults it, and a test
asserts that a connected participant is refused exactly what a disconnected one
is.

The presence write is the one database write that does not go through `Domain`.
It is not authorized (the session id comes from an already-authenticated
connection), not audited, and not subject to the meeting lock - a locked meeting
still has people looking at it. It lives in `app_db::presence`, can set one
column on one row, and refuses a revoked session.

## Decision 10: a locked meeting does not close sockets

Locking broadcasts `meeting.locked` to the room and leaves everybody connected.
The UI stops offering to edit; the backend refuses to edit regardless, because
every mutation re-reads the meeting's status inside its own transaction
(architecture rules section 15).

A participant who misses the event, ignores it, edits the bundle or calls the
API by hand receives the same refusal as one who saw it. The event makes the
screen honest; it never makes the meeting safe.

Establishing a *new* socket does require an open meeting, because joining a
meeting that is finished is not a thing to start doing.

## Decision 11: bounded channel, no delivery guarantees

A bounded `tokio::sync::broadcast` channel, 256 events deep. A socket that falls
behind that far is closed with 4408 and reconnects and refetches.

That is the correct trade precisely because the channel is not the source of
truth. Dropping a hint costs a refetch, and a refetch is what a reconnect does
anyway. Explicitly **not** implemented: a broker, a durable event log, delivery
acknowledgements, at-least-once semantics, per-client queues.

An unbounded queue was the alternative, and it means a participant whose phone
went into a tunnel gets memory allocated on the Host's machine on their behalf
until somebody notices.

## Consequences

- One `EventSink`, one `Domain`, one database. Adding a `DomainEvent` variant
  forces an explicit audience decision at compile time.
- The LAN transport gains a second credential-bearing header. It resolves
  through the same session lookup as the first, so there is one revocation
  story and not two.
- `app-server` gains a write path it did not have (the presence timestamp). The
  structural guard against raw database primitives is unchanged and still
  non-vacuous; the write goes through a typed `app-db` store.
- The boundary guard that forbade realtime code now allows WebSocket and
  continues to reject relays, tunnels, hole punching, outbound HTTP clients,
  `EventSource` and polling timers.
- Stopping the LAN server now signals open sockets before shutting down.
  Without that, a graceful stop would wait for connections that have no reason
  to end, and "stopped" would never mean the port was free.
- No migration. `participant_sessions.last_seen_at` already existed.
- The remote form is untouched and remains fully offline: it has no server to
  talk to and gains none here.
