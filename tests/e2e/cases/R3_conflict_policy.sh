#!/usr/bin/env bash
# R3 Conflict policy.
# Tests overwrite (default for SSH transfers) vs refuse (--on-conflict refuse in daemon mode):
# 1. Standard transfer: existing destination file is overwritten cleanly.
# 2. Daemon with --on-conflict refuse: re-transferring without -c is rejected, leaving the original file intact.
# 3. Daemon with --on-conflict refuse: re-transferring with -c (resume) is permitted.
source "$(dirname -- "${BASH_SOURCE[0]}")/../lib/common.sh"

prepare_dirs
port="${QZ_DAEMON_PORT:-55451}"
rdir="$REMOTE_DIR"

stop_daemon() {
  remote "if [ -f $(q "$rdir/daemon.pid") ]; then kill \$(cat $(q "$rdir/daemon.pid")) 2>/dev/null || true; fi" || true
  cleanup_remote_dir
}
trap stop_daemon EXIT

# --- Part 1: Overwrite in standard transfer ---
dest_std="$rdir/std_dest"
remote "mkdir -p -- $(q "$dest_std")"
remote "printf 'ORIGINAL_CONTENT\n' > $(q "$dest_std/test.txt")"
orig_hash="$(remote_sha "$dest_std/test.txt")"

src_std="$WORK/test.txt"
printf 'UPDATED_OVERWRITTEN_CONTENT\n' > "$src_std"
new_hash="$(local_sha "$src_std")"
[[ "$orig_hash" != "$new_hash" ]] || fail "hashes should be different"

send_file "std_overwrite" "$src_std" "$dest_std" --checksum
expect_ok "std_overwrite"
assert_same_file "$src_std" "$dest_std/test.txt"
log "Standard transfer overwrite succeeded"
metric "standard_overwrite_ok" true

# --- Part 2: Refuse policy in daemon mode ---
client_tp="$(client_identity "$WORK/client-identity")"
remote "mkdir -p $(q "$rdir/daemon_recv") && printf '# authorized\n%s\n' $client_tp > $(q "$rdir/allow")"
remote "cd $(q "$rdir") && (nohup $(q "$QZ_REMOTE_WORKER_BIN") --daemon --port $port \
  --allow-thumbprints allow --save-dir daemon_recv --identity-dir daemon-identity \
  --on-conflict refuse > daemon.out 2> daemon.err < /dev/null & echo \$! > daemon.pid) >/dev/null 2>&1 < /dev/null"

server_tp=""
for _ in $(seq 1 100); do
  server_tp="$(remote "sed -n 's/.*\"thumbprint\":\"\\([0-9A-F]*\\)\".*/\\1/p' $(q "$rdir/daemon.out") 2>/dev/null | head -n 1" || true)"
  [[ -n "$server_tp" ]] && break
  sleep 0.2
done
[[ "$server_tp" =~ ^[0-9A-F]{40}$ ]] || fail "daemon did not report readiness"

server_ip="$(resolve_ipv4 "$QZ_SERVER_HOST")"
endpoint="$server_ip:$port"

# Send initial file
src_daemon="$WORK/daemon_file.bin"
make_file "$src_daemon" 1048576
initial_hash="$(local_sha "$src_daemon")"

qz direct_initial direct "$endpoint" "$src_daemon" --thumbprint "$server_tp" \
  --identity-dir "$WORK/client-identity" --checksum --no-progress
expect_ok direct_initial
assert_same_file "$src_daemon" "$rdir/daemon_recv/daemon_file.bin"

# Attempt to overwrite with altered content without -c
src_daemon_alt="$WORK/daemon_file.bin"
printf 'MUTATED_NEW_CONTENT' > "$src_daemon_alt"
qz direct_refuse direct "$endpoint" "$src_daemon_alt" --thumbprint "$server_tp" \
  --identity-dir "$WORK/client-identity" --no-progress
expect_fail direct_refuse

# Verify target file is untouched on server
current_hash="$(remote_sha "$rdir/daemon_recv/daemon_file.bin")"
[[ "$current_hash" == "$initial_hash" ]] || fail "Refused file was modified on server"
log "Daemon refuse-on-conflict preserved target file"
metric "daemon_refusal_preserved_file" true

# Attempt re-transfer with -c (resume allowed under refuse policy)
make_file "$src_daemon" 1048576
qz direct_resume direct "$endpoint" "$src_daemon" --thumbprint "$server_tp" \
  --identity-dir "$WORK/client-identity" --no-progress -c
expect_ok direct_resume
metric "daemon_resume_allowed_under_refuse" true

log "R3 conflict policy passed"
