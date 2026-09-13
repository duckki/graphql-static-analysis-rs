# ExactCase definition alignment and profiling (2026-09-13)

Rust already had a supplied-variable specialization before the new Lean forest
definition. Rust commit `27e8b55` (`Optimize ExactCase evaluation`) was authored at
2026-08-29 15:57:09 -07:00; Lean commit
`4102b52fef79782145ffef7401c393319edc4f16` (`Optimize ExactCases summary`) was authored
at 18:42:10 that day and committed on 2026-09-12. The timestamps support the user's
suggestion that Lean formalized existing Rust optimization work. They do not establish
that the implementations have identical evaluation schedules or prove a speedup.

The forest port aligns the supplied-variable schedule with that Lean revision. This
subsequent audit also found and fixed premature symbolic child-join compaction. The
existing numeric observations agreed even before that fix; they were insufficient to
check the executable definitions' placement of parent field transfers.

## Definition correspondence

All Lean names below refer to the pinned revision, in
`GraphQL/Theories/TreeSummary/{Core,ExactCases}.lean`. Rust source is in
[`src/engine/exact_cases/`](../src/engine/exact_cases/) unless stated otherwise.
The later [organization guide](engine-architecture.md) maps the split modules and
shared helpers while this report retains the original measurements.

| Lean definition | Rust implementation | Required correspondence |
| --- | --- | --- |
| `ExactCases.summarizeOperation` / `summarizeOperationWithVariables` | `Engine::summarize_operation` | Absent values select the symbolic cursor; any supplied map selects the complete forest, after applying defaults. |
| `CaseCursor.BooleanEnvironment` | `BooleanEnvironment` and `VariableEnvironment` | Symbolic assignments are shared across child scopes but do not become request values for extraction-time pruning. |
| `CaseCursor.selectBranch`, `namedFields` | `CaseCursor`, persistent field chunks and pending branches | Selected child branches precede remaining siblings; cross-node response names are collected before field callbacks. |
| `Internal.BooleanDecision.map`, `restrict`, `zipWith` | `BooleanDecision` | Correlate shared variables, retain structural type alternatives, and map recursive field transfers over their leaves. |
| `BooleanDecision.joinMap`, `CaseCursor.summarizeDecisionWithPruning` | `join_decisions`, `summarize_decision` | Construct structural `Join` nodes, including when both children are leaves. |
| `Internal.summarizeConditionTreeDecision` | Final `compact` in `summarize_operation` | Compact only after all recursive field transfers have run, then collapse. |
| `CaseForest.branches`, `resolveActiveTrees`, `namedFields` | `CaseForest::branches`, `resolve_branches`, `complete_field_groups` | Resolve the whole active frontier; retain cleared parent fields before selected children and later siblings. |
| `CaseForest.typeRegions`, `summarizeTypeRegions` | `possible_type_regions`, `summarize_complete_forest` | Refine by every active type condition, included region first; join results right-associatively. |
| `CaseForest.booleanValue`, `extendBooleanCondition` | Complete Boolean assignments and case-condition tracking | Known values resolve directly; modeled missing/null/non-Boolean values select false. Record inherited assignments and isolate sibling regions. |
| `childParentTypes`, `childInheritedBooleanCondition` | Complete/symbolic child traversal and `CollectedFieldGroup` | Deduplicate runtime output types in order, merge all child selections, and preserve inherited Boolean context. |
| `combineMap`, `joinMap` | Field-group folds and alternative folds | Preserve alternative nesting; singleton alternatives do not add an empty case. |

Rust deliberately uses indexed type sets, shared immutable storage, iterative loops,
and an arena of condition-tree nodes. Checking one representative's membership is
valid only after refinement has made each region uniform for every active condition.
Discarding a cleared forest node with no fields preserves the observable forest.

Two representation differences remain intentional. Rust elides a terminal
`combine(summary, empty)` under the documented identity law. It also skips child
extraction for scalar fields with no selections. The alignment audit normalizes
combine identity/associativity and canonicalizes set-like group annotations; it
does not claim raw free-term equality for every imaginable algebra. The shared model
covers validated/coerced operations in the bounded IR. Named fragments and ignored
custom directives additionally run through the Rust-only lane.

The child-output loops currently query every retained field occurrence for every
runtime parent. Lean uses one representative occurrence per runtime parent. Valid
field merging makes these equivalent for a collected type case, but the extra
lookups are an optimization opportunity; all child selection sets must still be merged.

## Alignment fix and checks

The previous symbolic engine called `join_cases` while constructing type alternatives
and joining child output types. That helper immediately joined two leaf summaries,
although their parent field transfer had not yet run. Lean constructs a structural
join at those two sites and compacts at the completed operation boundary.

For a parent field `node` with alternative children `a` and `b`, Rust previously
produced `node{(a{}|b{})}`. It now produces `(node{a{}}|node{b{}})`, matching Lean's
symbolic definition. The supplied-variable forest intentionally retains the former
shape. Numeric max/cost transfers can hide this difference; arbitrary custom field
transfers need not distribute over `join`.

