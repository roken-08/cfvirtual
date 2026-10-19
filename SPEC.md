# CFVirtual — Build Specification

**Deliverable:** a working, locally buildable Rust project named `cfvirtual`: a terminal-based virtual contest runner for Codeforces with solo mode, multiplayer rooms, simulated opponents, live leaderboard, timer, problem links, and post-contest review.

**Audience:** an autonomous coding agent. Read this document fully, then build in the milestone order in §20. Do not stop to ask questions; where a detail is unspecified, choose the simplest option consistent with this document and record the choice in `DECISIONS.md`.

---

## 0. Ground rules

- Build a new repository from scratch. Do not depend on, import, or reference any private or external practice-plan project.
- No personal data, usernames, machine paths, or environment-specific assumptions in code, docs, tests, or examples.
- Codeforces integration is **read-only public API**. Never ask for a password, API key, cookie, or session token.
- Never scrape HTML. Never automate submissions. Users submit in their browser; CFVirtual detects results by polling the public API.
- Bots are synthetic. Never reuse real Codeforces handles for bots. Mark every bot clearly in the UI.
- Keyboard-only operation. It must work over SSH and in an 80×24 terminal.
- License: MIT. Language in docs: neutral English.

---

## 1. Product

CFVirtual runs a practice contest without needing a real Codeforces round:

1. It reads a player's public submission history.
2. It computes per-tag weaknesses.
3. It assembles a fresh set of unsolved problems weighted toward those weaknesses and the player's rating.
4. It runs a timed contest in a TUI with a live leaderboard and links to each problem.
5. It detects accepted submissions from the Codeforces API and scores them automatically.
6. It produces a post-contest review and queues failed problems for later practice.

The core loop is the product: **contest → automatic scoreboard → post-mortem → next contest**.

Two modes:

- **Solo:** one human + optional simulated bots.
- **Room:** a host creates a room; other players join with a code; the contest is built from the host's profile; everyone competes on one authoritative leaderboard.

---

## 2. Scope

### Must have (MVP)

- `cfvirtual start` solo contest with configurable problem count, duration, tags, rating window.
- `cfvirtual room create` / `cfvirtual join` over a local WebSocket server.
- Public Codeforces history fetch, weak-tag analysis, problem selection.
- Timer, problem list with openable links, ICPC scoring, live leaderboard.
- Bots with fictional handles and plausible behavior.
- SQLite persistence, contest history, post-contest summary.
- Markdown review export.
- `cfvirtual demo` offline demo mode for screenshots/tests.

### Should have

- Review queue with spaced repetition (`cfvirtual review`).
- `cfvirtual stats`.
- JSON output mode for all read commands.
- Spectator join.
- Reconnect after disconnect.
- Prebuilt release binaries and an AUR PKGBUILD.

### Not in scope

- Automatic submission, code execution, judging, or sample testing.
- Account systems, global public matchmaking, or a hosted service.
- Real-time chat (leave a protocol hook; do not build UI).
- Anti-cheat guarantees. Rooms are trust-based.
- LeetCode or other platforms.

---

## 3. Stack

- Rust, edition 2021 or later, single binary.
- TUI: `ratatui` + `crossterm` (+ `unicode-width`).
- Async/runtime: `tokio`.
- HTTP: `reqwest` with `rustls-tls` and `json`.
- Server: `axum` with `ws` feature.
- Serialization: `serde`, `serde_json`, `toml`.
- CLI: `clap` (derive).
- Persistence: `rusqlite` with `bundled` feature.
- Time/IDs/randomness: `chrono`, `uuid`, `rand`, `rand_chacha`.
- Misc: `anyhow`, `thiserror`, `tracing`, `tracing-subscriber`, `dirs`, `arboard` (clipboard), `open` (browser), `url`, `base32`.
- Do not add a database ORM, a web frontend, or a plugin system.

Minimum Rust: current stable. CI must pin a toolchain file.

---

## 4. Repository layout

