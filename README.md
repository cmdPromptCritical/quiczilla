# Quiczilla

> **High-Throughput Encrypted P2P File Transfers Over QUIC and mTLS**

Quiczilla is a cross-platform peer-to-peer direct file transfer system combining the simplicity and universal accessibility of **SSH** with the massive throughput and multiplexing of **QUIC (RFC 9000)** and **mTLS** encryption.

* **Phase 1 (CLI):** Feature-comparable with `scp` over SSH, but powered by direct UDP/QUIC data streams with a zero-install, cached remote worker.
* **Phase 2 (GUI):** A new FileZilla-style desktop client will be built on the completed CLI transport, with remote browsing, transfer queues, and resumption.

---

## Quick Install

### Linux (Bash)
```bash
curl -fsSL https://raw.githubusercontent.com/cmdPromptCritical/quiczilla/master/install.sh | bash
```

### Windows (PowerShell)
```powershell
irm https://raw.githubusercontent.com/cmdPromptCritical/quiczilla/master/install.ps1 | iex
```

### From Source (Cargo)
```bash
git clone https://github.com/cmdPromptCritical/quiczilla.git
cd quiczilla
# Linux source builds need the native MsQuic runtime. Official release
# archives already include it.
sudo apt update
sudo apt install -y libmsquic
cargo build --release
# Binary available at target/release/quiczilla-cli (or install.ps1 / ./install.sh)
```

