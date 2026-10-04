#!/usr/bin/env bash
# S4 File-size edges.
# Covers empty and 1-byte files, the 64 KiB and 2 MiB buffer boundaries, the
# 20 MiB resume-staging threshold, and one large file (QZ_E2E_LARGE_BYTES,
# default 256 MiB).
source "$(dirname -- "${BASH_SOURCE[0]}")/../lib/common.sh"

prepare_dirs
trap cleanup_remote_dir EXIT

MIB=1048576
sizes=(0 1 65536 $((2 * MIB - 1)) $((2 * MIB)) $((2 * MIB + 1))
       $((20 * MIB - 1)) $((20 * MIB)) $((20 * MIB + 1)) "${QZ_E2E_LARGE_BYTES:-268435456}")

for size in "${sizes[@]}"; do
  src="$WORK/size-$size.dat"
  make_file "$src" "$size"
  label="size-$size"
  send_file "$label" "$src" "$REMOTE_DIR" --checksum
  expect_ok "$label"
  expect_transport "$label" 'direct-quic|stun-quic|manual-quic' >/dev/null || exit 1
  assert_same_file "$src" "$REMOTE_DIR/size-$size.dat"
  # No staging artifact may remain after a completed transfer.
  if remote "ls -A $(q "$REMOTE_DIR") | grep -q 'quic-part'"; then
    fail "$label left a .quic-part staging file"
  fi
  rm -f -- "$src"
  metric "seconds_$size" "$QZ_ELAPSED"
done
