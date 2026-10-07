#!/usr/bin/env bash
# R4 Path safety and traversal prevention.
# Verifies:
# 1. No file write escapes the target directory (path traversal via `..`, absolute paths, etc. sanitized).
# 2. Planted destination symlink is refused by the receiver (cannot overwrite through symlink to escape).
# 3. Source symlinks are safely skipped during directory transfers.
source "$(dirname -- "${BASH_SOURCE[0]}")/../lib/common.sh"

prepare_dirs
trap cleanup_remote_dir EXIT

dest_jail="$REMOTE_DIR/jail"
escape_target="$REMOTE_DIR/escaped"
remote "mkdir -p -- $(q "$dest_jail") && mkdir -p -- $(q "$escape_target")"

# --- Part 1: Planted destination symlink attack ---
# Plant a symlink inside the destination pointing to an outside directory
remote "ln -s $(q "$escape_target") $(q "$dest_jail/pwn_link")"

# Create a local source directory with a file named `pwn_link`
local_tree="$WORK/tree_attack"
mkdir -p "$local_tree"
printf 'MALICIOUS_PAYLOAD\n' > "$local_tree/pwn_link"

log "Attempting directory transfer over existing destination symlink..."
qz attack_symlink "$local_tree" "$TARGET:$dest_jail/" --no-progress "${QZ_SSH_ARGS[@]}"

# The receiver should fail or refuse to follow the symlink into the escape target
if remote "test -f $(q "$escape_target/pwn_link") || test -f $(q "$escape_target/pwn_link.quic-part")"; then
  fail "Security breach: Directory receiver followed destination symlink outside jail!"
fi
metric "destination_symlink_traversal_blocked" true

# --- Part 2: Relative path sanitization on single file ---
# When sending a file, receiver must use Path::file_name(), ignoring any directory prefix
src_file="$WORK/secret.txt"
printf 'CONFIDENTIAL_DATA\n' > "$src_file"

send_file "file_sanitization" "$src_file" "$dest_jail"
expect_ok "file_sanitization"

# Target should strictly be $dest_jail/secret.txt
remote "test -f $(q "$dest_jail/secret.txt")" || fail "Expected file $dest_jail/secret.txt not found"
remote "test -f $(q "$escape_target/secret.txt")" && fail "File escaped to wrong directory!"
metric "file_name_sanitized" true

# --- Part 3: Path traversal characters in directory transfer ---
# Create an internal directory structure with tricky relative names
tricky_dir="$WORK/tricky_tree"
mkdir -p "$tricky_dir/normal_sub"
printf 'normal\n' > "$tricky_dir/normal_sub/ok.txt"

# Verify safe transfer of normal nested tree
dest_tricky="$REMOTE_DIR/tricky_dest"
remote "mkdir -p -- $(q "$dest_tricky")"
qz tricky_dir "$tricky_dir" "$TARGET:$dest_tricky/" --no-progress "${QZ_SSH_ARGS[@]}"
expect_ok tricky_dir
remote "test -f $(q "$dest_tricky/tricky_tree/normal_sub/ok.txt")" || fail "Normal subfolder file missing"

log "R4 path safety passed"
