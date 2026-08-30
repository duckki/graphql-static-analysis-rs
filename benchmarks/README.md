# Performance benchmarks

This standalone Cargo package measures reusable `MaxResponseSizeEstimator::estimate`
and IBM `CostEstimator::estimate` calls. Schema and operation construction, validation,
request-variable construction, estimator/model construction, and result checking are
outside the timed region.

It compares four configurations: ExactCase and Syntactic, each with absent request
variables and with `{ "includeBranch": true, "skipBranch": true }`. The generated
corpus has 80 overlapping subset interfaces; each object implements four consecutive
subsets. Even query fragments use `@include`, odd fragments use `@skip`, and fields
share response names. Every result is asserted before timings are reported.

The primary experiment has three independent axes:

- `schema-size`: 1,024 through 10,240 objects, with 8 spreads.
- `query-size`: 8 through 80 spreads, with 1,024 objects.
- `pathological-booleans`: two disjoint object-type regions with `K = 1..6`
  independent Boolean variables per region. Without request values, an eager join of
  the disjoint Boolean supports constructs their cross-product; ExactCase structural
  joins keep the two decision trees factored. Supplied `true` values provide a
  no-branching control. Every configuration returns `K + 1` response fields.

The matched IBM-cost experiment uses the same schema and operation family, attaches
field weights 1 and 7, supplies both Boolean request variables as `true`, and reuses one
schema-derived `CostModel`/`CostEstimator`. Its `cost-schema-size` and `cost-query-size`
commands isolate the production estimator boundary used by callers. ExactCase returns
type/field cost `2/2`; Syntactic returns `2/3` on this deliberately distinguishing
family.

Secondary `cost-topology-point` and `cost-structure-point` commands vary the
information hidden by those fixed values: number of abstract types, possible-type
incidences per object (overlap density), nesting depth, and repeated response-name
fan-in. `multivariate_campaign.py` runs those four dimensions in randomized fresh
processes. They are reported separately from the two primary axes so changing one
topology parameter never silently changes the definition of “schema size.”

Each configuration warms up twice, calibrates a power-of-two iteration count to a
100 ms sample, records five samples, and emits every sample plus the median total time
and integer nanoseconds per estimate as CSV. The estimator is reused for all timed
calls. `scaling.py` groups each axis independently, reports log-log residual fit, and
uses a hierarchical bootstrap over fresh-process rows and within-process samples.

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
target/release/graphql-static-analysis-benchmark cost-schema-size > cost-schema.csv
target/release/graphql-static-analysis-benchmark cost-query-size > cost-query.csv
python3 scaling.py schema.csv query.csv cost-schema.csv cost-query.csv
```

For the paper campaign, randomize every backend/axis/size point and run each in a fresh
process. This produces both one-row-per-process and one-row-per-sample datasets plus a
provenance manifest:

```sh
python3 campaign.py --replicates 10 --output-dir ../.scratch/cost-campaign
python3 multivariate_campaign.py --replicates 10 \
  --output-dir ../.scratch/cost-multivariate-campaign
python3 scaling.py ../.scratch/cost-campaign/cost-campaign-wide.csv \
  ../.scratch/cost-multivariate-campaign/multivariate-campaign-wide.csv
```

Individual secondary points are also available for profiling and smoke tests:

```text
target/release/graphql-static-analysis-benchmark \
  cost-topology-point OBJECTS ABSTRACT_TYPES INCIDENCES SPREADS BACKEND
target/release/graphql-static-analysis-benchmark \
  cost-structure-point NESTING_DEPTH RESPONSE_FAN_IN BACKEND
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
