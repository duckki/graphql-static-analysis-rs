//! Maximum GraphQL response-size analysis with a uniform list-size bound.
//!
//! The estimate counts response-object fields at every nesting level. Lists do not
//! themselves add fields, but every list wrapper multiplies the size of its selected
//! children by the caller-supplied global bound.

use crate::Algebra;
use crate::AnalysisError;
use crate::AnalysisMode;
use crate::Analyzer;
use crate::CollectedFieldGroup;
use apollo_compiler::ast::Type;
use apollo_compiler::executable::ExecutableDocument;
use apollo_compiler::executable::Operation;
use apollo_compiler::response::JsonMap;
use apollo_compiler::validation::Valid;
use apollo_compiler::Schema;

/// Reusable maximum-response-size estimator for one schema.
pub struct MaxResponseSizeEstimator<'schema> {
    analyzer: Analyzer<'schema>,
    mode: AnalysisMode,
}

impl<'schema> MaxResponseSizeEstimator<'schema> {
    /// Creates an exact-case estimator and indexes the schema for both analysis
    /// backends.
    ///
    /// Use [`Self::mode`] to opt into the faster, potentially less precise
    /// [`AnalysisMode::Syntactic`] traversal.
    pub fn new(schema: &'schema Schema) -> Self {
        Self {
            analyzer: Analyzer::new(schema),
            mode: AnalysisMode::default(),
        }
    }

    /// Selects the analysis traversal. The default is [`AnalysisMode::ExactCase`].
    pub fn mode(mut self, mode: AnalysisMode) -> Self {
        self.mode = mode;
        self
    }

    /// Estimates one operation with a uniform maximum length for every list layer.
    ///
    /// Pass `None` to conservatively analyze every feasible variable case. Supplying
    /// already-coerced request values applies operation defaults and prunes selections
    /// whose `@skip` or `@include` condition is known false.
    ///
    /// Arithmetic saturates at [`u64::MAX`]. This is the Rust representation's cap;
    /// the Lean model uses unbounded natural numbers.
    pub fn estimate<'analysis>(
        &'analysis self,
        document: &'analysis ExecutableDocument,
        operation: &'analysis Operation,
        list_size: u64,
        variable_values: Option<&'analysis Valid<JsonMap>>,
    ) -> Result<u64, AnalysisError> {
        let algebra = MaxResponseSizeAlgebra {
            schema: self.analyzer.schema(),
            list_size,
        };
        let analysis = self
            .analyzer
            .operation(document, operation)
            .mode(self.mode)
            .analysis_name("maximum response size");
        match variable_values {
            Some(variable_values) => analysis.variable_values(variable_values).analyze(&algebra),
            None => analysis.analyze(&algebra),
        }
    }
}

/// Convenience entry point that constructs an estimator for this call.
pub fn estimate(
    schema: &Schema,
    document: &ExecutableDocument,
    operation: &Operation,
    mode: AnalysisMode,
    list_size: u64,
    variable_values: Option<&Valid<JsonMap>>,
) -> Result<u64, AnalysisError> {
    MaxResponseSizeEstimator::new(schema).mode(mode).estimate(
        document,
        operation,
        list_size,
        variable_values,
    )
}

struct MaxResponseSizeAlgebra<'schema> {
    schema: &'schema Schema,
    list_size: u64,
}

impl Algebra for MaxResponseSizeAlgebra<'_> {
    type Summary = u64;

    fn empty(&self) -> Self::Summary {
        0
    }

    fn field(&self, group: &CollectedFieldGroup, child_summary: Self::Summary) -> Self::Summary {
        1_u64.saturating_add(
            self.field_list_multiplier(group)
                .saturating_mul(child_summary),
        )
    }

    fn combine(&self, left: Self::Summary, right: Self::Summary) -> Self::Summary {
        left.saturating_add(right)
    }

    fn join(&self, left: Self::Summary, right: Self::Summary) -> Self::Summary {
        left.max(right)
    }
}

impl MaxResponseSizeAlgebra<'_> {
    fn field_list_multiplier(&self, group: &CollectedFieldGroup) -> u64 {
        let field = group.representative_field();
        group
            .possible_types
            .iter()
            .filter_map(|parent_type| {
                self.schema
                    .type_field(parent_type, &field.name)
                    .ok()
                    .map(|definition| list_multiplier(self.list_size, &definition.ty))
            })
            .fold(1, u64::max)
    }
}

