//! Rust TreeSummary execution and canonical observation algebras.

use super::input::OBSERVATION_COUNT;
use super::operation::variable_values;
use super::operation::SCHEMA;
use super::TreeSummaryInput;
use apollo_compiler::response::JsonMap;
use apollo_compiler::ExecutableDocument;
use apollo_compiler::Schema;
use graphql_static_analysis::cost::CostEstimator;
use graphql_static_analysis::cost::CostModel;
use graphql_static_analysis::max_response_size::MaxResponseSizeEstimator;
use graphql_static_analysis::Algebra;
use graphql_static_analysis::AnalysisMode;
use graphql_static_analysis::Analyzer;
use graphql_static_analysis::CollectedFieldGroup;
use std::hint::black_box;

impl TreeSummaryInput {
    pub fn rust_result(&self) -> String {
        self.rust_result_for(self.mode(), self.observation, false)
    }

    /// Exercises both backends, all algebras, and named-fragment traversal without
    /// claiming that Rust-only syntax has a Lean counterpart.
    pub fn exercise_rust_paths(&self) {
        for mode in [AnalysisMode::ExactCase, AnalysisMode::Syntactic] {
            for observation in 0..OBSERVATION_COUNT {
                black_box(self.rust_result_for(mode, observation, false));
                black_box(self.rust_result_for(mode, observation, true));
            }
        }
    }

    fn rust_result_for(
        &self,
        mode: AnalysisMode,
        observation: u8,
        named_fragments: bool,
    ) -> String {
        let schema = Schema::parse_and_validate(SCHEMA, "schema.graphql")
            .expect("shared fuzz schema is valid");
        let query = self.query_with_fragments(named_fragments);
        let document = ExecutableDocument::parse_and_validate(&schema, &query, "case.graphql")
            .unwrap_or_else(|errors| panic!("shared fuzz operation is valid: {query}\n{errors:?}"));
        let schema = schema.into_inner();
        let document = document.into_inner();
        let operation = document
            .operations
            .get(Some("Case"))
            .expect("Case operation");
        let variables = variable_values(self.variable_case);

        match observation {
            0 => {
                let estimator = MaxResponseSizeEstimator::new(&schema).mode(mode);
                let result = estimator
                    .estimate(
                        &document,
                        operation,
                        u64::from(self.list_size),
                        variables.as_ref(),
                    )
                    .expect("maximum response-size analysis");
                format!("max:{result}")
            }
            1 => {
                let result = analyze(
                    &schema,
                    &document,
                    operation,
                    variables.as_ref(),
                    mode,
                    &CaseSizes,
                );
                format_case_sizes(&result)
            }
            2 => {
                let result = analyze(
                    &schema,
                    &document,
                    operation,
                    variables.as_ref(),
                    mode,
                    &Trace,
                );
                format_trace(&result)
            }
            _ => self.rust_cost_result(mode, named_fragments),
        }
    }

    fn rust_cost_result(&self, mode: AnalysisMode, named_fragments: bool) -> String {
        let (type_cost, field_cost) = self.rust_cost_for(mode, named_fragments);
        assert!(type_cost.is_finite() && type_cost >= 0.0 && type_cost.fract() == 0.0);
        assert!(field_cost.is_finite() && field_cost >= 0.0 && field_cost.fract() == 0.0);
        format!("cost:{type_cost:.0},{field_cost:.0}")
    }

    fn rust_cost_for(&self, mode: AnalysisMode, named_fragments: bool) -> (f64, f64) {
        let schema = Schema::parse_and_validate(SCHEMA, "schema.graphql")
            .expect("shared fuzz schema is valid");
        let query = self.query_with_fragments(named_fragments);
        let document = ExecutableDocument::parse_and_validate(&schema, &query, "case.graphql")
            .unwrap_or_else(|errors| panic!("shared fuzz operation is valid: {query}\n{errors:?}"));
        let schema = schema.into_inner();
        let document = document.into_inner();
        let operation = document
            .operations
            .get(Some("Case"))
            .expect("Case operation");
        let variables = variable_values(self.variable_case).unwrap_or_default();
        let model = CostModel::from_schema(&schema).expect("cost model from shared schema");
        let cost = CostEstimator::new(model)
            .mode(mode)
            .default_list_size(u64::from(self.list_size))
            .estimate(&document, operation, &variables)
            .expect("IBM cost analysis");
        (cost.type_cost, cost.field_cost)
    }
}

