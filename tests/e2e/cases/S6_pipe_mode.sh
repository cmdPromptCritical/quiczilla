#!/usr/bin/env bash
# S6 Pipe mode.
# (a) tar a tree into a remote `tar -x`: checks one-way delivery and that the
#     remote command sees EOF (QUIC FIN) and exits, so the CLI returns.
# (b) stream 64 MiB into a remote `sha256sum`: checks the return path to
#     local stdout and orderly completion in both directions.
# Each run is bounded by QZ_TRANSFER_TIMEOUT; a timeout indicates an EOF hang.
source "$(dirname -- "${BASH_SOURCE[0]}")/../lib/common.sh"

prepare_dirs
trap cleanup_remote_dir EXIT

src="$WORK/src"
mkdir -p "$src/sub/empty"
for i in $(seq 1 50); do head -c $((RANDOM * 8)) /dev/urandom > "$src/sub/p$i.bin"; done
make_file "$src/big.dat" 8388608

pipe_run() {
  local label="$1" stdin_file="$2"
  shift 2
  local start end
  start="$(date +%s.%N)"
  timeout --kill-after=10 "$QZ_TRANSFER_TIMEOUT" "$QZ_BIN" pipe "$TARGET" "$@" \
    --no-progress "${QZ_SSH_ARGS[@]}" < "$stdin_file" \
    > "$CASE_DIR/$label.out" 2> "$CASE_DIR/$label.err"
  QZ_RC=$?
  end="$(date +%s.%N)"
  QZ_ELAPSED="$(awk -v s="$start" -v e="$end" 'BEGIN{printf "%.3f", e-s}')"
}

# (a) tar extraction on the server.
tar -C "$src" -cf "$WORK/tree.tar" .
dest="$REMOTE_DIR/extracted"
pipe_run tar "$WORK/tree.tar" "mkdir -p $(q "$dest") && tar -xf - -C $(q "$dest")"
expect_ok tar
assert_same_tree "$src" "$dest"
metric tar_seconds "$QZ_ELAPSED"

# (b) remote digest returned on stdout.
make_file "$WORK/stream.dat" 67108864
pipe_run digest "$WORK/stream.dat" "sha256sum"
expect_ok digest
remote_digest="$(awk '{print $1; exit}' "$CASE_DIR/digest.out")"
[[ "$remote_digest" == "$(local_sha "$WORK/stream.dat")" ]] \
  || fail "remote digest '$remote_digest' does not match the streamed data"
metric digest_seconds "$QZ_ELAPSED"
