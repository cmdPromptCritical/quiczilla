# Quiczilla Container E2E Test Suite

This directory contains the automated end-to-end container test suite for
Quiczilla, implementing the layers defined in
[`docs/PRODUCTION_TEST_PLAN.md`](../../docs/PRODUCTION_TEST_PLAN.md).

It verifies cross-container SSH bootstrap, managed remote worker deployment,
mTLS authentication, QUIC streaming, STUN discovery, network impairment,
zero-trust `noexec` compliance, and failure recovery.

---

## Topologies

### Topology T1: Single Host (Docker Compose)

Client and server run as two Linux containers connected via a private Docker bridge.
An optional `coturn` sidecar is available to test STUN locally without hammering production.

```text
client container (NET_ADMIN) ──[SSH / QUIC]──> server container (sshd, worker)
        │                                              │
        └──────────── (Optional local coturn) ─────────┘
```

> [!IMPORTANT]
> A shared Docker bridge cannot validate NAT traversal over the Internet. Both containers share the same host egress IP. Use T1 for protocol correctness, regression testing, and smoke verification.

### Topology T2: Split-Site (Kubernetes + Docker)

Client runs in a Kubernetes cluster (egress through cluster NAT), targeting a server running on a separate Docker host (home or edge router NAT), with STUN queries routed to the production STUN server.

```text
k8s Job client (cluster NAT) ──[SSH / STUN-QUIC]──> Docker server (edge NAT)
        │                                                    │
        └──────────> Production STUN (UDP 3478) <────────────┘
```

---

## Test Tiers

| Tier | Script Pattern | Target Runtime | Purpose |
| :--- | :--- | :--- | :--- |
| **Smoke** (`smoke`) | `cases/S*.sh` | ≤ 5 min | Fast validation on every code revision: build identity, STUN canary, forced transports, size edges, directory trees, pipe EOF, daemon. |
| **Regression** (`regression`) | `cases/R*.sh` | ≤ 30 min | Deep fault injection: fallback ladder, interruption & resume, conflict policy, path traversal safety, destination I/O errors, `noexec` cache, version skew, concurrency, `tc netem` impairment, hostile UDP fuzzing, memory ceilings. |
| **Split-Site** (`split`) | `cases/X*.sh` | On demand | Real WAN STUN traversal: NAT characterization, end-to-end STUN transfer, auto mode with direct path blocked, pipe over STUN. |

---

## Running the Tests (Topology T1)

Run from the repository root on any machine with Docker and Docker Compose:

### 1. Fast Smoke Test (Current Working Tree)
```bash
./tests/e2e/run.sh smoke
```

### 2. Smoke Test with STUN Canary
Pass your production STUN server (must resolve to IPv4, or pass `IP:3478`):
```bash
./tests/e2e/run.sh smoke --stun stun.example.com:3478
```

Or run with the isolated local `coturn` sidecar:
```bash
./tests/e2e/run.sh smoke --local-stun
```

### 3. Run a Specific Test Case
```bash
./tests/e2e/run.sh smoke --case S3
./tests/e2e/run.sh regression --case R1
```

### 4. Full Regression Suite
```bash
./tests/e2e/run.sh regression
```

### 5. Validate a Published Release Archive
Tests clean installation of the published GitHub release archive without compiling source:
```bash
./tests/e2e/run.sh smoke --source release --tag v0.1.12
```

---

## Running Split-Site Tests (Topology T2)

1. Deploy the server container on your Docker host with port forwarding for SSH and UDP:
   ```bash
   docker compose -f tests/e2e/compose.yaml up -d server
   ```
2. Create the secret and configmap in Kubernetes:
   ```bash
   kubectl create secret generic quiczilla-e2e-ssh-key --from-file=id_ed25519=~/.ssh/id_ed25519
   kubectl create configmap quiczilla-e2e-config \
     --from-literal=server_host="your-server.example.net" \
     --from-literal=server_ssh_port="22" \
     --from-literal=stun_server="stun.example.com:3478"
   ```
3. Apply the client job:
   ```bash
   kubectl apply -f tests/e2e/k8s/client-job.yaml
   kubectl logs -f job/quiczilla-e2e-client
   ```

---

## Test Results & Artifacts

Test runs generate structured machine-readable reports in `tests/e2e/results/<run-id>/`:
- `summary.json`: Top-level run metadata, duration, Git commit, image ID, and pass/fail summary.
- `results.jsonl`: One JSON object per test case with verdict, exit code, duration, failure reason, and recorded metrics.
- `<case-id>/case.log`: Full stdout/stderr trace of each test execution.
