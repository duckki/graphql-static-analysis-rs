//! Request-level IBM GraphQL Cost Directives evaluation.

use super::model::CostModel;
use super::model::ListSize;
use super::Cost;
use super::CostError;
use crate::Algebra;
use crate::AnalysisMode;
use crate::Analyzer;
use crate::CollectedFieldGroup;
use apollo_compiler::ast::FieldDefinition;
use apollo_compiler::ast::InputValueDefinition;
use apollo_compiler::ast::Type;
use apollo_compiler::ast::Value;
use apollo_compiler::collections::HashMap;
use apollo_compiler::executable::ExecutableDocument;
use apollo_compiler::executable::Operation;
use apollo_compiler::response::JsonMap;
use apollo_compiler::response::JsonValue;
use apollo_compiler::schema::ExtendedType;
use apollo_compiler::schema::{self};
use apollo_compiler::validation::Valid;
use apollo_compiler::Name;
use apollo_compiler::Schema;
use std::rc::Rc;

/// Reusable IBM cost estimator for one schema.
pub struct CostEstimator<'schema> {
    analyzer: Analyzer<'schema>,
    cost_model: CostModel<'schema>,
    mode: AnalysisMode,
    default_list_size: Option<u64>,
}

impl<'schema> CostEstimator<'schema> {
    /// Creates an exact-case estimator and indexes the schema associated with
    /// `cost_model`.
    ///
    /// Use [`Self::mode`] to opt into the faster, potentially less precise
    /// [`AnalysisMode::Syntactic`] traversal.
    pub fn new(cost_model: CostModel<'schema>) -> Self {
        let analyzer = Analyzer::new(cost_model.schema);
        Self {
            analyzer,
            cost_model,
            mode: AnalysisMode::default(),
            default_list_size: None,
        }
    }

    /// Selects the analysis traversal. The default is [`AnalysisMode::ExactCase`].
    pub fn mode(mut self, mode: AnalysisMode) -> Self {
        self.mode = mode;
        self
    }

    /// Uses a finite bound for list fields without an applicable `@listSize` value.
    /// Without this option, such lists conservatively have infinite cost.
    pub fn default_list_size(mut self, default_list_size: u64) -> Self {
        self.default_list_size = Some(default_list_size);
        self
    }

    /// Estimates one operation using already-coerced request variables.
    pub fn estimate(
        &self,
        document: &ExecutableDocument,
        operation: &Operation,
        variable_values: &Valid<JsonMap>,
    ) -> Result<Cost, CostError> {
        let analysis = CostAlgebra {
            analyzer: &self.analyzer,
            operation,
            variable_values: variable_values.as_ref(),
            model: &self.cost_model,
            default_list_size: self
                .default_list_size
                .map(|size| size as f64)
                .unwrap_or(f64::INFINITY),
        };
        let selection_cost = self
            .analyzer
            .operation(document, operation)
            .mode(self.mode)
            .analysis_name("IBM cost")
            .variable_values(variable_values)
            .analyze(&analysis)?
            .evaluate(&[]);
        let root_cost = analysis.named_type_cost(&operation.selection_set.ty);
        Ok(Cost {
            type_cost: (root_cost + selection_cost.type_cost).max(0.0),
            field_cost: selection_cost.field_cost,
        })
    }
}

/// Convenience entry point using the IBM-required infinite fallback for unbounded lists.
///
/// This constructs a cost model and estimator for the call. For repeated estimates
/// against one schema, reuse [`CostEstimator`].
pub fn estimate(
    schema: &Schema,
    document: &ExecutableDocument,
    operation: &Operation,
    mode: AnalysisMode,
    variable_values: &Valid<JsonMap>,
) -> Result<Cost, CostError> {
    let estimator = CostEstimator::new(CostModel::from_schema(schema)?).mode(mode);
    estimator.estimate(document, operation, variable_values)
}

#[derive(Clone, Debug)]
struct SizedField {
    field_name: Name,
    size: f64,
}

type EvaluateCost<'a> = dyn Fn(&[SizedField]) -> Cost + 'a;

#[derive(Clone)]
enum CostSummary<'a> {
    Constant(Cost),
    Dynamic(Rc<EvaluateCost<'a>>),
}

impl<'a> CostSummary<'a> {
    fn evaluate(&self, sized_fields: &[SizedField]) -> Cost {
        match self {
            Self::Constant(cost) => *cost,
            Self::Dynamic(evaluate) => evaluate(sized_fields),
        }
    }
}

struct CostAlgebra<'analysis, 'schema> {
    analyzer: &'analysis Analyzer<'schema>,
    operation: &'analysis Operation,
    variable_values: &'analysis JsonMap,
    model: &'analysis CostModel<'schema>,
    default_list_size: f64,
}

