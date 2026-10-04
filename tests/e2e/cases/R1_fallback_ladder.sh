#!/usr/bin/env bash
# R1 Fallback ladder.
# Exercises transport degradation and recovery using iptables:
# (a) When STUN is blocked, `auto` mode degrades gracefully to `direct-quic`.
# (b) When all UDP to the server is blocked, `auto` mode falls back to `ssh-fallback`.
# (c) When `--transport stun` is forced but STUN is blocked, transfer fails deterministically without hanging.
source "$(dirname -- "${BASH_SOURCE[0]}")/../lib/common.sh"
source "$(dirname -- "${BASH_SOURCE[0]}")/../lib/net.sh"

prepare_dirs
trap 'net_reset_all; cleanup_remote_dir' EXIT

server_ip="$(resolve_ipv4 "$QZ_SERVER_HOST")"
[[ -n "$server_ip" ]] || fail "cannot resolve $QZ_SERVER_HOST"

src="$WORK/fallback-payload.dat"
make_file "$src" 1048576

# Use local dummy or configured STUN IP for the drop rule
stun_spec="$(resolve_stun || echo "192.0.2.1:3478")"
stun_ip="${stun_spec%:*}"
stun_port="${stun_spec##*:}"

# --- Test (a): Block STUN UDP port ---
log "Testing (a): STUN blocked, auto should select direct-quic"
net_clear_iptables
net_block_udp_port "$stun_ip" "$stun_port"

send_file "step_a_stun_blocked" "$src" "$REMOTE_DIR/step_a" --transport auto --stun-server "$stun_spec" --checksum
expect_ok "step_a_stun_blocked"
transport_a="$(expect_transport "step_a_stun_blocked" 'direct-quic')"
assert_same_file "$src" "$REMOTE_DIR/step_a/$(basename -- "$src")"
metric "step_a_transport" "$transport_a"

# --- Test (b): Block all UDP to server IP ---
log "Testing (b): All UDP blocked to server, auto should fall back to ssh-fallback"
net_clear_iptables
net_block_udp_to_ip "$server_ip"

send_file "step_b_all_udp_blocked" "$src" "$REMOTE_DIR/step_b" --transport auto --checksum
expect_ok "step_b_all_udp_blocked"
transport_b="$(expect_transport "step_b_all_udp_blocked" 'ssh-fallback')"
assert_same_file "$src" "$REMOTE_DIR/step_b/$(basename -- "$src")"
metric "step_b_transport" "$transport_b"

# --- Test (c): Forced STUN transport when STUN is unreachable ---
log "Testing (c): Forced --transport stun with STUN blocked should fail deterministically"
net_clear_iptables
net_block_udp_port "$stun_ip" "$stun_port"

send_file "step_c_forced_stun_blocked" "$src" "$REMOTE_DIR/step_c" --transport stun --stun-server "$stun_spec"
expect_fail "step_c_forced_stun_blocked"
metric "step_c_exit_code" "$QZ_RC"

net_reset_all
log "R1 fallback ladder passed"
