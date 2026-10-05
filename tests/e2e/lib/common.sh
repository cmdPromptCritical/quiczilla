#!/usr/bin/env bash
# Shared helpers for Quiczilla E2E cases. Each case runs as its own bash
# process launched by runner.sh with CASE_ID and CASE_DIR exported.
#
# Exit-code contract for a case: 0 = pass, 1 = fail, 77 = skip.
# Cases record metrics with `metric key value`; runner.sh merges them into the
# results line. Never record real public addresses or hostnames in metrics.
set -uo pipefail

: "${CASE_ID:?CASE_ID must be set by runner.sh}"
: "${CASE_DIR:?CASE_DIR must be set by runner.sh}"
: "${QZ_SERVER_HOST:=server}"
: "${QZ_SERVER_USER:=qz}"
: "${QZ_SERVER_SSH_PORT:=22}"
: "${QZ_BIN:=/opt/quiczilla/quiczilla}"
: "${QZ_WORKER_BIN:=/opt/quiczilla/quiczilla-worker}"
: "${QZ_REMOTE_WORKER_BIN:=/opt/quiczilla/quiczilla-worker}"
: "${QZ_REMOTE_BASE:=/home/qz/e2e}"
: "${QZ_TRANSFER_TIMEOUT:=300}"

WORK="/tmp/qz-e2e/$CASE_ID"
REMOTE_DIR="$QZ_REMOTE_BASE/$CASE_ID"
TARGET="$QZ_SERVER_USER@$QZ_SERVER_HOST"

# Options shared by every bootstrap transfer. An explicit SSH port is passed so
# the CLI and its ssh child agree even without the client ssh config.
QZ_SSH_ARGS=()
if [[ "$QZ_SERVER_SSH_PORT" != "22" ]]; then
  QZ_SSH_ARGS=(-p "$QZ_SERVER_SSH_PORT")
fi

log() { printf '[%s] %s\n' "$CASE_ID" "$*" >&2; }

fail() {
  log "FAIL: $*"
  printf '%s\n' "$*" >> "$CASE_DIR/failure.txt"
  exit 1
}

skip() {
  log "SKIP: $*"
  printf '%s\n' "$*" > "$CASE_DIR/skip.txt"
  exit 77
}

# metric <key> <json-value-or-string>
metric() {
  local key="$1" value="$2"
  if jq -e . >/dev/null 2>&1 <<<"$value"; then
    jq -cn --arg k "$key" --argjson v "$value" '{k:$k, v:$v}' >> "$CASE_DIR/metrics.jsonl"
  else
    jq -cn --arg k "$key" --arg v "$value" '{k:$k, v:$v}' >> "$CASE_DIR/metrics.jsonl"
  fi
}

# Quote one argument for the remote POSIX/bash shell.
q() { printf '%q' "$1"; }

# remote <shell command string>: run on the server as $QZ_SERVER_USER.
remote() {
  ssh -o BatchMode=yes -p "$QZ_SERVER_SSH_PORT" "$TARGET" "$@"
}

# Fresh local and remote working directories for this case.
prepare_dirs() {
  rm -rf -- "$WORK"
  mkdir -p -- "$WORK"
  remote "rm -rf -- $(q "$REMOTE_DIR") && mkdir -p -- $(q "$REMOTE_DIR")" \
    || fail "cannot prepare remote directory over SSH"
}

cleanup_remote_dir() {
  [[ "${QZ_KEEP_REMOTE:-0}" == "1" ]] || remote "rm -rf -- $(q "$REMOTE_DIR")" || true
}

# make_file <path> <bytes>: random content of an exact size.
make_file() {
  local path="$1" bytes="$2"
  mkdir -p -- "$(dirname -- "$path")"
  if [[ "$bytes" -eq 0 ]]; then
    : > "$path"
  else
    head -c "$bytes" /dev/urandom > "$path"
  fi
}

local_sha() { sha256sum -- "$1" | cut -d' ' -f1; }

remote_sha() {
  remote "sha256sum -- $(q "$1") 2>/dev/null" | cut -d' ' -f1
}

remote_exists() { remote "test -e $(q "$1") || test -L $(q "$1")"; }

# assert_same_file <local-path> <remote-path>
assert_same_file() {
  local local_hash remote_hash
  local_hash="$(local_sha "$1")"
  remote_hash="$(remote_sha "$2")"
  [[ -n "$remote_hash" ]] || fail "remote file missing: $2"
  [[ "$local_hash" == "$remote_hash" ]] \
    || fail "SHA-256 mismatch for $(basename -- "$1"): local=$local_hash remote=$remote_hash"
}

# File and directory manifests, relative to a root. Symlinks are excluded from
# the file list and reported separately.
manifest_files_local() {
  (cd -- "$1" && find . -type f -print0 | LC_ALL=C sort -z | xargs -0r sha256sum)
}
manifest_dirs_local() {
  (cd -- "$1" && find . -type d | LC_ALL=C sort)
}
manifest_files_remote() {
  remote "cd -- $(q "$1") && find . -type f -print0 | LC_ALL=C sort -z | xargs -0r sha256sum"
}
manifest_dirs_remote() {
  remote "cd -- $(q "$1") && find . -type d | LC_ALL=C sort"
}

