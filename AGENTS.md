# Repository agent memory

## TreeSummary fuzzing

- Read [`docs/fuzzing.md`](docs/fuzzing.md) before changing the TreeSummary engine,
  shared fuzz IR, Lean oracle adapter, observations, or retained corpus. It is the
  canonical fuzzing architecture and operations guide.
- Run the deterministic differential profile before a coverage-guided campaign. Keep
  the recorded Lean revision sidecar and do not bypass the stale-oracle check except
  for an intentional cross-revision comparison.
- Preserve both lanes: `differential` checks the shared Lean/Rust model, while
  `rust_only` covers Rust behavior outside that model. Source coverage and the mutation
  sentinel validate harness reachability but do not replace differential comparison.
- Keep small, named decision-path seeds in `fuzz/corpus/`. Generated corpora, oracle
  binaries, coverage results, and crash artifacts must remain in the ignored locations
  documented in `docs/fuzzing.md`.
- The minimized ExactCase inputs document previously fixed missing-value and inherited-
  context mismatches. Keep them green and update the alignment status in
  `docs/fuzzing.md` whenever the Rust implementation or pinned Lean model changes.

## Performance benchmarking

- The canonical response-size benchmark is the standalone Cargo package in
  `benchmarks/`. Read [`docs/performance-benchmark.md`](docs/performance-benchmark.md)
  before changing the engine or interpreting a benchmark result.
- Benchmark only release builds. From `benchmarks/`, build with
  `cargo build --release --locked`, then invoke the compiled binary directly; Cargo
  startup and compilation must stay outside the measurement.
- The timed boundary is one reusable `MaxResponseSizeEstimator::estimate` call. Do
  not move schema/operation parsing, validation, variable creation, or estimator
  construction into the timed region without creating a separately named benchmark
  and baseline.
- Preserve the generated corpus, four mode/variable configurations, response-size
  assertions, and sampling settings when making an engine performance comparison.
  The expected results are ExactCase `21`/`11` and Syntactic `41`/`21`, for
  absent/supplied variables respectively.
- Store temporary CSVs and profiler artifacts under `.scratch/` or outside the
  repository. When recording a new baseline, update
  `docs/performance-benchmark.md` with the revision, clean/dirty state, toolchain,
  host details, endpoint timings, and scaling exponents.
- Do not call a small timing change a regression or improvement from one process.
  For a decision, run three fresh processes per variant in alternating order and use
  pointwise medians on the same host.