impl<'analysis> Algebra for CostAlgebra<'analysis, '_> {
    type Summary = CostSummary<'analysis>;

    fn empty(&self) -> Self::Summary {
        CostSummary::Constant(Cost::ZERO)
    }

    fn field(&self, group: &CollectedFieldGroup, child_summary: Self::Summary) -> Self::Summary {
        let (empty_cost, depends_on_inherited_size) =
            self.group_cost_with_dependency(group, &child_summary, &[]);
        if !depends_on_inherited_size {
            return CostSummary::Constant(empty_cost);
        }

        let group = group.clone();
        let operation = self.operation;
        let variable_values = self.variable_values;
        let model = self.model;
        let default_list_size = self.default_list_size;
        let analyzer = self.analyzer;
        CostSummary::Dynamic(Rc::new(move |sized_fields| {
            if sized_fields.is_empty() {
                return empty_cost;
            }
            let analysis = CostAlgebra {
                analyzer,
                operation,
                variable_values,
                model,
                default_list_size,
            };
            analysis.group_cost(&group, &child_summary, sized_fields)
        }))
    }

    fn combine(&self, left: Self::Summary, right: Self::Summary) -> Self::Summary {
        match (left, right) {
            (CostSummary::Constant(left), CostSummary::Constant(right)) => {
                CostSummary::Constant(left.add(right))
            }
            (left, right) => CostSummary::Dynamic(Rc::new(move |sized_fields| {
                left.evaluate(sized_fields)
                    .add(right.evaluate(sized_fields))
            })),
        }
    }

    fn join(&self, left: Self::Summary, right: Self::Summary) -> Self::Summary {
        match (left, right) {
            (CostSummary::Constant(left), CostSummary::Constant(right)) => {
                CostSummary::Constant(left.max(right))
            }
            (left, right) => CostSummary::Dynamic(Rc::new(move |sized_fields| {
                left.evaluate(sized_fields)
                    .max(right.evaluate(sized_fields))
            })),
        }
    }

    fn requires_variables(&self) -> bool {
        true
    }
}

impl CostAlgebra<'_, '_> {
    fn group_cost(
        &self,
        group: &CollectedFieldGroup,
        child_summary: &CostSummary<'_>,
        inherited_sized_fields: &[SizedField],
    ) -> Cost {
        self.group_cost_with_dependency(group, child_summary, inherited_sized_fields)
            .0
    }

    fn group_cost_with_dependency(
        &self,
        group: &CollectedFieldGroup,
        child_summary: &CostSummary<'_>,
        inherited_sized_fields: &[SizedField],
    ) -> (Cost, bool) {
        let field = group.representative_field();
        let mut cost = Cost::ZERO;
        let mut depends_on_inherited_size = false;
        for parent_type in &group.possible_types {
            let (use_cost, is_list) =
                self.field_use_cost(parent_type, field, child_summary, inherited_sized_fields);
            cost = cost.max(use_cost);
            depends_on_inherited_size |= is_list;
        }
        (cost, depends_on_inherited_size)
    }

    fn field_use_cost(
        &self,
        parent_type: &Name,
        field: &apollo_compiler::executable::Field,
        child_summary: &CostSummary<'_>,
        inherited_sized_fields: &[SizedField],
    ) -> (Cost, bool) {
        let Ok(definition) = self.analyzer.schema().type_field(parent_type, &field.name) else {
            return (child_summary.evaluate(&[]), false);
        };
        let coordinate = (parent_type.clone(), field.name.clone());
        let list_size = self.model.list_sizes.get(&coordinate);
        let expected_size = list_size
            .and_then(|list_size| self.expected_list_size(list_size, definition, &field.arguments));
        let child_sized_fields = list_size
            .and_then(|list_size| {
                expected_size.map(|size| {
                    list_size
                        .sized_fields
                        .iter()
                        .map(|field_name| SizedField {
                            field_name: field_name.clone(),
                            size,
                        })
                        .collect::<Vec<_>>()
                })
            })
            .unwrap_or_default();
        let child_cost = child_summary.evaluate(&child_sized_fields);
        let instance_count = if definition.ty.is_list() {
            inherited_sized_fields
                .iter()
                .filter(|sized| sized.field_name == field.name)
                .map(|sized| sized.size)
                .reduce(f64::max)
                .unwrap_or_else(|| {
                    let sizes_self =
                        list_size.is_none_or(|list_size| list_size.sized_fields.is_empty());
                    sizes_self
                        .then_some(expected_size)
                        .flatten()
                        .unwrap_or(self.default_list_size)
                })
        } else {
            1.0
        };
        let output_and_children =
            self.named_type_cost(definition.ty.inner_named_type()) + child_cost.type_cost;
        (
            Cost {
                type_cost: scale(instance_count, output_and_children),
                field_cost: self.field_call_cost(parent_type, field, definition)
                    + scale(instance_count, child_cost.field_cost),
            },
            definition.ty.is_list(),
        )
    }

    fn field_call_cost(
        &self,
        parent_type: &Name,
        field: &apollo_compiler::executable::Field,
        definition: &FieldDefinition,
    ) -> f64 {
        let coordinate = (parent_type.clone(), field.name.clone());
        let own_weight = self
            .model
            .field_weights
            .get(&coordinate)
            .copied()
            .unwrap_or_else(|| self.default_weight_for_type(&definition.ty));
        let arguments_cost = definition
            .arguments
            .iter()
            .fold(0.0, |cost, argument_definition| {
                let supplied = field
                    .arguments
                    .iter()
                    .find(|argument| argument.name == argument_definition.name)
                    .map(|argument| InputValueRef::Ast(&argument.value))
                    .unwrap_or(InputValueRef::Missing);
                let coordinate = (
                    parent_type.clone(),
                    field.name.clone(),
                    argument_definition.name.clone(),
                );
                let own_weight = self
                    .model
                    .argument_weights
                    .get(&coordinate)
                    .copied()
                    .unwrap_or_else(|| self.default_weight_for_type(&argument_definition.ty));
                self.input_value_cost(supplied, argument_definition, own_weight)
                    .map_or(cost, |value_cost| cost + value_cost)
            });
        let directives_cost = field.directives.0.iter().fold(0.0, |cost, directive| {
            let Some(definition) = self
                .analyzer
                .schema()
                .directive_definitions
                .get(&directive.name)
            else {
                return cost;
            };
            cost + definition
                .arguments
                .iter()
                .fold(0.0, |cost, argument_definition| {
                    let supplied = directive
                        .arguments
                        .iter()
                        .find(|argument| argument.name == argument_definition.name)
                        .map(|argument| InputValueRef::Ast(&argument.value))
                        .unwrap_or(InputValueRef::Missing);
                    let weight = self
                        .model
                        .directive_argument_weights
                        .get(&(directive.name.clone(), argument_definition.name.clone()))
                        .copied()
                        .unwrap_or_else(|| self.default_weight_for_type(&argument_definition.ty));
                    self.input_value_cost(supplied, argument_definition, weight)
                        .map_or(cost, |value_cost| cost + value_cost)
                })
        });
        (own_weight + arguments_cost + directives_cost).max(0.0)
    }

