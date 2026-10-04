#!/usr/bin/env bash
# R9 Network impairment.
# Exercises QUIC transport resilience under adverse network conditions via tc netem:
# - Latency injection (50ms delay)
# - Packet loss (1% packet loss + 10ms delay)
# - Packet reordering (1% reordering + 10ms delay)
# Records throughput, elapsed duration, and verifies bit-for-bit SHA-256 integrity.
source "$(dirname -- "${BASH_SOURCE[0]}")/../lib/common.sh"
source "$(dirname -- "${BASH_SOURCE[0]}")/../lib/net.sh"

prepare_dirs
trap 'net_reset_all; cleanup_remote_dir' EXIT

file_bytes=16777216 # 16 MiB
src="$WORK/impairment_test.bin"
make_file "$src" "$file_bytes"

run_scenario() {
  local name="$1"
  shift
  log "Testing scenario: $name (impairment args: $*)"
  net_impair "$@"
  
  local dest_sub="$REMOTE_DIR/$name"
  remote "mkdir -p -- $(q "$dest_sub")"
  
  send_file "impair_$name" "$src" "$dest_sub" --checksum
  expect_ok "impair_$name"
  assert_same_file "$src" "$dest_sub/impairment_test.bin"
  
  local mbps
  mbps="$(receipt "impair_$name" '.average_bytes_per_second // 0' | awk '{printf "%.2f", $1 / 1048576}')"
  metric "${name}_duration_sec" "$QZ_ELAPSED"
  metric "${name}_throughput_mibs" "$mbps"
  log "Scenario $name completed in ${QZ_ELAPSED}s (${mbps} MiB/s)"
}

# 1. Baseline without impairment
run_scenario "baseline"

# 2. 50ms RTT latency
run_scenario "delay_50ms" --delay 25

# 3. 1% Packet Loss + 10ms delay
run_scenario "loss_1pct" --loss 1 --delay 5

# 4. 1% Packet Reorder + 10ms delay
run_scenario "reorder_1pct" --reorder 1 --delay 5

net_reset_all
log "R9 network impairment tests passed"