fn list_multiplier(list_size: u64, ty: &Type) -> u64 {
    match ty {
        Type::Named(_) | Type::NonNullNamed(_) => 1,
        Type::List(inner) | Type::NonNullList(inner) => {
            list_size.saturating_mul(list_multiplier(list_size, inner))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apollo_compiler::response::serde_json_bytes::json;
    use apollo_compiler::ExecutableDocument;
    use pretty_assertions::assert_eq;

    fn parse(schema: &str, query: &str) -> (Schema, ExecutableDocument) {
        let schema = Schema::parse_and_validate(schema, "schema.graphql").unwrap();
        let document =
            ExecutableDocument::parse_and_validate(&schema, query, "query.graphql").unwrap();
        (schema.into_inner(), document.into_inner())
    }

    fn estimate_query(
        schema_source: &str,
        query: &str,
        mode: AnalysisMode,
        list_size: u64,
        variables: Option<&JsonMap>,
    ) -> u64 {
        let (schema, document) = parse(schema_source, query);
        let operation = document.operations.iter().next().unwrap();
        MaxResponseSizeEstimator::new(&schema)
            .mode(mode)
            .estimate(
                &document,
                operation,
                list_size,
                variables.map(Valid::assume_valid_ref),
            )
            .unwrap()
    }

    const SIMPLE_SCHEMA: &str = r#"
        type Query { answer: String }
    "#;

    const NESTED_SCHEMA: &str = r#"
        type Query { node: Node nodes: [[Node]] }
        type Node { name: String }
    "#;

    #[test]
    fn one_scalar_response_field_has_size_one() {
        for mode in [AnalysisMode::Syntactic, AnalysisMode::ExactCase] {
            assert_eq!(
                estimate_query(SIMPLE_SCHEMA, "{ answer }", mode, 10, None),
                1,
            );
        }
    }

    #[test]
    fn every_list_wrapper_multiplies_selected_children() {
        for mode in [AnalysisMode::Syntactic, AnalysisMode::ExactCase] {
            assert_eq!(
                estimate_query(NESTED_SCHEMA, "{ nodes { name } }", mode, 3, None),
                10,
            );
            assert_eq!(
                estimate_query(NESTED_SCHEMA, "{ node { name } }", mode, 10, None),
                2,
            );
        }
    }

    #[test]
    fn response_fields_are_counted_at_every_nesting_level() {
        for mode in [AnalysisMode::Syntactic, AnalysisMode::ExactCase] {
            assert_eq!(
                estimate_query(
                    NESTED_SCHEMA,
                    "{ node { first: name second: name } }",
                    mode,
                    10,
                    None,
                ),
                3,
            );
        }
    }

    #[test]
    fn unrelated_list_fields_do_not_inflate_a_selected_singular_field() {
        let schema = r#"
            type Query { node: Node }
            type Node { name: String }
            type Unrelated { large: [[String]] }
        "#;
        for mode in [AnalysisMode::Syntactic, AnalysisMode::ExactCase] {
            assert_eq!(
                estimate_query(schema, "{ node { name } }", mode, 10, None),
                2,
            );
        }
    }

    #[test]
    fn complementary_boolean_siblings_are_alternatives() {
        let query = r#"
            query Example($x: Boolean!) {
              a: answer @include(if: $x)
              b: answer @skip(if: $x)
            }
        "#;
        for mode in [AnalysisMode::Syntactic, AnalysisMode::ExactCase] {
            assert_eq!(estimate_query(SIMPLE_SCHEMA, query, mode, 10, None), 1);
        }
    }

    #[test]
    fn independent_boolean_siblings_can_execute_together() {
        let query = r#"
            query Example($x: Boolean!, $y: Boolean!) {
              a: answer @include(if: $x)
              b: answer @include(if: $y)
            }
        "#;
        for mode in [AnalysisMode::Syntactic, AnalysisMode::ExactCase] {
            assert_eq!(estimate_query(SIMPLE_SCHEMA, query, mode, 10, None), 2);
            let variables = json!({ "x": false, "y": false })
                .as_object()
                .unwrap()
                .clone();
            assert_eq!(
                estimate_query(SIMPLE_SCHEMA, query, mode, 10, Some(&variables)),
                0,
            );
        }
    }

    #[test]
    fn one_boolean_assignment_is_shared_across_child_scopes() {
        let query = r#"
            query Example($x: Boolean!) {
              left: node { name @include(if: $x) }
              right: node { name @skip(if: $x) }
            }
        "#;
        assert_eq!(
            estimate_query(NESTED_SCHEMA, query, AnalysisMode::ExactCase, 10, None),
            3,
        );
        let missing = JsonMap::new();
        assert_eq!(
            estimate_query(
                NESTED_SCHEMA,
                query,
                AnalysisMode::ExactCase,
                10,
                Some(&missing),
            ),
            3,
        );
        for value in [false, true] {
            let variables = json!({ "x": value }).as_object().unwrap().clone();
            assert_eq!(
                estimate_query(
                    NESTED_SCHEMA,
                    query,
                    AnalysisMode::ExactCase,
                    10,
                    Some(&variables),
                ),
                3,
            );
        }
    }

    #[test]
    fn supplied_values_apply_operation_defaults() {
        let query = r#"
            query Example($showName: Boolean = false) {
              node { name @include(if: $showName) }
            }
        "#;
        let missing = JsonMap::new();
        for mode in [AnalysisMode::Syntactic, AnalysisMode::ExactCase] {
            assert_eq!(estimate_query(NESTED_SCHEMA, query, mode, 10, None), 2);
            assert_eq!(
                estimate_query(NESTED_SCHEMA, query, mode, 10, Some(&missing)),
                1,
            );
        }
    }

    #[test]
    fn exact_case_globally_groups_a_conditional_response_name() {
        let query = r#"
            query Example($x: Boolean!) {
              node {
                label: name
                label: name @include(if: $x)
              }
            }
        "#;
        assert_eq!(
            estimate_query(NESTED_SCHEMA, query, AnalysisMode::ExactCase, 10, None),
            2,
        );
        assert_eq!(
            estimate_query(NESTED_SCHEMA, query, AnalysisMode::Syntactic, 10, None),
            3,
        );

        let (schema, document) = parse(NESTED_SCHEMA, query);
        let operation = document.operations.get(Some("Example")).unwrap();
        assert_eq!(
            MaxResponseSizeEstimator::new(&schema)
                .estimate(&document, operation, 10, None)
                .unwrap(),
            2,
        );
    }

    #[test]
    fn response_size_saturates_on_overflow() {
        assert_eq!(
            estimate_query(
                NESTED_SCHEMA,
                "{ nodes { name } }",
                AnalysisMode::ExactCase,
                u64::MAX,
                None,
            ),
            u64::MAX,
        );
    }
}
