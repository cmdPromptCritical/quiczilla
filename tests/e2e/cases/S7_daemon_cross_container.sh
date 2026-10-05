#!/usr/bin/env bash
# S7 Cross-container persistent daemon.
# Port of scripts/test_daemon.sh across a real network hop: the daemon runs on
# the server, `quic direct` runs on the client. Checks two verified
# transfers, refuse-on-conflict, rejection of an unauthorized client identity,
# rejection of a wrong daemon pin, and that the daemon survives all of it.
source "$(dirname -- "${BASH_SOURCE[0]}")/../lib/common.sh"

prepare_dirs
port="${QZ_DAEMON_PORT:-55449}"
rdir="$REMOTE_DIR"

stop_daemon() {
  remote "if [ -f $(q "$rdir/daemon.pid") ]; then kill \$(cat $(q "$rdir/daemon.pid")) 2>/dev/null || true; fi" || true
  remote "cat $(q "$rdir/daemon.err") 2>/dev/null" > "$CASE_DIR/daemon.err" || true
  cleanup_remote_dir
}
trap stop_daemon EXIT

client_tp="$(client_identity "$WORK/client-identity")"
rogue_tp="$(client_identity "$WORK/rogue-identity")"
[[ "$client_tp" =~ ^[0-9A-F]{40}$ && "$rogue_tp" =~ ^[0-9A-F]{40}$ ]] \
  || fail "could not create client identities"
[[ "$client_tp" != "$rogue_tp" ]] || fail "identities are not distinct"

remote "mkdir -p $(q "$rdir/recv") && printf '# authorized e2e client\n%s\n' $client_tp > $(q "$rdir/allow")"
remote "(cd $(q "$rdir") && nohup $(q "$QZ_REMOTE_WORKER_BIN") --daemon --port $port \
  --allow-thumbprints allow --save-dir recv --identity-dir server-identity \
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

make_file "$WORK/first.bin" 1048576
make_file "$WORK/second.bin" 524288
for name in first second; do
  qz "direct-$name" direct "$endpoint" "$WORK/$name.bin" --thumbprint "$server_tp" \
    --identity-dir "$WORK/client-identity" --checksum --no-progress
  expect_ok "direct-$name"
  assert_same_file "$WORK/$name.bin" "$rdir/recv/$name.bin"
done

before="$(remote_sha "$rdir/recv/first.bin")"
make_file "$WORK/replacement/first.bin" 4096
qz conflict direct "$endpoint" "$WORK/replacement/first.bin" --thumbprint "$server_tp" \
  --identity-dir "$WORK/client-identity" --no-progress
expect_fail conflict
conflict_rc="$QZ_RC"
[[ "$(remote_sha "$rdir/recv/first.bin")" == "$before" ]] || fail "refused transfer modified the existing file"

make_file "$WORK/unauthorized.bin" 4
qz rogue direct "$endpoint" "$WORK/unauthorized.bin" --thumbprint "$server_tp" \
  --identity-dir "$WORK/rogue-identity" --no-progress
expect_fail rogue
rogue_rc="$QZ_RC"
remote_exists "$rdir/recv/unauthorized.bin" && fail "daemon accepted an unauthorized client"

qz wrong-pin direct "$endpoint" "$WORK/unauthorized.bin" \
  --thumbprint 0000000000000000000000000000000000000000 \
  --identity-dir "$WORK/client-identity" --no-progress
expect_fail wrong-pin
wrong_pin_rc="$QZ_RC"
remote_exists "$rdir/recv/unauthorized.bin" && fail "file delivered despite a wrong daemon pin"

remote "kill -0 \$(cat $(q "$rdir/daemon.pid"))" || fail "daemon exited during the test"
# Snapshot of current failure codes; diffs show up when structured exit codes land.
metric rejected_exit_codes "{\"conflict\":$conflict_rc,\"rogue\":$rogue_rc,\"wrong_pin\":$wrong_pin_rc}"
