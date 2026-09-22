# PROJECT_STATE.md — Quiczilla Public Project State

This document records the current public-facing state of the project and the
next work items. Do not add real hostnames, usernames, addresses, credentials,
or machine-specific paths here. Use RFC 5737 documentation addresses and
`*.example.net` names in examples.

## Current status

- **Release line:** `v0.1.10`.
- **Platforms:** x86_64 Windows and Linux release archives are assembled by
  GitHub Actions. Each archive contains the CLI, the matching worker, and its
  MsQuic runtime.
- **Transfer model:** SSH supplies authenticated bootstrap. The data path uses
  mTLS-authenticated QUIC when reachable, with STUN-assisted discovery and SSH
  streaming fallback.
- **Worker refresh:** Bootstrap SHA-256-checks an installed worker. A match
  runs it; a mismatch uses a content-addressed managed bundle. If that bundle
  cannot execute on a `noexec` filesystem, bootstrap warns and retries the
  installed worker during this alpha phase.

## Verification baseline

Run these from the repository root before merging substantial changes:

```powershell
cargo check --workspace
cargo test --workspace --lib
cargo test -p quiczilla-cli
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release
```

The daemon test scripts exercise the local loopback path. Use hosts and
addresses you control for any external transfer or performance testing; do not
commit those values or their output.

## Active priorities

1. Maintain a reliable, high-throughput CLI for SSH-managed infrastructure.
2. Benchmark direct QUIC and SCP on the same route before making performance
   claims.
3. Add protocol-compatibility negotiation before relying on an outdated
   installed worker as a production fallback.
4. Keep daemon authorization explicit through mTLS thumbprint allow-lists.

## Architectural invariants

- Sanitize received file names with `Path::file_name()`.
- Verify custom TLS handshake signatures cryptographically; never use a blind
  verifier assertion.
- Use separate or correctly framed QUIC streams for independent transfers.
- Keep release documentation and examples free of operator-specific data.

## Recent public handover

- Added `CONTRIBUTING.md` with contributor and coding-agent safeguards plus the
  required formatting, build, unit-test, lint, release-build, and end-to-end
  verification expectations.
- Extended `scripts/benchmark_transfer.ps1` with optional `qcp` support. It
  uses a temporary SSH profile for a non-default SSH port, verifies remote
  SHA-256 values, and clearly skips a tool unavailable on the local machine.
  qcp requires installation on both ends and a receiver UDP port/range that is
  directly reachable; it cannot use Quiczilla's STUN broker.
- Recorded a public-safe Windows-to-Ubuntu STUN-assisted benchmark in the
  README. The 512 MiB median favoured Quiczilla's end-to-end route by 35% over
  SCP; small files remained SCP-favoured because of bootstrap overhead. rsync
  and qcp were unavailable in that environment, so no misleading comparison is
  claimed.
- `v0.1.9` derives its worker bundle label from `CARGO_PKG_VERSION`, avoiding a
  manually maintained version string.
- Same-platform source builds prefer a newly built sibling worker when it is
  available. Official archives embed CI-built workers for both target
  platforms.
- Managed worker bundles use a SHA-256-addressed cache directory, so refreshes
  do not overwrite a binary still used by another transfer.
- One-shot transfers flush their final output and take a process-terminal
  success path after the peer confirms completion, avoiding a native MsQuic
  teardown hang after a successful transfer.
