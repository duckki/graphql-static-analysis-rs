# TreeSummary differential fuzzing

This document is the canonical guide to the TreeSummary fuzzing system. The
standalone Cargo package is in [`../fuzz/`](../fuzz/). It checks the Rust engine
against the executable Lean model and independently drives Rust-only behavior that
is outside the shared model.

The differential result is evidence of behavioral equivalence over the encoded input
space. It is not a proof that every GraphQL operation is equivalent: confidence comes
from combining the Lean oracle, deterministic case matrices, retained decision-path
seeds, coverage-guided mutation, source coverage, and an end-to-end mutation sentinel.

## Architecture

Both implementations consume the same bounded structural selection-tree IR. Decoding
is deterministic and total for every byte string, and inputs are capped at 64 bytes.
The first seven bytes select the legacy family, variable case, analysis mode, list
size, observation, operation default, and structural lane. Remaining bytes recursively
select fields, aliases, nested output fields, inline fragments, overlapping type
conditions, and stacked directives.

The oracle protocol is `TS2`. It sends the raw structural bytes to one persistent
native process built from the Lean model, avoiding interpreter startup for each test
case. The Rust side builds the equivalent validated schema and operation from those
same bytes.

A separate `TS2S` request uses the same decoder with an order-sensitive schedule
observation. It supplements the four stable fuzz observations without changing their
byte encoding or matrix sizes.

| Target | Lean oracle | Purpose | Retained corpus |
| --- | --- | --- | --- |
| `differential` | Required | Compare Rust and Lean observations | `fuzz/corpus/differential/` |
| `rust_only` | No | Exercise the wider Rust surface and panic paths | `fuzz/corpus/rust_only/` |

Four independent observations reduce the chance that two incorrect implementations
accidentally agree:

- `max` compares the maximum response size.
- `cases` compares the canonical multiset of exact-case sizes.
- `trace` compares a canonical recursive trace of response groups, possible types,
  inherited Boolean conditions, retained fields, and children.
- `cost` compares integral IBM type and field costs under the shared schema's default
  cost model and generated list bound.

The Rust-only target evaluates both analysis modes and all four observations for every
input. It additionally exercises named-fragment traversal, repeated-fragment
visitation, and ignored custom directives.

The package layout keeps entry points separate from shared code and generated data:

```text
fuzz/
├── corpus/          named decision-path seeds for each target
├── fuzz_targets/    libFuzzer entry points only
├── lean/            executable Lean reference-model adapter
├── runners/         deterministic Cargo example targets
├── scripts/         oracle build, coverage, and mutation checks
└── src/             code shared by targets and runners
    └── tree_summary/
        ├── input.rs        bounded byte decoder and case matrices
        ├── operation.rs    schema, operation grammar, and variables
        ├── observation.rs  Rust execution and canonical observations
        └── schedule.rs     order-sensitive ExactCase definition audit
```

The deterministic runners are explicitly registered as Cargo examples, so
`cargo fuzz list` contains only the two libFuzzer targets.

## Prerequisites

Run commands from the repository root. Differential fuzzing requires:

- a checkout of the GraphQL Lean project containing the TreeSummary model;
- Lean and Lake usable in that checkout;
- Rust nightly and `cargo-fuzz`;
- nightly's `llvm-tools-preview` component for source coverage.

Install the coverage component once for the active nightly toolchain:

```sh
rustup component add llvm-tools-preview --toolchain nightly
```

## Build the Lean oracle

