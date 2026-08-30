# `graphql-static-analysis`

`graphql-static-analysis` is a composable static-analysis engine for validated GraphQL
operations. The engine owns GraphQL-specific reasoning—possible runtime types,
`@include` and `@skip`, field collection, fragments, and merged child selection
sets—while an analysis supplies the values and operations used to summarize a query.

For the motivation behind this work and an introduction to its verified approach, read
[Static Analysis for GraphQL, Verified in Lean](https://duckki.github.io/2026/08/30/static-analysis-for-graphql-verified-in-lean.html).

The crate includes two example analyses:

- **Static Cost Estimate** is the primary example, implementing the
  [IBM GraphQL Cost Directives specification](https://ibm.github.io/graphql-specs/cost-spec.html).
  It reads `@cost` and `@listSize` schema metadata and reports independent type and
  field costs.
- **Maximum response size** is a simple example analysis that computes an upper bound
  on response size under a global maximum list-length configuration.

See [Adding a custom analysis](docs/custom-analyses.md) to implement another analysis
with the public `Algebra` API.

## Installation

Add the analysis engine with Cargo:

```sh
cargo add graphql-static-analysis
```

The API uses schema, operation, validation, and variable-coercion types from
`apollo-compiler`. If your project does not already depend on it, add it as well:

```sh
cargo add apollo-compiler
```

## Input validation and variable coercion

The engine expects schemas and executable documents to have passed GraphQL validation.
It does not repeat validation during analysis. Apollo Compiler can parse and validate
both inputs:

```rust,ignore
use apollo_compiler::{ExecutableDocument, Schema};

let schema = Schema::parse_and_validate(schema_source, "schema.graphql")?;
let document = ExecutableDocument::parse_and_validate(
    &schema,
    operation_source,
    "operation.graphql",
)?;
```

Supplied variable values are likewise expected to have passed GraphQL variable
coercion. Use `apollo_compiler::request::coerce_variable_values` before analysis:

```rust,ignore
use apollo_compiler::request::coerce_variable_values;

let operation = document.operations.iter().next().expect("one operation");
let variables = coerce_variable_values(&schema, operation, &raw_variables)?;
```

Coercion rejects incompatible values and missing required variables, applies defaults,
and returns the `Valid<JsonMap>` required by the analysis APIs. The analysis engine
itself does not return a variable-coercion error. If a caller explicitly bypasses
coercion with `Valid::assume_valid`, invalid supplied values are treated as missing
during Boolean directive evaluation.

## Using the example analyses

Construct estimators once per schema and reuse them across operations.

### [IBM GraphQL Cost Directives](https://ibm.github.io/graphql-specs/cost-spec.html)

Build the schema cost model, construct an estimator, and supply already-coerced values
for the operation's variables to each estimate:

```rust,ignore
use graphql_static_analysis::cost::{CostEstimator, CostModel};

let cost_model = CostModel::from_schema(&schema)?;
let estimator = CostEstimator::new(cost_model)
    // Optional deployment fallback for lists without an applicable @listSize.
    .default_list_size(100);

let cost = estimator.estimate(&document, operation, &variables)?;
println!("type cost: {}", cost.type_cost);
println!("field cost: {}", cost.field_cost);
```

Without `default_list_size`, a list lacking an applicable `@listSize` bound has
infinite cost, which is the conservative IBM behavior.

### Maximum response size

```rust,ignore
use graphql_static_analysis::max_response_size::MaxResponseSizeEstimator;

let estimator = MaxResponseSizeEstimator::new(&schema);
let ahead_of_execution = estimator.estimate(&document, operation, 100, None)?;
let for_this_request =
    estimator.estimate(&document, operation, 100, Some(&variables))?;
```

The `100` is the assumed maximum length of every list layer. Arithmetic saturates at
`u64::MAX`. Variable values are optional for this analysis and can be `None`.

## Analysis time and variable assignments

An operation can be analyzed with or without query variable values. This choice
supports two analysis times:

- **Without variable assignments** supports ahead-of-execution analysis, such as at
  build time, persisted-operation registration, or admission-policy preparation. The
  engine considers every feasible value of unresolved `@include` and `@skip`
  conditions. The resulting conservative estimate does not depend on any particular
  request's variable values and can be reused across requests.
- **With variable assignments** supports analysis for a particular request at
  execution time. The engine applies operation defaults and resolves conditional
  selections from the request values. This normally does less work and can produce a
  tighter result.

These are options on the same engine, independent of the analysis mode described
below. An analysis whose meaning inherently depends on argument values can require a
variable map through `Algebra::requires_variables`; the IBM cost analysis does so.

## Precision and performance modes

The engine offers two traversal modes:

| | `AnalysisMode::ExactCase` | `AnalysisMode::Syntactic` |
| --- | --- | --- |
| Behavior | Partitions possible runtime types into exact compatibility regions and globally groups fields that can execute together under one response name. | Summarizes canonical condition-tree branches once and reuses those summaries, keeping some fields from different syntactic conditions separate. |
| Precision | The most precise result and, for algebras satisfying the formal best-transfer contracts, the best possible static approximation represented by the model. | A conservative result that can be less precise because some syntactically separate fields remain separate. |
| Performance | Performs the additional work needed for exact field collection. | Usually faster, especially ahead of execution without variable assignments. |
| Default | Yes. | No; select it explicitly. |

Use `ExactCase` unless analysis latency is more important than precision. Opt into the
performance–precision trade-off with:

```rust,ignore
let estimator = CostEstimator::new(cost_model).mode(AnalysisMode::Syntactic);
```

## Lean formalization and Rust confidence

The engine is implemented from the TreeSummary formal model in
[GraphQL.lean](https://github.com/duckki/GraphQL.lean). The Lean development connects
local algebra obligations to execution-level soundness. For the Lean model's
`ExactCases` analysis, optional best-transfer contracts additionally prove that an
estimate is the least upper bound over the modeled feasible outcomes—the best
approximation expressible by that analysis.

Those proofs establish the model, not this Rust source code directly. The repository
therefore also runs deterministic and coverage-guided differential tests against a
native executable built from the Lean model. The test matrix compares response size,
exact cases, recursively collected-field traces, and IBM cost. This provides a high
level of confidence that the Rust engine implements the verified model faithfully;
see [TreeSummary differential fuzzing](docs/fuzzing.md) for its scope and limitations.

## License

Licensed under the MIT License. See the [license file](./LICENSE).
