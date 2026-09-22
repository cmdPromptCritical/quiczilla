# Quiczilla Implementation Plan — Fast, Reliable SSH-Managed Transfers

## Mission and boundaries

Quiczilla should win when an operator needs to move large files between known
Windows and Linux hosts that already support SSH. The product is not a generic
consumer sharing application, synchronization product, or object-storage
service.

The primary UX remains one familiar command:

```text
quic <source> <ssh-target:destination-directory>
```

The daemon is an optional authenticated receiver for stable sites and private
networks. It remains upload-only until a separately designed pull protocol
exists.

## Performance contract

`Faster than SCP` applies to the **direct QUIC data path** for large sequential
files on a network where UDP is permitted and neither endpoint is disk- or
CPU-bound. It does not apply to SSH fallback, small files dominated by
bootstrap, or every possible LAN and WAN.

Before accepting a performance change, measure release binaries against SCP on
the same hosts and route. Keep bootstrap time and payload time separate.

| Metric | Target | Required report fields |
|---|---|---|
| Large-file payload throughput | At least 1.15× SCP median on the defined WAN fixture | bytes, payload seconds, MB/s, p50/p95 |
| Bootstrap | Do not regress the cache-hit baseline without justification | cache state, bootstrap seconds |
| CPU and memory | No unbounded memory growth or callback starvation | CPU, RSS, chunk/buffer settings |
| Reliability | No corrupt completed destination; all failures are detectable | exit code, receipt, hash mode/result |

Initial benchmark fixture: 1 GiB and 10 GiB incompressible files, cache-hit
worker, three runs per path. Capture RTT/loss and record whether the path was
direct QUIC, STUN QUIC, manual QUIC, or SSH fallback. Add a constrained WAN
profile only after the baseline is repeatable.

## Milestone 0 — Benchmark and observability foundation

### PERF-BASELINE: reproducible comparison harness

**Purpose:** Establish whether Quiczilla beats SCP today and prevent false
optimizations.

**Implementation:**

1. Add a benchmark driver under `scripts/` that creates or accepts fixed
   fixtures, warms the worker cache, and runs Quiczilla, `scp`, and `rsync -e
   ssh` with equivalent destination and verification rules.
2. Emit one JSON record per run; do not parse the interactive progress display.
3. Capture host OS/version, CPU, storage, SSH port, file size, transport path,
   bootstrap duration, payload duration, throughput, verification mode, and
   result.
4. Store only benchmark scripts and small metadata in Git; never commit large
   fixtures or benchmark secrets.

**Validation:** Byte-compare/checksum every result; run locally on Windows to
Linux and in a controlled CI loopback smoke test. Publish a dated WAN results
table only from repeatable measurements.

**Performance impact:** Neutral at runtime. Essential measurement guardrail.

### TRANSFER-RESULT: truthful receipts and machine output

**Purpose:** Make automation and benchmarking see the actual path and outcome.

**Implementation:**

1. Define a versioned transfer-result structure in `core`.
2. Return it from QUIC and SSH-fallback paths rather than composing unrelated
   terminal strings in the CLI.
3. Add `--json`; reserve stdout for one result document in that mode and retain
   human progress on stderr otherwise.
4. Define stable exit classes: invalid invocation, local I/O, authentication,
   network/path failure, remote rejection, integrity failure, cancellation.

**Validation:** Unit-test serialization and exit mapping; add Windows/Linux E2E
assertions for direct, fallback, rejected, cancelled, and checksum-failed paths.

**Performance impact:** Negligible; do not log per-chunk events by default.

## Milestone 1 — Correct transfer lifecycle

### SAFE-COMMIT: atomic receive and resume

**Purpose:** A completed destination must never be a partially written file.

**Implementation:**

1. Place transfer data in a destination-adjacent unique partial name, retaining
   the existing `.quic-part` resume convention where applicable.
2. Persist a small manifest containing source name, expected size, resume
   fingerprint, requested hash mode, and transfer ID. Treat malformed/stale
   metadata as non-resumable rather than trusted.
3. After the data stream finishes and requested verification succeeds, flush the
   file and atomically rename it into the final basename on the same filesystem.
4. Define conflict handling at final-name commit: overwrite, refuse, and later
   generated rename. Do not pre-delete an existing final file.
