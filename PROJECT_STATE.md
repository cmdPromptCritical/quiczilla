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
3. Keep daemon authorization explicit through mTLS thumbprint allow-lists.
4. Prepare Phase 2 desktop GUI integration atop the verified CLI/MsQuic engine.

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
- Added `docs/PRODUCTION_TEST_PLAN.md`, defining the container topology,
  protocol/fuzz/network/load/security/release test layers, platform limits,
  daemon operational requirements, and alpha/release-candidate/production
  gates. It records that containers are the repeatable Linux layer, not a
  substitute for Windows, physical storage, or public-NAT testing.
- Added `tests/e2e/` providing an automated end-to-end container test suite for
  Docker and Kubernetes:
  - Multi-stage `Dockerfile` supporting local working-tree source builds and
    checksum-verified release archives in a clean Ubuntu runtime without
    system MsQuic packages.
  - Topology T1 Docker Compose environment with non-root SSH daemon, per-run key
    isolation, and optional local `coturn` sidecar.
  - Topology T2 Kubernetes Job manifest (`k8s/client-job.yaml`) for split-site
    cluster NAT-to-edge NAT traversal validation.
  - Complete test tier suite: Smoke (S1–S7), Fault Injection / Regression
    (R1–R11 with `tc netem` impairment and iptables simulation), and Split-Site
    WAN STUN (X1–X4).
  - Machine-readable structured reports (`summary.json` and `results.jsonl`)
    with SHA-256 manifests, receipt assertions, and metrics collection.
- Hardened CLI and worker teardown and streaming paths against native MsQuic hangs:
  - Extended process-terminal exit (`std::process::exit(0)`) after receipts are
    flushed to `run_directory_transfer_cli`, `run_pipe_cli`, `run_identity_cli`,
    and `run_direct_cli`. This avoids indefinite blocks inside `RegistrationClose`
    during Linux runtime shutdown.
  - Corrected pipe duplex stream lifecycle in `core/src/pipe.rs` so receiving remote
    EOF/FIN does not cancel the outbound sending task before bidirectional
    commands (e.g. `sha256sum`) finish outputting their results to the client.
  - Updated worker pipe execution to gracefully drain child process output without
    arbitrary premature cancellation.
  - Added timeout safeguards to identity generation in the E2E test runner harness.
  - Bounded directory temporary staging file names (`.quic-part`) so long names
    (up to 254 bytes) do not exceed filesystem `NAME_MAX` (255 bytes).
  - Selected concurrently between directory transfer sending and receiver ACK
    reading in the CLI, surfacing receiver rejection immediately and preventing
    stream flow control deadlocks.
  - Fully detached background worker daemons launched across SSH in test cases,
    ensuring standard streams detach cleanly and OpenSSH returns immediately.
- Containerized E2E Smoke suite verified green on remote test host (`7 passed, 0 failed, 0 skipped`):
  - `S1`: Identity and binary digest verification (0.68s).
  - `S2`: Coturn STUN canary (10.67s).
  - `S3`: Forced transports ladder (`direct-quic`, `manual-quic`, `ssh-fallback`, `auto`) (6.29s).
  - `S4`: Size edge matrix from 0 bytes up to 256 MiB (14.44s).
  - `S5`: Native streamed directory tree with 307 files, deep nesting, and long names (2.34s).
  - `S6`: Bidirectional raw pipe tar extraction and remote SHA-256 digest streaming (3.58s).
  - `S7`: Cross-container persistent daemon verified transfers, conflict refusal, and rogue/wrong-pin rejection (2.17s).
- GitHub Actions CI matrix (`ubuntu-22.04` and `windows-2022`) verified 100% green:
  - Resolved `clippy::incompatible-msrv` failure in `core/src/directory.rs` by replacing
    `floor_char_boundary` (Rust 1.91+) with an `is_char_boundary` loop compatible
    with the repository's Rust 1.85.0 MSRV.
  - All workflow stages (Workspace Check, Unit/Integration Tests, Clippy Linter `-D warnings`,
    binary build, and persistent daemon E2E on Linux & Windows) completed with success.
