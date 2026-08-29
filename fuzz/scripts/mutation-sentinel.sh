#!/usr/bin/env bash
set -euo pipefail

if [[ $# -gt 1 ]]; then
  echo "usage: $0 [LEAN_ORACLE]" >&2
  exit 2
fi

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
fuzz_root="$repo_root/fuzz"
oracle="${1:-$fuzz_root/target/tree-summary-lean-oracle}"
sentinel="$fuzz_root/corpus/differential/mutation_sentinel_syntactic"
baseline_log="$fuzz_root/target/tree-summary-mutation-baseline.log"
mutant_log="$fuzz_root/target/tree-summary-mutation-sentinel.log"

GRAPHQL_STATIC_ANALYSIS_LEAN_ORACLE="$oracle" \
  cargo +nightly fuzz run differential "$sentinel" \
  --fuzz-dir "$fuzz_root" -- -runs=1 \
  > "$baseline_log" 2>&1

set +e
GRAPHQL_STATIC_ANALYSIS_LEAN_ORACLE="$oracle" \
GRAPHQL_STATIC_ANALYSIS_FUZZ_SENTINEL_MUTANT=1 \
  cargo +nightly fuzz run differential "$sentinel" \
  --fuzz-dir "$fuzz_root" -- -runs=1 \
  > "$mutant_log" 2>&1
mutant_status=$?
set -e

if [[ $mutant_status -eq 0 ]]; then
  echo "differential harness failed to detect the sentinel mutation" >&2
  exit 1
fi

sed -n '/assertion `left == right` failed:/,/right:/p' "$mutant_log"
echo "differential harness detected the end-to-end sentinel mutation"
