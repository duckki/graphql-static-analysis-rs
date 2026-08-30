# Performance benchmark baseline

This document records the baseline for `MaxResponseSizeEstimator` before the next
engine change. Compare future results using the same benchmark package and machine
conditions; timings from different machines are not directly comparable.

The standalone runner is in [`../benchmarks/`](../benchmarks/). This document is the
canonical guide for running it and recording baselines.

## Benchmark boundary

The benchmark times one reusable `MaxResponseSizeEstimator::estimate` or IBM
`CostEstimator::estimate` call, including the operation-name lookup. Schema and
operation generation, parsing, validation, request-variable construction,
estimator/model construction, and result validation are outside the timed region.

The generated corpus uses 80 overlapping subset interfaces. Each object implements
four consecutive subsets; even inline fragments use `@include` and odd fragments use
`@skip`. The response-size list bound is 10.

The additional `pathological-booleans` stress case uses two disjoint object-type
regions with `K = 1..6` independent Boolean variables per region. It is designed to
distinguish an eager cross-product of unrelated Boolean supports from ExactCase's
factored structural joins. Supplied `true` values are the no-branching control.

The IBM-cost runner also exposes isolated abstract-type-count, possible-type-incidence,
nesting-depth, and response-name-fan-in points. These dimensions are intentionally not
folded into the primary schema/query axes; see `multivariate_campaign.py`.

For each point and configuration, the runner performs two untimed warm-up estimates,
calibrates a power-of-two iteration count to at least 100 ms, records five samples,
and reports every sample plus the median integer nanoseconds per operation. See
[`benchmarks/README.md`](../benchmarks/README.md) for the complete procedure and
reproduction commands.

## Baseline run

- Date: 2026-08-29
- Repository revision: `8ac30c32662c9193201467acb863adee377610cc`
- Repository state: benchmark harness and its `.gitignore` entries were uncommitted;
  no engine source files were modified.
- Rust: `rustc 1.95.0 (59807616e 2026-04-14)`, LLVM 22.1.2
- Cargo: `1.95.0 (f2d3ce0bd 2026-03-21)`
- Host: Apple M3 MacBook Air (8 cores: 4 performance, 4 efficiency), 16 GB memory
- Operating system: macOS 26.6.2 (arm64)
- Build: `cargo build --release --locked` in `benchmarks/`

All points returned the expected result: ExactCase is 21 without variables and 11
with variables; Syntactic is 41 without variables and 21 with variables.

### Schema-size axis

The query has 8 spreads; the schema varies from 1,024 to 10,240 object types.
`p` is the ordinary-least-squares exponent fitted over all ten points.

| Backend | Variables | 1,024 objects | 10,240 objects | `p` |
| --- | --- | ---: | ---: | ---: |
| ExactCase | absent | 99.6 µs | 644.3 µs | 0.801 |
| ExactCase | supplied | 23.3 µs | 178.8 µs | 0.874 |
| Syntactic | absent | 39.0 µs | 287.0 µs | 0.859 |
| Syntactic | supplied | 19.3 µs | 149.7 µs | 1.135 |

### Query-size axis

The schema has 1,024 object types; the query varies from 8 to 80 spreads.

| Backend | Variables | 8 spreads | 80 spreads | `p` |
| --- | --- | ---: | ---: | ---: |
| ExactCase | absent | 101.3 µs | 875.7 µs | 0.925 |
| ExactCase | supplied | 27.8 µs | 177.8 µs | 0.837 |
| Syntactic | absent | 41.9 µs | 375.3 µs | 0.944 |
| Syntactic | supplied | 21.2 µs | 196.7 µs | 0.944 |

## Re-benchmarking after the engine change

Build the updated runner in release mode, execute both axes, and calculate the
exponents. Store transient outputs under `.scratch/` or outside the repository:

```sh
cd benchmarks
cargo build --release --locked
mkdir -p ../.scratch
target/release/graphql-static-analysis-benchmark schema-size > ../.scratch/after-schema.csv
target/release/graphql-static-analysis-benchmark query-size > ../.scratch/after-query.csv
target/release/graphql-static-analysis-benchmark pathological-booleans \
  > ../.scratch/after-pathological-booleans.csv
target/release/graphql-static-analysis-benchmark cost-schema-size \
  > ../.scratch/after-cost-schema.csv
target/release/graphql-static-analysis-benchmark cost-query-size \
  > ../.scratch/after-cost-query.csv
python3 scaling.py ../.scratch/after-schema.csv ../.scratch/after-query.csv \
  ../.scratch/after-cost-schema.csv ../.scratch/after-cost-query.csv
```

For a quick correctness and smoke-performance check, run:

```sh
target/release/graphql-static-analysis-benchmark endpoints
```

Use mains power, avoid sustained concurrent CPU work, and retain the CSV files with
the updated revision and toolchain details. Keep the corpus, response-size assertions,
timed boundary, Rust toolchain, release flags, and hardware fixed. Run baseline and
candidate in alternating order to avoid a systematic thermal or scheduling advantage.
For a decision based on small differences, run each axis in three fresh processes and
use a pointwise median before comparing against this single-process baseline.

## Reusable IBM-cost campaign after the current optimization

The reusable `CostEstimator` boundary was measured on 2026-08-29 at revision
`c3f92a8c25bf3a0f1356595b74f7c92306cc9b30`. The harness changes were uncommitted;
the engine source was clean. A randomized campaign retained five timed samples from
each of three fresh process replicates per point. Three replicates are a development
check, not the ten-replicate publication target.

| Axis | Backend | Small endpoint | Large endpoint | Empirical `p` | Hierarchical-bootstrap 95% CI | log-log `R²` |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Query size, 8→80 spreads | ExactCase | 29.4 µs | 260.5 µs | 0.950 | [0.831, 1.018] | 1.000 |
| Query size, 8→80 spreads | Syntactic | 28.5 µs | 257.9 µs | 0.968 | [0.852, 1.061] | 0.999 |
| Schema size, 1,024→10,240 objects | ExactCase | 28.3 µs | 239.8 µs | 0.942 | [0.930, 1.115] | 0.999 |
| Schema size, 1,024→10,240 objects | Syntactic | 27.2 µs | 232.8 µs | 0.927 | [0.904, 1.019] | 1.000 |

These exponents are empirical summaries of this topology, not worst-case complexity
bounds. The wide and long campaign files include run order, process replicate, sample
index, iteration count, timestamp, toolchain, platform, Git revision, and dirty state.
The checked-in driver regenerates them with:

```sh
python3 benchmarks/campaign.py --replicates 10 --output-dir OUTPUT_DIRECTORY
```

Archive the Git revision and status, `rustc -Vv`, `cargo -V`, CPU and operating-system
details, CSV files, and profiler artifacts. Report same-host ratios and scaling
exponents rather than absolute timings across machines. Update this document only
with a fully identified run.
