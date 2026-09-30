# Quiczilla Production Test Plan

This plan defines the evidence required before Quiczilla is described as ready
for production workloads. It is intentionally broader than the current CI
smoke tests. A passing test suite demonstrates the tested behavior; it does
not establish an unlimited capacity or reliability guarantee.

Use documentation addresses and disposable test data in examples. Do not add
real hostnames, usernames, credentials, public addresses, or private transfer
data to this repository.

## Current baseline

The repository currently verifies Rust formatting, workspace compilation,
workspace tests, CLI tests, Clippy, release builds, and local daemon end-to-end
transfers on Linux and Windows. Published x86_64 archives have also been
validated with a Windows-to-Linux transfer and SHA-256 checks.

The next test layer should use clean release artifacts, not binaries copied
from a developer checkout.

## Container strategy

Containers are the fast, repeatable Linux test layer. Two containers are enough
for protocol, framing, authentication, upgrade, and basic load tests:

```text
client container  ─────────  worker/daemon container
```

For NAT and STUN behavior, use a topology with separate network namespaces:

```text
client ── impaired link ── router/NAT ── server
                              │
                              └── STUN server
```

The default Docker bridge is not a realistic public-NAT test. It generally
provides an easy private route and therefore must not be treated as evidence
that Internet traversal works.

Container tests should install the client and worker from the same release
archive, then deliberately run different client/worker versions for upgrade
and compatibility tests. Capture the image digest, Quiczilla version, commit,
network topology, and test parameters with every result.

## Test layers and required evidence

### 1. Correctness and protocol tests

Cover:

- Empty, tiny, medium, and multi-gigabyte files.
- Empty directories, deep trees, Unicode names, long names, and large file
  counts.
- Existing destinations, overwrite/refuse behavior, and atomic finalization.
- Source and destination symlinks, special files, absolute paths, `..`, and
  Windows path separators.
- Truncated frames, unknown frame types, invalid lengths, duplicate end frames,
  missing end frames, and oversized declared values.
- Incorrect checksums and corrupted payloads.
- Cancellation, process termination, remote restart, and reconnect behavior.
- Disk-full, permission-denied, read-only, and `noexec` destinations.
- Concurrent use of the same managed worker cache.
- Interrupted worker uploads and incomplete cache directories.

Acceptance criteria:

- No destination path escapes the requested root.
- No incomplete file is presented as complete.
- Failed transfers return deterministic nonzero exit codes.
- Temporary files are removed or clearly reported for recovery.
- Malformed peer input cannot cause a panic, deadlock, unbounded allocation,
  or process-wide crash.

### 2. Fuzzing and property testing

Add fuzz targets for:

- Directory frame decoding.
- Control-message decoding.
- Relative-path validation.
- Length and count fields.
- Worker/CLI handshake state transitions.

Keep minimized failing inputs as regression fixtures. Run a short fuzz job on
pull requests and longer jobs nightly. The Rust Fuzz Book and `cargo-fuzz`
provide the standard libFuzzer workflow:
<https://rust-fuzz.github.io/book/>.

### 3. Network impairment

With Linux namespaces and `tc netem`, test:

- 0%, 0.1%, 1%, and 5% packet loss.
- 10–500 ms round-trip delay.
- Reordering, duplication, and bandwidth limits.
- UDP blocked, STUN unavailable, direct UDP unavailable, and SSH fallback.
- NAT and symmetric-NAT behavior.
- Client, worker, router, and STUN process restarts during transfers.

Record completion rate, recovery time, final checksum, retransmission behavior,
selected transport, and partial-file state.

### 4. Load, soak, and capacity

Run a controlled matrix across:

