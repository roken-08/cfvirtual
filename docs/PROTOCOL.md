# Room WebSocket protocol v1

The server accepts JSON **text** frames at `/` and `/ws`, over `ws://` (or
`wss://` behind a TLS proxy). Each request and response includes integer `v: 1`
and a snake-case `type`. Other versions produce `VERSION_MISMATCH` and close
the connection. Unknown types are ignored; malformed known messages produce
`INVALID_MESSAGE`. WebSocket ping/pong control frames are also supported.

The server owns the clock, contest, polling, bot simulation and scores. Clients
render snapshots and send actions; they never send scores or submission verdicts.
Rooms are trust-based: validating a public Codeforces handle does not prove its
ownership.

## Client requests

```json
{"v":1,"type":"hello","code":"K7QM2X","handle":"guest","password":"optional"}
{"v":1,"type":"reconnect","code":"K7QM2X","token":"private-session-uuid"}
{"v":1,"type":"spectate","code":"K7QM2X","password":"optional"}
{"v":1,"type":"start"}
{"v":1,"type":"end"}
{"v":1,"type":"ping","t":1730000000000}
```

`password` can be omitted or null. Codes are case-insensitive six-character
strings drawn from `23456789ABCDEFGHJKMNPQRSTUVWXYZ` (no `0`, `O`, `1`, `I`, `L`).

- `hello`: validate the public handle with `user.info`, then reserve a player
  slot. Handles compare case-insensitively. An existing handle needs its token,
  even if its previous socket disconnected. Players may join only in the lobby.
- `reconnect`: resume any phase, bypassing handle validation and the password.
  The token identifies the participant and its role. The new socket replaces
  the previous one; stale actions/disconnects cannot affect the replacement.
- `spectate`: reserve a spectator slot in any phase. No handle or CF lookup is
  required. A generated public spectator name and private reconnect token are
  returned. Spectators cannot start/end or receive per-problem player details.
- `start`: host-only, lobby-only. Sets the authoritative Unix-second start time
  and starts polling. Prepared problems remain hidden in the lobby.
- `end`: host-only, running-only. The UI must obtain confirmation **before**
  sending this action. Freezes the inclusive submission cutoff at the earlier
  of now and the original deadline, then drains final verdicts.
- `ping`: valid before or after joining. `t` is an opaque signed 64-bit client
  millisecond timestamp, echoed by `pong`.

One socket belongs to one room. A second handshake on an attached socket gets
`ALREADY_JOINED`; open another connection to change rooms.

### Remote creation extension

Headless servers support creating and attaching the host in one request:

```json
{"v":1,"type":"create","options":{"handle":"host","problems":5,"duration_sec":7200,"rating":null,"tags":[],"bots":0,"seed":42,"blind":true,"refresh":false},"password":null}
```

All `options` fields above are required and match `service::ContestOptions`.
`rating` is an optional unsigned integer. The server invokes
`service::prepare(options, api, data_dir, "room")`, validates the host through
that flow, installs the lobby, and replies with `welcome`. Preparation failures
do not reserve a room or join the socket; retry is allowed. Preparation has a
180-second timeout; join validation has a 90-second timeout. The protocol is
sequential per socket, so use other sockets while preparation is in progress.

## Server responses

Every example below also has `v: 1`. Type names and field names are stable.

| Type | Fields |
|---|---|
| `welcome` | `participant_id: string`, `token: string`, `state: Snapshot` |
| `roster` | `participants: PublicParticipant[]`, `bots: PublicBot[]`, `settings: Settings` |
| `contest_started` | `contest_id: string`, `started_at: i64`, `duration_sec: u32`, `problems: PublicProblem[]`, `blind: bool`, `server_time: i64` |
| `tick` | `server_time: i64`, `standings: PublicStanding[]`, `problem_status: OwnProblemStatus`, `state: Snapshot` |
| `contest_ended` | `reason: "timeout" \| "host"`, `standings: PublicStanding[]`, `state: Snapshot` |
| `error` | `code: string`, `message: string` |
| `pong` | `t: i64`, `server_time: i64` |