Build a native oracle from a clean Lean checkout at the revision pinned by
[`LEAN_MODEL_COMMIT`](../fuzz/src/tree_summary/input.rs). The current revision is
recorded in [Current alignment status](#current-alignment-status) below:

```sh
fuzz/scripts/build-oracle.sh /path/to/graphql-lean
```

The default output is `fuzz/target/tree-summary-lean-oracle`. The script records the
exact Lean revision beside it in
`fuzz/target/tree-summary-lean-oracle.model-commit`. The differential runner rejects a
missing or stale revision sidecar. Set
`GRAPHQL_STATIC_ANALYSIS_ALLOW_STALE_LEAN_ORACLE=1` only when intentionally comparing
against an oracle built from another revision.

## Deterministic differential checks

Run the exhaustive profile before starting a fuzzing campaign:

```sh
cargo run --manifest-path fuzz/Cargo.toml --example differential -- \
  --lean-oracle fuzz/target/tree-summary-lean-oracle --exhaustive
```

Use `--mode exact` or `--mode syntactic` to isolate one backend. Use
`--observation max|cases|trace|cost` or `--variable-case 0..9` to isolate one dimension.
The runner also accepts deterministic generated batches through `--seed N --cases N`.

Every disagreement reports a replayable hexadecimal input. Replay it with:

```sh
cargo run --manifest-path fuzz/Cargo.toml --example differential -- \
  --lean-oracle fuzz/target/tree-summary-lean-oracle \
  --input-hex 000100000000
```

Preserve the reported hex input, mode, observation, rendered schema and operation,
variables, and both results when diagnosing a mismatch. Minimize a genuine regression
and add a descriptively named seed to the appropriate retained corpus.

### ExactCase schedule alignment

The canonical trace sorts and flattens alternatives, so it cannot detect every change
in where `join` occurs relative to a parent `field` transfer. Run the complementary
audit after changing ExactCase scheduling:

```sh
cargo run --manifest-path fuzz/Cargo.toml --example schedule_alignment -- \
  fuzz/target/tree-summary-lean-oracle
```

This checks 2,979 ExactCase inputs: the exhaustive matrix with duplicate observation
variants removed, plus ExactCase inputs from deterministic seeds 1 through 2,000.
The schedule algebra normalizes `combine` identity and associativity using list
append, but preserves binary `join` nesting, field order, recursive child terms, and
group annotations. Set-like annotations are sorted. This is an executable-definition
diagnostic, not a new soundness algebra: it intentionally also detects some ordering
changes that lawful commutative `combine` implementations cannot observe. Named
fragments remain covered by the Rust-only lane.

Replay the premature child-join regression directly with:

```sh
cargo run --manifest-path fuzz/Cargo.toml --example schedule_alignment -- \
  fuzz/target/tree-summary-lean-oracle --input-hex 070000000000
```

The same input already belongs to the deterministic matrix. A Rust unit regression,
`symbolic_child_alternatives_keep_parent_field_transfers_separate`, independently
checks the parent-transfer boundary without requiring Lean.

## Coverage-guided campaigns

Run the differential target with a persistent oracle:

```sh
mkdir -p fuzz/target/differential-campaign
GRAPHQL_STATIC_ANALYSIS_LEAN_ORACLE=fuzz/target/tree-summary-lean-oracle \
  cargo +nightly fuzz run differential \
  fuzz/target/differential-campaign \
  fuzz/corpus/differential
```

Run the wider Rust-only target separately:

```sh
mkdir -p fuzz/target/rust-only-campaign
cargo +nightly fuzz run rust_only \
  fuzz/target/rust-only-campaign \
  fuzz/corpus/rust_only
```

libFuzzer writes discoveries to its first corpus directory. The commands above keep
the small named corpus reviewable by writing discoveries beneath ignored
`fuzz/target/` paths.

## Coverage and harness integrity

Generate the deterministic 4,748-input path corpus, replay it with LLVM source
coverage, and enforce a 90% region, function, and line floor over `src/engine`:

```sh
fuzz/scripts/coverage.sh
```

The script discovers engine Rust sources recursively, including the split ExactCase,
possible-type, variable, and field-group modules. Test-only `tests.rs` is excluded;
moving an implementation into a new module does not remove it from the coverage gate.

Rust currently emits no branch counters in this setup, so LLVM regions are the closest
available control-flow metric. Inspect uncovered regions rather than treating the
percentage as proof of correctness. Defensive impossible-schema paths and
compiler-generated generic instantiations can remain uncovered even when semantic
branches are exercised.

Verify the oracle transport, comparison, panic path, and libFuzzer process with a
deliberately corrupted Rust observation:

```sh
fuzz/scripts/mutation-sentinel.sh
```

The sentinel first proves its selected Syntactic case agrees, then requires that same
case to fail when the sentinel mutation is enabled. This establishes that a mismatch
can travel through the complete harness; it does not mutation-score the engine.

For ordinary package checks, run:

```sh
cargo test --manifest-path fuzz/Cargo.toml
cargo clippy --manifest-path fuzz/Cargo.toml --all-targets -- -D warnings
cargo fmt --manifest-path fuzz/Cargo.toml --check
```

## Current alignment status

The current Rust engine was rechecked on 2026-09-14 against merged Lean `main` commit
`41f1c4a240c30419c4ba0ffcdfeae612ee4d5810`
(`Optimize exact cases (#8)`):

- all 15,840 deterministic ExactCase/Syntactic and max/cases/trace/cost profiles agree;
- another 5,000 generated profiles agree (seed `20260914`);
- all 2,979 order-sensitive ExactCase schedules agree after retaining symbolic child
  joins until parent transfers have run; the previous Rust implementation compacted
  two child leaves prematurely, which the four canonical observations did not detect;
- minimized input `--input-hex 6162` verifies that a complete request missing `$x`
  selects the modeled Boolean `false` behavior;
- minimized input `--input-hex 010200000200` verifies inherited exact-case Boolean
  context in the recursive trace;
- field analyses use one representative occurrence's validated field name and
  equivalent arguments; child output types and IBM cost retain per-runtime-parent
  lookup, while response size uses the validated field's invariant list depth.
  Lean now uses its first possible runtime definition; its
  `fieldListMultiplier_eq_forDefinition` theorem equates that lookup with Rust's
  definition-based calculation when runtime definitions are covariant with it;
- without supplied variables, ExactCase retains the model's incremental branch-local
  cursor, binary Boolean decisions, structural joins, and completed-boundary compaction;
- with supplied variables (including an empty map), ExactCase uses the batched
  `CaseForest` scheduler: it partitions the entire active type frontier, resolves
  Boolean-only frontiers directly, preserves field occurrence order, and folds
  completed region summaries without a Boolean decision tree or persistent cursor;
- regression tests check the forest's right-associated region joins and nested field
  order, as well as isolation of inherited Boolean assignments between type regions.

The supplied-variable scheduler intentionally follows the new model's join schedule.
An arbitrary algebra can produce different join/field terms from the earlier cursor
even when numeric max/cost observations are unchanged. The symbolic and
supplied-variable evaluators have separate soundness contracts in Lean; do not assert
term equality between them as a generic optimization invariant.

Validation of the initial forest port, before the additional symbolic scheduling fix,
also replayed the 4,748-input coverage corpus (91.33% engine
regions, 94.02% functions, 93.82% lines), ran separate 30-second campaigns with 18,463
differential and 1,181 Rust-only executions, and passed the end-to-end mutation
sentinel. Both retained seed directories remain unchanged. These bounded campaigns
are additional evidence, not a claim of exhaustive coverage of the structural IR.
The subsequent scheduling fix passed the full Rust test suite, both packages' Clippy
checks, both deterministic oracle checks above, and a fresh 4,748-input coverage
replay (91.60% engine regions, 95.05% functions, 93.72% lines). The
[alignment and profiling report](exact-case-alignment-and-profiling.md) records the
definition correspondence, the mismatch, and the final release profile.

The unfiltered deterministic runner and retained differential corpus are expected to
remain green. Do not weaken or remove minimized cases to hide a future disagreement;
update this status only when the Rust behavior or pinned Lean model intentionally
changes.

The subsequent [organizational refactor](engine-architecture.md) preserves both
observations and evaluator schedules. Its shared child-output helper retains the
existing runtime-parent/field-occurrence lookup and output deduplication order;
representative-only lookup remains a separate performance proposal.
Both deterministic comparisons remain green after the split (15,840 standard cases
and 2,979 schedules). A fresh 4,748-input coverage replay includes all eleven engine
implementation files and passes the unchanged gate: 91.86% regions, 96.69% functions,
and 93.88% lines. These totals reflect the new module/helper layout.

The subsequent [response-size list-shape optimization](response-size-list-shape-performance.md)
removes runtime-parent multiplier scans without changing either evaluator. The
production estimator still agrees on all 15,840 standard profiles and all 2,979
schedules. A regression additionally covers covariant output types and nullability,
nested lists, merged child selections, zero bounds, and saturation in all four
backend/variable configurations. Lean's maximum fold starts at 1; the optimized
calculation preserves that floor even for a zero list bound. The oracle revision
and both retained corpora remain unchanged; the coverage totals above describe the
organizational refactor, not a new coverage campaign.

The final [2026-09-14 release-readiness audit](release-readiness-audit.md) rebuilt the
native oracle from clean merged Lean revision `41f1c4a240c30419c4ba0ffcdfeae612ee4d5810`
in `~/work/apollo-graphql/graphql-lean` and updated the Rust revision guard. The
exhaustive, generated, and schedule comparisons above pass with the stale-oracle
override unset. The previous PR revision sidecar is rejected, and both minimized
mismatch inputs still pass. Separate 30-second campaigns completed
26,476 differential and 1,918 Rust-only executions without failures. The mutation
sentinel detected its intentional mismatch, and a fresh 4,748-input coverage replay
passed with 91.86% regions, 96.69% functions, and 93.88% lines. These bounded checks
exercise the new executable Lean multiplier; they are not a proof of Rust equivalence.

## Repository hygiene

Keep small, named, semantically distinct seeds under `fuzz/corpus/`. Oracle binaries,
generated corpora, campaign discoveries, coverage data, crash artifacts, and build
outputs belong under the ignored `fuzz/target/`, `fuzz/coverage/`, and
`fuzz/artifacts/` directories. Temporary investigation notes belong under
`.scratch/`.

When the shared decoder, oracle protocol, observations, analysis modes, or expected
baseline changes, update this document in the same reviewable slice.