    fn expected_list_size(
        &self,
        list_size: &ListSize,
        definition: &FieldDefinition,
        arguments: &[apollo_compiler::Node<apollo_compiler::ast::Argument>],
    ) -> Option<f64> {
        list_size
            .slicing_arguments
            .iter()
            .filter_map(|argument_name| {
                let argument_definition = definition.argument_by_name(argument_name)?;
                let supplied = arguments
                    .iter()
                    .find(|argument| argument.name == *argument_name)
                    .map(|argument| InputValueRef::Ast(argument.value.as_ref()))
                    .unwrap_or(InputValueRef::Missing);
                let value = self.effective_value(supplied, argument_definition)?;
                self.integer_value(value).map(|value| value.max(0) as f64)
            })
            .reduce(f64::max)
            .or(list_size.assumed_size.map(|size| size as f64))
    }

    fn integer_value(&self, value: InputValueRef<'_>) -> Option<i64> {
        match value {
            InputValueRef::Ast(Value::Int(value)) => value.as_str().parse().ok(),
            InputValueRef::Json(JsonValue::Number(value)) => value.as_i64(),
            _ => None,
        }
    }

    fn input_value_cost(
        &self,
        value: InputValueRef<'_>,
        definition: &InputValueDefinition,
        own_weight: f64,
    ) -> Option<f64> {
        let value = self.effective_value(value, definition)?;
        // Lean first materializes a finite effective value and then folds its cost.
        // Rust fuses those passes so nested defaults do not require an allocation.
        Some(self.resolved_input_value_cost(value, definition, own_weight))
    }

    fn resolved_input_value_cost(
        &self,
        value: InputValueRef<'_>,
        definition: &InputValueDefinition,
        own_weight: f64,
    ) -> f64 {
        match value {
            InputValueRef::Ast(Value::List(values)) => {
                own_weight
                    + values
                        .iter()
                        .map(|value| {
                            self.resolved_input_value_cost(
                                self.resolve(InputValueRef::Ast(value)),
                                definition,
                                0.0,
                            )
                        })
                        .sum::<f64>()
            }
            InputValueRef::Json(JsonValue::Array(values)) => {
                own_weight
                    + values
                        .iter()
                        .map(|value| {
                            self.resolved_input_value_cost(
                                InputValueRef::Json(value),
                                definition,
                                0.0,
                            )
                        })
                        .sum::<f64>()
            }
            InputValueRef::Ast(Value::Object(fields)) => {
                own_weight + self.ast_input_object_fields_cost(definition, fields)
            }
            InputValueRef::Json(JsonValue::Object(fields)) => {
                own_weight + self.json_input_object_fields_cost(definition, fields)
            }
            _ => own_weight,
        }
    }

    fn ast_input_object_fields_cost(
        &self,
        definition: &InputValueDefinition,
        fields: &[(Name, apollo_compiler::Node<Value>)],
    ) -> f64 {
        let Some(input_object) = self
            .analyzer
            .schema()
            .get_input_object(definition.ty.inner_named_type())
        else {
            return 0.0;
        };
        let supplied_fields: HashMap<&str, &Value> = fields
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_ref()))
            .collect();
        input_object.fields.values().fold(0.0, |cost, definition| {
            let supplied = supplied_fields
                .get(definition.name.as_str())
                .copied()
                .map(InputValueRef::Ast)
                .unwrap_or(InputValueRef::Missing);
            cost + self.input_object_field_cost(input_object, definition, supplied)
        })
    }

    fn json_input_object_fields_cost(
        &self,
        definition: &InputValueDefinition,
        fields: &JsonMap,
    ) -> f64 {
        let Some(input_object) = self
            .analyzer
            .schema()
            .get_input_object(definition.ty.inner_named_type())
        else {
            return 0.0;
        };
        input_object.fields.values().fold(0.0, |cost, definition| {
            let supplied = fields
                .get(definition.name.as_str())
                .map(InputValueRef::Json)
                .unwrap_or(InputValueRef::Missing);
            cost + self.input_object_field_cost(input_object, definition, supplied)
        })
    }

    fn input_object_field_cost(
        &self,
        input_object: &schema::InputObjectType,
        definition: &InputValueDefinition,
        value: InputValueRef<'_>,
    ) -> f64 {
        let weight = self
            .model
            .input_field_weights
            .get(&(input_object.name.clone(), definition.name.clone()))
            .copied()
            .unwrap_or_else(|| self.default_weight_for_type(&definition.ty));
        let Some(value) = self.effective_value(value, definition) else {
            return 0.0;
        };
        self.resolved_input_value_cost(value, definition, weight)
    }

    fn effective_value<'a>(
        &'a self,
        value: InputValueRef<'a>,
        definition: &'a InputValueDefinition,
    ) -> Option<InputValueRef<'a>> {
        match self.resolve(value) {
            InputValueRef::Missing => definition.default_value.as_deref().map(InputValueRef::Ast),
            value => Some(value),
        }
    }

    fn resolve<'a>(&'a self, value: InputValueRef<'a>) -> InputValueRef<'a> {
        let InputValueRef::Ast(Value::Variable(variable_name)) = value else {
            return value;
        };
        if let Some(value) = self.variable_values.get(variable_name.as_str()) {
            return InputValueRef::Json(value);
        }
        self.operation
            .variables
            .iter()
            .find(|definition| definition.name == *variable_name)
            .and_then(|definition| definition.default_value.as_deref())
            .map(InputValueRef::Ast)
            .unwrap_or(InputValueRef::Missing)
    }

    fn named_type_cost(&self, type_name: &Name) -> f64 {
        match self.analyzer.schema().types.get(type_name) {
            Some(ExtendedType::Interface(_)) | Some(ExtendedType::Union(_)) => self
                .analyzer
                .runtime_types(type_name)
                .map(|possible_type| {
                    self.model
                        .type_weights
                        .get(possible_type)
                        .copied()
                        .unwrap_or(1.0)
                })
                .reduce(f64::max)
                .unwrap_or(1.0),
            Some(definition) => self
                .model
                .type_weights
                .get(type_name)
                .copied()
                .unwrap_or_else(|| default_type_weight(definition)),
            None => 0.0,
        }
    }

    fn default_weight_for_type(&self, ty: &Type) -> f64 {
        self.analyzer
            .schema()
            .types
            .get(ty.inner_named_type())
            .map(default_type_weight)
            .unwrap_or(0.0)
    }
}