`welcome` is sent first for successful handshakes, followed by roster/full tick.
Tokens are bearer credentials and appear **only in that participant's own
welcome**. `participant_id` is a separate, public UUID; it does not authorize
anything. Roster changes and score polls trigger snapshots; the server also
broadcasts full snapshots every five seconds and on elapsed bot events.

All times are Unix **seconds**, except **both** `pong.t` and `pong.server_time`
which use milliseconds. Penalties and acceptance offsets use seconds. A client
can estimate its clock offset with a ping round trip and compute
`max(0, deadline - estimated_server_now)`. `deadline` already reflects early end.

### Full snapshot schema

```text
Snapshot {
  code: string, contest_id: string, host_handle: string,
  phase: "lobby" | "running" | "finishing" | "ended",
  started_at: i64,                 // zero in lobby
  deadline: i64 | null,            // inclusive cutoff; null in lobby
  ended_at: i64 | null,            // cutoff when finishing/ended
  end_reason: "timeout" | "host" | null,
  server_time: i64,
  settings: Settings,
  participants: PublicParticipant[], bots: PublicBot[],
  problems: PublicProblem[], standings: PublicStanding[],
  problem_status: OwnProblemStatus
}
Settings {
  duration_sec: u32, problem_count: unsigned integer,
  blind: bool, password_required: bool
}
PublicParticipant {
  participant_id: string, handle: string,
  is_host: bool, is_spectator: bool, connected: bool
}
PublicBot { name: string, rating: u32, is_bot: true }
PublicProblem {
  slot: single-character string, contest_id: u32, index: string,
  name: string, url: string, rating: u32 | null, tags: string[]
}
PublicStanding {
  name: string, is_bot: bool, rating: u32,
  solved: u32, penalty: u64, last_ac: u32
}
OwnProblemStatus = { "recipient_handle": { "A": ProblemStatus, ... } }
ProblemStatus {
  solved: bool, accepted_at_sec: u32 | null,
  rejected: u32, attempts: u32, first_attempt_at_sec: u32 | null
}
```

Only the recipient's own per-problem statuses are present, even at the end.
Spectators receive `{}`. Public standings **never** contain a `problems` map.
Snapshots contain no participant tokens, submissions, seed, or bot solve
schedule. Bots are always marked `is_bot: true`; only elapsed bot events affect
scores. Problem lists are empty in the lobby. In blind mode, ratings are null
and tags empty until `ended`, at which point the full problem metadata appears.
The full recipient-filtered state in `welcome`, `tick` and `contest_ended` is
sufficient to rebuild lobby, contest and results screens after reconnecting.

### Polling and finalization

The production backend uses the shared `CfClient` limiter (at least two seconds
between process-wide CF calls). A room polls all human handles, including
disconnected players, in order, on a 20-second cycle; a slow API may stretch the
cycle. Spectators and bots never cause API calls. Only matching authors,
selected problem keys and timestamps in the inclusive contest window are kept.
The scoring function is `domain::scoring::score` for both humans and bots.

The timer automatically changes `running` to `finishing` at the deadline.
Finalization runs **two additional full polling passes after the cutoff**, with
a two-second gap. A submission created in-window but judged during those passes
can still count; a submission created after the cutoff never does. Individual
recent calls time out after 15 seconds. An in-flight normal call may finish
first (at most 15 seconds), then final draining is bounded to 180 seconds total.
The UI should show zero remaining time and "finalizing" during this phase.
`contest_ended` is emitted only when scores are frozen. Verdicts arriving later
than this bounded drain are not counted. API errors retain observed results.

Final contest and full standings are saved through `Store::save_contest` on a
blocking task, with participant tokens cleared. Save failures produce a warning
and do not terminate a room. `contest_ended` may precede completion of the save.
Rooms/sessions are in-memory and do not survive a server restart.

## Limits and errors

