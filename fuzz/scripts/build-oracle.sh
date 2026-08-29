#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 || $# -gt 2 ]]; then
  echo "usage: $0 LEAN_PROJECT [OUTPUT]" >&2
  exit 2
fi

lean_project="$(cd "$1" && pwd)"
repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
source_file="$repo_root/fuzz/lean/TreeSummaryOracle.lean"
output="${2:-$repo_root/fuzz/target/tree-summary-lean-oracle}"
if [[ "$output" != /* ]]; then
  output="$(pwd)/$output"
fi
output_dir="$(dirname "$output")"
c_file="$output_dir/TreeSummaryOracle.c"
object_file="$output_dir/TreeSummaryOracle.o"
response_file="$output_dir/tree-summary-oracle.rsp"
benchmark_response="$lean_project/.lake/build/bin/exact-cases-bench.rsp"
syntactic_object="$lean_project/.lake/build/ir/GraphQL/Theories/TreeSummary/Syntactic.c.o.export"
field_collection_object="$lean_project/.lake/build/ir/GraphQL/Theories/ConditionTree/FieldCollection.c.o.export"
exact_optimality_object="$lean_project/.lake/build/ir/GraphQL/Theories/TreeSummary/ExactCasesOptimality.c.o.export"
static_cost_object="$lean_project/.lake/build/ir/GraphQL/Theories/TreeSummary/StaticCost.c.o.export"

mkdir -p "$output_dir"

if [[ -z "${CPLUS_INCLUDE_PATH:-}" ]] && command -v xcrun >/dev/null 2>&1; then
  sdk_root="$(xcrun --sdk macosx --show-sdk-path)"
  export CPLUS_INCLUDE_PATH="$sdk_root/usr/include/c++/v1"
fi

(
  cd "$lean_project"
  lean_prefix="$(lake env lean --print-prefix)"
  lake build GraphQL exact-cases-bench \
    GraphQL.Theories.TreeSummary.Syntactic:c.o \
    GraphQL.Theories.TreeSummary.ExactCasesOptimality:c.o \
    GraphQL.Theories.TreeSummary.StaticCost:c.o \
    GraphQL.Theories.ConditionTree.FieldCollection:c.o
  lake env lean --stdin -c "$c_file" < "$source_file"
  "$lean_prefix/bin/leanc" -I "$lean_prefix/include" -c "$c_file" -o "$object_file"
)

if [[ ! -f "$benchmark_response" ]]; then
  echo "missing Lake linker response: $benchmark_response" >&2
  exit 1
fi
if [[ ! -f "$syntactic_object" ]]; then
  echo "missing Syntactic native object: $syntactic_object" >&2
  exit 1
fi
if [[ ! -f "$field_collection_object" ]]; then
  echo "missing FieldCollection native object: $field_collection_object" >&2
  exit 1
fi
if [[ ! -f "$static_cost_object" ]]; then
  echo "missing StaticCost native object: $static_cost_object" >&2
  exit 1
fi
if [[ ! -f "$exact_optimality_object" ]]; then
  echo "missing ExactCasesOptimality native object: $exact_optimality_object" >&2
  exit 1
fi

sed '/Benchmarks\/ExactCases\.c\.o\.export/d' "$benchmark_response" > "$response_file"
printf '"%s"\n' "$syntactic_object" >> "$response_file"
printf '"%s"\n' "$field_collection_object" >> "$response_file"
printf '"%s"\n' "$exact_optimality_object" >> "$response_file"
printf '"%s"\n' "$static_cost_object" >> "$response_file"

(
  cd "$lean_project"
  lean_prefix="$(lake env lean --print-prefix)"
  "$lean_prefix/bin/leanc" -o "$output" "$object_file" "@$response_file"
)

git -C "$lean_project" rev-parse HEAD > "$output.model-commit"
echo "built $output from Lean commit $(cat "$output.model-commit")"
