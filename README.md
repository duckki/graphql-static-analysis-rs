# `graphql-static-analysis`

Composable static analyses for validated GraphQL operations.

The crate defaults to its optimal condition-aware analysis:

- `AnalysisMode::ExactCase` partitions possible runtime types into exact compatibility
  regions and resolves every simultaneously active branch before grouping fields by
  response name. Object types that activate the same branches remain grouped. This is
  the default.
- `AnalysisMode::Syntactic` builds a condition tree, summarizes each branch once,
  and reuses those summaries across runtime alternatives. It is faster, but can be less
  precise because it keeps response-name groups from different syntactic conditions
  separate. Select it explicitly when that performance-precision trade-off is useful.

Analyses implement the `Algebra` trait. The engine may be run with or without
coerced operation-variable values; an algebra can require values by overriding
`requires_variables`. Construct an `Analyzer` once per schema and reuse it across
operations; it owns the schema indexes shared by both analysis backends.

The `cost` module is an example algebra implementing the IBM GraphQL Cost Directives
estimate and requires variable values. Construct a reusable estimator from the
schema's cost model, then supply the operation inputs to each estimate:

```rust,ignore
let cost_model = cost::CostModel::from_schema(&schema)?;
let estimator = cost::CostEstimator::new(cost_model);
let cost = estimator.estimate(&document, operation, &variables)?;
```

The `max_response_size` module bounds the number of response-object fields using one
global list-size assumption and no custom schema directives. It supports both
variable-independent and request-specialized estimates:

```rust,ignore
let estimator = max_response_size::MaxResponseSizeEstimator::new(&schema);
let conservative = estimator.estimate(&document, operation, 100, None)?;
let specialized = estimator.estimate(&document, operation, 100, Some(&variables))?;
```

## License

Licensed under the MIT License. See [`LICENSE`](LICENSE).
