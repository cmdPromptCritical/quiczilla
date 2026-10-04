#!/usr/bin/env bash
# X4 Pipe mode over STUN-assisted QUIC.
# Streams an archive over quic pipe with --transport stun, verifying stream
# alignment and orderly EOF completion over public STUN traversal.
source "$(dirname -- "${BASH_SOURCE[0]}")/../lib/common.sh"

[[ -n "${QZ_E2E_STUN:-}" ]] || skip "QZ_E2E_STUN not configured"
stun="$(resolve_stun)" || fail "cannot resolve STUN server '${QZ_E2E_STUN}' to IPv4"
prepare_dirs
trap cleanup_remote_dir EXIT

src="$WORK/stream_data.bin"
make_file "$src" 16777216 # 16 MiB

log "Streaming pipe payload over STUN..."
timeout --kill-after=10 "$QZ_TRANSFER_TIMEOUT" "$QZ_BIN" pipe "$TARGET" "sha256sum" \
  --transport stun --stun-server "$stun" --no-progress "${QZ_SSH_ARGS[@]}" < "$src" \
  > "$CASE_DIR/pipe_stun.out" 2> "$CASE_DIR/pipe_stun.err"
QZ_RC=$?

expect_ok "pipe_stun"
remote_hash="$(awk '{print $1; exit}' "$CASE_DIR/pipe_stun.out")"
local_hash="$(local_sha "$src")"

[[ "$remote_hash" == "$local_hash" ]] || fail "STUN pipe stream hash mismatch: $remote_hash != $local_hash"
metric "x4_pipe_stun_sha_matched" true

log "X4 pipe mode over STUN passed"