```
cfvirtual/
├── Cargo.toml
├── Cargo.lock
├── rust-toolchain.toml
├── README.md
├── LICENSE
├── CHANGELOG.md
├── DECISIONS.md
├── docs/
│   ├── PROTOCOL.md
│   ├── SELF_HOSTING.md
│   └── DEMO.md
├── src/
│   ├── main.rs
│   ├── cli.rs
│   ├── config.rs
│   ├── cf/
│   │   ├── mod.rs
│   │   ├── client.rs      # rate-limited API client
│   │   ├── cache.rs       # problemset cache
│   │   └── models.rs      # API deserialization types
│   ├── domain/
│   │   ├── mod.rs
│   │   ├── problem.rs
│   │   ├── contest.rs
│   │   ├── submission.rs
│   │   ├── scoring.rs
│   │   ├── weakness.rs
│   │   ├── selection.rs
│   │   ├── bots.rs
│   │   └── review.rs
│   ├── store/
│   │   ├── mod.rs
│   │   └── db.rs
│   ├── server/
│   │   ├── mod.rs
│   │   ├── room.rs        # lobby state machine
│   │   ├── ws.rs          # websocket handler
│   │   ├── poller.rs      # CF polling loop
│   │   └── proto.rs       # wire messages
│   ├── client/
│   │   ├── mod.rs
│   │   └── net.rs
│   └── tui/
│       ├── mod.rs
│       ├── app.rs
│       ├── lobby.rs
│       ├── contest.rs
│       ├── results.rs
│       ├── history.rs
│       ├── widgets.rs
│       └── theme.rs
└── tests/
    ├── selection.rs
    ├── scoring.rs
    ├── bots.rs
    └── room_flow.rs
```

---

## 5. CLI

Binary: `cfvirtual`. Every command supports `--help`.

```
cfvirtual start [--handle H] [--problems N] [--duration MIN] [--rating R]
                [--tags a,b] [--bots N] [--seed S] [--blind] [--json]
cfvirtual room create [same options] [--port 7331] [--listen 0.0.0.0]
                      [--password P] [--public-url URL]
cfvirtual join --server URL --code CODE --handle H [--spectate]
cfvirtual serve [--listen 0.0.0.0:7331] [--public-url URL]
cfvirtual history [--json] [--limit N]
cfvirtual review [--due] [--limit N] [--json]
cfvirtual stats [--json]
cfvirtual config [get|set] [key] [value]
cfvirtual demo [--seed S]
```

Rules:

- Defaults: `--problems 5`, `--duration 120`, `--bots 4` in solo, `--bots 0` in rooms unless set.
- Host profile: if `--handle` is absent, use `config.handle`; if that is absent, prompt once and save.
- Room contest parameters come from the host. Joining players cannot change the problem set.
- `room create` runs the server in the same process and opens the lobby TUI.
- `join` connects to an already-running server. `serve` is headless for self-hosting.
- `demo` needs no network: generates a fixed 6-problem set, 3 bots, and a 90-second contest using pre-baked data.
- All read commands support `--json` and must emit only JSON on stdout when set.
- Exit codes: `0` success, `1` runtime error, `2` usage error, `3` Codeforces unavailable.

Example:

```
cfvirtual start --handle your_handle --problems 5 --duration 120 --bots 4
cfvirtual room create --port 7331 --problems 5 --duration 120 --bots 3
cfvirtual join --server ws://192.168.1.20:7331 --code K7QM2X --handle your_handle
```

---

## 6. Config and data

Config: `~/.config/cfvirtual/config.toml` (respect `XDG_CONFIG_HOME`).
Data: `~/.local/share/cfvirtual/` (respect `XDG_DATA_HOME`): `cfvirtual.db`, `problemset.json`.

Example config:

```toml
handle = "your_handle"
server = "ws://127.0.0.1:7331"
theme = "default"
default_problems = 5
default_duration = 120
default_bots = 4
open_command = ""
```

- Never store credentials.
- `config set` validates keys and writes atomically.
- Unknown keys are ignored with a warning, never fatal.

---

## 7. Domain model

Implement these as plain Rust structs with serde support where needed:

