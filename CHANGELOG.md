# Changelog

Changes are recorded here before publication. The crate version is `0.1.0`;
this file does not imply that a tag, binary release, or AUR package exists.

## Unreleased

### Added

- A mandatory one-minute normal-contest preparation window: handle validation,
  full public-history analysis, weak-tag reporting, and contest selection happen
  before the authoritative clock starts. Bots now use progressive minute-scale
  schedules rather than instant solves.
- Bare `cfvirtual` launches the offline demo with seed `42` as a zero-setup first run.
- Read-only Codeforces client, process-wide request pacing, retry handling,
  and a disk-cached problemset.
- Seeded weakness-weighted selection, ICPC scoring, and fictional bot schedules.
- Authoritative WebSocket rooms, reconnect tokens, spectators, and bounded
  final verdict polling.
- SQLite snapshots, host history/statistics, review scheduling, and reproducible
  Markdown review generation.
- Getting-started, offline-demo, and self-hosting documentation, with an explicitly
  illustrative contest screen and an animated-recording placeholder.
- Configurable Arch source-package template and pinned-toolchain CI/release
  workflows for Linux, macOS, and Windows.

### Release preparation

- Set the real repository URL and source-archive metadata before distributing
  install instructions or publishing the Arch package.
- Verify the final CLI help against the documented options and run all quality
  gates and platform builds before tagging.
- Publish version-matching `v<version>` tags to run the release workflow. It
  creates archives and SHA-256 sidecars; no published assets are assumed here.