# assert_same_tree <local-root> <remote-root>
assert_same_tree() {
  manifest_files_local "$1" > "$CASE_DIR/manifest.local"
  manifest_files_remote "$2" > "$CASE_DIR/manifest.remote" || fail "remote tree missing: $2"
  diff -u "$CASE_DIR/manifest.local" "$CASE_DIR/manifest.remote" > "$CASE_DIR/manifest.diff" \
    || fail "file manifest differs (see manifest.diff)"
  manifest_dirs_local "$1" > "$CASE_DIR/dirs.local"
  manifest_dirs_remote "$2" > "$CASE_DIR/dirs.remote"
  diff -u "$CASE_DIR/dirs.local" "$CASE_DIR/dirs.remote" > "$CASE_DIR/dirs.diff" \
    || fail "directory list differs (see dirs.diff)"
}

# qz <label> <args...>: run the CLI with a timeout, capturing stdout/stderr to
# CASE_DIR/<label>.out|.err. Sets QZ_RC. Never aborts the case by itself.
qz() {
  local label="$1"
  shift
  local start end
  start="$(date +%s.%N)"
  timeout --kill-after=10 "$QZ_TRANSFER_TIMEOUT" "$QZ_BIN" "$@" \
    > "$CASE_DIR/$label.out" 2> "$CASE_DIR/$label.err" < /dev/null
  QZ_RC=$?
  end="$(date +%s.%N)"
  QZ_ELAPSED="$(awk -v s="$start" -v e="$end" 'BEGIN{printf "%.3f", e-s}')"
  printf '%s rc=%s elapsed=%ss: %s %s\n' "$label" "$QZ_RC" "$QZ_ELAPSED" "$QZ_BIN" "$*" \
    >> "$CASE_DIR/commands.log"
  return 0
}

# expect_ok <label>: the last qz call must have exited 0 (124 = timeout/hang).
expect_ok() {
  local label="$1"
  if [[ "$QZ_RC" -eq 124 || "$QZ_RC" -eq 137 ]]; then
    fail "$label timed out after ${QZ_TRANSFER_TIMEOUT}s (possible hang); stderr tail: $(tail -n 5 "$CASE_DIR/$label.err" | tr '\n' ' ')"
  fi
  [[ "$QZ_RC" -eq 0 ]] \
    || fail "$label exited $QZ_RC; stderr tail: $(tail -n 5 "$CASE_DIR/$label.err" | tr '\n' ' ')"
}

# expect_fail <label>: the last qz call must have exited nonzero without hanging.
expect_fail() {
  local label="$1"
  [[ "$QZ_RC" -ne 124 && "$QZ_RC" -ne 137 ]] || fail "$label hung instead of failing"
  [[ "$QZ_RC" -ne 0 ]] || fail "$label unexpectedly succeeded"
}

# receipt <label> <jq-filter>: read a field from the last JSON line on stdout.
receipt() {
  local label="$1" filter="$2"
  grep -E '^\{' "$CASE_DIR/$label.out" | tail -n 1 | jq -r "$filter"
}

# expect_transport <label> <allowed-regex>
expect_transport() {
  local label="$1" allowed="$2" got
  got="$(receipt "$label" '.transport // empty')"
  [[ "$(receipt "$label" '.status // empty')" == "completed" ]] \
    || fail "$label: receipt status is not completed: $(tail -n 1 "$CASE_DIR/$label.out")"
  [[ "$got" =~ ^($allowed)$ ]] || fail "$label: transport '$got' does not match '$allowed'"
  printf '%s' "$got"
}

# send_file <label> <local-file> <remote-dir> [extra CLI args...]
send_file() {
  local label="$1" file="$2" dir="$3"
  shift 3
  qz "$label" "$file" "$TARGET:$dir/" --no-progress --json "${QZ_SSH_ARGS[@]}" "$@"
}

# Resolve QZ_E2E_STUN (host:port or IPv4:port) to IPv4:port, because the CLI
# and worker parse --stun-server as a socket address and reject hostnames.
resolve_stun() {
  local spec="${QZ_E2E_STUN:-}"
  [[ -n "$spec" ]] || return 1
  local host="${spec%:*}" port="${spec##*:}"
  [[ "$host" != "$spec" ]] || port=3478
  if [[ "$host" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    printf '%s:%s\n' "$host" "$port"
    return 0
  fi
  local ip
  ip="$(getent ahostsv4 "$host" | awk 'NR==1 {print $1}')"
  [[ -n "$ip" ]] || return 1
  printf '%s:%s\n' "$ip" "$port"
}

# is_global_ip <ip>: true when the address is publicly routable.
is_global_ip() {
  python3 - "$1" <<'PY'
import ipaddress, sys
sys.exit(0 if ipaddress.ip_address(sys.argv[1]).is_global else 1)
PY
}

# client_identity <dir>: create/print a persistent client thumbprint.
client_identity() {
  timeout 15 "$QZ_BIN" identity --identity-dir "$1" 2>/dev/null | tail -n 1 | tr -d '\r'
}

# resolve_ipv4 <host>
resolve_ipv4() {
  getent ahostsv4 "$1" | awk 'NR==1 {print $1}'
}
