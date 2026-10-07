#!/usr/bin/env bash
# R6 noexec worker cache.
# Verifies zero-trust / noexec compliance:
# 1. When ~/.cache/quiczilla is mounted noexec and a compatible worker is installed,
#    bootstrap detects execution failure and falls back to the installed worker.
# 2. When ~/.cache/quiczilla is mounted noexec and NO installed worker exists,
#    bootstrap fails gracefully with an informative error rather than hanging.
source "$(dirname -- "${BASH_SOURCE[0]}")/../lib/common.sh"

prepare_dirs
cache_dir="/home/qz/.cache/quiczilla"

cleanup_noexec() {
  remote "sudo umount $(q "$cache_dir") 2>/dev/null || true"
  remote "rm -rf ~/.local/bin" || true
  cleanup_remote_dir
}
trap cleanup_noexec EXIT

src="$WORK/test_noexec.bin"
make_file "$src" 1048576

# Ensure ~/.cache/quiczilla exists and mount it as noexec
remote "mkdir -p $(q "$cache_dir") && sudo mount -t tmpfs -o noexec tmpfs $(q "$cache_dir") && sudo chown qz:qz $(q "$cache_dir")"

# --- Part 1: Noexec cache WITH preinstalled worker fallback ---
log "Testing noexec cache with pre-installed worker installed in ~/.local/bin..."
remote "mkdir -p ~/.local/bin && cp /opt/quiczilla/quiczilla-worker ~/.local/bin/quiczilla-worker && cp /opt/quiczilla/libmsquic.so* ~/.local/bin/ && chmod 755 ~/.local/bin/quiczilla-worker"

send_file "noexec_with_fallback" "$src" "$REMOTE_DIR/part1" --checksum
expect_ok "noexec_with_fallback"
assert_same_file "$src" "$REMOTE_DIR/part1/$(basename -- "$src")"
metric "noexec_installed_fallback_ok" true

# --- Part 2: Noexec cache WITHOUT installed worker ---
log "Testing noexec cache without any pre-installed worker..."
remote "rm -rf ~/.local/bin"

send_file "noexec_no_fallback" "$src" "$REMOTE_DIR/part2"
expect_fail "noexec_no_fallback"
metric "noexec_without_fallback_exit_code" "$QZ_RC"

remote "sudo umount $(q "$cache_dir")"
log "R6 noexec cache tests passed"