```rust
struct ProblemKey { contest_id: u32, index: String } // index like "A", "B1"

struct Problem {
    key: ProblemKey,
    name: String,
    rating: Option<u32>,
    tags: Vec<String>,
    solved_count: Option<u32>,
}

struct ContestProblem {
    slot: char,            // display label A..E (contest-local)
    problem: Problem,
    tier: Tier,            // Weak | Core | Stretch
}

enum Tier { Weak, Core, Stretch }

struct Contest {
    id: String,            // uuid
    code: Option<String>,  // room code if multiplayer
    host_handle: String,
    started_at: i64,       // unix seconds, authoritative
    duration_sec: u32,
    problems: Vec<ContestProblem>,
    participants: Vec<Participant>,
    bots: Vec<Bot>,
    seed: u64,
}

struct Participant { handle: String, is_host: bool, is_spectator: bool, token: String }

struct Bot { name: String, rating: u32, solves: Vec<BotSolve> }

struct Solve {
    slot: char,
    accepted_at_sec: u32,  // seconds after start
    rejected_attempts: u32,
}
```

---

## 8. Codeforces integration

Base: `https://codeforces.com/api/`.

Endpoints used:

1. `problemset.problems` — full problem list. Cache to disk for 24h. Refresh on `--refresh`.
2. `user.status?handle=H&from=1&count=10000` — full submission history for weakness analysis.
3. `user.status?handle=H&from=1&count=100` — recent submissions for live polling.
4. `user.info?handles=H` — validate a handle and read current rating. Missing rating → default 1200.

Requirements:

- A single process-wide limiter: **at least 2.0 seconds between any two API calls**. Queue requests through one async mutex.
- On HTTP 429 or `status: FAILED` with "Call limit exceeded": back off 5s, retry up to 3 times with jitter.
- Descriptive `User-Agent`: `cfvirtual/<version> (open-source practice tool)`.
- Treat API failure as non-fatal: show a clear error, keep local state, allow retry.
- Submission objects: use `contestId`, `problem.index`, `creationTimeSeconds`, `verdict`, `author.members[].handle`.
- Accepted means `verdict == "OK"`.
- Problem URLs:
  - Page: `https://codeforces.com/problemset/problem/{contestId}/{index}`
  - Never construct submit URLs; open the problem page only.

History fetch rules:

- Consider only submissions with `contestId` present.
- Deduplicate by problem key. A problem counts as solved if any submission is OK.
- Filter out non-standard problems when selecting: contests with `April Fools`, `Kotlin Heroes`, `Q#`, `Unknown Language`, or no rating.
- Cap accepted-problem history for analysis at the most recent 10,000 submissions.

---

## 9. Weakness model

Compute from history only; no ML, no external services.

For each tag `t`:

```
attempted[t]  = number of distinct problems with tag t that were submitted
solved[t]     = number of those with an OK submission
subs[t]       = total submissions on those problems
ac_subs[t]    = number of OK submissions
solve_rate[t] = solved[t] / attempted[t]
efficiency[t] = ac_subs[t] / subs[t]
recent_fail[t] = share of non-OK submissions in the last 90 days on tag t, decayed by age
weakness[t] = 0.5*(1 - solve_rate[t])
            + 0.3*(1 - efficiency[t])
            + 0.2*recent_fail[t]
```

- Require `attempted[t] >= 3`; otherwise shrink toward the mean (add a small prior).
- Normalize all tag weights to sum to 1.
- Track `median_solved_rating` per tag; expose it in `stats`.
- If history is empty, fall back to uniform weights and the provided/default rating.

---

## 10. Contest assembly

Inputs: rating `R` (from `user.info` unless `--rating`), solved set, tag weakness weights, count `N`, optional tag filter, seed.

Algorithm (deterministic for a given seed):

1. Build candidate pool: unsolved, has rating, passes §8 filters, `R-200 <= rating <= R+400`.
2. Split slots: 40% weak, 40% core, 20% stretch (round down; remainder to core).
   - Weak: sample tags by weakness weight; candidate must carry a chosen tag.
   - Core: any tag; prefer rating in `[R-100, R+200]`.
   - Stretch: rating in `[R+200, R+400]`.
3. Within a slot, weight candidates by tag weakness sum plus a small random jitter. Use `rand_chacha` seeded from `seed`.
4. Never pick the same problem twice. Avoid duplicate problem names.
5. If a slot cannot be filled, relax the rating window by 100 in each direction, then fill from the remaining pool.
6. Sort selected problems by rating ascending; assign display slots `A..E` in that order.
7. If fewer than `N` candidates exist, fail with a clear message suggesting `--rating` or fewer problems.

