#!/usr/bin/env bash
# Host-side driver for the Quiczilla container E2E suite (topology T1).
# Run on a Docker host from a checkout of the repository.
#
#   tests/e2e/run.sh [smoke|regression|split|all] [options]
#
# Options:
#   --case ID          Run only this case (repeatable, e.g. --case S3)
#   --source local     Build the working tree (default)
#   --source release --tag vX.Y.Z
#                      Install a published archive (checksum-verified)
#   --stun HOST:PORT   STUN server for STUN cases (also QZ_E2E_STUN). Only the
#                      smoke canary uses it unless --local-stun is given.
#   --local-stun       Start the coturn sidecar and use it instead
#   --no-build         Reuse the existing quiczilla-e2e image
#   --keep             Leave containers running for debugging
#
# Results are written to tests/e2e/results/<run-id>/ (gitignored).
set -euo pipefail

here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd -- "$here/../.." && pwd)"
tier=smoke
cases=()
build=1
keep=0
local_stun=0
export QZ_SOURCE="${QZ_SOURCE:-local}"
export QZ_RELEASE_TAG="${QZ_RELEASE_TAG:-}"
export QZ_E2E_STUN="${QZ_E2E_STUN:-}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    smoke|regression|split|all) tier="$1" ;;
    --case) cases+=("$2"); shift ;;
    --source) QZ_SOURCE="$2"; shift ;;
    --tag) QZ_RELEASE_TAG="$2"; shift ;;
    --stun) QZ_E2E_STUN="$2"; shift ;;
    --local-stun) local_stun=1 ;;
    --no-build) build=0 ;;
    --keep) keep=1 ;;
    -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
  shift
done

if [[ "$QZ_SOURCE" == "release" && -z "$QZ_RELEASE_TAG" ]]; then
  echo "--source release requires --tag" >&2
  exit 2
fi

export QZ_GIT_COMMIT
QZ_GIT_COMMIT="$(git -C "$repo" rev-parse --short=12 HEAD 2>/dev/null || echo unknown)"
if [[ -n "$(git -C "$repo" status --porcelain 2>/dev/null)" ]]; then
  QZ_GIT_COMMIT="${QZ_GIT_COMMIT}-dirty"
fi
export QZ_IMAGE_TAG="${QZ_IMAGE_TAG:-${QZ_SOURCE}}"
run_id="$(date -u +%Y%m%dT%H%M%SZ)-${tier}"
export QZ_RESULTS_DIR="$here/results"
mkdir -p "$QZ_RESULTS_DIR"

compose=(docker compose -f "$here/compose.yaml")
profiles=()
if [[ "$local_stun" -eq 1 ]]; then
  profiles=(--profile local-stun)
  export QZ_E2E_STUN="coturn:3478"
  export QZ_E2E_STUN_ALLOW_PRIVATE=1
fi

cleanup() {
  if [[ "$keep" -eq 0 ]]; then
    "${compose[@]}" "${profiles[@]}" down -v --remove-orphans >/dev/null 2>&1 || true
  else
    echo "Containers kept. Shell: docker compose -f $here/compose.yaml exec client bash"
  fi
}
trap cleanup EXIT

# Start from a clean slate: fresh keys volume and containers.
"${compose[@]}" "${profiles[@]}" down -v --remove-orphans >/dev/null 2>&1 || true

if [[ "$build" -eq 1 ]]; then
  if [[ -n "${QUICZILLA_GITHUB_TOKEN:-}" ]]; then
    # Passed as a BuildKit secret; never stored in an image layer.
    DOCKER_BUILDKIT=1 docker build -f "$here/docker/Dockerfile" \
      --secret id=gh_token,env=QUICZILLA_GITHUB_TOKEN \
      --build-arg QZ_SOURCE --build-arg QZ_RELEASE_TAG --build-arg QZ_GIT_COMMIT \
      -t "quiczilla-e2e:${QZ_IMAGE_TAG}" "$repo"
  else
    "${compose[@]}" build server
  fi
fi
export QZ_IMAGE_ID
QZ_IMAGE_ID="$(docker image inspect --format '{{.Id}}' "quiczilla-e2e:${QZ_IMAGE_TAG}")"

"${compose[@]}" "${profiles[@]}" up -d --no-build --wait

set +e
"${compose[@]}" exec -T -e QZ_RUN_ID="$run_id" client /e2e/runner.sh "$tier" "${cases[@]}"
status=$?
set -e

echo "Results: $QZ_RESULTS_DIR/$run_id/summary.json"
exit "$status"