The fix changes both premature compaction sites to structural joins and documents
the valid use of the compaction helper. A Rust regression checks the example above
and a covariant-output example exercising the separate child-type join boundary.
The new [`schedule_alignment`](../fuzz/runners/schedule_alignment.rs) runner uses a
separate `TS2S` oracle observation that preserves field/alternative nesting. It found
the mismatch at input `070000000000` in the existing deterministic matrix.

Validation of the corrected implementation:

- 74 library tests and one public-API integration test pass on Rust 1.95 and 1.90.
- The fuzz package's two tests and both packages' strict Clippy checks pass.
- All 15,840 standard Lean/Rust profiles agree at the pinned revision.
- All 2,979 ExactCase schedule comparisons agree; direct regression replay agrees.
- Formatting checks pass; the benchmark package builds in locked release mode.
- The 4,748-input coverage replay passes the 90% engine gate: 91.60% regions,
  95.05% functions, and 93.72% lines.

These checks establish agreement over their tested inputs, not a proof of the Rust
implementation. The [fuzzing guide](fuzzing.md) records the protocols, reproduction
commands, retained corpus, coverage gate, and distinction between both lanes.

## Corrected-build performance measurements

The corrected release binary was built with `cargo build --release --locked --offline`
from `benchmarks/`, using Rust 1.95.0, LLVM 22.1.2, on an Apple M3 MacBook Air with
8 cores (4 performance, 4 efficiency), 16 GB RAM, macOS 26.6.2 (25G83), mains power.
Its SHA-256 is
`cc44c872c8c7a399d3bcee020ff5b4503e0698e24dc80986f72673a1bf51c7d7`.
The source was dirty at Rust revision
`ca85a645cf4e08c91f76a7bddd83d65990c0bef4`: forest port plus symbolic alignment fix.
No benchmark corpus, result assertion, dependency, or timed-boundary change was made.

### Fresh endpoint comparison

Three fresh processes per variant ran serially in alternating before/after order,
after CPU profiling and before additional checks. The baseline is the original
`ca85a64` engine, with the benchmark lockfile's stale local version corrected. Both
variants retain all four configurations and the canonical calibration, warm-ups, five
samples, and result assertions. Values below are medians of three process medians,
in microseconds per reusable `estimate` call. All 72 measured configurations passed.
This comparison measures the combined forest port and alignment fix.

| Types / spreads | Exact absent, before → aligned | Exact supplied, before → aligned | Syntactic absent, before → aligned | Syntactic supplied, before → aligned |
| --- | ---: | ---: | ---: | ---: |
| 1,024 / 8 | 69.58 → 71.16 | 19.74 → 18.12 | 39.39 → 38.03 | 19.41 → 19.24 |
| 10,240 / 8 | 645.18 → 709.13 | 117.05 → 113.09 | 256.25 → 256.80 | 128.92 → 130.20 |
| 1,024 / 80 | 678.30 → 681.65 | 162.76 → 161.28 | 359.33 → 358.31 | 177.86 → 177.14 |

Supplied-variable medians improve 8.2% and 3.4% at the schema endpoints, while the
large-query change is only 0.9%. The corrected symbolic large-schema median is 9.9%
slower. Its process ranges overlap substantially: 638.51–721.00 µs before and
707.07–722.77 µs after. This is a follow-up signal, not an isolated estimate of the
fix's cost. The source change restores the model's schedule; it must not be reversed
solely to recover an unverified timing difference.

