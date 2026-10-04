#!/usr/bin/env bash
# X3 Auto mode with direct candidate blocked.
# Simulates WAN / NAT traversal where private direct IP is unreachable:
# Blocks direct UDP path to server; auto mode must race STUN candidate and
# successfully establish transfer via 'stun-quic' without falling back to SSH.
source "$(dirname -- "${BASH_SOURCE[0]}")/../lib/common.sh"
source "$(dirname -- "${BASH_SOURCE[0]}")/../lib/net.sh"

[[ -n "${QZ_E2E_STUN:-}" ]] || skip "QZ_E2E_STUN not configured"
stun="$(resolve_stun)" || fail "cannot resolve STUN server '${QZ_E2E_STUN}' to IPv4"
prepare_dirs
trap 'net_reset_all; cleanup_remote_dir' EXIT

server_ip="$(resolve_ipv4 "$QZ_SERVER_HOST")"
src="$WORK/auto_stun.bin"
make_file "$src" 8388608 # 8 MiB

# If direct private IP is known, block direct UDP to server while leaving STUN reachable
log "Testing auto mode with direct candidate blocked..."
net_block_udp_to_ip "$server_ip"

send_file "x3_auto_stun" "$src" "$REMOTE_DIR" --transport auto --stun-server "$stun" --checksum
expect_ok "x3_auto_stun"

transport="$(expect_transport "x3_auto_stun" 'stun-quic')"
assert_same_file "$src" "$REMOTE_DIR/auto_stun.bin"
metric "x3_selected_transport" "$transport"

net_reset_all
log "X3 auto mode with direct candidate blocked passed"
