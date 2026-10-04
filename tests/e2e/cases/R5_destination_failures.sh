#!/usr/bin/env bash
# R5 Destination failures.
# Exercises server-side I/O error handling:
# 1. Read-only destination directory.
# 2. Permission denied (chmod 000).
# 3. Disk full simulation via small tmpfs mount (2 MiB disk vs 10 MiB payload).
# In all cases, transfer must fail deterministically with nonzero exit code and
# no incomplete file should be reported as completed.
source "$(dirname -- "${BASH_SOURCE[0]}")/../lib/common.sh"

prepare_dirs
tiny_mount="$REMOTE_DIR/tiny_disk"

cleanup_disk() {
  remote "sudo umount $(q "$tiny_mount") 2>/dev/null || true"
  cleanup_remote_dir
}
trap cleanup_disk EXIT

src_small="$WORK/payload_small.bin"
make_file "$src_small" 1048576

# --- Test 1: Read-only directory ---
ro_dir="$REMOTE_DIR/ro_target"
remote "mkdir -p $(q "$ro_dir") && chmod 555 $(q "$ro_dir")"

log "Testing write to read-only directory..."
send_file "dest_ro" "$src_small" "$ro_dir"
expect_fail "dest_ro"
metric "ro_exit_code" "$QZ_RC"

# Clean up read-only permissions so subsequent cleanups succeed
remote "chmod 755 $(q "$ro_dir")"

# --- Test 2: Permission denied (chmod 000) ---
locked_dir="$REMOTE_DIR/locked_target"
remote "mkdir -p $(q "$locked_dir") && chmod 000 $(q "$locked_dir")"

log "Testing write to chmod 000 directory..."
send_file "dest_locked" "$src_small" "$locked_dir"
expect_fail "dest_locked"
metric "locked_exit_code" "$QZ_RC"

remote "chmod 755 $(q "$locked_dir")"

# --- Test 3: Disk full via small tmpfs ---
remote "mkdir -p $(q "$tiny_mount") && sudo mount -t tmpfs -o size=2M tmpfs $(q "$tiny_mount") && sudo chown qz:qz $(q "$tiny_mount")"

src_large="$WORK/payload_10mb.bin"
make_file "$src_large" 10485760 # 10 MiB, exceeds 2 MiB tmpfs

log "Testing transfer exceeding available filesystem capacity..."
send_file "dest_disk_full" "$src_large" "$tiny_mount"
expect_fail "dest_disk_full"
metric "disk_full_exit_code" "$QZ_RC"

# Verify file on server is not marked complete
if remote "test -f $(q "$tiny_mount/payload_10mb.bin")"; then
  actual_len="$(remote "stat -c%s $(q "$tiny_mount/payload_10mb.bin") 2>/dev/null || stat -f%z $(q "$tiny_mount/payload_10mb.bin")")"
  if [[ "$actual_len" -ge 10485760 ]]; then
    fail "Incomplete transfer wrote full file size despite disk full!"
  fi
fi

remote "sudo umount $(q "$tiny_mount")"
log "R5 destination failures passed"
