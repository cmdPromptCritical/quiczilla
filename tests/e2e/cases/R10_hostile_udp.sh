#!/usr/bin/env bash
# R10 Hostile UDP fuzz / noise injection.
# Sends malformed datagrams and random byte bursts directly to the active
# receiver UDP port during an ongoing transfer.
# Asserts:
# 1. The active transfer does not crash or corrupt data.
# 2. The daemon process survives the packet flood without crashing or deadlocking.
source "$(dirname -- "${BASH_SOURCE[0]}")/../lib/common.sh"

prepare_dirs
port="${QZ_DAEMON_PORT:-55455}"
rdir="$REMOTE_DIR/daemon_fuzz"
remote "mkdir -p $(q "$rdir/recv")"

client_tp="$(client_identity "$WORK/client-identity")"
remote "printf '# authorized\n%s\n' $client_tp > $(q "$rdir/allow")"
remote "cd $(q "$rdir") && (nohup $(q "$QZ_REMOTE_WORKER_BIN") --daemon --port $port \
  --allow-thumbprints allow --save-dir recv --identity-dir daemon-identity \
  --on-conflict overwrite > daemon.out 2> daemon.err < /dev/null & echo \$! > daemon.pid) >/dev/null 2>&1 < /dev/null"

stop_daemon() {
  remote "if [ -f $(q "$rdir/daemon.pid") ]; then kill \$(cat $(q "$rdir/daemon.pid")) 2>/dev/null || true; fi" || true
  cleanup_remote_dir
}
trap stop_daemon EXIT

server_tp=""
for _ in $(seq 1 100); do
  server_tp="$(remote "sed -n 's/.*\"thumbprint\":\"\\([0-9A-F]*\\)\".*/\\1/p' $(q "$rdir/daemon.out") 2>/dev/null | head -n 1" || true)"
  [[ -n "$server_tp" ]] && break
  sleep 0.2
done
[[ "$server_tp" =~ ^[0-9A-F]{40}$ ]] || fail "daemon failed to start"

server_ip="$(resolve_ipv4 "$QZ_SERVER_HOST")"
endpoint="$server_ip:$port"

src="$WORK/target_payload.bin"
make_file "$src" 8388608 # 8 MiB

# Start direct transfer in background
qz direct_fuzz direct "$endpoint" "$src" --thumbprint "$server_tp" \
  --identity-dir "$WORK/client-identity" --checksum --no-progress &
transfer_pid=$!

# Spray hostile/malformed UDP datagrams toward the port
python3 - "$server_ip" "$port" <<'PY' &
import socket, sys, time, os
ip = sys.argv[1]
port = int(sys.argv[2])
sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)

# 1. Truncated QUIC Initial-like headers
fake_initial = b'\xc0\x00\x00\x00\x01\x08' + os.urandom(64)
# 2. Zero-length and garbage bursts
for _ in range(50):
    try:
        sock.sendto(b'', (ip, port))
        sock.sendto(fake_initial, (ip, port))
        sock.sendto(os.urandom(1200), (ip, port))
        sock.sendto(b'MALFORMED_HEADER_DATA', (ip, port))
        time.sleep(0.01)
    except Exception:
        pass
PY
fuzz_pid=$!

wait "$transfer_pid" && QZ_RC=0 || QZ_RC=$?
wait "$fuzz_pid" 2>/dev/null || true

expect_ok "direct_fuzz"
assert_same_file "$src" "$rdir/recv/target_payload.bin"

# Verify daemon process is still running healthy
remote "kill -0 \$(cat $(q "$rdir/daemon.pid"))" || fail "Daemon crashed from hostile UDP injection"
metric "daemon_survived_hostile_udp" true
log "R10 hostile UDP injection passed"
