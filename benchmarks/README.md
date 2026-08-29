# Performance benchmarks

This standalone Cargo package measures reusable
`MaxResponseSizeEstimator::estimate` calls. Schema and operation construction,
validation, request-variable construction, estimator construction, and result checking
are outside the timed region.

It compares four configurations: ExactCase and Syntactic, each with absent request
variables and with `{ "includeBranch": true, "skipBranch": true }`. The generated
corpus has 80 overlapping subset interfaces; each object implements four consecutive
subsets. Even query fragments use `@include`, odd fragments use `@skip`, and fields
share response names. Every result is asserted before timings are reported.

Three independent axes are available:

- `schema-size`: 1,024 through 10,240 objects, with 8 spreads.
- `query-size`: 8 through 80 spreads, with 1,024 objects.
- `pathological-booleans`: two disjoint object-type regions with `K = 1..6`
  independent Boolean variables per region. Without request values, an eager join of
  the disjoint Boolean supports constructs their cross-product; ExactCase structural
  joins keep the two decision trees factored. Supplied `true` values provide a
  no-branching control. Every configuration returns `K + 1` response fields.

Each configuration warms up twice, calibrates a power-of-two iteration count to a
100 ms sample, records five samples, and emits the median total time and integer
nanoseconds per estimate as CSV. The estimator is reused for all timed calls.

## Run

Run these commands from this directory. Build once, then invoke the binary directly
so Cargo startup and compilation are not included in the surrounding observation.

```sh
cargo build --release --locked
target/release/graphql-static-analysis-benchmark endpoints
target/release/graphql-static-analysis-benchmark schema-size > schema.csv
target/release/graphql-static-analysis-benchmark query-size > query.csv
target/release/graphql-static-analysis-benchmark pathological-booleans \
  > pathological-booleans.csv
python3 scaling.py schema.csv query.csv
```

On the initial build, use `cargo build --release` if a lockfile has not been created
yet; retain that lockfile with the result archive and use `--locked` afterward.

For native sampling profilers, use a fixed iteration count:

```text
target/release/graphql-static-analysis-benchmark \
  profile OBJECTS SPREADS BACKEND VARIABLES ITERATIONS
```

Useful endpoints are `(10240, 8)` and `(1024, 80)`. Profile inclusive samples below
`MaxResponseSizeEstimator::estimate`; schema parsing and estimator construction run
only once before the profiling loop.

The disjoint-Boolean stress case has a matching fixed-iteration profile command:

```text
target/release/graphql-static-analysis-benchmark \
  profile-pathological-booleans K BACKEND VARIABLES ITERATIONS
```

For comparable runs, keep the Rust toolchain, revision, and release build flags the
same; use mains power and avoid competing sustained CPU work. Archive `schema.csv`,
`query.csv`, this source, `Cargo.lock`, `git rev-parse HEAD`, `git status --short`,
`rustc -Vv`, `cargo -V`, OS/CPU details, and profiler output. Compare same-host ratios
and the fitted exponent rather than absolute times across machines.