5. Remove partial state only after successful commit or explicit user cleanup;
   preserve it after interruption for `--resume`.

**Validation:** Inject cancellation, disk-write failure, checksum mismatch,
process death, and destination races. Confirm old final content remains intact
and successful transfers never expose a partial final name.

**Performance impact:** One metadata write and a same-filesystem rename; no
full-file copy. `fsync` policy can reduce throughput, so expose a documented
durability level and measure it before selecting a default.

### STREAM-STATE: one framed QUIC stream per transfer

**Purpose:** Prevent stream desynchronization and make cancellation/parallelism
safe.

**Implementation:**

1. Define a versioned file-offer/accept/result control message with a transfer
   ID in `core/src/peer.rs` or its successor protocol module.
2. Open one bidirectional QUIC stream for each accepted transfer; put an
   unambiguous header and metadata on that stream.
3. Map FIN, RESET, and connection-close events to explicit transfer outcomes.
4. Ensure a rejection or reset releases only its transfer resources and leaves
   the control connection usable when protocol state permits.
5. Add a bounded transfer state machine: `offered → accepted → transferring →
   verified → committed`, plus rejection/cancellation/failure terminals.

**Validation:** Unit-test state transitions; integration-test two sequential
transfers where the first is rejected, cancelled, and reset. Use platform E2E
tests for Windows↔Linux combinations.

**Performance impact:** Positive enabler for concurrent files and avoids repair
work after failed streams. Small per-stream control overhead; do not parallelize
a single file until profiling demonstrates a benefit.

### DAEMON-SAFETY: explicit receiver contract

**Purpose:** Make a persistent daemon safe to operate unattended.

**Implementation:**

1. Require `--save-dir` for `quic daemon`, canonicalize it at startup, verify
   it is writable, and include it in the ready JSON and human startup line.
2. Keep `--on-conflict overwrite|refuse`; add generated rename only after atomic
   commit exists.
3. State clearly in help/README that `quic direct` sends to the daemon and
   saves only the sanitized basename in its configured directory.
4. Refuse unsafe startup before binding the UDP port.

**Validation:** CLI validation tests, read-only/missing-directory startup tests,
and daemon E2E checks for each conflict policy.

**Performance impact:** None in the data path.

## Milestone 2 — Throughput work backed by profiling

### DISK-PIPELINE: bounded asynchronous disk and hash pipeline

**Purpose:** Prevent disk I/O and hashing from starving transport callbacks.

**Implementation:**

1. Profile CPU, disk latency, allocation rate, MsQuic callback delay, and
   throughput before changing buffer sizes.
2. Use bounded read/write staging buffers sized from measurements; retain
   backpressure to prevent RAM growth.
3. Move blocking file operations to appropriate Tokio blocking/I/O paths.
4. Compute requested hashes in the read/write pipeline without rereading a file.
5. Test slow-disk and fast-network cases separately from slow-network cases.

**Validation:** Benchmark with checksum off/on, local NVMe, and intentionally
constrained storage. Ensure memory has a firm upper bound.

**Performance impact:** High potential. Incorrect buffering can reduce
throughput or increase latency; merge only with benchmark improvement.

### TRANSPORT-TUNING: measured MsQuic and socket tuning

**Purpose:** Extract the best sustained WAN throughput without losing portability.

**Implementation:**

1. Establish a parameter table for flow-control windows, stream credit, socket
   send/receive buffers, chunk size, MTU, congestion control, pacing, and GSO.
2. Test one variable at a time on Windows and Linux release artifacts.
3. Detect unsupported options and keep safe portable defaults; never assume
   `target-cpu=native` or a particular NIC offload exists on user machines.
4. Keep a small regression suite for loss, high RTT, and constrained bandwidth.

**Validation:** Three-run median comparison to SCP and prior Quiczilla baseline;
require no reliability regression and no severe regression on the other OS.

**Performance impact:** High potential and high regression risk.

### NETWORK-AUTO: fast connection selection and fallback

**Purpose:** Prefer direct QUIC without making an unreachable UDP route feel slow.

**Implementation:**

1. Define a deadline budget for direct, STUN-assisted, and manual candidates.
2. Report candidate attempts and the selected path in the transfer result.
3. Begin SSH fallback promptly when the budget is exhausted; avoid duplicate
   writes or competing active transfers.
