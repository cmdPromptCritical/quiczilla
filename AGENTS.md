# AGENTS.md — Agentic DevOps Guide for Quiczilla

Welcome to **Quiczilla**. This file provides instructions, architectural invariants, and protocols for autonomous and pair-programming AI agents (Antigravity, Gemini, Claude, Cursor, Copilot).

---

## 1. Project Overview & Technology Stack

* **Application:** Cross-platform encrypted P2P file sharing over SSH bootstrap, QUIC, and mTLS.
* **Architecture:**
  * **CLI:** `cli/` provides the `quiczilla` / `quic` command and SSH bootstrap.
  * **Remote worker:** `worker/` receives transfers and supports pipe and daemon modes.
  * **Networking engine:** `core/` provides the Rust/Tokio/MsQuic transport and mTLS protocol.
  * **Future GUI:** Phase 2 will introduce a new desktop client over the SSH control plane and MsQuic data plane.
* **Reference Upstream:** C# / .NET 9 Avalonia project located in [`reference-csharp/`](reference-csharp/).

---

## 2. Cross-Session State Protocol (CRITICAL)

To maintain continuity across stateless agent sessions, all agents must adhere to the following protocol:

1. **At Session Start:**
   * Read [`PROJECT_STATE.md`](PROJECT_STATE.md) first to understand the current milestone, active blockers, recently completed changes, and the next recommended actions.
   * Run quick sanity checks to verify the environment (see Section 3).

2. **During Development:**
   * Respect the Architectural Rules (Section 4).
   * Follow planning mode for major architectural changes.

3. **At Session End / Milestone Completion:**
   * Append an entry to the **Session Handover Log** in [`PROJECT_STATE.md`](PROJECT_STATE.md).
   * Update the task status checklist in [`PROJECT_STATE.md`](PROJECT_STATE.md) and [`README.md`](README.md).
   * Ensure tests pass before concluding.

---

## 3. Standard Verification Commands

| Command | Purpose | Working Directory |
| :--- | :--- | :--- |
| `cargo check --workspace` | Fast Rust syntax and type checking | Project root |
| `cargo test --workspace --lib` | Run Rust core unit tests | Project root |

> **Note:** If running `quiczilla.exe` is currently locked on Windows by an active debug process, run `cargo check` or `cargo test --lib` to avoid file-lock errors.

---

## 4. Architectural Rules & Invariants

1. **Security Invariants:**
   * **Never accept raw filenames without sanitization:** Always use `Path::file_name()` to strip path separators and prevent directory traversal.
   * **Cryptographic Verification:** Handshake signatures in custom TLS verifiers must be verified via `rustls::crypto::ring::default_provider().signature_verification_algorithms`. Never use blind assertion stubs.

2. **Transport Integrity:**
   * QUIC stream management must preserve stream alignment. Do not reuse a single stream across multiple independent file transfers without framing or per-transfer stream allocation.

3. **Preserve Documentation:**
   * Keep comments, docstrings, and architectural diagrams accurate and synchronized when refactoring.