- 32 players **including the host**, eight spectators, 16 bots per room.
  Disconnected members retain their slots for reconnect; spectators do too.
- 128 retained rooms per server instance; restart to clear old rooms.
- 1–26 problems, duration 1–86400 seconds. Passwords at most 1024 UTF-8 bytes.
- Incoming/outgoing frames/messages at most 64 KiB. Outbound queues hold 32
  messages per socket. A full queue, oversized output or five-second write
  timeout disconnects that socket; it can reconnect and recover a full state.
- An unattached connection must send its next request within 30 seconds.

Typical error codes: `ROOM_NOT_FOUND`, `BAD_PASSWORD`, `BAD_TOKEN`,
`INVALID_HANDLE`, `HANDLE_IN_USE`, `CONTEST_STARTED`, `ROOM_FULL`, `SERVER_FULL`,
`NOT_JOINED`, `ALREADY_JOINED`, `NOT_HOST`, `INVALID_STATE`, `INVALID_OPTIONS`,
`INVALID_PASSWORD`, `INVALID_MESSAGE`, `VERSION_MISMATCH`, `PREPARE_FAILED`,
`CF_UNAVAILABLE`, `SLOW_CLIENT`. Errors normally permit another request;
version mismatch closes the connection, oversized input closes at the transport
layer. Messages never echo request passwords/tokens or raw backend errors.

A room code is not authentication. Passwords are a soft gate, stored only as
salted SHA-256 hashes in server memory, never written to the database. Tokens
are private UUIDv4 bearer credentials, not included in logs, roster or exports.
Use TLS termination for untrusted networks: plain `ws://` exposes traffic.

## Rust integration APIs

```rust,no_run
use cfvirtual::{cf::CfClient, server::{self, State}, service,
    client::net::{Client, ClientMessage, ReconnectSession}};

# async fn example(options: service::ContestOptions, data_dir: std::path::PathBuf) -> anyhow::Result<()> {
let api = CfClient::new();
let state = State::new(api.clone(), data_dir.clone());
let contest = service::prepare(&options, &api, &data_dir, "room").await?;
let created = state.create_room(contest, None).await?;
let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
let url = format!("ws://{}", listener.local_addr()?);
let app = server::router(state.clone());
tokio::spawn(async move { axum::serve(listener, app).await });
let session = ReconnectSession {
    room: created.code,
    participant_token: created.host_token,
};
let mut host = Client::resume(&url, &session).await?;
// resume consumes welcome and caches its full state:
let _lobby = host.snapshot();
host.send(ClientMessage::Start).await?;
while let Some(event) = host.recv().await? {
    // Render event, or use host.snapshot() for the latest full state.
    // Clone host.session() into memory before dropping/reconnecting transport.
    let _ = event;
}
# Ok(()) }
```

For headless `serve`, just bind and `axum::serve(listener, server::router(state))`.
For remote `room create --server`, connect, send `ClientMessage::Create`, and
receive `ServerMessage::Welcome`; `Client::session()` then contains the host
token. `Client::send(ClientMessage)` adds the version envelope automatically;
`recv()` returns `Result<Option<ServerMessage>>` and caches full state/session.
`close()` closes gracefully. Retry/backoff and the UI's event/input `select!`
loop are caller policy; ordinary `recv()` cancellation is safe. `resume()` waits
for welcome and returns an attached client, surfacing handshake errors.

Direct serde users serialize `ClientEnvelope::from(message)` and deserialize
`ServerEnvelope`; enums alone contain `type` but not `v`.

Offline tests use `State::with_backend(Arc<dyn RoomBackend>, data_dir)`.
This is an explicit trusted server adapter for validation, public recent
submissions and preparation, not a runtime validation-disable flag or a wire
feature. Production `State::new` always uses real CF validation and room-mode
preparation. `create_room` accepts a **trusted, already-prepared host contest**;
it resets start/end/code/tokens and initializes only the host. Other players
must use `hello`. Neither constructor binds TCP.
