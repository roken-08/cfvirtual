# Offline demo and recording

`cfvirtual demo` uses a fixed six-problem fixture, three synthetic bots, and
scripted activity. It requires no Codeforces account, credentials, cached
problemset, or network requests.

## Interactive run

From a checkout with dependencies installed:

```sh
cargo run --locked -- demo --seed 42
```

Or use an installed binary:

```sh
cfvirtual demo --seed 42
```

The interactive contest lasts **90 seconds** and shows a results screen afterward;
close that screen to exit. The seed makes the simulated run reproducible with the
same application version. No real Codeforces submission is made or required.
This is a product demonstration, not a live integration test.

The demo intentionally uses a shortened preparation countdown and compressed bot
event times. Normal solo and room contests use a one-minute preparation window
and progressively scheduled synthetic opponents.

The demo saves a local snapshot in demo mode and can appear in local history and
statistics. Repeating the same seed replaces that fixture's deterministic contest
ID rather than creating another independent practice record.

Use at least 80 columns by 24 rows; a wider terminal leaves more room for
standings. Try `cfvirtual --ascii demo --seed 42` for plain status symbols or
`cfvirtual --theme colorblind demo --seed 42` for the colorblind-safe theme. Press `?` for
controls, `j`/`k` to move, and `q` to leave. The solo `p` overlay does **not** stop
the clock. Opening problem links is an optional browser action and is unnecessary
for an offline recording.

## Headless / JSON smoke check

When stdin/stdout are not terminals, or when `--json` is set, the demo automatically
uses an **accelerated simulated clock**. It completes without waiting 90 real
seconds, without raw-mode terminal setup, and without requiring keypresses.

```sh
cfvirtual demo --seed 42 --json
cargo run --locked --offline -- demo --seed 42 --json
```

The first form uses the installed binary; the second assumes Cargo dependencies
have already been fetched/built. Cargo's `--offline` controls dependency access;
the demo itself makes no API calls in either form. JSON stdout is reserved for
the result document, with diagnostics on stderr. Simulated elapsed times and
penalties remain seconds even though playback is accelerated.

## Record a short terminal animation

No animated recording is bundled yet. The README's recording block is an
intentional placeholder. To replace it with a genuine capture:

1. Use a clean terminal with a neutral prompt, without personal handles, machine
   names, paths, or notifications in view. Keep the window size fixed.
2. Start a terminal recorder, run `cfvirtual demo --seed 42`, and capture the timer,
   a few status changes, and the final standings. Use only the offline fixture.
3. Stop recording after closing results. Trim setup/idle time and optionally
   speed up playback to keep the animation short. Review every frame for exposed
   environment data before publishing.
4. Add the resulting recording/animation asset in a later change, link it from
   the README, and label any playback acceleration. Never label the illustration
   as a captured screenshot.

For example, with `asciinema` installed separately:

```sh
asciinema rec -c 'cfvirtual demo --seed 42' demo.cast
```

Keep capture output local until reviewed; uploading a recording is a separate,
explicit action. Rendering a cast to GIF/SVG depends on your chosen recording
tools and is not a runtime dependency of CFVirtual.

## Existing image

[`assets/contest.svg`](../assets/contest.svg) is a static **illustration** with
invented labels and sample scores. Its caption and embedded accessible text say
so. It demonstrates the layout without claiming real contest results or a
completed screenshot check.