- Containerized E2E Fault Injection & Regression Suite (R1–R11) verified 100% green (`11 passed, 0 failed, 0 skipped`):
  - `R1`: Fallback ladder across blocked STUN and non-QUIC routes (14.49s). Worker soft-fails STUN discovery.
  - `R2`: Transfer interruption and cold resumption with prefix fingerprint verification (6.36s).
  - `R3`: Remote destination conflict policies (`overwrite` and `refuse`) (2.52s).
  - `R4`: Path traversal sanitization, escaping, and illegal character refusal (3.23s).
  - `R5`: Destination disk failure handling, cancellation propagation, and hang-free teardown (3.56s).
  - `R6`: Zero-trust `noexec` cache detection with preinstalled worker fallback (3.30s).
  - `R7`: Version skew managed bundle refresh and 3-bundle retention pruning (4.20s).
  - `R8`: High-concurrency 4x parallel bootstrap transfers against cold cache and 8x concurrent daemon streams (3.87s). Resolved bundle upload races with atomic staging directories (`staging-{hash}-{token}`).
  - `R9`: QUIC transport under `tc netem` impairment (50ms delay, 1% packet loss, 1% reordering) with SHA-256 integrity (7.87s).
  - `R10`: Hostile UDP fuzzing, malformed headers, and garbage packet bursts against listening daemon (1.99s).
  - `R11`: Resource ceilings verifying bounded client memory consumption (Peak RSS << 256 MB) on 64 MiB payloads (2.82s).
- Implemented protocol compatibility and capability negotiation:
  - Worker advertises `protocol_version: 1` and supported features (`directory`, `pipe`, `exec_hex`, `storage_profiles`, `conflict_policy`, `checksum`, `resume`) in its JSON ready banners (`ready` and `daemon_ready`).
  - Client validates worker protocol version during SSH bootstrap and installed worker fallback (`MIN_SUPPORTED_PROTOCOL_VERSION = 1`, `CURRENT_PROTOCOL_VERSION = 1`).
  - Commands verify feature availability early (e.g. `directory`, `pipe`, `exec_hex`) before starting stream transmission. Legacy unversioned workers are permitted for baseline single-file transfers.
- Implemented deterministic structured failure exit codes & error taxonomy:
  - `0`: Success
  - `2`: Invalid CLI invocation / argument parsing error
  - `3`: Authentication / mTLS thumbprint failure
  - `4`: Network failure / timeout (all UDP & SSH fallbacks exhausted)
  - `5`: Data integrity failure (SHA-256 verification mismatch)
  - `6`: Destination conflict / file policy refusal
  - `7`: Protocol or version incompatibility
  - `130`: Transfer cancelled by user
  - Verified in containerized test `S7` where `rejected_exit_codes` cleanly captures `{"conflict": 6, "rogue": 3, "wrong_pin": 3}`.
- Hardened stream length bounds and security invariants:
  - Enforced `MAX_CONTROL_MESSAGE_BYTES` (64 KiB) across stream readers in `core/src/peer.rs` and `core/src/peer_generic.rs` to prevent memory exhaustion from hostile unbounded length headers.
  - Hardened relative path validation in `core/src/directory.rs` against leading tildes, environment variable syntax (`$`), and redundant dot-segment obfuscation (`.`, `..`).
- Implemented libFuzzer harness and property test suite:
  - Added `fuzz/` crate with libFuzzer targets: `fuzz_directory_frames`, `fuzz_control_message`, `fuzz_relative_path`, and `fuzz_handshake`.
  - Added companion unit and property test suite in `core/tests/fuzz_property_tests.rs` verifying parser robustness, path safety, and control message decoding on standard toolchains.
  - All workspace tests (`cargo test --workspace`) and E2E regression tests (R1–R11) pass 100% green.
