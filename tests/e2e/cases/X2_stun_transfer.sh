#!/usr/bin/env bash
# X2 End-to-end STUN transfer.
# Forces --transport stun with the configured STUN server and transfers a 16 MiB payload.
# Verifies:
# 1. Receipt reports selected transport as 'stun-quic'.
# 2. Complete bit-for-bit SHA-256 match.
source "$(dirname -- "${BASH_SOURCE[0]}")/../lib/common.sh"

[[ -n "${QZ_E2E_STUN:-}" ]] || skip "QZ_E2E_STUN not configured"
stun="$(resolve_stun)" || fail "cannot resolve STUN server '${QZ_E2E_STUN}' to IPv4"
prepare_dirs
trap cleanup_remote_dir EXIT

src="$WORK/stun_payload.bin"
make_file "$src" 16777216 # 16 MiB

log "Performing STUN-assisted transfer over $QZ_E2E_STUN..."
send_file "x2_stun" "$src" "$REMOTE_DIR" --transport stun --stun-server "$stun" --checksum
expect_ok "x2_stun"

transport="$(expect_transport "x2_stun" 'stun-quic')"
assert_same_file "$src" "$REMOTE_DIR/stun_payload.bin"
metric "x2_stun_transport" "$transport"
metric "x2_duration_seconds" "$QZ_ELAPSED"

log "X2 STUN transfer passed"
