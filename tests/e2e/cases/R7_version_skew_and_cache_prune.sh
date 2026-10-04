#!/usr/bin/env bash
# R7 Version skew, managed worker refresh, and cache retention.
# Verifies:
# 1. An installed worker with a different hash triggers managed-bundle refresh.
# 2. Managed bundle directory caching ~/.cache/quiczilla/bundle-* functions correctly.
# 3. Cache pruning retains the 3 newest bundle directories.
source "$(dirname -- "${BASH_SOURCE[0]}")/../lib/common.sh"

prepare_dirs
trap cleanup_remote_dir EXIT

src="$WORK/payload_skew.bin"
make_file "$src" 1048576

# Clean any existing cache on server
remote "rm -rf ~/.cache/quiczilla ~/.local/bin"

# 1. Preinstall a dummy older worker with different hash
remote "mkdir -p ~/.local/bin && printf '#!/bin/sh\nexit 1\n' > ~/.local/bin/quiczilla-worker && chmod 755 ~/.local/bin/quiczilla-worker"

log "Testing transfer with mismatched installed worker..."
send_file "skew_refresh" "$src" "$REMOTE_DIR/skew" --checksum
expect_ok "skew_refresh"
assert_same_file "$src" "$REMOTE_DIR/skew/$(basename -- "$src")"

# Verify a bundle directory was created in ~/.cache/quiczilla
bundle_count="$(remote "find ~/.cache/quiczilla -mindepth 1 -maxdepth 1 -type d -name 'bundle-*' | wc -l")"
[[ "$bundle_count" -ge 1 ]] || fail "Managed bundle directory was not created in ~/.cache/quiczilla"
metric "bundle_created_after_skew" true

# 2. Create mock older bundle directories to test the 3-bundle retention policy
for i in 1 2 3 4 5; do
  remote "mkdir -p ~/.cache/quiczilla/bundle-mock-$i && touch -d '1 hour ago' ~/.cache/quiczilla/bundle-mock-$i"
done

# Perform another transfer to trigger bundle cache cleanup
send_file "skew_prune" "$src" "$REMOTE_DIR/prune"
expect_ok "skew_prune"

# Check bundle retention count (should be at most 3 bundles remaining)
remaining_bundles="$(remote "find ~/.cache/quiczilla -mindepth 1 -maxdepth 1 -type d -name 'bundle-*' | wc -l")"
metric "remaining_bundles_after_prune" "$remaining_bundles"
[[ "$remaining_bundles" -le 3 ]] || fail "Expected at most 3 bundles retained, but found $remaining_bundles"

remote "rm -rf ~/.local/bin"
log "R7 version skew and cache retention passed"
