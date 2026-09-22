# Contributing to Quiczilla

Thanks for helping make Quiczilla a reliable cross-platform file-transfer tool.
Bug reports, documentation corrections, tests, performance investigations, and
small focused code changes are all welcome.

## Before you start

- Read [AGENTS.md](AGENTS.md) and [PROJECT_STATE.md](PROJECT_STATE.md).
- Search existing issues and pull requests before opening a duplicate.
- Keep each pull request focused. Explain the user-visible change, the platforms
  tested, and any compatibility or performance trade-off.
- Use examples such as `ops@receiver.example.net`, never an operator's real
  host name, account name, address, path, credentials, or transfer data.

Please do not report a security vulnerability in a public issue. Use the
repository's private security-reporting channel when one is available, or
contact the maintainers privately.

## Development rules

Quiczilla's security and protocol invariants are not optional:

- Sanitize received names with `Path::file_name()`; never trust a remote path.
- Cryptographically verify custom TLS handshake signatures. Do not replace a
  verifier with a blind assertion.
- Keep separate file transfers on separate QUIC streams, or add unambiguous
  framing before sharing a stream.
- Preserve the SSH bootstrap, mTLS authentication, and integrity-verification
  behaviour unless a change is explicitly documented and tested.

Format Rust changes with `cargo fmt`. Keep comments, CLI help, and README
examples accurate when changing behaviour.

## Required verification

Run the following from the repository root before requesting review:

```powershell
cargo fmt --check
cargo check --workspace
cargo test --workspace --lib
cargo test -p quiczilla-cli
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release
```

For changes that affect packaging, embedded workers, networking, SSH bootstrap,
or the daemon, also perform a relevant end-to-end smoke test on every platform
changed. Confirm the transferred file's SHA-256 at both ends and state which
transport was selected (`direct-quic`, `stun-quic`, `manual-quic`, or SSH
fallback). Do not commit real endpoint details, logs containing them, or test
payloads.

For performance claims, include the command, payload sizes, number of runs,
median wall-clock throughput, selected transport, and checksum result. Compare
like with like: identify any difference in route, VPN/mesh use, congestion,
encryption, or server configuration. Benchmarks that fall back to SSH must not
be described as direct-QUIC results.

## Guidance for coding agents

Agents follow the same contribution rules as people, plus these safeguards:

- Read `AGENTS.md` and `PROJECT_STATE.md` before changing code.
- Do not commit secrets, personal identifiers, private network details, or
  generated benchmark artifacts.
- Preserve unrelated working-tree changes; do not reset, rewrite history, or
  delete files unless the task explicitly authorizes it.
- Update `PROJECT_STATE.md`'s public-safe handover when completing a milestone.
- Report commands run and verification results honestly, including failures or
  unavailable external dependencies.

## Pull-request checklist

- [ ] The change is focused and documented.
- [ ] Required verification passed, or exceptions are explained.
- [ ] Platform-specific and network-sensitive behaviour was tested where needed.
- [ ] No secrets, personal endpoint details, or generated payloads are included.
- [ ] Documentation and release notes reflect user-visible changes.
