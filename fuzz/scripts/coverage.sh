#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
fuzz_root="$repo_root/fuzz"
corpus="$fuzz_root/target/tree-summary-coverage-corpus"

nightly_sysroot="$(rustc +nightly --print sysroot)"
target_triple="$(rustc +nightly -vV | sed -n 's/^host: //p')"
llvm_path="$nightly_sysroot/lib/rustlib/$target_triple/bin"
llvm_cov="$llvm_path/llvm-cov"
coverage_log="$fuzz_root/target/tree-summary-coverage.log"

if [[ ! -x "$llvm_cov" ]]; then
  echo "missing nightly llvm-tools-preview; run:" >&2
  echo "  rustup component add llvm-tools-preview --toolchain nightly" >&2
  exit 2
fi

cargo run --quiet --manifest-path "$fuzz_root/Cargo.toml" \
  --example generate_coverage_corpus -- "$corpus"
if ! cargo +nightly fuzz coverage rust_only "$corpus" \
  --fuzz-dir "$fuzz_root" --llvm-path "$llvm_path" \
  > "$coverage_log" 2>&1
then
  cat "$coverage_log" >&2
  exit 1
fi
tail -n 3 "$coverage_log"

binary="$repo_root/target/$target_triple/coverage/$target_triple/release/rust_only"
profile="$fuzz_root/coverage/rust_only/coverage.profdata"
report="$($llvm_cov report "$binary" -instr-profile="$profile" \
  --show-instantiation-summary=false --show-branch-summary=false \
  --sources "$repo_root/src/engine/condition_tree.rs" \
  --sources "$repo_root/src/engine/exact_cases.rs" \
  --sources "$repo_root/src/engine/mod.rs" \
  --sources "$repo_root/src/engine/syntactic.rs")"
printf '%s\n' "$report"

read -r region_coverage function_coverage line_coverage < <(
  awk '/^TOTAL/ { gsub(/%/, ""); print $4, $7, $10 }' <<< "$report"
)
minimum="${TREE_SUMMARY_MIN_COVERAGE:-90}"

for metric in \
  "regions:$region_coverage" \
  "functions:$function_coverage" \
  "lines:$line_coverage"
do
  name="${metric%%:*}"
  actual="${metric#*:}"
  if ! awk -v actual="$actual" -v minimum="$minimum" 'BEGIN { exit actual >= minimum ? 0 : 1 }'; then
    echo "$name coverage $actual% is below the $minimum% gate" >&2
    exit 1
  fi
done

echo "TreeSummary engine coverage meets the $minimum% region/function/line gate"
