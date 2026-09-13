# Engine organization

The engine keeps public configuration separate from runtime-type indexing, request
values, field-group helpers, and the two ExactCase evaluation schedules. These are
internal modules; the crate's public API is unchanged.

```text
src/engine/
├── mod.rs                  public API and backend dispatch
├── field_group.rs          common child outputs, selections, and Boolean context
├── possible_types.rs       indexed sets, regions, partitions, and schema indexes
├── variables.rs            immutable supplied values and operation defaults
├── condition_tree.rs       selection extraction and condition canonicalization
├── syntactic.rs            Syntactic traversal
├── tests.rs                existing engine behavior and API regressions
└── exact_cases/
    ├── mod.rs              entry point and completed-operation compaction
    ├── cursor.rs           incremental symbolic traversal
    ├── forest.rs           batched supplied-variable traversal
    ├── decision.rs         correlated Boolean decisions and structural joins
    └── context.rs          persistent symbolic case assignments
```

## Correspondence with Lean

The reference is Lean revision `4102b52fef79782145ffef7401c393319edc4f16`, in
`GraphQL/Theories/TreeSummary/Core.lean` and `ExactCases.lean`.

| Rust module | Lean definitions / responsibility |
| --- | --- |
| [`exact_cases/mod.rs`](../src/engine/exact_cases/mod.rs) | `summarizeOperation`, `summarizeOperationWithVariables`, and completed-boundary `compact`/`collapse` |
| [`exact_cases/cursor.rs`](../src/engine/exact_cases/cursor.rs) | `CaseCursor`, its Boolean environment, and `summarizeDecisionWithPruning` |
| [`exact_cases/forest.rs`](../src/engine/exact_cases/forest.rs) | `CaseForest`, complete frontier resolution, and direct field/region folds |
| [`exact_cases/decision.rs`](../src/engine/exact_cases/decision.rs) | `Internal.BooleanDecision.map`, `restrict`, `zipWith`, `compact`, and `collapse` |
| [`exact_cases/context.rs`](../src/engine/exact_cases/context.rs) | Persistent assignments used by `Internal.extendBooleanCondition` |
| [`possible_types.rs`](../src/engine/possible_types.rs) | `possibleTypeRegions`, included-before-excluded refinement, and indexed schema runtime types |
| [`field_group.rs`](../src/engine/field_group.rs) | `childParentTypes`, `mergedSelectionSet`, `childInheritedBooleanCondition`, and canonical context extension |
| [`variables.rs`](../src/engine/variables.rs) | Immutable request values after operation defaults, used independently of speculative symbolic assignments |

The cursor maps parent field transfers over each structural child alternative before
the operation entry point compacts decisions. The forest joins completed child
summaries before applying the parent transfer. Sharing helpers must preserve that
difference and the right-associated order of alternative joins.

`CollectedFieldGroup` shares child-selection detection, merged selection slices,
ordered child-output discovery, and inherited Boolean normalization across the
evaluators. Output discovery retains the existing loop over every runtime parent and
field occurrence, followed by ordered deduplication. It is a mechanical extraction;
representative-only lookup and caching belong to a separate performance change.

## Checks and measurement tools

Run the ordinary Rust test/Clippy/format checks, then both the standard differential
profile and the order-sensitive schedule audit from the [fuzzing guide](fuzzing.md).
The original four canonical observations, `TS2S` scheduling observation, decoder,
pinned oracle revision, and both retained corpora are preserved. The coverage script
discovers implementation modules recursively so the split remains inside its gate.

The [benchmark tools](../benchmarks/README.md#captured-beforeafter-comparisons) provide
release snapshot capture, alternating comparisons, and macOS estimate-stack sampling.
They record source and binary hashes and keep artifacts under ignored `.scratch/`
directories or outside the repository. Existing randomized IBM-cost campaigns and
scaling tools remain available for their separately defined study workloads.

The organizational change passes the 75 existing Rust tests on Rust 1.95 and 1.90,
both packages' Clippy checks, documentation/format checks, both Lean comparisons,
and the recursive coverage gate. Six measurement-tool tests pass. End-to-end tooling
checks captured a release snapshot, ran six alternating endpoint processes with
72 result checks, and parsed native samples from supplied and symbolic workloads.
The comparison smoke test deliberately used the same binary on both sides; it is
workflow validation, not a new performance result. Local validation artifacts are
under `.scratch/organization/`.