4. Add a configurable connection deadline only after selecting a safe default.

**Validation:** Simulate blocked UDP, reachable UDP, invalid STUN, and delayed
remote readiness. Assert completion through SSH fallback and accurate receipts.

**Performance impact:** Strongly improves perceived latency and avoids long
failed UDP waits; no impact on sustained payload throughput after connection.

### INTEGRITY-MODES: honest, measured verification

**Purpose:** Let users choose validation without hiding its cost.

**Implementation:**

1. Replace the boolean option internally with a named mode while accepting
   `--checksum` as a compatible alias for `sha256`.
2. Keep `none` explicit in JSON/receipts. Do not label transport reliability as
   end-to-end file verification.
3. Benchmark SHA-256 on both supported platforms. Consider a faster streaming
   hash only if profiling shows hashing is material and the algorithm/version is
   included in the manifest and receipt.

**Validation:** Test match, mismatch, unsupported mode, and resume with every
supported mode.

**Performance impact:** Potentially material on slower CPUs/storage; must be
reported, never guessed.

## Milestone 3 — High-value workflow features

### MULTI-FILE: recursive directory transfer with a manifest

**Purpose:** Move release bundles, datasets, backups, and media trees without
falling back to tar or a separate tool.

**Implementation:**

1. Introduce `quic send <source...> <target>` with source-first semantics;
   retain the existing two-positional command as a compatible alias.
2. Walk source trees without following unsafe links by default; create a
   canonical relative-path manifest with sizes, metadata policy, and optional
   hashes.
3. Negotiate the manifest, receive files atomically, and resume per file.
4. Use bounded concurrent streams, selected by benchmark rather than a fixed
   maximal number.
5. Produce a final manifest receipt with transferred/skipped/failed counts.

**Validation:** Trees with duplicate basenames, empty directories, symlinks,
deep paths, interrupted resumes, partial failures, and Windows/Linux path
rules. Compare resulting manifest and hashes.

**Performance impact:** High upside for many independent files. Manifest work
and too much concurrency can harm small-file performance; enforce bounds.

### PAIRING: painless persistent peer enrollment

**Purpose:** Make two controlled daemons easy without weakening mTLS.

**Implementation:**

1. Define a peer record containing a label, endpoint, certificate pin, role,
   creation time, and optional policy. Store it with strict local permissions.
2. Add `quic peer add`, `list`, and `remove`; do not expose private keys.
3. For a two-host pair, configure exactly one remote certificate pin. Retain
   daemon allow-lists for multi-client use.
4. Design SHA-256 user-facing pins for new records while preserving the current
   Windows certificate-store lookup mechanics until migration is tested.
5. Add explicit re-enrollment and revocation behavior; never silently replace a
   changed remote identity.

**Validation:** Pair success, unpaired rejection, wrong pin, revoked peer,
certificate rotation, and Windows/Linux persistence tests.

**Performance impact:** None after connection setup.

### DAEMON-OPS: managed receiver controls

**Purpose:** Support an ingest/backup receiver without turning it into a broad
file-service platform.

**Implementation:**

1. Add `daemon status` and `daemon print-identity` based on local state and a
   small local control endpoint; avoid exposing an unauthenticated network admin
   port.
2. Handle graceful stop: stop accepting, let a bounded grace period complete,
   then preserve partial state for resume.
3. Add structured logs and stable connection/transfer IDs.
4. Add connection limits, optional peer-to-directory mappings, and quota hooks.
5. Keep systemd and Task Scheduler templates synchronized with the final CLI.

**Validation:** Service restart, graceful stop during a transfer, quota denial,
concurrent client limit, and log/receipt correlation tests.

**Performance impact:** Limits protect throughput under load; default logging
must avoid per-chunk synchronous writes.

## Release gates

Do not call a milestone complete until all are true:

1. `cargo check --workspace`, unit tests, and Clippy pass.
2. Windows and Linux E2E transfers cover success, rejection, interruption,
   resume, and integrity failure as applicable.
3. Direct QUIC and SSH fallback both emit a valid final receipt.
4. The benchmark report compares the change to the prior release baseline and
   SCP; performance claims use only direct-QUIC results and name the fixture.
5. README/help/examples match the shipping command behavior.