On Ubuntu/Debian systems where `libmsquic` is not yet available, add
[Microsoft's package repository](https://packages.microsoft.com/config/ubuntu/)
for your distribution first, then install the package above. The runtime must
be discoverable as `libmsquic.so`/`libmsquic.so.2`, or copied beside the CLI.
When present, the Linux source build embeds that same runtime for zero-install
remote-worker bootstrap as well.
Windows source builds use the installed `msquic.dll` from the .NET runtime.

`cargo build --release` also builds the local-platform worker. A source-built
CLI prefers that fresh sibling worker for a same-platform remote host; for a
cross-platform target, use the official release archive so it has the matching
CI-built worker for that target.

To build every active binary, including the remote worker:

```bash
cargo build --release
```

When run from a checkout, the installers use a release binary already present at `target/release/`; otherwise they download the matching self-contained release artifact. Each official archive includes the CLI, the pre-installed worker, and its matching MsQuic runtime; Rust, Cargo, .NET, and a system MsQuic package are not required.

Direct downloads always resolve to the newest published release:

* Windows x64: `https://github.com/cmdPromptCritical/quiczilla/releases/latest/download/quiczilla-windows-x86_64.zip`
* Linux x64: `https://github.com/cmdPromptCritical/quiczilla/releases/latest/download/quiczilla-linux-x86_64.tar.gz`

For a private GitHub repository, set `QUICZILLA_GITHUB_TOKEN` to a token that can read repository contents and releases
before running the installer. Public repositories need no token.

For a private repository, authenticate the script fetch as well:

```powershell
$env:QUICZILLA_GITHUB_TOKEN = "<github-token>"
$h = @{ Authorization = "Bearer $env:QUICZILLA_GITHUB_TOKEN" }
Invoke-RestMethod https://raw.githubusercontent.com/cmdPromptCritical/quiczilla/master/install.ps1 -Headers $h | Invoke-Expression
```

```bash
export QUICZILLA_GITHUB_TOKEN='<github-token>'
curl -fsSL -H "Authorization: Bearer $QUICZILLA_GITHUB_TOKEN" \
  https://raw.githubusercontent.com/cmdPromptCritical/quiczilla/master/install.sh | bash
```

### Publishing a release

Push a version tag to produce a GitHub Release, both x64 archives, and `SHA256SUMS`:

```bash
git tag v0.1.1
git push origin v0.1.1
```

The release workflow builds each worker on its native OS, embeds both worker targets and their native MsQuic runtimes into each CLI, then packages the matching local runtime beside the installed binaries. Linux artifacts are built on Ubuntu 22.04 using Microsoft’s supported `libmsquic` package and target modern glibc x64 distributions.

For operating Quiczilla's optional public default STUN service, see
[deploy/STUN.md](deploy/STUN.md). The service performs candidate discovery only;
file data remains direct QUIC or SSH fallback.

### Benchmarking against SCP

The repeatable PowerShell harness compares release-mode Quiczilla, `scp`, and
optionally `rsync` against the same remote Linux host. It records both wall-clock
time and Quiczilla’s payload time, reports the selected transport, and verifies
each destination with SHA-256:

```powershell
.\scripts\benchmark_transfer.ps1 `
  -SourcePath .\test_1gb.bin `
  -SshTarget ops@receiver.example.net `
  -RemoteDir /srv/quiczilla-benchmark `
  -SshPort 2222 `
  -QuicPath .\target\release\quiczilla-cli.exe
```

The script intentionally retains uniquely named remote benchmark artifacts so
results can be inspected; remove them after recording a result. Compare medians
from at least three runs and do not treat SSH-fallback measurements as direct
QUIC performance. Add `-SkipRsync` when rsync is unavailable on the client;
the harness then compares Quiczilla and SCP only. Add `-RequireDirectQuic` to
fail immediately if Quiczilla falls back to SSH. Add `-StunServer host:port`
to force the Quiczilla leg through the configured STUN-assisted path.

---

## How It Works (SSH Bootstrap + QUIC Tunnel)

Traditional `scp` and `sftp` suffer from single-TCP stream bottlenecks, head-of-line blocking, and high latency over lossy or cross-region connections. Traditional P2P tools require persistent background daemons (`sshd`-like services) or third-party cloud signaling servers.

Quiczilla takes a **hybrid zero-install approach**:

```
[Local Machine (Windows / Linux)]
    │
    │  1. SSH Handshake: Probe OS & Check SHA-256 Cache
    │  2. If cache miss: Stream embedded lightweight static worker
    │  3. Remote executes worker: echoes UDP port & ephemeral thumbprint
    │
    ├─── Control Plane (SSH Standard I/O)
    │       ├── Target probe (`uname -sm` / Windows arch)
    │       └── Ephemeral mTLS thumbprint exchange
    │
    └─── Data Plane (Direct UDP / QUIC Tunnel)
            ├── High-throughput UDP file chunking (2 MiB chunks)
            ├── Strict mTLS handshake verification
            └── Real-time SHA-256 verification & auto parent dir creation
```

1. **Ephemeral mTLS:** Local CLI generates a one-time self-signed certificate and calculates its thumbprint.
2. **OS Probe & Userspace Cache:** Checks `~/.cache/quiczilla/` (Linux) or `%LOCALAPPDATA%\quiczilla\` (Windows).
   * **Cache Hit:** If the worker binary with matching SHA-256 is already cached, it launches immediately (<0.85s bootstrap).
   * **Cache Miss:** Automatically pipes the compressed embedded worker binary over SSH stdin and sets executable permissions.
3. **Direct QUIC Tunnel:** The remote worker binds to a dynamic UDP port and prints its port and thumbprint. The local CLI dials the remote IP over UDP, completes the mTLS handshake, and blasts file streams at wire speed.

---

## CLI Usage

> **Note:** The CLI is available interchangeably as both `quic` and `quiczilla`. Both commands are registered by the installers.

### 1. Direct File Transfer
Transfer local files to a remote destination with automatic SSH bootstrap, userspace binary caching, and direct QUIC streaming:

```bash
quic <local_file> <user@host:destination_folder/> [-c|--resume] [--checksum] [-i|--identity <key>]
    [-q|--quiet|--no-progress|--verbose|--json] [-p|--port <ssh-port>]
    [--transport auto|direct|stun|manual|ssh] [--stun-server <host:port>]
    [--quic-host <host-or-ip>] [--quic-port <port>]

# Examples:
quic dataset.tar.gz ops@receiver.example.net:/srv/incoming/
quic -c -i ~/.ssh/id_ed25519 large_disk.iso admin@192.0.2.50:C:\Transfers\
```

The default terminal display updates every 500 ms with the file name, a progress bar, transferred bytes, rate, and ETA. Progress is written to standard error so standard output remains available to callers.

For automation and benchmarking, add `--json`. On successful file transfers it
emits one JSON receipt on standard output containing the selected transport,
payload duration, byte count, throughput, and integrity result. Human progress
and diagnostics remain on standard error. Structured failure exit codes are
planned as a follow-up to the transfer lifecycle refactor.

* **Hot Pause / Resume:** Press <kbd>Space</kbd> or <kbd>p</kbd> at any time during an active transfer to instantly pause transmission without closing the connection. Press <kbd>Space</kbd> or <kbd>p</kbd> again to resume. Press <kbd>q</kbd> or <kbd>Ctrl+C</kbd> to cancel.
* **Cold Resumption (`-c`, `--resume`):** Interrupted transfers can be resumed at any time by specifying `-c` or `--resume`. For files ≥ 20 MiB, transfers stage in `<file>.quic-part` aligned to 2 MiB boundaries with a rapid 64 KiB prefix fingerprint check. If the local file changes, Quiczilla automatically falls back to restarting from byte 0.

### Connection selection

`--transport auto` is the default. It gives the SSH hostname, LAN, VPN, or mesh
candidate a 150 ms head start, then races the STUN-reflexive public candidate
when a STUN service is configured. The first mTLS-authenticated QUIC connection
wins; STUN never carries file data. If neither UDP candidate works, auto mode
falls back to SSH streaming.

Set `QUICZILLA_STUN_SERVER=stun.example.net:3478` for a per-user default, or
set the GitHub repository variable `QUICZILLA_DEFAULT_STUN_SERVER` before a
release to bake the official service into release binaries. Command-line
`--stun-server` takes precedence over both.

```bash
# STUN-assisted UDP, with SSH used only for bootstrap or fallback
quic archive.tar ops@receiver.example.net:/srv/incoming/ \
  -p 2222 --stun-server stun.example.net:3478

# Prefer an explicitly routed LAN, VPN, mesh, or known public address while
# retaining SSH only for authenticated bootstrap.
quic archive.tar ops@receiver.example.net:/srv/incoming/ \
  -p 2222 --transport direct --quic-host 192.0.2.44

# A fixed port you have forwarded as UDP on the remote router
quic archive.tar ops@receiver.example.net:/srv/incoming/ \
  -p 2222 --transport manual --quic-host 203.0.113.10 --quic-port 55441

# Force SSH streaming when only the SSH port is intentionally exposed
quic archive.tar ops@receiver.example.net:/srv/incoming/ -p 2222 --transport ssh
```

### 2. Universal Raw Stream / Pipe Mode (`quic pipe`)
Transform Quiczilla into an encrypted, multiplexed UNIX pipe. Replaces `nc` over WireGuard for arbitrary streams, unseekable inputs, and live process pipelines:

```bash
# Push a ZFS snapshot across the WAN directly into remote zfs receive
zfs send pool/dataset@snap | quic pipe user@remote "zfs receive backup/dataset"

# Stream an on-the-fly tarball to a remote directory without temporary files
tar czf - /var/data | quic pipe user@remote "tar xzf - -C /backup"

# Pull a remote database dump or command output directly into local stdout
quic pipe user@remote "pg_dump production_db" > local_backup.sql

# Optional: Add --verify to compute and log end-to-end SHA-256 wire checksums
cat dump.bin | quic pipe user@remote "cat > /data/dump.bin" --verify
```

### 3. Persistent Daemon Mode (`quic daemon`)
For environments with persistent network meshes (WireGuard, Tailscale, LAN) or locked-down servers that prefer an always-on listener without per-transfer SSH negotiation:

```bash
# 1. On each client, create its persistent mTLS identity and copy its printed thumbprint.
quic identity
# 720A73366D921F1E7FEB13E3BB758AFA3E34753E

# 2. On the receiving host, authorize that client (one thumbprint per line;
# blank lines and comments are allowed), then start the receiver.
printf '%s\n' 720A73366D921F1E7FEB13E3BB758AFA3E34753E \
  | sudo tee /etc/quiczilla/authorized_thumbprints
quic daemon --port 55441 --allow-thumbprints /etc/quiczilla/authorized_thumbprints --save-dir /backup --on-conflict refuse
# {"status":"daemon_ready","udp_port":55441,"thumbprint":"A9D91656F38DB941CC35498552AE4FF9FA4F979B"}

# 3. On the authorized client, pin the daemon thumbprint printed at startup.
quic direct receiver.example:55441 archive.tar \
  --thumbprint A9D91656F38DB941CC35498552AE4FF9FA4F979B --checksum
```

The daemon requires a non-empty allow-list; it authenticates every client certificate against every listed thumbprint and saves only the local basename into `--save-dir`. Existing files are overwritten by default for compatibility; use `--on-conflict refuse` to reject a transfer when that basename already exists (resumes remain allowed when `--resume` is requested). `quic direct` requires the daemon thumbprint, so both sides are authenticated and pinned. Open or forward UDP (not TCP) for the selected daemon port when the hosts are not already on the same private mesh.

Identities are stable across restarts: Linux stores protected PEM material under `~/.config/quiczilla/{client,daemon}` (respecting `XDG_CONFIG_HOME`); Windows stores the private key in the current user's Windows certificate store and records its selected thumbprint under `%LOCALAPPDATA%\quiczilla`. `--identity-dir <dir>` makes that identity selection explicit for automation and testing. Use `quic daemon --help`, `quic direct --help`, or `quic identity --help` for the complete command-specific syntax. A thumbprint is a certificate identifier; TLS 1.3 and mTLS provide the actual cryptographic authentication.

### 4. Zero-Trust / NoExec Compliance
For early alpha releases, Quiczilla checks the SHA-256 of a detected installed
remote worker against the worker embedded in the client. A match runs the
installed worker directly. A mismatch refreshes the managed worker bundle in
`~/.cache/quiczilla/bundle-<sha256>/` (Linux) or
`%LOCALAPPDATA%\quiczilla\bundle-<sha256>\` (Windows), so a new client
automatically uses its matching worker/runtime pair. The content-addressed
directory never overwrites a worker that another transfer is still executing.
If that managed location cannot execute—for example, due to a `noexec`
policy—Quiczilla warns and uses the existing installed worker instead. Future
releases will add an explicit protocol-compatibility check to that fallback.

### Benchmark Results (Live Tailscale Test)

Illustrative benchmark format for a Windows client and a Linux receiver:

| Mode / Payload | Bootstrap Latency | QUIC Handshake | Transfer Duration | Average Throughput | Verification |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **Pipe Upload** (`quiczilla pipe`) | 0.71s (Pre-installed) | 8.23 ms | 0.01s | Stream Wire Speed | ✅ SHA-256 Verified |
| **Pipe Download** (`quiczilla pipe`) | 0.72s (Pre-installed) | 7.26 ms | 0.01s | Stream Wire Speed | ✅ SHA-256 Verified |
| **File Transfer (25 MB)** | 0.76s (Pre-installed) | 7.02 ms | 2.65s | **9.41 MB/s** | ✅ SHA-256 Verified |
| **File Transfer (50 MB)** | 0.76s (Pre-installed) | 7.02 ms | 5.31s | **9.41 MB/s** | ✅ SHA-256 Verified |

### Running Automated Benchmarks

An automated test script is included to test transfers, log bootstrap and throughput metrics, and verify remote SHA-256 integrity:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\test_transfer.ps1 -Target "ops@receiver.example.net:/srv/quiczilla-test/" -SizeMB 25
```

---

## Workspace Architecture

The project is structured as a modular Cargo Workspace:

| Crate / Subdirectory | Purpose |
| :--- | :--- |
| [`core/`](core/) | Reusable `quiczilla-core` library: MsQuic transport, mTLS certificate setup, framing, and chunk streaming. |
| [`worker/`](worker/) | Lightweight `quiczilla-worker` binary: Remote receiver executed over SSH that dynamically binds UDP and receives files. |
| [`cli/`](cli/) | Main `quiczilla-cli` executable: Handles CLI arguments, SSH bootstrap pipeline, embedded binary matrix, and QUIC dialing. |

---

## Development & Verification

### Prerequisites
* **Rust:** 1.85+ (Edition 2024)
* **OpenSSH:** Standard `ssh` client on your system PATH.

### Standard Commands

```bash
# Fast syntax and type checking
cargo check --workspace

# Run Rust unit tests across all workspace crates
cargo test --workspace --lib

# Build all release binaries
cargo build --release
```

---

## Roadmap

- [x] **Phase 1: Zero-Install SSH-Bootstrapped QUIC CLI**
  - [x] Cargo workspace modularization (`core`, `worker`, `cli`).
  - [x] Cross-platform SSH remote probe & userspace caching (`~/.cache/quiczilla` & `%LOCALAPPDATA%\quiczilla`).
  - [x] Embedded binary matrix (`include_bytes!`) for Linux x86_64 and Windows x86_64.
  - [x] Dynamic UDP port allocation and handshake verification.
  - [x] Path traversal protection (`Path::file_name()`) and automatic parent directory creation.
  - [x] Automated benchmark script and throughput logging.
  - [x] One-line installation scripts (`install.sh` and `install.ps1`).
- [ ] **Phase 2: FileZilla-Style Desktop GUI**
  - [ ] Establish a new desktop application shell and GUI architecture.
  - [ ] Remote directory tree browsing (`ls`, `stat`, `mkdir`) over SSH control channel.
  - [ ] Drag-and-drop transfer queue with concurrent QUIC streams.
  - [ ] Per-transfer stream isolation and clean transfer cancellation.
