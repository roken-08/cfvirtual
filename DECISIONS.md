# Implementation decisions

These choices resolve underspecified behavior in the build specification. Phase
headings record the earlier layers' assumptions as well as packaging decisions;
they are not claims that acceptance checks have passed on every platform.

## M1 — Public API and cache

- Use only public, read-only Codeforces APIs. No credentials, scraping, or
  submission automation are part of the application.
- Share one process-wide request limiter across independently constructed clients
  as well as clones. Space requests by at least two seconds; rate-limit retries
  wait five seconds plus jitter, with at most three retries.
- Use a 24-hour problemset cache and accept valid stale data when a refresh fails,
  including an explicitly requested refresh. A successful fetch remains usable
  even if writing the cache fails.
- Fetch the latest 10,000 submissions for history and 100 for live polls. An
  unrated profile uses 1200. Solo preparation may fall back to uniform weights
  when history is unavailable; room preparation requires host-profile validation.
- The problemset API/model has no source-contest title. Filtering known special
  events is therefore based on available problem **names/tags**, plus rated,
  standard-key checks. It cannot promise full contest-title exclusion without an
  additional source of title metadata.

## M2 — Domain model and priors

- Weakness combines unsolved rate, submission inefficiency, and recent failures
  with weights 0.5/0.3/0.2. Recent failures use a 30-day half-life inside an
  inclusive 90-day window. Tags with fewer than three distinct attempted problems
  shrink their combined weakness toward the population mean with enough prior
  observations to reach three; raw reported rates remain unmodified.
- An empty history weight map means a uniform prior over candidate tags, not an
  absence of selectable problems. Historical statistics include all non-OK
  verdicts, including compilation errors; live scoring deliberately differs.
- Keep all solved keys supplied to analysis, even when its statistical sample is
  capped. This does not recover older solves omitted by the API history limit.
- Selection uses 40% weak, 40% core, 20% stretch, rounding spare slots into core.
  The candidate pool is first bounded to `[R-200, R+400]`; reserve stretch first,
  expand tier windows once by 100 points, then use remaining members of that
  bounded pool. Keep tag filters in every fallback; avoid duplicate keys and
  normalized names. Reproducibility requires the same seed **and input data**.
- Scoring uses inclusive start/end timestamps and integer seconds. Ignore pending,
  skipped, and **compilation-error** verdicts as attempts/penalties; ignore all
  submissions after a first AC. Only solved problems add elapsed acceptance time
  and 1,200 seconds per earlier scored rejection. Deduplicate submission IDs and
  prefer judged results over pending snapshots, then AC over other verdicts.
- Compare handles case-insensitively. Human standings use the host's rating as a
  shared difficulty baseline, not a fetched rating for every participant.
- Bot solve probability is logistic in rating difference. Solve times use a
  log-normal distribution with `sigma = 0.45` and median based on
  `max(2, 25 - rating_difference / 40)` minutes, clamped into the contest window.
  Rejections are aggregate counts revealed at AC, not invented timestamped events.
- Bot names are fictional adjective/noun/number combinations, unique within the
  generated set and always marked as bots. No API lookup or worldwide uniqueness
  guarantee is made; a coincidental real handle match is possible.

## M3 — Terminal and demo

- The solo pause key opens an overlay; the authoritative clock keeps running.
  This avoids claiming that API submissions or elapsed time can be paused.
- Blind metadata is a UI/protocol presentation choice, not an anti-cheat boundary.
  Browser submission stays manual; copy/open failures must leave a usable URL.
  Results reveal ratings, tags, and assessment hints automatically after the
  contest; `b` remains an explicit reveal action during permitted states.
- The demo is offline, six problems and three bots, with a 90-second interactive
  contest. A non-TTY or `--json` invocation automatically advances the simulated
  clock without a real-time wait. Acceleration changes playback, not score units;
  this headless path makes smoke tests deterministic and fast.
- The bundled SVG is an explicitly labeled illustration, not a captured run.
  Leave the animated recording as a visible placeholder until one is recorded.

## M4 — Persistence and review

- Save a full contest/standings snapshot alongside normalized SQLite rows, using
  transactions and embedded migrations. Never serialize participant session
  tokens. Only confirmed corruption triggers a preserved backup and recreation;
  permission, busy, and incompatible-schema errors remain errors.
- History and aggregate statistics describe saved **host** results, excluding bots
  from human totals. Per-tag denominators include every assigned problem,
  including unattempted ones. These differ from the public-history weakness rates.
- A streak counts consecutive **UTC dates** with saved contests, ending today or
  yesterday. Multiple contests on one UTC date count as one streak day.
- Queue unsolved problems idempotently, initially due after one day. Success
  advances 1 → 3 → 7 → 14 → 30 days; skip/failure resets to one day and adds a
  lapse. Requeueing an existing entry preserves its schedule.
- Exports use recorded acceptance offsets and attempts. Submission timestamps do
  not reveal thinking time: never invent time-on-task or a longest-unsolved
  duration. Export recommendations alone do not prove review-queue persistence.

## M5 — Rooms and protocol

- Protocol v1 uses versioned JSON text frames at `/` and `/ws`. The server owns
  contest time, scoring, polling, and bot schedules; clients render snapshots.
  Remote creation is a `create` extension used by `room create --server`.
- Rooms are trust-based: public profile validation does not establish handle
  ownership. A room code is not authentication. Optional passwords are a
  **soft gate**, salted SHA-256 hashes held only in memory, not account security.
  Reconnect tokens are private bearer credentials; plain WebSockets provide no
  transport secrecy. Terminate TLS externally for remote use.
- Public snapshots omit tokens, histories, future bot schedules, and other
  players' per-problem details. Reconnect returns a recipient-filtered snapshot.
- Poll on a nominal 20-second cycle and broadcast snapshots at least every five
  seconds while connected. The global API limiter and failures can stretch polls.
- Freeze the cutoff on timeout/host end, then make two additional polling passes
  separated by two seconds. Final draining is bounded to 180 seconds after an
  in-flight call finishes. Delayed verdicts beyond this bound may be missed.
- Keep active rooms and sessions in memory; database snapshots are not a restart
  recovery mechanism. Bound player/spectator/bot counts, retained rooms, message
  sizes, send queues, and write timeouts as specified in `docs/PROTOCOL.md`.
- Spectator projections are never written to local contest history. A participant
  projection is stored for a player only, and the full host snapshot is the one
  included in aggregate history/statistics when both records exist.

## M6 — Distribution

- Use generic contributor attribution and MIT licensing. There is no configured
  upstream URL; `<repository-url>` and source-archive metadata are explicit
  placeholders until maintainers select a repository and release source.
- Pin checks/builds to Rust 1.99.1, matching `rust-toolchain.toml`. CI covers
  Ubuntu, macOS, and Windows; tests use fixtures/local mocks, not live CF access.
- Bundle SQLite, requiring a C compiler. The locked `arboard` configuration uses
  `x11rb` without `xcb` FFI, so Linux builds do not require `libxcb` development
  headers. musl additionally needs `musl-tools` and a musl linker/compiler.
- Tag automation builds five target archives and per-archive SHA-256 sidecars,
  uploading explicit paths with `gh release upload`. Only the publication job
  receives `contents: write`. Workflows do not establish prior publication.
- The Arch recipe is a configurable, checksummed source-package template. No AUR
  availability or binary signing/notarization is claimed.
