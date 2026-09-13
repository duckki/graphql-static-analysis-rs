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

Secondary `cost-topology-point`, `cost-unused-abstract-point`, and
`cost-structure-point` commands vary the information hidden by those fixed values:
abstract partition count, unrelated declared abstract types with queried memberships
fixed, possible-type incidences per object (overlap density), nesting depth, and
repeated response-name fan-in. `multivariate_campaign.py` runs those five dimensions in
randomized fresh processes. They are reported separately from the two primary axes so
changing one topology parameter never silently changes the definition of “schema
size.”

Each configuration warms up twice, calibrates a power-of-two iteration count to a
100 ms sample, records five samples, and emits every sample plus the median total time
and integer nanoseconds per estimate as CSV. The estimator is reused for all timed
calls. `scaling.py` groups each axis independently, reports log-log residual fit, and
uses a hierarchical bootstrap over fresh-process rows and within-process samples.

## Run

### Captured before/after comparisons

Use the maintained Python 3.10+ tools for engine comparisons. From this directory,
capture the baseline before editing the engine, then capture the candidate:

```sh
python3 artifacts.py --output-dir ../.scratch/before
# Make the engine change, then capture its release build.
python3 artifacts.py --output-dir ../.scratch/after
python3 compare.py --before ../.scratch/before --after ../.scratch/after \
  --output-dir ../.scratch/comparison
```

`artifacts.py` runs `cargo build --release --locked` before copying the compiled
binary and source/configuration files into a new snapshot directory. Add `--offline`
when dependencies are already cached. Its manifest records source and binary SHA-256
hashes, the Git revision and dirty state, toolchain, explicit build overrides, and
host details. Source hashes are checked before and after the build and after copying
the snapshot. Artifact directories
must be new and under `.scratch/` or outside the repository.

`compare.py` verifies both snapshots, requires matching toolchain/host and benchmark
source/lockfile, and runs only their frozen binaries. The default campaign covers
schema size, query size, Boolean stress, and cost endpoints in three fresh processes
per variant, alternating before/after. Use `--axes endpoints` for a shorter endpoint
comparison or select other documented axes. The generated inputs, four response-size
configurations, result assertions, and built-in sampling settings stay unchanged.

The output includes every process CSV, a manifest with run times, arguments and
hashes, pointwise process medians in `comparison.csv`, and schema/query log-log
exponents in `scaling.json`. Endpoint-only runs produce no scaling fit. Minimum and
maximum process medians are retained to help assess variation. Smaller changes still
need judgment; the script does not label a timing difference a regression or speedup.

For the matching macOS sampling campaign:

```sh
python3 profile_estimate.py --snapshot ../.scratch/after \
  --output-dir ../.scratch/profile
```

This runs two five-second captures for each of the five recorded ExactCase workloads,
using `sample` at a requested one-millisecond interval. Select workloads with
`--workloads schema-supplied query-supplied`; native sampling requires permission to
inspect the child benchmark process. Stack captures, logs, arguments and snapshot
provenance remain in the output directory. `cpu-summary.csv` excludes startup stacks
outside `MaxResponseSizeEstimator::estimate` and counts recursive frames once per
sample. Its inclusive categories overlap and must not be added. Inlining can limit
symbol attribution. The profiler stops only benchmark processes it launches.

Integrity tests for snapshot tampering, incompatible benchmarks, process medians,
scaling, incomplete CSVs, and recursive sample attribution run in CI:

```sh
python3 -m unittest discover -s . -p 'test_*.py'
```

### Direct invocation

Run these commands from this directory. Build once, then invoke the binary directly
so Cargo startup and compilation are not included in the surrounding observation.

```sh
cargo build --release --locked
mkdir -p ../.scratch/direct
target/release/graphql-static-analysis-benchmark endpoints
target/release/graphql-static-analysis-benchmark schema-size > ../.scratch/direct/schema.csv
target/release/graphql-static-analysis-benchmark query-size > ../.scratch/direct/query.csv
target/release/graphql-static-analysis-benchmark pathological-booleans \
  > ../.scratch/direct/pathological-booleans.csv
target/release/graphql-static-analysis-benchmark cost-schema-size > ../.scratch/direct/cost-schema.csv
target/release/graphql-static-analysis-benchmark cost-query-size > ../.scratch/direct/cost-query.csv
python3 scaling.py ../.scratch/direct/schema.csv ../.scratch/direct/query.csv \
  ../.scratch/direct/cost-schema.csv ../.scratch/direct/cost-query.csv
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
  cost-unused-abstract-point OBJECTS DECLARED_ABSTRACT_TYPES \
  MEMBERSHIP_ABSTRACT_TYPES INCIDENCES SPREADS BACKEND
target/release/graphql-static-analysis-benchmark \
  cost-structure-point NESTING_DEPTH RESPONSE_FAN_IN BACKEND
```

The main binary also runs an inline-schema JSON corpus through both IBM analysis modes:

```text
target/release/graphql-static-analysis-benchmark study GENERATED_CORPUS.json
```

Build and run the separate counting-allocator binary to record `CostModel`,
`CostEstimator`, and per-estimate heap activity without changing the timed binary:

```text
target/release/cost_allocations
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
