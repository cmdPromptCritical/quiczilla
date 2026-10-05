#!/usr/bin/env bash
# In-container case runner. Executes inside the client container (or a
# Kubernetes client Job) and writes results to $QZ_RESULTS (default /results).
#
#   runner.sh <smoke|regression|split|all> [CASE_ID ...]
#
# Each case is a separate bash process: exit 0 = pass, 1 = fail, 77 = skip.
# Results: <run>/results.jsonl (one line per case), <run>/summary.json, and
# <run>/<CASE_ID>/ logs. Exit status is 1 when any case fails.
set -uo pipefail

here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
tier="${1:-smoke}"
shift || true
requested=("$@")

: "${QZ_RESULTS:=/results}"
: "${QZ_CASE_TIMEOUT:=1200}"
: "${QZ_TOPOLOGY:=unknown}"
run_id="${QZ_RUN_ID:-$(date -u +%Y%m%dT%H%M%SZ)}"
run_dir="$QZ_RESULTS/$run_id"
mkdir -p -- "$run_dir"
results="$run_dir/results.jsonl"
: > "$results"

# The suite controls STUN explicitly per case. An empty value (not unset)
# also suppresses a release build's compiled-in default STUN server, so
# production STUN is contacted only by cases that pass --stun-server.
export QUICZILLA_STUN_SERVER=""

case "$tier" in
  smoke) pattern='S*.sh' ;;
  regression) pattern='R*.sh' ;;
  split) pattern='X*.sh' ;;
  all) pattern='[SRX]*.sh' ;;
  *) echo "unknown tier '$tier' (smoke|regression|split|all)" >&2; exit 2 ;;
esac

if [[ ${#requested[@]} -gt 0 ]]; then
  # Explicitly named cases are selected from every tier.
  cases=()
  for r in "${requested[@]}"; do
    while IFS= read -r match; do
      cases+=("$match")
    done < <(cd "$here/cases" && compgen -G "${r}_*.sh")
  done
  mapfile -t cases < <(printf '%s\n' "${cases[@]}" | LC_ALL=C sort -u | sed '/^$/d')
else
  mapfile -t cases < <(cd "$here/cases" && compgen -G "$pattern" | LC_ALL=C sort)
fi
if [[ ${#cases[@]} -eq 0 ]]; then
  echo "no cases selected" >&2
  exit 2
fi

version="$(quic --version 2>/dev/null | head -n 1 || echo unknown)"
build_info="$(cat /opt/quiczilla/BUILD_INFO.json 2>/dev/null || echo '{}')"
echo "Quiczilla E2E: $version, tier=$tier, topology=$QZ_TOPOLOGY, run=$run_id"

pass=0 failn=0 skipn=0
for case_file in "${cases[@]}"; do
  id="${case_file%%_*}"
  id="${id%.sh}"
  case_dir="$run_dir/$id"
  mkdir -p -- "$case_dir"
  start="$(date +%s.%N)"
  CASE_ID="$id" CASE_DIR="$case_dir" \
    timeout --kill-after=15 "$QZ_CASE_TIMEOUT" bash "$here/cases/$case_file" \
    > "$case_dir/case.log" 2>&1
  rc=$?
  end="$(date +%s.%N)"
  duration="$(awk -v s="$start" -v e="$end" 'BEGIN{printf "%.3f", e-s}')"

  case "$rc" in
    0) verdict=pass; pass=$((pass + 1)) ;;
    77) verdict=skip; skipn=$((skipn + 1)) ;;
    124|137) verdict=fail; failn=$((failn + 1))
       echo "case timed out after ${QZ_CASE_TIMEOUT}s" >> "$case_dir/failure.txt" ;;
    *) verdict=fail; failn=$((failn + 1)) ;;
  esac

  metrics='{}'
  if [[ -s "$case_dir/metrics.jsonl" ]]; then
    metrics="$(jq -s 'map({(.k): .v}) | add' "$case_dir/metrics.jsonl")"
  fi
  reason=""
  [[ -s "$case_dir/failure.txt" ]] && reason="$(head -n 3 "$case_dir/failure.txt" | tr '\n' ' ')"
  [[ -s "$case_dir/skip.txt" ]] && reason="$(head -n 1 "$case_dir/skip.txt")"

  jq -cn --arg id "$id" --arg file "$case_file" --arg verdict "$verdict" \
    --argjson rc "$rc" --argjson duration "$duration" --arg reason "$reason" \
    --argjson metrics "$metrics" \
    '{case:$id, file:$file, verdict:$verdict, exit_code:$rc, duration_seconds:$duration, reason:$reason, metrics:$metrics}' \
    >> "$results"
  printf '  %-4s %-5s %8ss  %s\n' "$id" "$verdict" "$duration" "$reason"
done

jq -n --arg run "$run_id" --arg tier "$tier" --arg topology "$QZ_TOPOLOGY" \
  --arg version "$version" --argjson build "$build_info" \
  --arg image "${QZ_IMAGE_ID:-unknown}" --arg kernel "$(uname -r)" \
  --argjson pass "$pass" --argjson fail "$failn" --argjson skip "$skipn" \
  --slurpfile cases "$results" \
  '{run:$run, tier:$tier, topology:$topology, version:$version, build:$build,
    image_id:$image, kernel:$kernel, passed:$pass, failed:$fail, skipped:$skip,
    cases:$cases}' > "$run_dir/summary.json"

echo "Result: $pass passed, $failn failed, $skipn skipped -> $run_dir"
[[ "$failn" -eq 0 ]]
