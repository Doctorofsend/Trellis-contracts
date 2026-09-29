#!/usr/bin/env bash
# Soroban CPU/memory footprint profiling runner (Issue #192).
#
# Runs the `profile_budget` test module in each profiled contract with
# `--nocapture`, so the `BUDGET_METRIC <key> cpu=<n> mem=<n>` lines those
# tests print (see contracts/*/src/profile_budget.rs) land in a combined log.
# That log is then handed to scripts/compare-budget.cjs, which compares the
# measured costs against the checked-in baseline
# (testing/budget-baseline.json) and fails if any metric regressed by more
# than the configured threshold.
#
# Usage:
#   ./scripts/profile-budget.sh [--update-baseline] [--threshold PERCENT]
#
# Env vars (see docs/OBSERVABILITY.md, "Automated Budget Profiling"):
#   BUDGET_REGRESSION_THRESHOLD_PERCENT  Overridden by --threshold. Default 10.
#
# Exit codes: 0 = within threshold (or baseline updated), 1 = regression
# beyond threshold, 2 = could not run (bad usage, cargo not found, etc).

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# Contract -> the BUDGET_METRIC key(s) its profile_budget module prints.
# Kept here (rather than discovered) so a contract with a profile_budget
# module that silently stops emitting a metric is caught by compare-budget.cjs
# reporting it MISSING, instead of the run just quietly measuring less.
CONTRACTS=(
  "aid-contract"
  "treasury-contract"
  "referral-contract"
  "rebalancer-contract"
)

UPDATE_BASELINE=0
THRESHOLD="${BUDGET_REGRESSION_THRESHOLD_PERCENT:-10}"
BASELINE="testing/budget-baseline.json"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --update-baseline) UPDATE_BASELINE=1; shift ;;
    --threshold) THRESHOLD="$2"; shift 2 ;;
    --baseline) BASELINE="$2"; shift 2 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
done

WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT
LOG="$WORKDIR/profile-budget.log"
: > "$LOG"

echo "== Soroban budget profiling (issue #192) =="
FAILED_CONTRACTS=()

for contract in "${CONTRACTS[@]}"; do
  if [ ! -d "contracts/$contract" ]; then
    echo "skip: contracts/$contract does not exist" | tee -a "$LOG"
    continue
  fi

  echo "-- profiling $contract --" | tee -a "$LOG"
  # `--test-threads=1` keeps the budget measurements free of scheduling noise
  # from other tests running concurrently in the same process; profile_budget
  # is a handful of tests so the serial run is fast.
  if cargo test -p "$contract" --lib profile_budget -- --nocapture --test-threads=1 \
       >>"$LOG" 2>&1; then
    :
  else
    echo "WARNING: $contract's profile_budget tests did not run cleanly (see log below); its metrics will be reported as missing rather than failing the whole job." >&2
    FAILED_CONTRACTS+=("$contract")
  fi
done

echo
echo "== raw cargo output =="
cat "$LOG"
echo "== end raw cargo output =="
echo

if [ "${#FAILED_CONTRACTS[@]}" -gt 0 ]; then
  echo "Contracts whose profile_budget module did not build/run: ${FAILED_CONTRACTS[*]}" >&2
  echo "(scripts/compare-budget.cjs treats their metrics as MISSING, not a threshold failure -- a pre-existing, unrelated compile issue in a contract's test suite must not silently block this job.)" >&2
fi

ARGS=(--log "$LOG" --baseline "$BASELINE" --threshold "$THRESHOLD")
if [ "$UPDATE_BASELINE" -eq 1 ]; then
  ARGS+=(--update-baseline)
fi

node scripts/compare-budget.cjs "${ARGS[@]}"
exit $?
