#!/usr/bin/env bash
# S3 Forced transports.
# Sends the same 1 MiB file with each transport and asserts the receipt's
# selected transport, a completed status, verified integrity, and a matching
# SHA-256 on the server.
source "$(dirname -- "${BASH_SOURCE[0]}")/../lib/common.sh"

prepare_dirs
trap cleanup_remote_dir EXIT
src="$WORK/s3-payload.dat"
make_file "$src" 1048576
server_ip="$(resolve_ipv4 "$QZ_SERVER_HOST")"
[[ -n "$server_ip" ]] || fail "cannot resolve $QZ_SERVER_HOST"

# label | expected transport regex | extra args
run_transport() {
  local label="$1" expected="$2"
  shift 2
  local dir="$REMOTE_DIR/$label"
  remote "mkdir -p -- $(q "$dir")"
  send_file "$label" "$src" "$dir" --checksum "$@"
  expect_ok "$label"
  local got
  got="$(expect_transport "$label" "$expected")" || exit 1
  [[ "$(receipt "$label" '.verified')" == "true" ]] || fail "$label: receipt not verified"
  [[ "$(receipt "$label" '.bytes')" == "1048576" ]] || fail "$label: wrong byte count"
  assert_same_file "$src" "$dir/$(basename -- "$src")"
  metric "${label}_transport" "$got"
  metric "${label}_seconds" "$QZ_ELAPSED"
}

run_transport direct 'direct-quic' --transport direct
run_transport manual 'manual-quic' --transport manual --quic-host "$server_ip" --quic-port 55500
run_transport ssh 'ssh-fallback' --transport ssh
# With STUN suppressed, auto must pick the routable direct candidate.
run_transport auto 'direct-quic' --transport auto
run_transport default 'direct-quic'
