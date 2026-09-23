# PROJECT_STATE.md — Quiczilla Public Project State

This document records the current public-facing state of the project and the
next work items. Do not add real hostnames, usernames, addresses, credentials,
or machine-specific paths here. Use RFC 5737 documentation addresses and
`*.example.net` names in examples.

## Current status

- **Release line:** `v0.1.12` (published and live-validated on Windows-to-Linux).
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
- Moved the benchmark harness to `benchmarks/benchmark_transfer.ps1` and
  extended it with optional `qcp` support, configurable rsync SSH invocation,
  and `-UsePipe` for raw-stream measurements. It
  uses a temporary SSH profile for a non-default SSH port, verifies remote
  SHA-256 values, and clearly skips a tool unavailable on the local machine.
  qcp requires installation on both ends and a receiver UDP port/range that is
  directly reachable; it cannot use Quiczilla's STUN broker.
- Added `benchmarks/benchmark_batched_tree.ps1` for an explicit many-file
  workload: it packs a directory into one uncompressed `quic pipe` tar stream,
  recursively copies the same tree with SCP and rsync, and gates every result
  on a deterministic per-file SHA-256 manifest. The public documentation now
  records three exact-1-GiB, three-run medians: 16,384 64 KiB files (Quiczilla
  27.78 MiB/s; SCP 2.42; rsync 15.22), 102 10 MiB files plus a 4 MiB tail
  (27.30; 18.72; 18.09), and one 1 GiB file in normal file mode (28.06; 19.85;
  19.42). The QUIC leg used `stun-quic`; SCP and rsync used SSH/TCP on the same
  public endpoint. These are point-in-time results, not a universal claim.
- Added native source-directory detection (`quic <directory> target` or
  `quic send <directory> target`). It streams framed, logical 16/32/64 MiB
  packs through one QUIC connection without a temporary archive, full-tree
  pre-scan, or unbounded queue. `--storage-profile hdd|auto|nvme` selects the
  pack target; all profiles currently retain one in-flight pack for bounded,
  sequential disk I/O. The receiver rejects unsafe relative paths and existing
  destination symlinks; source symlinks and special files are skipped.
- Pipe mode now accepts the same SSH-port, STUN, preferred-host, and manual
  QUIC candidate options as normal file transfer. It sends an orderly QUIC FIN
  after local EOF so one-way remote commands can receive EOF and exit.
- Published `v0.1.11` with the pipe EOF-completion and STUN-option fixes. A
  GitHub-release Windows client refreshed the matching CI-built Linux worker;
  its public-DDNS STUN benchmark is recorded in the README. The 512 MiB median
  favours Quiczilla by 45% over SCP and 56% over rsync. Pipe-mode small-file
  figures are complete and show expected bootstrap overhead. qcp remains
  unavailable and cannot use Quiczilla's STUN broker.
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
- Published `v0.1.12` was verified from its GitHub Windows archive (`quiczilla
  0.1.12`) against a Linux worker: both default directory detection and the
  explicit `send` command completed over STUN-assisted QUIC. A nested tree,
  empty directory, text files, and a 1 MiB binary were checked with matching
  SHA-256 hashes; temporary remote fixtures were removed afterward.
- Managed remote worker caches now retain the three newest content-addressed
  bundles after a worker starts. Pruning is best-effort and non-fatal to handle
  active Windows executable locks, read-only caches, and administrator policy.