| Dimension | Minimum matrix |
| --- | --- |
| File size | 0 B, 1 KiB, 64 KiB, 1 MiB, 1 GiB, 10 GiB+ |
| Tree shape | 1 file, 1,000 files, 100,000 files, deep tree |
| Storage | HDD, SATA SSD, NVMe |
| Network | 1 Gbps, 10 Gbps, high-latency WAN |
| Concurrent transfers | 1, 2, 4, 8, 16 |
| Duration | 15-minute, 1-hour, and overnight soak |

Measure p50/p95/p99 duration and throughput, bootstrap time, peak RSS, CPU,
open handles, temporary disk usage, disk queue, packet loss, and error rate.
Establish a documented safe concurrency and file-count limit with margin;
do not infer capacity from one three-run benchmark.

### 5. Security validation

Maintain a threat model covering:

- A malicious SSH-authenticated sender.
- An unauthorized daemon client.
- A compromised worker or release artifact.
- STUN abuse and reflection/amplification.
- Path traversal and symlink races.
- Resource exhaustion through counts, lengths, files, or streams.
- Worker downgrade and version-skew attacks.
- Leaked logs, identities, and transfer metadata.

Run `cargo audit` (or an equivalent RustSec check) in CI, review all unsafe
MsQuic FFI boundaries, and add explicit limits for frame sizes, directory
counts, concurrent transfers, and cache growth. Threat modeling should be
revisited as the protocol and daemon evolve.

### 6. Packaging and release assurance

For every release:

1. Build from an immutable tag with pinned toolchain and dependency inputs.
2. Produce Windows and Linux archives plus SHA-256 checksums.
3. Generate an SBOM and release provenance attestation.
4. Sign the release metadata or artifacts.
5. Verify the artifacts from clean Windows and Linux machines.
6. Verify embedded worker/runtime checksums.
7. Test upgrade, rollback, worker refresh, and `noexec` behavior.
8. Retain build logs, test reports, and the exact release inputs.

GitHub artifact attestations and SLSA provenance are suitable foundations for
this workflow:

- <https://docs.github.com/en/actions/how-tos/secure-your-work/use-artifact-attestations>
- <https://slsa.dev/spec/v1.2-rc1/build-requirements>

### 7. Platform matrix

Containers cover Linux behavior but cannot replace host testing. Release tests
should include Ubuntu 22.04/24.04, Debian stable, one Fedora/Rocky target,
Windows 11, and Windows Server. Include clean systems with and without a
system MsQuic installation, restricted SSH accounts, non-ASCII paths, IPv4,
IPv6, dual-stack, read-only, and `noexec` environments.

### 8. Daemon operations

Before production daemon use, provide:

- systemd and Windows service packaging;
- an unprivileged service account and explicit save-directory permissions;
- connection, transfer, and resource limits;
- structured redacted logs and log rotation;
- readiness/health status and stable exit codes;
- certificate/key rotation and revocation procedures;
- restart policy, rollback, and upgrade documentation;
- metrics for active transfers, bytes, failures, duration, and cache size.

## Release gates

### Alpha

Trusted personal or pilot use only. Require current CI, manual cross-platform
transfers, checksum verification, basic malformed-input tests, and dependency
auditing. State that compatibility, resume, and capacity guarantees are
limited.

### Release candidate

Require the network-impairment matrix, 24-hour soak, concurrent transfers,
large-directory tests, interruption/restart tests, representative HDD/NVMe
measurements, Linux distro coverage, artifact signing/provenance, and a
documented threat-model review.

### Production

Require repeatable capacity limits with safety margin, p95/p99 reliability
data, operational daemon packaging, rollback and incident procedures, ongoing
dependency/security maintenance, and independent review of transport,
authentication, and upgrade code.

## Honest current claim

Until the release-candidate gates are complete, the accurate description is:

> Quiczilla has passed cross-platform functional tests and controlled live
> transfer benchmarks for x86_64 Windows/Linux. It is suitable for alpha and
> trusted pilot use. Production-load readiness remains unproven until
> fault-injection, fuzzing, sustained-load, security, and release-assurance
> gates are complete.

