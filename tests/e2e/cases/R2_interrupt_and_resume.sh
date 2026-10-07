#!/usr/bin/env bash
# R2 Interruption and cold resumption.
# Verifies:
# 1. An interrupted transfer for a file >= 20 MiB stages in .quic-part and does
#    not expose an incomplete file as completed.
# 2. Resuming with -c / --resume completes successfully with valid SHA-256.
# 3. If source file changes, resumption safely restarts from byte 0.
source "$(dirname -- "${BASH_SOURCE[0]}")/../lib/common.sh"

prepare_dirs
trap cleanup_remote_dir EXIT

file_size=$((100 * 1024 * 1024)) # 100 MiB (triggers >= 20 MiB staging)
src="$WORK/interrupted.dat"
make_file "$src" "$file_size"
orig_hash="$(local_sha "$src")"

# 1. Start transfer and kill client midway
dest_dir="$REMOTE_DIR/resume_test"
remote "mkdir -p -- $(q "$dest_dir")"

log "Starting background transfer to interrupt..."
"$QZ_BIN" "$src" "$TARGET:$dest_dir/" --no-progress "${QZ_SSH_ARGS[@]}" \
  > "$CASE_DIR/kill.out" 2> "$CASE_DIR/kill.err" &
transfer_pid=$!

# Wait for transfer to establish connection and start streaming bytes
for _ in {1..100}; do
  if grep -q "Transferring" "$CASE_DIR/kill.err" 2>/dev/null; then
    break
  fi
  sleep 0.02
done
# Let it stream a portion of the file, then kill it
sleep 0.05
kill -9 "$transfer_pid" 2>/dev/null || true
wait "$transfer_pid" 2>/dev/null || true

# Assert invariant: final file must NOT exist, or must not be marked complete
dest_file="$dest_dir/interrupted.dat"
part_file="$dest_dir/interrupted.dat.quic-part"

if remote "test -f $(q "$dest_file")"; then
  dest_len="$(remote "stat -c%s $(q "$dest_file") 2>/dev/null || stat -f%z $(q "$dest_file")")"
  if [[ "$dest_len" -ge "$file_size" ]]; then
    fail "Transfer was supposed to be interrupted but completed before kill"
  fi
  fail "Incomplete file was written directly as destination without staging"
fi

remote "test -f $(q "$part_file")" || fail "Expected staging part file $part_file not found"
part_len="$(remote "stat -c%s $(q "$part_file") 2>/dev/null || stat -f%z $(q "$part_file")")"
log "Interrupted successfully with part file size $part_len bytes"
metric "interrupted_bytes" "$part_len"

# 2. Resume the transfer with -c / --resume
log "Resuming transfer with -c..."
send_file "resume" "$src" "$dest_dir" --checksum -c
expect_ok "resume"
assert_same_file "$src" "$dest_file"
remote "test -f $(q "$part_file")" && fail "Staging part file still exists after completed resume"
metric "resumed_sha_matched" true

# 3. Alter source file and test resumption fingerprint check
log "Testing source mutation: resume should detect mismatch and restart from byte 0"
# Corrupt part of file
printf 'MUTATED_CORRUPTED_CONTENT' | dd of="$src" bs=1 seek=1024 conv=notrunc 2>/dev/null
mutated_hash="$(local_sha "$src")"

# Create a mock partial destination with old data
remote "cp $(q "$dest_file") $(q "$part_file") && rm -f $(q "$dest_file")"

send_file "resume_mutated" "$src" "$dest_dir" --checksum -c
expect_ok "resume_mutated"
assert_same_file "$src" "$dest_file"
metric "mutated_restart_ok" true

log "R2 interruption and resumption passed"
