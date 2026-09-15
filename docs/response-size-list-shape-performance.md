# Response-size list-shape optimization (2026-09-13)

The measurements below retain their original Lean revision and binary provenance.
The subsequent [2026-09-14 release audit](release-readiness-audit.md) checks Rust
against Lean's new executable shortcut and its equivalence proofs at `41f1c4a`.

`MaxResponseSizeAlgebra` now computes a field's list multiplier from its executable
field definition. Previously it looked up that field separately for every possible
runtime parent and took the maximum multiplier. This removes a scan over the type
case: multiplier calculation changes from O(T × (D + 1)) to O(D + 1), for T possible
parents and D list depth. No cache, preparation API, or benchmark-boundary change is
needed.

## Why the result is preserved

The public API expects validated schemas and executable documents. Apollo Compiler
stores the selected field's schema definition on `executable::Field::definition`.
Its `validation::interface::is_valid_implementation_field_type` recursively requires
the same list structure in interface fields and their implementations. Implementations
can strengthen nullability or narrow the named output type, but neither changes the
number of list wrappers. Field-merging validation also preserves response shape.

Every applicable runtime field therefore has the same uniform list multiplier as
the executable definition. Lean's `MaxResponseSize.fieldListMultiplier` folds those
multipliers with `max`, starting at 1. Rust can calculate the common multiplier once
and take `max(1, multiplier)`. The floor matters when the caller supplies a zero list
bound; this change preserves it. Existing saturating arithmetic remains unchanged;
Lean uses unbounded natural numbers.

The named runtime output type is still needed for child traversal and IBM cost.
Those paths keep their existing runtime-specific lookups. Neither ExactCase scheduler
nor Syntactic traversal changes. This is an equivalence argument under validated-input
invariants, backed by the checks below, not a new formal proof of Rust.

## Captured artifacts and measurement method

- Before: clean revision `87d8e9a31012a70bfab5b84eef9f9f1315cc0a71`.
- After: that revision with the uncommitted `src/analyses/max_response_size.rs` change
  and its covariance regression test. Documentation was written after capture.
- Before binary SHA-256:
  `3b0147a014ed6fb12d34f2efbf8b1260e5554d79c330a6770c21d0bf742f4fb4`.
- After binary SHA-256:
  `51e11fcc0e3b4b548b66cc798b4fea59e34d7eaed72346d0cee1649a6be02d77`.
- Rust `1.95.0 (59807616e 2026-04-14)`, Cargo `1.95.0 (f2d3ce0bd 2026-03-21)`,
  LLVM 22.1.2; locked offline release builds, no build-environment overrides.
- Apple M3 MacBook Air, 8 cores, 16 GiB RAM, macOS 26.6.2 (25G83), AC power.
- The canonical corpus, all four backend/variable configurations, reusable-estimate
  timing boundary, two warm-ups, calibration of at least 100 ms, five samples,
  response assertions, and benchmark source/lockfile are unchanged.
- Three fresh processes per variant and axis run serially in alternating before/after
  order. Each reported point is the median of three process medians. Empirical scaling
  exponents fit all ten points of each primary axis. Tests and profiling finish before
  timing; no concurrent build, test, or profiler runs during the comparison.

Frozen binaries, source files and hashes, raw CSVs, per-process ranges, timestamps,
scaling fits, and CPU captures are retained under ignored `.scratch/algorithmic/`.
The preliminary empty-child prototype was discarded and its smoke timings are excluded.

Reproduce with the maintained tools, capturing `before` from the baseline checkout:

```sh
python3 benchmarks/artifacts.py --offline --output-dir .scratch/algorithmic/before
# Apply the optimization, then capture and compare:
python3 benchmarks/artifacts.py --offline --output-dir .scratch/algorithmic/validated-list-shape
python3 benchmarks/compare.py --before .scratch/algorithmic/before \
  --after .scratch/algorithmic/validated-list-shape \
  --output-dir .scratch/algorithmic/comparison
```

## Primary response-size results

Times are microseconds per estimate; arrows show before → after. The schema axis has
8 spreads and 1,024→10,240 object types. The query axis has 1,024 object types and
8→80 spreads. Each exponent is fitted across the ten pointwise medians, not just the
endpoints. These are empirical summaries of this corpus, not worst-case bounds.

| Axis | Backend | Variables | Small endpoint (µs) | Large endpoint (µs) | Scaling p |
| --- | --- | --- | ---: | ---: | ---: |
| Schema | ExactCase | absent | 71.87 → 60.54 | 700.62 → 239.34 | 1.007 → 0.642 |
| Schema | ExactCase | supplied | 17.97 → 14.82 | 110.54 → 73.41 | 0.799 → 0.711 |
| Schema | Syntactic | absent | 37.78 → 28.08 | 254.79 → 146.43 | 0.839 → 0.728 |
| Schema | Syntactic | supplied | 18.80 → 14.22 | 127.31 → 74.86 | 0.840 → 0.734 |
| Query | ExactCase | absent | 71.95 → 60.48 | 692.50 → 592.89 | 0.989 → 1.000 |
| Query | ExactCase | supplied | 18.32 → 15.07 | 164.01 → 134.00 | 0.957 → 0.960 |
| Query | Syntactic | absent | 38.83 → 28.26 | 361.83 → 262.70 | 0.971 → 0.979 |
| Query | Syntactic | supplied | 19.15 → 14.41 | 177.15 → 129.54 | 0.963 → 0.966 |