#[derive(Clone, Copy)]
enum InputValueRef<'a> {
    Ast(&'a Value),
    Json(&'a JsonValue),
    Missing,
}

fn default_type_weight(definition: &ExtendedType) -> f64 {
    match definition {
        ExtendedType::Scalar(_) | ExtendedType::Enum(_) => 0.0,
        ExtendedType::Object(_)
        | ExtendedType::Interface(_)
        | ExtendedType::Union(_)
        | ExtendedType::InputObject(_) => 1.0,
    }
}

fn scale(count: f64, cost: f64) -> f64 {
    if cost == 0.0 {
        0.0
    } else {
        count * cost
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apollo_compiler::response::serde_json_bytes::json;
    use apollo_compiler::ExecutableDocument;
    use pretty_assertions::assert_eq;

    fn valid(values: &JsonMap) -> &Valid<JsonMap> {
        Valid::assume_valid_ref(values)
    }

    const DIRECTIVES: &str = r#"
        directive @cost(weight: String!) on
          ARGUMENT_DEFINITION | ENUM | FIELD_DEFINITION |
          INPUT_FIELD_DEFINITION | OBJECT | SCALAR
        directive @listSize(
          assumedSize: Int
          slicingArguments: [String!]
          sizedFields: [String!]
          requireOneSlicingArgument: Boolean = true
        ) on FIELD_DEFINITION
    "#;

    fn parse(schema: &str, query: &str) -> (Schema, ExecutableDocument) {
        let schema =
            Schema::parse_and_validate(format!("{DIRECTIVES}\n{schema}"), "schema.graphql")
                .unwrap();
        let document =
            ExecutableDocument::parse_and_validate(&schema, query, "query.graphql").unwrap();
        (schema.into_inner(), document.into_inner())
    }

    const IBM_SCHEMA: &str = r#"
        type Query {
          book: Book
          bestsellers: [Book] @listSize(assumedSize: 5)
          newest(limit: Int): [Book] @listSize(slicingArguments: ["limit"])
          users(max: Int): [User] @listSize(slicingArguments: ["max"])
          rangeBooks(first: Int, last: Int): [Book] @listSize(
            assumedSize: 2
            slicingArguments: ["first", "last"]
            requireOneSlicingArgument: false
          )
          container(first: Int): Container @listSize(
            slicingArguments: ["first"]
            sizedFields: ["page"]
            requireOneSlicingArgument: false
          )
          defaultContainer(first: Int = 4): Container @listSize(
            slicingArguments: ["first"]
            sizedFields: ["page"]
          )
          listContainer(first: Int): [Container] @listSize(
            slicingArguments: ["first"]
            sizedFields: ["page"]
          )
          fieldWithCost(approx: Boolean @cost(weight: "-3")): Int
            @cost(weight: "5")
          publication: Publication
          inputWithCost(filter: Filter @cost(weight: "15")): Int
            @cost(weight: "5")
          listInputWithCost(ids: [ID] @cost(weight: "3")): Int
        }

        type Book implements Publication {
          title: String
          author: Author
          publisher: Publisher
        }
        type Magazine implements Publication @cost(weight: "7") { title: String }
        interface Publication { title: String }
        type Author { name: String }
        type User { age: Int @cost(weight: "2") }
        type Publisher { name: String, address: Address }
        type Address @cost(weight: "5") { zipCode: Int }
        type Container { page: [Book], recent: [Book], metadata: String }
        input Filter {
          approx: Boolean @cost(weight: "-12")
          nested: Filter
        }
    "#;

    const DEFAULTED_INPUT_SCHEMA: &str = r#"
        input DefaultedNestedFilter {
          approx: Boolean = true @cost(weight: "-12")
        }
        input DefaultedFilter {
          nested: DefaultedNestedFilter = {} @cost(weight: "4")
        }
        type Query {
          search(filter: DefaultedFilter = {} @cost(weight: "15")): Int
            @cost(weight: "5")
        }
    "#;

    const BOOK_SELECTIONS: &str = "title author { name } publisher { name address { zipCode } }";

    fn ibm_estimate(query: &str, variables: JsonMap, mode: AnalysisMode) -> Cost {
        let (schema, document) = parse(IBM_SCHEMA, query);
        let operation = document.operations.iter().next().unwrap();
        CostEstimator::new(CostModel::from_schema(&schema).unwrap())
            .mode(mode)
            .default_list_size(10)
            .estimate(&document, operation, valid(&variables))
            .unwrap()
    }

    fn exact_ibm_estimate(query: &str) -> Cost {
        ibm_estimate(query, JsonMap::new(), AnalysisMode::ExactCase)
    }

    fn negative_type_schema(query_cost: Option<i32>) -> String {
        let query_directive = query_cost
            .map(|weight| format!(" @cost(weight: \"{weight}\")"))
            .unwrap_or_default();
        format!(
            r#"
            scalar Text @cost(weight: "5")
            type Query{query_directive} {{ book: Book }}
            type Book @cost(weight: "-7") {{ title: Text, author: Author }}
            type Author {{ name: Text }}
            "#
        )
    }

    fn estimate_with_schema(
        schema_source: &str,
        query: &str,
        variables: JsonMap,
        mode: AnalysisMode,
    ) -> Cost {
        let (schema, document) = parse(schema_source, query);
        let operation = document.operations.iter().next().unwrap();
        CostEstimator::new(CostModel::from_schema(&schema).unwrap())
            .mode(mode)
            .default_list_size(10)
            .estimate(&document, operation, valid(&variables))
            .unwrap()
    }

    fn modeled_named_type_cost(schema_source: &str, type_name: &str) -> f64 {
        let (schema, document) = parse(schema_source, "{ __typename }");
        let operation = document.operations.get(None).unwrap();
        let variables = JsonMap::new();
        let analyzer = Analyzer::new(&schema);
        let model = CostModel::from_schema(&schema).unwrap();
        CostAlgebra {
            analyzer: &analyzer,
            operation,
            variable_values: &variables,
            model: &model,
            default_list_size: 10.0,
        }
        .named_type_cost(&Name::new(type_name).unwrap())
    }

    #[test]
    fn variables_are_an_explicit_estimate_input() {
        let (schema, document) = parse("type Query { value: Int }", "{ value }");
        let operation = document.operations.get(None).unwrap();
        let variables = JsonMap::new();
        let estimator = CostEstimator::new(CostModel::from_schema(&schema).unwrap());

        assert!(estimator
            .estimate(&document, operation, valid(&variables))
            .is_ok());
    }

    #[test]
    fn default_and_type_costs_match_the_lean_ibm_model() {
        let query = format!("{{ book {{ {BOOK_SELECTIONS} }} }}");

        assert_eq!(
            exact_ibm_estimate(&query),
            Cost {
                type_cost: 9.0,
                field_cost: 4.0,
            }
        );
    }

    #[test]
    fn abstract_type_uses_the_maximum_possible_object_weight() {
        assert_eq!(modeled_named_type_cost(IBM_SCHEMA, "Publication"), 7.0);
    }

    #[test]
    fn abstract_output_uses_the_maximum_possible_object_weight() {
        assert_eq!(
            exact_ibm_estimate("{ publication { title } }"),
            Cost {
                type_cost: 8.0,
                field_cost: 1.0,
            }
        );
    }

    #[test]
    fn negative_type_weight_is_preserved() {
        assert_eq!(
            modeled_named_type_cost(&negative_type_schema(None), "Book"),
            -7.0
        );
    }

    #[test]
    fn negative_type_weight_offsets_child_cost() {
        assert_eq!(
            estimate_with_schema(
                &negative_type_schema(None),
                "{ book { title } }",
                JsonMap::new(),
                AnalysisMode::ExactCase,
            ),
            Cost {
                type_cost: 1.0,
                field_cost: 1.0,
            }
        );
    }

    #[test]
    fn negative_root_type_weight_is_clamped_at_zero() {
        let schema = r#"
            scalar Text
            type Query @cost(weight: "-3") { value: Text }
        "#;

        for mode in [AnalysisMode::Syntactic, AnalysisMode::ExactCase] {
            assert_eq!(
                estimate_with_schema(schema, "{ value }", JsonMap::new(), mode),
                Cost::ZERO,
            );
        }
    }

    #[test]
    fn syntactic_analysis_preserves_signed_type_costs() {
        assert_eq!(
            estimate_with_schema(
                &negative_type_schema(None),
                "{ book { title } }",
                JsonMap::new(),
                AnalysisMode::Syntactic,
            ),
            Cost {
                type_cost: 1.0,
                field_cost: 1.0,
            }
        );
    }

    const SPLIT_CONDITIONAL_BOOK_QUERY: &str = r#"
        query Example($a: Boolean!, $b: Boolean!) {
          book @include(if: $a) { title }
          book @include(if: $b) { author { name } }
        }
    "#;

    #[test]
    fn syntactic_negative_type_cost_does_not_fall_back_to_exact_cases() {
        let variables = json!({ "a": true, "b": true }).as_object().unwrap().clone();

        assert_eq!(
            estimate_with_schema(
                &negative_type_schema(Some(0)),
                SPLIT_CONDITIONAL_BOOK_QUERY,
                variables,
                AnalysisMode::Syntactic,
            ),
            Cost {
                type_cost: 0.0,
                field_cost: 3.0,
            }
        );
    }

    #[test]
    fn exact_cases_preserve_signed_type_costs_when_collecting_fields() {
        let variables = json!({ "a": true, "b": true }).as_object().unwrap().clone();

        assert_eq!(
            estimate_with_schema(
                &negative_type_schema(Some(0)),
                SPLIT_CONDITIONAL_BOOK_QUERY,
                variables,
                AnalysisMode::ExactCase,
            ),
            Cost {
                type_cost: 4.0,
                field_cost: 2.0,
            }
        );
    }

    #[test]
    fn assumed_list_size_matches_the_lean_ibm_model() {
        let query = format!("{{ bestsellers {{ {BOOK_SELECTIONS} }} }}");

        assert_eq!(
            exact_ibm_estimate(&query),
            Cost {
                type_cost: 41.0,
                field_cost: 16.0,
            }
        );
    }

    #[test]
    fn numeric_slicing_argument_matches_the_lean_ibm_model() {
        let query = format!("{{ newest(limit: 3) {{ {BOOK_SELECTIONS} }} }}");

        assert_eq!(
            exact_ibm_estimate(&query),
            Cost {
                type_cost: 25.0,
                field_cost: 10.0,
            }
        );
    }

    #[test]
    fn operation_variable_default_is_used_for_slicing() {
        let query = format!(
            "query Example($limit: Int = 3) {{ newest(limit: $limit) {{ {BOOK_SELECTIONS} }} }}"
        );

        assert_eq!(
            exact_ibm_estimate(&query),
            Cost {
                type_cost: 25.0,
                field_cost: 10.0,
            }
        );
    }

    #[test]
    fn sized_field_matches_the_lean_ibm_model() {
        assert_eq!(
            exact_ibm_estimate("{ container(first: 3) { page { title } recent { title } } }"),
            Cost {
                type_cost: 15.0,
                field_cost: 3.0,
            }
        );
    }

    #[test]
    fn slicing_argument_uses_schema_default() {
        assert_eq!(
            exact_ibm_estimate("{ defaultContainer { page { title } } }"),
            Cost {
                type_cost: 6.0,
                field_cost: 2.0,
            }
        );
    }

    #[test]
    fn slicing_argument_uses_schema_default_for_undefined_variable() {
        assert_eq!(
            exact_ibm_estimate(
                "query Example($first: Int) { defaultContainer(first: $first) { page { title } } }"
            ),
            Cost {
                type_cost: 6.0,
                field_cost: 2.0,
            }
        );
    }

    #[test]
    fn explicit_null_slicing_argument_suppresses_schema_default() {
        assert_eq!(
            exact_ibm_estimate("{ defaultContainer(first: null) { page { title } } }"),
            Cost {
                type_cost: 12.0,
                field_cost: 2.0,
            }
        );
    }

    #[test]
    fn multiple_slicing_arguments_use_their_maximum() {
        assert_eq!(
            exact_ibm_estimate("{ rangeBooks(first: 3, last: 5) { title } }"),
            Cost {
                type_cost: 6.0,
                field_cost: 1.0,
            }
        );
    }

    #[test]
    fn slicing_argument_takes_precedence_over_assumed_size() {
        assert_eq!(
            exact_ibm_estimate("{ rangeBooks(first: 1) { title } }"),
            Cost {
                type_cost: 2.0,
                field_cost: 1.0,
            }
        );
    }

    #[test]
    fn sized_fields_redirect_size_from_the_annotated_list() {
        assert_eq!(
            exact_ibm_estimate("{ listContainer(first: 3) { page { title } } }"),
            Cost {
                type_cost: 41.0,
                field_cost: 11.0,
            }
        );
    }

    #[test]
    fn field_call_cost_is_not_multiplied() {
        assert_eq!(
            exact_ibm_estimate("{ fieldWithCost }"),
            Cost {
                type_cost: 1.0,
                field_cost: 5.0,
            }
        );
    }

    #[test]
    fn negative_argument_reduces_field_cost_before_clamping() {
        assert_eq!(
            exact_ibm_estimate("{ fieldWithCost(approx: true) }"),
            Cost {
                type_cost: 1.0,
                field_cost: 2.0,
            }
        );
    }

    #[test]
    fn undefined_weighted_inputs_are_omitted_but_null_is_present() {
        let cases = [
            (
                "query Example($approx: Boolean) { fieldWithCost(approx: $approx) }",
                5.0,
                2.0,
            ),
            (
                "query Example($approx: Boolean) { inputWithCost(filter: { approx: $approx }) }",
                20.0,
                8.0,
            ),
        ];

        for mode in [AnalysisMode::Syntactic, AnalysisMode::ExactCase] {
            for (query, undefined_cost, null_cost) in cases {
                assert_eq!(
                    ibm_estimate(query, JsonMap::new(), mode).field_cost,
                    undefined_cost,
                );
                assert_eq!(
                    ibm_estimate(
                        query,
                        json!({ "approx": null }).as_object().unwrap().clone(),
                        mode,
                    )
                    .field_cost,
                    null_cost,
                );
            }
        }
    }

    #[test]
    fn signed_input_cost_is_clamped_at_the_field_boundary() {
        assert_eq!(
            exact_ibm_estimate("{ inputWithCost(filter: { approx: true }) }"),
            Cost {
                type_cost: 1.0,
                field_cost: 8.0,
            }
        );
    }

    #[test]
    fn recursive_input_object_cost_terminates() {
        assert_eq!(
            exact_ibm_estimate("{ inputWithCost(filter: { nested: { approx: true } }) }"),
            Cost {
                type_cost: 1.0,
                field_cost: 9.0,
            }
        );
    }

    #[test]
    fn omitted_undefined_and_supplied_objects_materialize_nested_defaults() {
        let cases = [
            ("{ search }", JsonMap::new()),
            (
                "query Example($filter: DefaultedFilter) { search(filter: $filter) }",
                JsonMap::new(),
            ),
            (
                "query Example($filter: DefaultedFilter) { search(filter: $filter) }",
                json!({ "filter": {} }).as_object().unwrap().clone(),
            ),
        ];

        for mode in [AnalysisMode::Syntactic, AnalysisMode::ExactCase] {
            for (query, variables) in &cases {
                assert_eq!(
                    estimate_with_schema(DEFAULTED_INPUT_SCHEMA, query, variables.clone(), mode),
                    Cost {
                        type_cost: 1.0,
                        field_cost: 12.0,
                    },
                );
            }
        }
    }

    #[test]
    fn nested_undefined_uses_default_but_explicit_null_suppresses_it() {
        let cases = [
            (
                "query Example($nested: DefaultedNestedFilter) { search(filter: { nested: $nested }) }",
                12.0,
            ),
            ("{ search(filter: { nested: null }) }", 24.0),
            ("{ search(filter: null) }", 20.0),
        ];

        for mode in [AnalysisMode::Syntactic, AnalysisMode::ExactCase] {
            for (query, field_cost) in cases {
                assert_eq!(
                    estimate_with_schema(DEFAULTED_INPUT_SCHEMA, query, JsonMap::new(), mode)
                        .field_cost,
                    field_cost,
                );
            }
        }
    }

    #[test]
    fn supplied_null_overrides_an_operation_default() {
        let query = r#"
            query Example($filter: DefaultedFilter = { nested: null }) {
              search(filter: $filter)
            }
        "#;

        for mode in [AnalysisMode::Syntactic, AnalysisMode::ExactCase] {
            assert_eq!(
                estimate_with_schema(DEFAULTED_INPUT_SCHEMA, query, JsonMap::new(), mode)
                    .field_cost,
                24.0,
            );
            assert_eq!(
                estimate_with_schema(
                    DEFAULTED_INPUT_SCHEMA,
                    query,
                    json!({ "filter": null }).as_object().unwrap().clone(),
                    mode,
                )
                .field_cost,
                20.0,
            );
        }
    }

    #[test]
    fn list_argument_weight_is_paid_once() {
        assert_eq!(
            exact_ibm_estimate("{ listInputWithCost(ids: [\"a\", \"b\", \"c\"]) }"),
            Cost {
                type_cost: 1.0,
                field_cost: 3.0,
            }
        );
    }

    #[test]
    fn duplicate_collected_field_costs_once() {
        assert_eq!(
            exact_ibm_estimate("{ book { title } book { title } }"),
            Cost {
                type_cost: 2.0,
                field_cost: 1.0,
            }
        );
    }

    #[test]
    fn representative_field_uses_equivalent_reordered_arguments() {
        let query = r#"
            {
              books: rangeBooks(first: 3, last: 5) { title }
              books: rangeBooks(last: 5, first: 3) { title }
            }
        "#;

        for mode in [AnalysisMode::Syntactic, AnalysisMode::ExactCase] {
            assert_eq!(
                ibm_estimate(query, JsonMap::new(), mode),
                Cost {
                    type_cost: 6.0,
                    field_cost: 1.0,
                }
            );
        }
    }

    const DEFAULT_PRUNED_QUERY: &str = r#"
        query Example($skipBook: Boolean = true) {
          book @skip(if: $skipBook) {
            title
            author { name }
            publisher { name address { zipCode } }
          }
        }
    "#;

    #[test]
    fn operation_variable_default_prunes_exact_case_cost() {
        assert_eq!(
            ibm_estimate(
                DEFAULT_PRUNED_QUERY,
                JsonMap::new(),
                AnalysisMode::ExactCase,
            ),
            Cost {
                type_cost: 1.0,
                field_cost: 0.0,
            }
        );
    }

    #[test]
    fn operation_variable_default_prunes_syntactic_cost() {
        assert_eq!(
            ibm_estimate(
                DEFAULT_PRUNED_QUERY,
                JsonMap::new(),
                AnalysisMode::Syntactic,
            ),
            Cost {
                type_cost: 1.0,
                field_cost: 0.0,
            }
        );
    }

    #[test]
    fn ibm_users_example_costs_eleven() {
        let (schema, document) = parse(
            r#"
            type User { age: Int @cost(weight: "2.0") }
            type Query {
              users(max: Int): [User] @listSize(slicingArguments: ["max"])
            }
            "#,
            "query Example($max: Int!) { users(max: $max) { age } }",
        );
        let operation = document.operations.get(Some("Example")).unwrap();
        let variables = json!({ "max": 5 }).as_object().unwrap().clone();
        let estimator = CostEstimator::new(CostModel::from_schema(&schema).unwrap());
        let cost = estimator
            .estimate(&document, operation, valid(&variables))
            .unwrap();
        assert_eq!(
            cost,
            Cost {
                type_cost: 6.0,
                field_cost: 11.0,
            }
        );
    }

    #[test]
    fn argument_and_input_field_weights_are_applied() {
        let (schema, document) = parse(
            r#"
            input Filter { approximate: Boolean @cost(weight: "-12.0") }
            type Query {
              products(filter: Filter @cost(weight: "15.0")): String
                @cost(weight: "5.0")
            }
            "#,
            "query Example($filter: Filter) { products(filter: $filter) }",
        );
        let operation = document.operations.get(Some("Example")).unwrap();
        let variables = json!({ "filter": { "approximate": true } })
            .as_object()
            .unwrap()
            .clone();
        let estimator = CostEstimator::new(CostModel::from_schema(&schema).unwrap());
        let cost = estimator
            .estimate(&document, operation, valid(&variables))
            .unwrap();
        assert_eq!(cost.field_cost, 8.0);
    }

    #[test]
    fn sized_fields_apply_a_parent_slice_to_a_child_list() {
        let (schema, document) = parse(
            r#"
            type Edge { node: String }
            type Connection { edges: [Edge] }
            type Query {
              items(first: Int): Connection @listSize(
                slicingArguments: ["first"]
                sizedFields: ["edges"]
              )
            }
            "#,
            "query Example($first: Int!) { items(first: $first) { edges { node } } }",
        );
        let operation = document.operations.get(Some("Example")).unwrap();
        let variables = json!({ "first": 3 }).as_object().unwrap().clone();
        for mode in [AnalysisMode::Syntactic, AnalysisMode::ExactCase] {
            let estimator = CostEstimator::new(CostModel::from_schema(&schema).unwrap()).mode(mode);
            let cost = estimator
                .estimate(&document, operation, valid(&variables))
                .unwrap();
            assert_eq!(cost.type_cost, 5.0);
            assert_eq!(cost.field_cost, 2.0);
        }
    }

    #[test]
    fn custom_directive_argument_weights_affect_field_cost() {
        let (schema, document) = parse(
            r#"
            directive @approx(tolerance: Float! @cost(weight: "-1.0")) on FIELD
            type Query { value: String @cost(weight: "5.0") }
            "#,
            "{ value @approx(tolerance: 0.5) }",
        );
        let operation = document.operations.get(None).unwrap();
        let variables = JsonMap::new();
        let estimator = CostEstimator::new(CostModel::from_schema(&schema).unwrap());
        let cost = estimator
            .estimate(&document, operation, valid(&variables))
            .unwrap();
        assert_eq!(cost.field_cost, 4.0);
    }

    #[test]
    fn operation_and_schema_argument_defaults_bound_lists() {
        let (schema, document) = parse(
            r#"
            type User { name: String }
            type Query {
              users(first: Int = 3): [User] @listSize(slicingArguments: "first")
            }
            "#,
            r#"
            query Example($first: Int = 4) {
              schemaDefault: users { name }
              operationDefault: users(first: $first) { name }
            }
            "#,
        );
        let operation = document.operations.get(Some("Example")).unwrap();
        let variables = JsonMap::new();
        let estimator = CostEstimator::new(CostModel::from_schema(&schema).unwrap());
        let cost = estimator
            .estimate(&document, operation, valid(&variables))
            .unwrap();
        assert_eq!(cost.type_cost, 8.0);
    }

    #[test]
    fn schema_argument_default_applies_when_an_optional_variable_is_undefined() {
        let (schema, document) = parse(
            r#"
            type User { name: String }
            type Query {
              users(first: Int = 3): [User] @listSize(slicingArguments: ["first"])
            }
            "#,
            "query Example($first: Int) { users(first: $first) { name } }",
        );
        let operation = document.operations.get(Some("Example")).unwrap();
        let variables = JsonMap::new();
        let estimator = CostEstimator::new(CostModel::from_schema(&schema).unwrap());
        let cost = estimator
            .estimate(&document, operation, valid(&variables))
            .unwrap();

        assert_eq!(
            cost,
            Cost {
                type_cost: 4.0,
                field_cost: 1.0,
            }
        );
    }

    #[test]
    fn syntactic_type_products_merge_response_names_across_redundant_types() {
        let (schema, document) = parse(
            r#"
            interface Node { id: ID! }
            interface S0 implements Node { id: ID! included: String skipped: String }
            interface S1 implements Node { id: ID! included: String skipped: String }
            interface S2 implements Node { id: ID! included: String skipped: String }
            interface S3 implements Node { id: ID! included: String skipped: String }
            type A implements Node & S0 & S1 & S2 & S3 {
              id: ID!
              included: String @cost(weight: "1")
              skipped: String @cost(weight: "7")
            }
            type B implements Node & S0 & S1 & S2 & S3 {
              id: ID!
              included: String @cost(weight: "1")
              skipped: String @cost(weight: "7")
            }
            type Query { node: Node }
            "#,
            r#"
            query Example($includeBranch: Boolean!, $skipBranch: Boolean!) {
              node {
                ... on S0 @include(if: $includeBranch) { shared: included }
                ... on S1 @skip(if: $skipBranch) { ignored: skipped }
                ... on S2 @include(if: $includeBranch) { shared: included }
                ... on S3 @skip(if: $skipBranch) { ignored: skipped }
              }
            }
            "#,
        );
        let operation = document.operations.get(Some("Example")).unwrap();
        let variables = json!({ "includeBranch": true, "skipBranch": true })
            .as_object()
            .unwrap()
            .clone();
        let model = CostModel::from_schema(&schema).unwrap();

        let syntactic = CostEstimator::new(model.clone())
            .mode(AnalysisMode::Syntactic)
            .estimate(&document, operation, valid(&variables))
            .unwrap();
        let exact = CostEstimator::new(model)
            .mode(AnalysisMode::ExactCase)
            .estimate(&document, operation, valid(&variables))
            .unwrap();

        assert_eq!(
            syntactic,
            Cost {
                type_cost: 2.0,
                field_cost: 2.0,
            }
        );
        assert_eq!(
            exact,
            Cost {
                type_cost: 2.0,
                field_cost: 2.0,
            }
        );
    }

    #[test]
    fn estimator_can_be_reused_across_operations() {
        let (schema, document) = parse(
            "type Query { value: String @cost(weight: \"2.0\") }",
            "query First { value } query Second { value }",
        );
        let variables = JsonMap::new();
        let model = CostModel::from_schema(&schema).unwrap();
        let estimator = CostEstimator::new(model);

        for operation_name in ["First", "Second"] {
            let operation = document.operations.get(Some(operation_name)).unwrap();
            let cost = estimator
                .estimate(&document, operation, valid(&variables))
                .unwrap();

            assert_eq!(cost.field_cost, 2.0);
        }
    }
}
