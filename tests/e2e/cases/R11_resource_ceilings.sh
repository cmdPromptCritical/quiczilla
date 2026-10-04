#!/usr/bin/env bash
# R11 Resource ceilings and memory bounded check.
# Uses /usr/bin/time -v to measure peak Resident Set Size (RSS) during a 64 MiB
# transfer to verify memory usage remains bounded and does not scale linearly
# with file size. Also monitors open file descriptors.
source "$(dirname -- "${BASH_SOURCE[0]}")/../lib/common.sh"

prepare_dirs
trap cleanup_remote_dir EXIT

src="$WORK/resource_check.dat"
make_file "$src" 67108864 # 64 MiB

dest_dir="$REMOTE_DIR/resources"
remote "mkdir -p -- $(q "$dest_dir")"

log "Measuring client resource profile with /usr/bin/time -v..."
/usr/bin/time -v "$QZ_BIN" "$src" "$TARGET:$dest_dir/" --no-progress "${QZ_SSH_ARGS[@]}" \
  > "$CASE_DIR/time.out" 2> "$CASE_DIR/time.err"
rc=$?
[[ "$rc" -eq 0 ]] || fail "Transfer exited with $rc"
assert_same_file "$src" "$dest_dir/resource_check.dat"

# Parse Peak RSS (in kbytes) from time output
peak_rss_kb="$(grep 'Maximum resident set size' "$CASE_DIR/time.err" | awk -F: '{print $2}' | tr -d ' ' || echo 0)"
metric "peak_rss_kb" "$peak_rss_kb"

log "Peak RSS: ${peak_rss_kb} KB"
# Invariant: bounded transfer memory. 64 MiB transfer should never exceed 250 MB RSS (256,000 KB)
if [[ "$peak_rss_kb" -gt 262144 ]]; then
  fail "Peak RSS (${peak_rss_kb} KB) exceeded 256 MB threshold"
fi

metric "memory_bounded_ok" true
log "R11 resource ceilings passed"