Supplied-variable ExactCase takes 33.6% less time at the large schema endpoint and
18.3% less at the large query endpoint. Median reductions across their ten points are
29.3% and 18.1%. Syntactic takes 41.2–42.5% less time at the large schema endpoint and
26.9–27.4% less at the large query endpoint. The scan removal improves both backends;
it is not a supplied-variable scheduler change.

Symbolic ExactCase's large-schema process medians range from 614.55–703.96 µs before
and 237.77–452.20 µs after. Every after process is faster than every before process
at that endpoint, but the 65.8% median reduction and fitted exponent summarize a wide
range. Do not treat that number as a stable universal speedup. The supplied-variable
ExactCase ranges are much tighter: 110.52–111.44 µs before, 73.36–75.66 µs after.

The query-size exponent remains close to one. The optimization removes repeated
type scans from field callbacks; it does not eliminate the per-selection extraction
and traversal work that grows with query size.

## Boolean stress and IBM-cost controls

All 24 timed processes completed, covering 660 configurations and producing 110
pointwise medians. Response sizes remain ExactCase 21/11 and Syntactic 41/21 for
absent/supplied variables. Boolean stress returns K + 1 in all four configurations.
IBM-cost assertions also pass.

| Boolean stress backend | Variables | K = 1 (µs), before → after | K = 6 (µs), before → after |
| --- | --- | ---: | ---: |
| ExactCase | absent | 5.19 → 5.17 | 142.11 → 136.72 |
| ExactCase | supplied | 3.99 → 4.03 | 12.63 → 12.33 |
| Syntactic | absent | 4.01 → 4.07 | 12.87 → 12.68 |
| Syntactic | supplied | 3.93 → 4.01 | 12.72 → 12.49 |

| Types / spreads | IBM ExactCase (µs), before → after | IBM Syntactic (µs), before → after |
| --- | ---: | ---: |
| 1,024 / 8 | 24.87 → 25.11 | 27.91 → 28.48 |
| 10,240 / 8 | 185.70 → 180.91 | 233.64 → 231.54 |
| 1,024 / 80 | 217.65 → 212.33 | 265.49 → 255.13 |

IBM cost uses unchanged analysis code and is a whole-binary control. Its pointwise
medians range from 2.0% more time to 3.9% less, with overlapping before/after process
ranges at every endpoint. The small Boolean-stress differences are comparable to
that variation. This experiment establishes neither an IBM-cost nor a Boolean-stress
speedup. The retained change targets the substantial schema/query gains above.

## CPU samples

Each snapshot was sampled in two fresh processes per workload, for five seconds at
a requested one-millisecond interval. Captures contain 3,794–4,209 samples inside
`MaxResponseSizeEstimator::estimate`. The table reports inclusive stack membership;
columns overlap and must not be added. Symbol inlining limits attribution.

| ExactCase workload | Schema lookup, before → after | Extraction, before → after | Type partition, before → after |
| --- | ---: | ---: | ---: |
| 10,240 types / 8 spreads, supplied | 23.7–24.0% → <0.05% | 28.0–29.2% → 43.4–43.6% | 27.6–28.2% → 42.6–42.7% |
| 1,024 types / 80 spreads, supplied | 14.6–16.5% → <0.03% | 51.0–51.8% → 60.8–61.6% | 16.0–17.3% → 18.9–19.8% |
| 10,240 types / 8 spreads, absent | 19.7–20.5% → 0% | 10.0–10.4% → 27.0–27.3% | 12.1–13.3% → 33.2–34.3% |

The eliminated scan explains the disappearance of schema-lookup samples. Relative
shares of the remaining work increase as the denominator shrinks. Symbolic name
clone/drop samples also fall from 47.1–48.3% to 22.0–23.1%; this experiment does not
separately attribute compiler specialization and ownership effects.

Profiles can be regenerated for each snapshot with:

```sh
python3 benchmarks/profile_estimate.py --snapshot SNAPSHOT \
  --output-dir OUTPUT_DIRECTORY \
  --workloads schema-supplied query-supplied schema-symbolic
```

## Correctness checks

- 75 library tests and one public-API integration test pass on Rust 1.95 and 1.90.
- The new regression covers covariant named outputs, strengthened nullability,
  interface inheritance, repeated response names with merged child selections,
  nested list wrappers, zero/one/three/MAX bounds, and all four configurations.
- All 15,840 deterministic Lean/Rust max/cases/trace/cost profiles agree with pinned
  Lean revision `4102b52fef79782145ffef7401c393319edc4f16`. The `max` observation calls
  the production `MaxResponseSizeEstimator`, including this optimization.
- All 2,979 order-sensitive ExactCase schedules agree.
- Root strict Clippy, formatting, documentation with warnings denied, and fuzz package
  tests pass. The retained corpora and oracle revision are unchanged.

## Further algorithmic work

The next candidates are eliminating repeated normalization of selection scopes and
reducing repeated type-set scans. Extraction dominates the large supplied query;
intersection and partition dominate much of the large supplied schema. A prepared
operation could reuse immutable normalized structure across requests, but needs a
separate preparation benchmark and careful handling of merged selections, inherited
conditions, defaults, and supplied-value pruning. Its end-to-end benefit is unmeasured.

An alternative within the current API is memoizing repeated canonicalization work
inside one estimate, provided the full semantic context is part of the key. Any such
change must preserve the Lean schedule checks. Neither buffer tuning nor small
allocation/branch fast paths are included in this slice. Representative-only child
output lookup also remains a proposal until a repeated object-field workload shows a
substantial benefit; the existing response-fan-in fixture repeats a scalar field and
does not exercise that child-lookup path.