The earlier full-axis [forest comparison](performance-benchmark.md#supplied-variable-caseforest-comparison-2026-09-13)
records scaling exponents for the pre-audit snapshot. Endpoint-only measurements here
do not establish new scaling exponents. In particular, the earlier large apparent
symbolic speedup did not recur in this final comparison.

### CPU samples

Each workload was sampled twice in a fresh process with macOS `sample`: five seconds,
one-millisecond requested interval, using the compiled benchmark's existing `profile`
or `profile-pathological-booleans` loop. Only stacks inside
`MaxResponseSizeEstimator::estimate` enter the denominator, excluding parsing,
validation, variables, and estimator construction. Captures retained 3,484–3,826
estimate samples each. Percentages are inclusive stack membership and overlap;
columns must not be added. Inlining and linker-deduplicated symbols limit attribution.

| ExactCase workload | Condition-tree extraction | Type intersection | Region partition | Schema field lookup | Allocator |
| --- | ---: | ---: | ---: | ---: | ---: |
| 10,240 types / 8 spreads, supplied | 29–30% | 23–25% | 29% | 25–26% | 7% |
| 1,024 types / 80 spreads, supplied | 51% | 21% | 18% | 15% | 26% |
| Boolean stress K=6, supplied | 74–75% | <1% | <1% | 1% | 51–52% |
| 10,240 types / 8 spreads, absent | 9–17% | 7–14% | 13–21% | 21–32% | 4–7% |
| Boolean stress K=6, absent | 15% | <1% | <1% | 3–4% | 50–51% |

Symbolic schema captures are unusually variable: Apollo `Name` clone/drop represents
46.3% in one and 14.6% in the other; Boolean-decision frames represent 24.5–37.0%.
`Name` clone/drop manipulates `Arc` ownership in the installed Apollo source. These
samples identify avoidable ownership traffic but do not isolate the cause of the
between-process variation. Symbolic Boolean stress has 15.7–16.1% decision-frame
membership and roughly half its samples in the allocator.

### Allocation counts

The existing separate release `cost_allocations` binary measures supplied-variable
IBM cost, not response-size timings. ExactCase allocates 216 times / 45,044 requested
bytes at 1,024 types / 8 spreads; 228 / 264,532 bytes at 10,240 / 8; and
1,842 / 314,292 bytes at 1,024 / 80. All are transient (`net_bytes = 0`). These counts
support investigating per-estimate rebuilding; they are not resident-memory totals.

Raw artifacts remain ignored under `.scratch/exact-case-audit/`: `aligned.patch` and
`aligned-manifest.json`, the saved `final` binary, `final-profiles/` with sampling
scripts, argument/hash manifest, stack captures, `cpu-summary.csv`, and allocation
CSV, plus `final-endpoints/` with the alternating-run script, six CSVs, timestamps,
and `comparison.json`. The original baseline and full-axis results remain in
`.scratch/exact-case-forest/`.

## Recommended performance work

1. **Reduce extraction rebuilding and allocation.** Start with reusable vector/map
   storage in `condition_tree::Extractor` and avoid repeatedly cloning directive paths
   and temporary conditions. The large query and supplied Boolean stress put 51% and
   74–75% of samples in extraction. A larger follow-up is a prepared-operation layer
   caching immutable normalized structure. Any cache key must include the relevant
   schema, merged selection scope, inherited conditions, and request-pruning context;
   speculative symbolic assignments must remain separate from supplied values.

2. **Reduce repeated schema field lookup.** Factor out the `childParentTypes` helper
   using the representative field once per runtime parent, while retaining every
   occurrence's child selections. Then evaluate pre-indexed output-type/list-wrapper
   metadata for `MaxResponseSizeAlgebra::field_list_multiplier`, where schema lookup
   still happens per field callback. Lookups account for 25–26% of supplied large-schema
   samples. Preserve runtime-specific output types and covariance; caching only by
   field name would be incorrect.

3. **Improve type-set intersections before redesigning the scheduler.**
   `PossibleTypeSet::intersection` builds a vector even when the result is unchanged;
   partitions scan membership to count, then scan again to split. Compare a subset
   fast path and word-wise bitset operations against the current sparse ordered scan.
   Keep stable object/region order and account for sparse versus dense schemas.
   Intersection and partition are distinct substantial costs in the supplied
   large-schema profile; their inclusive shares are not a speedup forecast.

4. **Reduce symbolic ownership and decision allocation.** Delay materializing
   `Vec<Name>` until field callbacks where possible; use indexed/borrowed scope data
   internally. `zip_with_owned` currently consumes only the leaf/leaf case and falls
   back to borrowed recursion plus cloning for larger decisions. Investigate consuming
   restriction/zip operations or an arena with shared immutable decision nodes.
   Keep structural joins until parent transfers finish—the alignment regression must
   remain green. This targets both measured `Name` traffic and Boolean-stress allocation.

5. **Treat frontier bookkeeping as a secondary optimization.** Reusing frontier
   metadata and resolution vectors may help, but the profiles point first to extraction,
   schema lookup, type sets, and symbolic ownership. Avoid another scheduler rewrite
   without evidence that it dominates a representative workload.

Evaluate each item separately against the four canonical configurations, the schedule
audit, and retained edge cases. Use three alternating processes per variant; include
the symbolic large-schema case to investigate its variation. No numerical speedup is
claimed for these unimplemented suggestions.

## Recommended organization

These recommendations are implemented in the organizational follow-up; see the
[current module map](engine-architecture.md) and
[maintained measurement workflow](../benchmarks/README.md#captured-beforeafter-comparisons).

- Split `exact_cases.rs` into internal `cursor`, `forest`, and `decision` modules,
  with a small entry-point module. This matches the Lean namespaces and makes the
  different compaction boundaries explicit. Perform this as a mechanical change.
- Move indexed sets, regions, and schema indexes from `engine/mod.rs` into an internal
  `possible_types` module; keep the public API facade separate from these algorithms.
- Share child-output/context helpers between the evaluators, with comments naming
  corresponding Lean definitions. Keep evaluator schedules separate: their difference
  in parent-transfer placement is intentional.
- Retain both canonical semantic observations and the new schedule audit. A sorted
  trace is useful for semantic comparison but cannot guard every scheduling change.
- Promote the small benchmark comparison/profiling orchestration into maintained
  tooling if this work continues. Record source and binary hashes automatically and
  keep raw artifacts ignored, so an earlier candidate cannot be confused with the
  final reviewed implementation.
