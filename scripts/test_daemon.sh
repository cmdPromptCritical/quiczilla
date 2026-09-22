#!/usr/bin/env bash
set -euo pipefail

cli_path="${1:-target/debug/quiczilla-cli}"
worker_path="${2:-target/debug/quiczilla-worker}"
cli_path="$(realpath "$cli_path")"
worker_path="$(realpath "$worker_path")"
test_root="$(mktemp -d "${RUNNER_TEMP:-/tmp}/quiczilla-daemon-e2e.XXXXXX")"
daemon_pid=""

cleanup() {
  if [[ -n "$daemon_pid" ]] && kill -0 "$daemon_pid" 2>/dev/null; then
    kill "$daemon_pid" 2>/dev/null || true
    wait "$daemon_pid" 2>/dev/null || true
  fi
  rm -rf -- "$test_root"
}
trap cleanup EXIT

client_identity="$test_root/client-identity"
server_identity="$test_root/server-identity"
rogue_identity="$test_root/rogue-identity"
receive_dir="$test_root/received"
mkdir -p "$receive_dir"

if ! client_thumbprint="$("$cli_path" identity --identity-dir "$client_identity")"; then
  echo "Failed to create the Linux client identity" >&2
  exit 1
fi
client_thumbprint="$(printf '%s\n' "$client_thumbprint" | tail -n 1)"
[[ "$client_thumbprint" =~ ^[0-9A-F]{40}$ ]]
allow_file="$test_root/authorized_thumbprints"
printf '# authorized CI client\n%s\n' "$client_thumbprint" > "$allow_file"

daemon_port=55449
stdout_log="$test_root/daemon.stdout.log"
stderr_log="$test_root/daemon.stderr.log"
"$worker_path" --daemon --port "$daemon_port" \
  --allow-thumbprints "$allow_file" --save-dir "$receive_dir" \
  --identity-dir "$server_identity" --on-conflict refuse >"$stdout_log" 2>"$stderr_log" &
daemon_pid=$!

for _ in $(seq 1 200); do
  [[ -s "$stdout_log" ]] && break
  kill -0 "$daemon_pid" 2>/dev/null || {
    cat "$stderr_log" >&2
    exit 1
  }
  sleep 0.1
done

first_destination="$receive_dir/first.bin"
[[ -s "$stdout_log" ]] || {
  cat "$stderr_log" >&2
  exit 1
}
server_thumbprint="$(sed -n 's/.*"thumbprint":"\([0-9A-F]*\)".*/\1/p' "$stdout_log" | head -n 1)"
[[ "$server_thumbprint" =~ ^[0-9A-F]{40}$ ]]

head -c 1048576 /dev/urandom > "$test_root/first.bin"
head -c 524288 /dev/urandom > "$test_root/second.bin"
for source in "$test_root/first.bin" "$test_root/second.bin"; do
  "$cli_path" direct "127.0.0.1:$daemon_port" "$source" \
    --thumbprint "$server_thumbprint" --identity-dir "$client_identity" \
    --checksum --no-progress
  destination="$receive_dir/$(basename "$source")"
  [[ "$(sha256sum "$source" | cut -d' ' -f1)" == "$(sha256sum "$destination" | cut -d' ' -f1)" ]]
done

first_destination="$receive_dir/first.bin"
first_hash_before="$(sha256sum "$first_destination" | cut -d' ' -f1)"
if "$cli_path" direct "127.0.0.1:$daemon_port" "$test_root/first.bin" \
  --thumbprint "$server_thumbprint" --identity-dir "$client_identity" --no-progress; then
  echo "Daemon accepted a transfer over an existing destination" >&2
  exit 1
fi
[[ "$(sha256sum "$first_destination" | cut -d' ' -f1)" == "$first_hash_before" ]]

"$cli_path" identity --identity-dir "$rogue_identity" >/dev/null
printf '\x01\x02\x03\x04' > "$test_root/unauthorized.bin"
if "$cli_path" direct "127.0.0.1:$daemon_port" "$test_root/unauthorized.bin" \
  --thumbprint "$server_thumbprint" --identity-dir "$rogue_identity" --no-progress; then
  echo "Daemon accepted an unauthorized client identity" >&2
  exit 1
fi
[[ ! -e "$receive_dir/unauthorized.bin" ]]

wrong_pin=0000000000000000000000000000000000000000
if "$cli_path" direct "127.0.0.1:$daemon_port" "$test_root/unauthorized.bin" \
  --thumbprint "$wrong_pin" --identity-dir "$client_identity" --no-progress; then
  echo "Client accepted an incorrect daemon thumbprint" >&2
  exit 1
fi
kill -0 "$daemon_pid"

echo "Linux daemon E2E passed: two verified transfers and mutual rejection checks."
