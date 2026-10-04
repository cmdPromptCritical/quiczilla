#!/usr/bin/env bash
# S1 Build identity and environment preconditions.
# Records the CLI version and artifact hashes, and checks that the server is
# reachable over SSH, reports Linux x86_64, and has no worker on PATH (so
# bootstrap transfers exercise the managed-bundle upload path).
source "$(dirname -- "${BASH_SOURCE[0]}")/../lib/common.sh"

version="$("$QZ_BIN" --version 2>&1 | head -n 1)"
[[ "$version" =~ ^quiczilla\ [0-9]+\.[0-9]+\.[0-9]+ ]] || fail "unexpected --version output: $version"
metric version "$version"

for artifact in quiczilla quiczilla-worker libmsquic.so; do
  [[ -f "/opt/quiczilla/$artifact" ]] || fail "missing /opt/quiczilla/$artifact"
  metric "sha256_$artifact" "$(local_sha "/opt/quiczilla/$artifact")"
done
metric build_info "$(cat /opt/quiczilla/BUILD_INFO.json)"

platform="$(remote 'uname -sm')" || fail "SSH to $TARGET failed"
[[ "$platform" == "Linux x86_64" ]] || fail "unexpected server platform: $platform"

installed="$(remote 'command -v quiczilla-worker quiczilla quic 2>/dev/null; ls ~/.local/bin 2>/dev/null' || true)"
[[ -z "$installed" ]] || fail "server unexpectedly has an installed worker/CLI: $installed"

# The server image's worker must match the client's sibling worker so daemon
# cases test the same build.
server_worker_sha="$(remote_sha "$QZ_REMOTE_WORKER_BIN")"
[[ "$server_worker_sha" == "$(local_sha "$QZ_WORKER_BIN")" ]] \
  || fail "server and client worker binaries differ"

log "version=$version platform=$platform"
