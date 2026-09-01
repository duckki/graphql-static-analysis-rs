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
        └── observation.rs  Rust execution and canonical observations
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

Build a native oracle from the Lean checkout whose behavior is under test:

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

## Coverage-guided campaigns

Run the differential target with a persistent oracle:

```sh
GRAPHQL_STATIC_ANALYSIS_LEAN_ORACLE=fuzz/target/tree-summary-lean-oracle \
  cargo +nightly fuzz run differential \
  fuzz/target/differential-campaign \
  fuzz/corpus/differential
```

Run the wider Rust-only target separately:

```sh
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

The current Rust engine was rechecked on 2026-09-01 against Lean commit
`a0d1ba3c7cb3306b0bb21775272716c5e9d809e5`
(`Improve tree-summary analysis using representativeField`):

- all 15,840 deterministic ExactCase/Syntactic and max/cases/trace/cost profiles agree;
- minimized input `--input-hex 6162` verifies that a complete request missing `$x`
  selects the modeled Boolean `false` behavior;
- minimized input `--input-hex 010200000200` verifies inherited exact-case Boolean
  context in the recursive trace;
- field analyses use one representative occurrence's validated field name and
  equivalent arguments while retaining per-runtime-parent schema lookup;
- the ExactCase implementation uses the model's incremental branch-local cursor,
  binary Boolean decisions, structural joins, and completed-boundary compaction.

The unfiltered deterministic runner and retained differential corpus are expected to
remain green. Do not weaken or remove minimized cases to hide a future disagreement;
update this status only when the Rust behavior or pinned Lean model intentionally
changes.

## Repository hygiene

Keep small, named, semantically distinct seeds under `fuzz/corpus/`. Oracle binaries,
generated corpora, campaign discoveries, coverage data, crash artifacts, and build
outputs belong under the ignored `fuzz/target/`, `fuzz/coverage/`, and
`fuzz/artifacts/` directories. Temporary investigation notes belong under
`.scratch/`.

When the shared decoder, oracle protocol, observations, analysis modes, or expected
baseline changes, update this document in the same reviewable slice.