Blind mode:

- `--blind` (default in rooms) hides rating and tags until the contest ends.
- The data is still present; the TUI simply redacts it. `b` toggles reveal only when the contest is over, unless the host started with `--reveal`.

Persist the seed and the full problem list so a contest can be replayed offline.

---

## 11. Scoring

ICPC-style:

- A problem is solved at the first OK submission after the start time.
- `ac_time = accepted_at_sec` (seconds since start, floored).
- `penalty = ac_time + 20*60*rejected_attempts_before_ac`.
- Rejected attempts after AC are ignored.
- Only submissions on the selected problem keys, with `creation_time >= started_at` and `<= started_at + duration`, count.
- Rank by: solved count desc, penalty asc, last AC time asc, name asc.
- Unsolved problems contribute no penalty.
- Track per-problem attempts and first-AC time for the review.

Timer:

- Server/host start time is authoritative. Clients compute an offset from a ping and display `remaining = duration - (now + offset - started_at)`.
- The contest ends automatically at the deadline. Late submissions do not count.
- Allow the host to end early; ask for confirmation.

---

## 12. Bots

Purpose: make the leaderboard feel alive without external services.

Generation:

- Names must be fictional and never match known real handles. Use an adjective+noun+2-digit-number generator with a curated word list, e.g. `silent_comet_42`, `amber_kite_07`. Never fetch names from the API.
- Add a `BOT` badge in every TUI view and in exports.
- Ratings sampled around the host rating: `R + [-250, +250]`, clamped to `[800, 3500]`.
- Default count: 4 in solo, 0 in rooms (host may set `--bots`).

Simulation (deterministic from the contest seed):

- For each bot and problem, compute `d = bot_rating - problem_rating`.
- `p_solve = 1 / (1 + exp(-(d + 100) / 250))`, clamped to `[0.05, 0.95]`.
- If solved, sample solve time from a log-normal centered at `25 - d/40` minutes, clamped to `[2, duration]`.
- With probability 0.25, add 1–3 rejected attempts before acceptance, adding 20-minute penalties.
- Bots never react to humans; they emit their scheduled events as time passes.
- In rooms, only the host/server simulates bots and broadcasts their state.

`cfvirtual demo` uses this model with a fixed seed and shortened times so it can be recorded in under 90 seconds.

---

## 13. Solo mode flow

1. Resolve handle and rating.
2. Load cached problemset; refresh if stale.
3. Fetch history; compute weaknesses.
4. Assemble contest; start timer immediately after a 3-second countdown.
5. Show contest screen. Poll `user.status` every 20s in a background task, filtered to contest problems.
6. Update leaderboard on each accepted submission and each bot event.
7. At the end, show results and write to the database.
8. Offer `e` to export a Markdown review and `r` to queue failed problems for review.

Solo must work offline if the problemset is cached and `--handle` history is unavailable: fall back to uniform weights.

---

## 14. Multiplayer

### Architecture

- One authoritative **room server**, embedded in `room create` or run headless via `serve`.
- Clients connect over WebSocket; server broadcasts a full leaderboard snapshot on every change and every 5 seconds.
- Server owns: room state, contest start time, problem set, bot simulation, CF polling, scoring.
- Clients own: rendering and local input. Clients never compute authoritative scores.
- Rooms are identified by a 6-character code using Crockford base32, excluding ambiguous characters (`0/O`, `1/I/L`).

### Flow

1. Host runs `room create`. Server binds, generates code and host token, prints the join command and LAN IP.
2. Optional room password; clients are prompted if required.
3. Players join with `join --server --code --handle`. Server validates the handle with `user.info`.
4. Lobby shows roster, host settings, bots, and connection status. Spectators may join with `--spectate`.
5. Host starts. Server selects problems from the host profile, broadcasts `contest_started`, and starts polling.
6. Server polls every 20s per human handle, respecting the 2s global API limit, and processes only in-window submissions on contest problems.
7. Late joiners are rejected once running; spectators may join anytime.
8. Reconnect: client stores `{room, participant_token}` for the session and replays `hello`; the server replies with a full snapshot.
9. On end, server broadcasts final standings; host can export; every participant can see their own summary.