fn analyze<A: Algebra>(
    schema: &Schema,
    document: &ExecutableDocument,
    operation: &apollo_compiler::executable::Operation,
    variables: Option<&JsonMap>,
    mode: AnalysisMode,
    algebra: &A,
) -> A::Summary {
    let analyzer = Analyzer::new(schema);
    let analysis = analyzer.operation(document, operation).mode(mode);
    match variables {
        Some(values) => analysis.variable_values(values).analyze(algebra),
        None => analysis.analyze(algebra),
    }
    .expect("TreeSummary analysis")
}

struct CaseSizes;

impl Algebra for CaseSizes {
    type Summary = Vec<u64>;

    fn empty(&self) -> Self::Summary {
        vec![0]
    }

    fn field(&self, _group: &CollectedFieldGroup, child_summary: Self::Summary) -> Self::Summary {
        child_summary
            .into_iter()
            .map(|children| children.saturating_add(1))
            .collect()
    }

    fn combine(&self, left: Self::Summary, right: Self::Summary) -> Self::Summary {
        let mut combined = left
            .into_iter()
            .flat_map(|left| right.iter().map(move |right| left.saturating_add(*right)))
            .collect::<Vec<_>>();
        combined.sort_unstable();
        combined
    }

    fn join(&self, mut left: Self::Summary, right: Self::Summary) -> Self::Summary {
        left.extend(right);
        left.sort_unstable();
        left
    }
}

#[derive(Clone)]
struct TraceCases(Vec<Vec<String>>);

struct Trace;

impl Algebra for Trace {
    type Summary = TraceCases;

    fn empty(&self) -> Self::Summary {
        TraceCases(vec![Vec::new()])
    }

    fn field(&self, group: &CollectedFieldGroup, child_summary: Self::Summary) -> Self::Summary {
        let mut possible_types = group
            .possible_types
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        possible_types.sort_unstable();
        let mut booleans = group
            .child_inherited_boolean_condition()
            .iter()
            .map(|literal| {
                format!(
                    "{}={}",
                    literal.variable_name,
                    u8::from(literal.required_value)
                )
            })
            .collect::<Vec<_>>();
        booleans.sort_unstable();
        let mut field_names = group
            .fields()
            .iter()
            .map(|field| field.name.to_string())
            .collect::<Vec<_>>();
        field_names.sort_unstable();
        let mut cases = child_summary
            .0
            .into_iter()
            .map(|children| {
                vec![format!(
                    "{}<{}>[{}]#{}{{{}}}",
                    group.response_name(),
                    possible_types.join("+"),
                    booleans.join("+"),
                    field_names.join("+"),
                    children.join("&")
                )]
            })
            .collect::<Vec<_>>();
        cases.sort_unstable_by_key(|case| case.join("&"));
        TraceCases(cases)
    }

    fn combine(&self, left: Self::Summary, right: Self::Summary) -> Self::Summary {
        let mut cases = left
            .0
            .into_iter()
            .flat_map(|left_case| {
                right.0.iter().map(move |right_case| {
                    let mut case = left_case.clone();
                    case.extend(right_case.iter().cloned());
                    case.sort_unstable();
                    case
                })
            })
            .collect::<Vec<_>>();
        cases.sort_unstable_by_key(|case| case.join("&"));
        TraceCases(cases)
    }

    fn join(&self, mut left: Self::Summary, right: Self::Summary) -> Self::Summary {
        left.0.extend(right.0);
        left.0.sort_unstable_by_key(|case| case.join("&"));
        left
    }
}

fn format_case_sizes(values: &[u64]) -> String {
    let values = values
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(",");
    format!("cases:{values}")
}

fn format_trace(trace: &TraceCases) -> String {
    let cases = trace
        .0
        .iter()
        .map(|case| {
            if case.is_empty() {
                "_".to_string()
            } else {
                case.join("&")
            }
        })
        .collect::<Vec<_>>()
        .join("|");
    format!("trace:{cases}")
}
