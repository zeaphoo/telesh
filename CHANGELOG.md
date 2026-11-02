# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0-rc.1] - 2026-11-02

### Added

- Agent / server tunnel control plane and data plane: expose local HTTP/TCP
  services through a public edge, with yamux multiplexing over an
  agent-initiated connection.
- P2P mode: map remote services to the local machine over iroh QUIC, with
  hole punching and automatic relay fallback.
- Local inspector dashboard for the agent and a management dashboard for the
  server, both embedded into their binaries with `rust-embed` (no external web
  assets required).
- CLI subcommands for `http`, `tcp`, `p2p`, `connect`, `publish` and `open`,
  plus configurable data root and dashboard port.
- Tauri desktop shells for agent and server: tray integration, window targeting
  the local dashboard, and bundled headless sidecar.
- Linux service packages (`deb` / `rpm`) with systemd units, dedicated service
  users, data directories and example configs.
- Standalone command-line archives for Linux/macOS (`tar.gz`) and Windows
  (`zip`), containing the binaries, example config, license and README.
- Desktop client packaging: Linux `deb` / `AppImage`, Windows NSIS / MSI /
  portable `zip`, and macOS `dmg` (optional Developer ID signing and
  notarization).
- GitHub Actions CI for tests, frontend builds, app checks and package builds.
- Tag-driven GitHub Release workflow for service packages, CLI archives and
  desktop clients, with version/tag validation and release notes extracted from
  this file.

### Changed

- Server now has a version-policy seam for future quota and subdomain rules.
- Desktop sidecar binaries are named `telesh-sidecar` /
  `telesh-server-sidecar` so their packages do not collide with the headless
  service packages on Linux.
- `cargo build --release` requires the corresponding frontend to be built;
  `TELESH_ALLOW_EMPTY_UI=1` remains available for UI-less source builds.

### Fixed

- `tunnel_resp` error variants could not be sent over the wire.
- Wrong-type dashboard responses are no longer mistaken for a running agent, and
  the CLI now prints actionable hints when the agent is not running.
- Windows: default data root and `~` expansion now fall back to `USERPROFILE`
  when `HOME` is not set.

[Unreleased]: https://github.com/zeaphoo/telesh/compare/v0.1.0-rc.1...HEAD
[0.1.0-rc.1]: https://github.com/zeaphoo/telesh/releases/tag/v0.1.0-rc.1