### Networking details

- Default bind `0.0.0.0:7331`; print a warning that this exposes the port on the LAN.
- `--public-url` lets the host advertise a tunnel URL.
- Optional TLS is out of scope; document reverse-proxy and `cloudflared` usage in `docs/SELF_HOSTING.md`.
- Room code is not authentication. Password is a soft gate, hashed in memory only. Say so in docs.
- Bound memory: max 32 participants, max 8 spectators, max 16 bots, max message size 64 KiB, per-client send queue cap.
- Protocol version field `v: 1`; reject mismatches with a clear error.

### Wire protocol (JSON text frames)

Client → server:

```json
{"v":1,"type":"hello","code":"K7QM2X","handle":"your_handle","password":"optional"}
{"v":1,"type":"reconnect","code":"K7QM2X","token":"uuid"}
{"v":1,"type":"spectate","code":"K7QM2X","password":"optional"}
{"v":1,"type":"start"}
{"v":1,"type":"ping","t":1730000000000}
```

Server → client:

```json
{"v":1,"type":"welcome","participant_id":"uuid","token":"uuid","state":{}}
{"v":1,"type":"roster","participants":[],"bots":[],"settings":{}}
{"v":1,"type":"contest_started","contest_id":"uuid","started_at":1730000000,"duration_sec":7200,
 "problems":[{"slot":"A","name":"...","url":"...","rating":800,"tags":["greedy"]}],
 "blind":true,"server_time":1730000000}
{"v":1,"type":"tick","server_time":1730000123,"standings":[{"name":"...","is_bot":false,"solved":2,"penalty":145}],
 "problem_status":{"your_handle":{"A":{"solved":true,"accepted_at_sec":412,"rejected":1}}}}
{"v":1,"type":"contest_ended","reason":"timeout","standings":[]}
{"v":1,"type":"error","code":"ROOM_NOT_FOUND","message":"..."}
{"v":1,"type":"pong","t":1730000000000,"server_time":1730000000123}
```

Rules:

- All messages include `v`.
- Unknown message types are ignored, not fatal.
- Server never sends full submission histories to other clients; only the aggregated leaderboard and each player's own per-problem status.
- Document the full schema in `docs/PROTOCOL.md`.

---

## 15. TUI

Use one `App` state machine with screens: `Setup`, `Lobby`, `Contest`, `Results`, `History`, `Review`, `Help`.

Layout rules:

- Minimum 80×24; degrade gracefully at 60×18.
- No Nerd Font requirement. Use ASCII/Unicode that renders in any common terminal; provide `--ascii`.
- Colors from a theme module; include a colorblind-safe theme. Never rely on color alone — always show text/status glyphs.
- Responsive: hide the standings panel below 100 columns; stack vertically.

Contest screen:

```
┌ CFVirtual · room K7QM2X · 01:23:45 remaining · 3/5 solved · penalty 87 ┐
│ #  Problem                        Rating  Status      Time   Tries   │
│ A  1872A Plus Minus               800     AC          00:06  1       │
│ B  1857C Assembly via Minimums    1200    WA (poll)   —      2       │
│ C  1832C Contrast Value           1200    unsolved    —      0       │
├ Standings ────────────────────────────────┬ Details ──────────────────┤
│ 1  your_handle          3     87          │ Tags: greedy, math        │
│ 2  silent_comet_42 BOT  2     54          │ Link: codeforces.com/...  │
│ 3  amber_kite_07 BOT    1     12          │ [o] open  [y] copy  [b]  │
└───────────────────────────────────────────┴───────────────────────────┘
```

Keybindings:

| Key | Action |
|---|---|
| `j`/`k`, arrows | Move selection |
| `Enter` | Open detail / confirm |
| `o` | Open selected problem in browser |
| `y` | Copy problem URL |
| `s` | Re-open the selected problem (submission stays manual) |
| `Tab` | Cycle panes |
| `b` | Reveal ratings/tags (only after end, or if allowed) |
| `p` | Pause overlay (solo only) |
| `e` | Export review (results) |
| `r` | Retry connection (client) |
| `?` | Help |
| `q` | Back; confirm before abandoning |

