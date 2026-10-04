#!/usr/bin/env bash
# R8 Concurrency.
# Tests concurrent transfer execution:
# 1. 4 concurrent SSH-bootstrap transfers against a shared managed cache.
# 2. 8 concurrent direct transfers against a single persistent daemon instance.
source "$(dirname -- "${BASH_SOURCE[0]}")/../lib/common.sh"

prepare_dirs
trap cleanup_remote_dir EXIT

# --- Part 1: 4 parallel bootstrap transfers ---
log "Running 4 parallel SSH-bootstrapped transfers..."
pids=()
labels=()
for i in $(seq 1 4); do
  src="$WORK/parallel_boot_$i.bin"
  make_file "$src" 2097152 # 2 MiB
  dest_dir="$REMOTE_DIR/boot_$i"
  remote "mkdir -p -- $(q "$dest_dir")"
  
  "$QZ_BIN" "$src" "$TARGET:$dest_dir/" --no-progress --checksum "${QZ_SSH_ARGS[@]}" \
    > "$CASE_DIR/boot_$i.out" 2> "$CASE_DIR/boot_$i.err" &
  pids+=("$!")
  labels+=("boot_$i")
done

# Wait for all bootstrap transfers
for idx in "${!pids[@]}"; do
  pid="${pids[$idx]}"
  label="${labels[$idx]}"
  wait "$pid" || fail "Parallel transfer $label failed"
  assert_same_file "$WORK/parallel_boot_$((idx+1)).bin" "$REMOTE_DIR/$label/parallel_boot_$((idx+1)).bin"
done
metric "parallel_bootstrap_4_ok" true

# --- Part 2: 8 parallel direct transfers against daemon ---
log "Starting persistent daemon for 8 concurrent direct transfers..."
port="${QZ_DAEMON_PORT:-55453}"
rdir="$REMOTE_DIR/daemon_concurrency"
remote "mkdir -p $(q "$rdir/recv")"

client_tp="$(client_identity "$WORK/client-identity")"
remote "printf '# authorized\n%s\n' $client_tp > $(q "$rdir/allow")"
remote "cd $(q "$rdir") && nohup $(q "$QZ_REMOTE_WORKER_BIN") --daemon --port $port \
  --allow-thumbprints allow --save-dir recv --identity-dir daemon-identity \
  --on-conflict overwrite > daemon.out 2> daemon.err < /dev/null & echo \$! > daemon.pid"

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

dpids=()
for i in $(seq 1 8); do
  dsrc="$WORK/daemon_concur_$i.bin"
  make_file "$dsrc" 1048576 # 1 MiB
  "$QZ_BIN" direct "$endpoint" "$dsrc" --thumbprint "$server_tp" \
    --identity-dir "$WORK/client-identity" --checksum --no-progress \
    > "$CASE_DIR/daemon_c_$i.out" 2> "$CASE_DIR/daemon_c_$i.err" &
  dpids+=("$!")
done

for idx in "${!dpids[@]}"; do
  dpid="${dpids[$idx]}"
  wait "$dpid" || fail "Direct transfer $idx against daemon failed"
  assert_same_file "$WORK/daemon_concur_$((idx+1)).bin" "$rdir/recv/daemon_concur_$((idx+1)).bin"
done
metric "parallel_daemon_8_ok" true

remote "kill -0 \$(cat $(q "$rdir/daemon.pid"))" || fail "daemon died during concurrent load"
log "R8 concurrency tests passed"
