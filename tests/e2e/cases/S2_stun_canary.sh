#!/usr/bin/env bash
# S2 STUN interop canary.
# Starts a one-shot worker with --stun-server on both client and server and
# checks that each reports a server-reflexive public_udp_addr. This validates
# Quiczilla's STUN client against the configured server (coturn in
# production) with at most one binding exchange per side.
#
# Records only booleans: never the discovered addresses themselves.
# Skips when QZ_E2E_STUN is not set.
source "$(dirname -- "${BASH_SOURCE[0]}")/../lib/common.sh"

[[ -n "${QZ_E2E_STUN:-}" ]] || skip "QZ_E2E_STUN not set"
stun="$(resolve_stun)" || fail "cannot resolve STUN server '${QZ_E2E_STUN}' to IPv4"
prepare_dirs
trap cleanup_remote_dir EXIT

thumb="$(client_identity "$WORK/identity")"
[[ "$thumb" =~ ^[0-9A-F]{40}$ ]] || fail "could not create a client identity"

# Client side: read the readiness line, then stop the waiting worker.
timeout 20 "$QZ_WORKER_BIN" --peer-thumbprint "$thumb" --stun-server "$stun" \
  --save-dir "$WORK" > "$CASE_DIR/client-ready.json" 2> "$CASE_DIR/client-worker.err" &
worker_pid=$!
for _ in $(seq 1 100); do
  [[ -s "$CASE_DIR/client-ready.json" ]] && break
  kill -0 "$worker_pid" 2>/dev/null || break
  sleep 0.1
done
kill "$worker_pid" 2>/dev/null || true
wait "$worker_pid" 2>/dev/null || true
client_addr="$(head -n 1 "$CASE_DIR/client-ready.json" | jq -r '.public_udp_addr // empty' 2>/dev/null)"
[[ -n "$client_addr" ]] \
  || fail "client worker reported no public_udp_addr: $(tail -n 3 "$CASE_DIR/client-worker.err" | tr '\n' ' ')"

# Server side: same check over SSH, bounded by a remote timeout.
remote "timeout 10 $(q "$QZ_REMOTE_WORKER_BIN") --peer-thumbprint $thumb --stun-server $stun --save-dir $(q "$REMOTE_DIR") 2>/dev/null | head -n 1" \
  > "$CASE_DIR/server-ready.json" || true
server_addr="$(jq -r '.public_udp_addr // empty' "$CASE_DIR/server-ready.json" 2>/dev/null)"
[[ -n "$server_addr" ]] || fail "server worker reported no public_udp_addr"

client_ip="${client_addr%:*}"
server_ip="${server_addr%:*}"
if [[ "${QZ_E2E_STUN_ALLOW_PRIVATE:-0}" != "1" ]]; then
  is_global_ip "$client_ip" || fail "client STUN mapping is not a public address"
  is_global_ip "$server_ip" || fail "server STUN mapping is not a public address"
fi

shared="false"
[[ "$client_ip" == "$server_ip" ]] && shared="true"
metric stun_client_ok true
metric stun_server_ok true
# true on a single Docker host: stun-quic between the containers would then
# depend on hairpin NAT and is not evidence of Internet traversal.
metric stun_shared_public_ip "$shared"
# Redact real addresses from the logs kept in results.
sed -i -E 's/"public_udp_addr":"[^"]*"/"public_udp_addr":"<redacted>"/' \
  "$CASE_DIR/client-ready.json" "$CASE_DIR/server-ready.json"
log "STUN discovery OK on both sides (shared public IP: $shared)"