Lobby:

- Show code, server URL, join command, roster with connection dots, settings, and a copy key.
- Host: `Enter` starts; non-host: read-only.
- Clearly label bots and spectators.

Results:

- Final standings, per-problem table, link list, and review hints.
- Show "should have solved", "solid", or "stretch" per problem based on rating delta.
- Export writes `cfvirtual-review-YYYY-MM-DD-HHMM.md` to the current directory via `e`.

Copy/open behavior must fail gracefully on headless machines: print the URL instead of erroring.

---

## 16. Persistence

SQLite, migrations embedded in the binary.

```sql
CREATE TABLE contests (
  id TEXT PRIMARY KEY, code TEXT, host_handle TEXT, started_at INTEGER,
  duration_sec INTEGER, seed INTEGER, mode TEXT, problems_json TEXT,
  created_at INTEGER
);
CREATE TABLE participants (
  contest_id TEXT, handle TEXT, is_host INTEGER, is_bot INTEGER,
  rating INTEGER, solved INTEGER, penalty INTEGER,
  PRIMARY KEY (contest_id, handle)
);
CREATE TABLE results (
  contest_id TEXT, handle TEXT, slot TEXT, accepted_at_sec INTEGER,
  rejected_attempts INTEGER, rating INTEGER, tags_json TEXT,
  PRIMARY KEY (contest_id, handle, slot)
);
CREATE TABLE review_queue (
  problem_key TEXT PRIMARY KEY, contest_id TEXT, added_at INTEGER,
  due_at INTEGER, interval_days INTEGER, ease REAL, lapses INTEGER
);
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);
```

Review scheduling: intervals `1, 3, 7, 14, 30` days. `review --due` lists due problems with links; solving or skipping updates the interval. Keep it simple and deterministic.

`history` lists past contests with mode, date, problems, solved/penalty. `stats` shows totals, per-tag solve rate, average penalty, and current streak.

---

## 17. Post-contest review and export

Generate a Markdown file:

```markdown
# CFVirtual review — 2026-10-19 18:40

Mode: room · Host: your_handle · Duration: 120 min
Problems: 5 · Solved: 3/5 · Penalty: 87 · Rank: 2/6

| Slot | Problem | Rating | Tags | Result | Time | Tries | Assessment |
|------|---------|--------|------|--------|------|-------|------------|
| A | 1872A Plus Minus | 800 | math | AC | 6m | 1 | should solve |
| B | 1857C Assembly via Minimums | 1200 | sortings | WA | — | 2 | at level |
| C | 1832C Contrast Value | 1200 | greedy | unsolved | — | 0 | at level |

## Where time went
- Longest unsolved: C (38m, 0 submissions)
- Most attempts: B (2)

## Next focus
- Weak tags this contest: greedy, sortings
- Queued for review: 1857C, 1832C

## Links
- A: https://codeforces.com/problemset/problem/1872/A
...
```

Rules:

- No personal data beyond the public handle used in the contest.
- Include bot names with a `(bot)` marker.
- Everything in the export must be reproducible from the database.

---

## 18. Error handling

- Missing handle → prompt, validate, save.
- Invalid handle → clear message, exit 2.
- Codeforces down/rate-limited → run offline if possible; otherwise exit 3 with retry advice.
- No internet in room mode → lobby shows disconnected state; host cannot start until the server and profiles are ready.
- Insufficient candidate problems → suggest a wider rating range or fewer problems.
- Server port in use → suggest another port.
- Corrupt database → back it up to `cfvirtual.db.bak` and recreate; never delete data silently.
- Terminal too small → explicit message, no panic.
- All panics must be caught at the top level with a readable message; never dump a raw backtrace unless `RUST_BACKTRACE=1`.

---

## 19. Testing

Unit tests:

- Scoring: first AC, penalties, out-of-window submissions, duplicate ACs, ties.
- Selection: 40/40/20 tiers, no repeats, solved exclusion, deterministic with a fixed seed, insufficient pool.
- Weakness: known synthetic histories produce expected ordering.
- Bots: deterministic event schedule; all events inside the duration; AC times monotonic.
- Review scheduling intervals.

Integration tests:

