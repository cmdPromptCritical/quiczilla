# Quiczilla Product Backlog

## Product decision

Quiczilla will be a **QUIC-accelerated, SSH-compatible file mover for large,
verified transfers between Windows and Linux hosts an operator controls**.

The flagship path is the existing SSH-bootstrap command:

```text
quic <local-file> <user@host:destination-directory>
```

The persistent daemon is a secondary **managed receiver / drop target** for
stable private networks. It is not a public sharing service.

## Deliberate non-goals

- Public invitation codes, public relays, consumer device discovery, or social
  sharing. Those belong to tools such as croc and Thruflux.
- Object-storage, browser, and cloud-workflow replacement.
- Continuous directory synchronization or broad FileZilla-style GUI work before
  the CLI is demonstrably faster and more reliable than SCP for its target use.

## Definition of success

For a direct QUIC path, Quiczilla should target at least **1.15× SCP's median
payload throughput** for large sequential files on the primary representative
WAN fixture, while retaining a safe SSH fallback. This is a benchmark target,
not a promise that any protocol wins on every LAN, disk, CPU, firewall, or
route.

Every completed transfer must report its selected transport, bytes, elapsed
time, integrity mode/result, destination, and a useful nonzero failure code.

## Now — reliability and evidence

- [ ] **PERF-BASELINE:** Build a repeatable release-mode benchmark suite that
      compares Quiczilla direct QUIC, Quiczilla SSH fallback, SCP, and rsync
      over the same 1 GiB and 10 GiB fixtures. Record bootstrap separately from
      payload time, plus RTT, loss, CPU, disk type, and path selected.
- [ ] **SAFE-COMMIT:** Receive to a unique partial file and atomically rename
      only after a successful transfer and requested verification. Preserve the
      existing destination on cancellation, rejection, or checksum failure.
- [ ] **STREAM-STATE:** Allocate a dedicated framed QUIC stream for every
      transfer; add explicit finish/reset/close state and cancellation. This
      removes byte-alignment risk after a failed transfer and enables safe future
      multi-file parallelism.
- [ ] **TRANSFER-RESULT:** Extend the new stable file/direct exit handling to
      every subcommand and document the final human receipt. File/direct
      transfers now support `--json` success/failure output and identify
      `direct-quic`, `stun-quic`, `manual-quic`, or `ssh-fallback`.
- [ ] **DAEMON-SAFETY:** Require `--save-dir` in daemon mode (or print the
      resolved absolute path before listening), keep explicit conflict policy,
      and document that direct daemon mode is upload-to-daemon only.

## Next — make the fast path faster

- [ ] **DISK-PIPELINE:** Profile and pipeline disk reads/writes and optional
      hashing so storage work does not starve MsQuic callbacks. Use bounded
      buffers and no extra full-file copy.
- [ ] **MULTI-FILE:** Add recursive directory transfer backed by a manifest,
      per-file atomic receive, resume metadata, and one stream per file.
      Schedule independent files with bounded concurrency.
- [ ] **NETWORK-AUTO:** Make path selection observable and bounded: direct
      QUIC first, optional STUN/manual QUIC, then SSH fallback. Add adjustable
      connection deadline and clear diagnostics without delaying the fallback
      indefinitely.
- [ ] **INTEGRITY-MODES:** Define explicit integrity modes (`none`, `sha256`,
      and later a fast streaming hash if profiling warrants it). Benchmark their
      overhead before changing defaults; never silently claim verification.
- [ ] **TRANSPORT-TUNING:** Profile MsQuic settings, socket buffers, pacing,
      MTU/GSO availability, flow-control windows, and chunk size on supported
      Windows and Linux versions. Land only measured improvements.

## Then — operational receiver and automation

- [ ] **PAIRING:** Add `quic peer add/list/remove` around existing mTLS
      identities. A paired two-host mode should pin exactly one counterpart;
      an allow-list remains for multi-client daemon use.
- [ ] **DAEMON-OPS:** Add `daemon status`, `daemon print-identity`, structured
      logs, graceful shutdown, connection limits, destination mappings, and
      quota/policy hooks.
- [ ] **SCRIPTABLE-CLI:** Introduce source-first `quic send` while retaining
      the existing transfer syntax as a compatible alias. Add shell completion
      and examples for SSH identities, verification, resume, and automation.
- [ ] **SERVICE-POLISH:** Keep the existing systemd and Windows Task Scheduler
      templates current; consider a native Windows Service wrapper only after
      daemon lifecycle APIs are stable.

## Completed foundation

- [x] Cross-platform release packaging for Windows x64 and modern Linux x64.
- [x] SSH bootstrap with native embedded worker/cache and SSH fallback.
- [x] Strict command validation, command-specific help, and `-q` support.
- [x] Persistent mTLS daemon/client identities with daemon client allow-lists
      and client daemon-certificate pinning.
- [x] Explicit daemon destination policy: `overwrite` or `refuse`.
- [x] Linux systemd and Windows Task Scheduler deployment guidance.

## Detailed implementation plans

See [IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md) for sequencing, file-level
design boundaries, performance impact, test strategy, and release gates.