- Spin the room server on an ephemeral port; connect two WebSocket clients; verify lobby, start, tick, end, reconnect.
- Mock Codeforces via a local Axum server injected through a base-URL config used only in tests; verify polling and API rate limiting.
- `demo` runs to completion without network.

Quality gates:

- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test`
- No `unwrap()` in request paths; use typed errors.
- CI matrix: Ubuntu, macOS, Windows; stable Rust.

---

## 20. Build order and acceptance criteria

**M1 — Skeleton and CF layer**
- `cargo run -- --help` works.
- Fetch `problemset.problems`, cache it, and print counts.
- Fetch `user.status` and `user.info` with the 2-second limiter.
- Accept: cache survives restart; tests use the mock server.

**M2 — Solo domain**
- Weakness computation, selection, scoring, bots.
- `cfvirtual start` timer works with a minimal text UI.
- Accept: unit tests in §19 pass; same seed gives the same contest.

**M3 — TUI**
- Setup, contest, results screens; links open/copy; keyboard-only; 80×24.
- Accept: full solo contest runnable; no panic on resize; demo mode works.

**M4 — Persistence and review**
- SQLite, history, stats, review queue, Markdown export.
- Accept: restart preserves history; export matches the schema in §17.

**M5 — Multiplayer**
- Room server, lobby, join, reconnect, spectators, host authority, server polling.
- Accept: integration test with two clients; scores match the authoritative snapshot.

**M6 — Polish and packaging**
- Bots in rooms, blind mode, themes, `--ascii`, `--json`.
- README with install and demo, `docs/SELF_HOSTING.md`, AUR PKGBUILD, release workflow.
- Accept: fresh clone builds with `cargo build --release` on all CI targets.

Commit after each milestone with a descriptive message. Do not squash milestones.

---

## 21. Packaging, README, CI

README must contain, in this order:

1. One-line pitch and a short animated terminal recording placeholder.
2. Feature list with a screenshot of the contest screen.
3. Install: prebuilt binary, `cargo install --git`, AUR.
4. Quick start: solo and room commands.
5. How scoring, bots, and detection work.
6. Self-hosting and LAN/tunnel instructions.
7. Limitations: read-only API, manual submission, trust-based rooms.
8. Contributing and license.

CI:

- `ci.yml`: fmt, clippy, test, build.
- `release.yml`: build Linux (gnu + musl), macOS (x86_64 + aarch64), Windows; attach binaries and checksums on tags.
- Keep the release workflow simple; do not add container images in the first version.

Optional `--json` schema: document in README so scripts can consume results.

---

## 22. Definition of done

- A fresh machine with Rust installed can run `cargo run -- demo` and complete a contest in under 2 minutes with no network.
- `cargo run -- start --handle <public handle>` produces a real contest and detects accepted submissions.
- Two machines on the same LAN can run `room create` and `join` and see the same leaderboard.
- All tests, clippy, and fmt pass in CI.
- No personal data or machine-specific paths exist anywhere in the repository.
- README lets a stranger go from clone to first contest in under 5 minutes.

---

## Appendix A — Defaults and decisions

- Name: `cfvirtual`; binary `cfvirtual`; crate `cfvirtual`.
- Difficulty tiers: 40% weak / 40% core / 20% stretch.
- Rating window: `[R-200, R+400]`, expanded by 100 when short on candidates.
- Default contest: 5 problems, 120 minutes.
- Default bots: 4 solo, 0 rooms.
- Poll interval: 20 seconds; API global limit: 2 seconds.
- Room code: 6 chars, Crockford base32, uppercase.
- Protocol version: 1.
- Scoring: ICPC with 20-minute penalty.
- Bot names: generated fictional handles, always marked `BOT`.
- License: MIT.

## Appendix B — Generic prior-art direction

A useful pattern to reimplement natively (do not import or reference any external project):

1. Download a public submission history.
2. Derive per-tag strengths and weaknesses with recency weighting.
3. Build a short, ordered practice plan from unsolved problems near the user's rating.
4. Keep everything local and dependency-light.

CFVirtual extends that pattern from a static plan into a timed contest with live scoring, rooms, bots, and a review loop. Implement the history and weakness logic directly in `domain/weakness.rs`; no external service is required.
