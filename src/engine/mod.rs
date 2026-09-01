//! The `graphql-static-analysis` engine follows the condition-tree-summary split used
//! by the Lean model:
//! selection conditions are framework-owned, while analyses only define how to
//! summarize a collected response-name group and how to compose simultaneous and
//! alternative summaries.

use apollo_compiler::ast::Value;
use apollo_compiler::collections::HashMap;
use apollo_compiler::executable::ExecutableDocument;
use apollo_compiler::executable::Operation;
use apollo_compiler::executable::{self};
use apollo_compiler::response::JsonMap;
use apollo_compiler::response::JsonValue;
use apollo_compiler::schema::ExtendedType;
use apollo_compiler::validation::Valid;
use apollo_compiler::Name;
use apollo_compiler::Node;
use apollo_compiler::Schema;
use fixedbitset::FixedBitSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::ops::Deref;
use std::sync::Arc;

mod condition_tree;
mod exact_cases;
mod syntactic;

/// Selects the condition-tree traversal used by [`Analyzer`].
///
/// [`AnalysisMode::ExactCase`] is the default because it provides the optimal
/// condition-aware analysis. Select [`AnalysisMode::Syntactic`] explicitly when
/// lower analysis cost is worth a potentially less precise result.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AnalysisMode {
    /// Enumerates feasible type-branch compatibility regions and groups every field
    /// that executes together under the same response name.
    #[default]
    ExactCase,

    /// A fast, conservative traversal that keeps fields from different syntactic
    /// condition-tree nodes in separate response-name groups. Equivalent cumulative
    /// conditions share one canonical node.
    Syntactic,
}

/// A Boolean condition contributed by `@include` or `@skip`.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct BooleanLiteral {
    pub variable_name: Name,
    pub required_value: bool,
}

/// The fields collected for one response name in one traversal case.
///
/// `fields` is non-empty. Its selections have already passed the framework's
/// runtime-type and Boolean-condition filtering. Child selection sets are merged by
/// the engine before their summary is passed to [`Algebra::field`].
#[derive(Clone, Debug)]
pub struct CollectedFieldGroup {
    /// Concrete runtime object types represented by this group.
    pub possible_types: Vec<Name>,

    /// Boolean condition inherited from the parent response field.
    ///
    /// Exact-case traversal also records the selected case assignment here before
    /// invoking the algebra.
    pub inherited_boolean_condition: Vec<BooleanLiteral>,

    /// The local condition-tree node's Boolean condition. Exact-case groups have an
    /// empty local condition because the case assignment has already been resolved.
    pub boolean_condition: Vec<BooleanLiteral>,

    /// Field occurrences sharing a response name.
    fields: Vec<Node<executable::Field>>,
}

impl CollectedFieldGroup {
    /// Returns a representative field occurrence from this non-empty group.
    ///
    /// GraphQL field-merging validation guarantees that fields collected under one
    /// response name and applicable to the same runtime object have the same field
    /// name and equivalent arguments. Analyses can therefore use this occurrence for
    /// resolver lookup and argument evaluation.
    pub fn representative_field(&self) -> &Node<executable::Field> {
        &self.fields[0]
    }

    /// Returns the shared response name.
    pub fn response_name(&self) -> &Name {
        self.representative_field().response_key()
    }

    /// Returns the non-empty list of collected field occurrences.
    pub fn fields(&self) -> &[Node<executable::Field>] {
        &self.fields
    }

    /// Returns the canonical Boolean condition inherited by merged child selections.
    pub fn child_inherited_boolean_condition(&self) -> Vec<BooleanLiteral> {
        let mut condition = self.inherited_boolean_condition.clone();
        condition.extend(self.boolean_condition.iter().cloned());
        condition_tree::canonical_boolean_condition(condition)
            .unwrap_or_else(|| self.inherited_boolean_condition.clone())
    }
}

/// A condition-tree summary algebra.
///
/// Condition handling, field collection, child traversal, and variable resolution
/// belong to the engine. An algebra supplies only the local field calculation,
/// simultaneous composition, and alternative join.
///
/// Sound upper-bound analyses should make `combine` an associative and commutative
/// operation with `empty` as its identity and least value. `join(left, right)` must
/// bound both inputs, and `combine` must be monotone with respect to that bound. The
/// syntactic backend evaluates each distinct materializable type-condition product;
/// it does not compare summary values or require `join` to be idempotent. Exact-case
/// factoring additionally assumes that delaying `combine` and `field` across a
/// `join` remains a sound upper bound.
pub trait Algebra {
    /// A summary can be reused across multiple runtime alternatives. Implementations
    /// should therefore make cloning cheap (for example, an `Rc`-backed expression).
    type Summary: Clone;

    fn empty(&self) -> Self::Summary;

    fn field(&self, group: &CollectedFieldGroup, child_summary: Self::Summary) -> Self::Summary;

    fn combine(&self, left: Self::Summary, right: Self::Summary) -> Self::Summary;

    fn join(&self, left: Self::Summary, right: Self::Summary) -> Self::Summary;

    /// Whether this analysis needs a concrete request-variable map.
    fn requires_variables(&self) -> bool {
        false
    }
}

#[derive(Clone, Debug, thiserror::Error, Eq, PartialEq)]
pub enum AnalysisError {
    #[error("the `{analysis}` analysis requires variable values")]
    VariablesRequired { analysis: &'static str },
}

/// Reusable schema-level analysis engine.
///
/// Construction indexes the schema's possible runtime object types once. Reuse this
/// value across operations analyzed against the same schema.
pub struct Analyzer<'schema> {
    schema: &'schema Schema,
    possible_types: PossibleTypesMap,
}

impl<'schema> Analyzer<'schema> {
    pub fn new(schema: &'schema Schema) -> Self {
        Self {
            schema,
            possible_types: build_possible_types(schema),
        }
    }

    /// Returns the schema indexed by this engine.
    pub fn schema(&self) -> &'schema Schema {
        self.schema
    }

    /// Configures an analysis of one operation.
    pub fn operation<'analysis>(
        &'analysis self,
        document: &'analysis ExecutableDocument,
        operation: &'analysis Operation,
    ) -> Analysis<'analysis, 'schema> {
        Analysis {
            analyzer: self,
            document,
            operation,
            mode: AnalysisMode::default(),
            variable_values: None,
            analysis_name: "static",
        }
    }

    pub(crate) fn runtime_types<'analysis>(
        &'analysis self,
        type_name: &Name,
    ) -> impl Iterator<Item = &'analysis Name> + 'analysis {
        self.possible_types.names(type_name)
    }
}

/// Configures and runs one operation analysis.
pub struct Analysis<'analysis, 'schema> {
    analyzer: &'analysis Analyzer<'schema>,
    document: &'analysis ExecutableDocument,
    operation: &'analysis Operation,
    mode: AnalysisMode,
    variable_values: Option<&'analysis Valid<JsonMap>>,
    analysis_name: &'static str,
}

impl<'analysis> Analysis<'analysis, '_> {
    pub fn mode(mut self, mode: AnalysisMode) -> Self {
        self.mode = mode;
        self
    }

    /// Supplies request variables produced by GraphQL variable coercion.
    pub fn variable_values(mut self, variable_values: &'analysis Valid<JsonMap>) -> Self {
        self.variable_values = Some(variable_values);
        self
    }

    /// Sets the name used if an algebra rejects missing variable values.
    pub fn analysis_name(mut self, analysis_name: &'static str) -> Self {
        self.analysis_name = analysis_name;
        self
    }

    pub fn analyze<A: Algebra>(&self, algebra: &A) -> Result<A::Summary, AnalysisError> {
        if algebra.requires_variables() && self.variable_values.is_none() {
            return Err(AnalysisError::VariablesRequired {
                analysis: self.analysis_name,
            });
        }

        let variables =
            VariableEnvironment::new(self.operation, self.variable_values.map(Valid::as_ref));
        match self.mode {
            AnalysisMode::Syntactic => Ok(syntactic::summarize(
                self.analyzer.schema,
                self.document,
                self.operation,
                algebra,
                variables,
                &self.analyzer.possible_types,
            )),
            AnalysisMode::ExactCase => Ok(exact_cases::summarize(
                self.analyzer.schema,
                self.document,
                self.operation,
                algebra,
                variables,
                &self.analyzer.possible_types,
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BooleanValue {
    Missing,
    Known(bool),
    Unknown,
}

struct VariableEnvironment<'a> {
    operation: &'a Operation,
    supplied: Option<&'a JsonMap>,
}

impl<'a> VariableEnvironment<'a> {
    fn new(operation: &'a Operation, supplied: Option<&'a JsonMap>) -> Self {
        Self {
            operation,
            supplied,
        }
    }

    fn boolean(&self, name: &Name) -> BooleanValue {
        let Some(supplied) = self.supplied else {
            return BooleanValue::Unknown;
        };
        if let Some(value) = supplied.get(name.as_str()) {
            return match value {
                JsonValue::Bool(value) => BooleanValue::Known(*value),
                JsonValue::Null => BooleanValue::Missing,
                _ => BooleanValue::Missing,
            };
        }
        match self
            .operation
            .variables
            .iter()
            .find(|definition| definition.name == *name)
            .and_then(|definition| definition.default_value.as_deref())
        {
            Some(Value::Boolean(value)) => BooleanValue::Known(*value),
            _ => BooleanValue::Missing,
        }
    }

    fn is_complete(&self) -> bool {
        self.supplied.is_some()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PossibleTypeSet(Arc<PossibleTypeSetData>);

impl Hash for PossibleTypeSet {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.fingerprint.hash(state);
    }
}

#[derive(Debug, Eq, PartialEq)]
struct PossibleTypeSetData {
    /// Membership is queried for every field occurrence and runtime-object case.
    bits: FixedBitSet,
    /// Retains schema order so traversal remains deterministic for arbitrary algebras.
    ordered: Vec<usize>,
    /// Cached semantic hash used by canonical condition-tree extraction.
    fingerprint: u64,
}

impl Deref for PossibleTypeSet {
    type Target = PossibleTypeSetData;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl PossibleTypeSet {
    fn empty(object_count: usize) -> Self {
        Self(Arc::new(PossibleTypeSetData {
            bits: FixedBitSet::with_capacity(object_count),
            ordered: Vec::new(),
            fingerprint: possible_type_fingerprint(&[]),
        }))
    }

    fn from_names<'a>(
        names: impl IntoIterator<Item = &'a Name>,
        object_indices: &HashMap<Name, usize>,
        object_count: usize,
    ) -> Self {
        let mut bits = FixedBitSet::with_capacity(object_count);
        let mut ordered = Vec::new();
        for name in names {
            let Some(&index) = object_indices.get(name) else {
                continue;
            };
            if !bits.contains(index) {
                bits.insert(index);
                ordered.push(index);
            }
        }
        let fingerprint = possible_type_fingerprint(&ordered);
        Self(Arc::new(PossibleTypeSetData {
            bits,
            ordered,
            fingerprint,
        }))
    }

    fn intersection(&self, other: &Self) -> Self {
        let ordered = self
            .ordered
            .iter()
            .copied()
            .filter(|&index| other.bits.contains(index))
            .collect::<Vec<_>>();
        if ordered.len() == self.ordered.len() {
            return self.clone();
        }
        let mut bits = FixedBitSet::with_capacity(self.bits.len());
        bits.extend(ordered.iter().copied());
        let fingerprint = possible_type_fingerprint(&ordered);
        Self(Arc::new(PossibleTypeSetData {
            bits,
            ordered,
            fingerprint,
        }))
    }

    fn is_empty(&self) -> bool {
        self.ordered.is_empty()
    }
}

/// A nonempty equivalence class of runtime object types that activate the same type
/// conditions. The stored indices preserve schema order.
#[derive(Clone, Debug)]
struct PossibleTypeRegion {
    ordered: Vec<usize>,
}

impl From<&PossibleTypeSet> for PossibleTypeRegion {
    fn from(possible_types: &PossibleTypeSet) -> Self {
        Self {
            ordered: possible_types.ordered.clone(),
        }
    }
}

/// Mirrors Lean's `possibleTypeRegions`: each condition splits the current nonempty
/// regions, so one representative denotes one materializable activation product.
fn possible_type_regions<'a>(
    scope: &PossibleTypeRegion,
    conditions: impl IntoIterator<Item = &'a PossibleTypeSet>,
) -> Vec<PossibleTypeRegion> {
    if scope.ordered.is_empty() {
        return Vec::new();
    }

    let mut regions = vec![scope.clone()];
    for allowed in conditions {
        let mut refined = Vec::with_capacity(regions.len().saturating_mul(2));
        for region in regions {
            split_possible_type_region(region, allowed, &mut refined);
        }
        regions = refined;
    }
    regions
}

fn split_possible_type_region(
    mut region: PossibleTypeRegion,
    allowed: &PossibleTypeSet,
    output: &mut Vec<PossibleTypeRegion>,
) {
    let included_count = region
        .ordered
        .iter()
        .filter(|&&index| allowed.bits.contains(index))
        .count();
    if included_count == 0 || included_count == region.ordered.len() {
        output.push(region);
        return;
    }

    let excluded_count = region.ordered.len() - included_count;
    if included_count <= excluded_count {
        let mut included = Vec::with_capacity(included_count);
        region.ordered.retain(|&index| {
            if allowed.bits.contains(index) {
                included.push(index);
                false
            } else {
                true
            }
        });
        output.push(PossibleTypeRegion { ordered: included });
        output.push(region);
    } else {
        let mut excluded = Vec::with_capacity(excluded_count);
        region.ordered.retain(|&index| {
            if allowed.bits.contains(index) {
                true
            } else {
                excluded.push(index);
                false
            }
        });
        output.push(region);
        output.push(PossibleTypeRegion { ordered: excluded });
    }
}

fn possible_type_fingerprint(ordered: &[usize]) -> u64 {
    let mut hasher = DefaultHasher::new();
    ordered.hash(&mut hasher);
    hasher.finish()
}

#[derive(Clone, Debug)]
pub(crate) struct PossibleTypesMap {
    object_names: Vec<Name>,
    by_type: HashMap<Name, PossibleTypeSet>,
}

impl PossibleTypesMap {
    fn get(&self, type_name: &Name) -> Option<&PossibleTypeSet> {
        self.by_type.get(type_name)
    }

    fn object_name(&self, index: usize) -> &Name {
        &self.object_names[index]
    }

    fn intersection(&self, possible_types: &PossibleTypeSet, type_name: &Name) -> PossibleTypeSet {
        self.get(type_name)
            .map(|condition_types| possible_types.intersection(condition_types))
            .unwrap_or_else(|| PossibleTypeSet::empty(self.object_names.len()))
    }

    pub(crate) fn names<'a>(&'a self, type_name: &Name) -> impl Iterator<Item = &'a Name> + 'a {
        self.by_type
            .get(type_name)
            .into_iter()
            .flat_map(|possible_types| possible_types.ordered.iter())
            .map(|&index| &self.object_names[index])
    }
}

pub(crate) fn build_possible_types(schema: &Schema) -> PossibleTypesMap {
    let implementers = schema.implementers_map();
    let object_names = schema
        .types
        .iter()
        .filter(|(_type_name, definition)| matches!(definition, ExtendedType::Object(_)))
        .map(|(type_name, _definition)| type_name.clone())
        .collect::<Vec<_>>();
    let object_indices = object_names
        .iter()
        .enumerate()
        .map(|(index, name)| (name.clone(), index))
        .collect::<HashMap<_, _>>();
    let by_type = schema
        .types
        .iter()
        .filter_map(|(type_name, definition)| {
            let possible_types = match definition {
                ExtendedType::Object(_) => PossibleTypeSet::from_names(
                    std::iter::once(type_name),
                    &object_indices,
                    object_names.len(),
                ),
                ExtendedType::Union(union) => PossibleTypeSet::from_names(
                    union.members.iter().map(|member| &member.name),
                    &object_indices,
                    object_names.len(),
                ),
                ExtendedType::Interface(_) => PossibleTypeSet::from_names(
                    implementers
                        .get(type_name)
                        .into_iter()
                        .flat_map(|implementers| implementers.objects.iter()),
                    &object_indices,
                    object_names.len(),
                ),
                _ => return None,
            };
            Some((type_name.clone(), possible_types))
        })
        .collect();
    PossibleTypesMap {
        object_names,
        by_type,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apollo_compiler::ExecutableDocument;

    fn valid(values: &JsonMap) -> &Valid<JsonMap> {
        Valid::assume_valid_ref(values)
    }

    #[derive(Clone, Debug, Default, Eq, PartialEq)]
    struct FieldCount(usize);

    struct Count;

    #[test]
    fn exact_case_is_the_default_analysis_mode() {
        assert_eq!(AnalysisMode::default(), AnalysisMode::ExactCase);
    }

    impl Algebra for Count {
        type Summary = FieldCount;

        fn empty(&self) -> Self::Summary {
            FieldCount(0)
        }

        fn field(
            &self,
            _group: &CollectedFieldGroup,
            child_summary: Self::Summary,
        ) -> Self::Summary {
            FieldCount(1 + child_summary.0)
        }

        fn combine(&self, left: Self::Summary, right: Self::Summary) -> Self::Summary {
            FieldCount(left.0 + right.0)
        }

        fn join(&self, left: Self::Summary, right: Self::Summary) -> Self::Summary {
            FieldCount(left.0.max(right.0))
        }
    }

    struct SumAlternatives;

    impl Algebra for SumAlternatives {
        type Summary = FieldCount;

        fn empty(&self) -> Self::Summary {
            FieldCount(0)
        }

        fn field(
            &self,
            _group: &CollectedFieldGroup,
            child_summary: Self::Summary,
        ) -> Self::Summary {
            FieldCount(1 + child_summary.0)
        }

        fn combine(&self, left: Self::Summary, right: Self::Summary) -> Self::Summary {
            FieldCount(left.0 + right.0)
        }

        fn join(&self, left: Self::Summary, right: Self::Summary) -> Self::Summary {
            FieldCount(left.0 + right.0)
        }
    }

    struct JoinTrace;

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct ContextEntry {
        response_name: String,
        inherited: Vec<(String, bool)>,
        local: Vec<(String, bool)>,
    }

    struct ContextTrace;

    impl Algebra for JoinTrace {
        type Summary = String;

        fn empty(&self) -> Self::Summary {
            String::new()
        }

        fn field(
            &self,
            group: &CollectedFieldGroup,
            child_summary: Self::Summary,
        ) -> Self::Summary {
            if group.response_name() == "node" {
                child_summary
            } else {
                group.response_name().to_string()
            }
        }

        fn combine(&self, left: Self::Summary, right: Self::Summary) -> Self::Summary {
            match (left.is_empty(), right.is_empty()) {
                (true, _) => right,
                (_, true) => left,
                _ => format!("{left}+{right}"),
            }
        }

        fn join(&self, left: Self::Summary, right: Self::Summary) -> Self::Summary {
            format!("({left}|{right})")
        }
    }

    impl Algebra for ContextTrace {
        type Summary = Vec<ContextEntry>;

        fn empty(&self) -> Self::Summary {
            Vec::new()
        }

        fn field(
            &self,
            group: &CollectedFieldGroup,
            child_summary: Self::Summary,
        ) -> Self::Summary {
            let literals = |condition: &[BooleanLiteral]| {
                condition
                    .iter()
                    .map(|literal| (literal.variable_name.to_string(), literal.required_value))
                    .collect()
            };
            let mut summary = vec![ContextEntry {
                response_name: group.response_name().to_string(),
                inherited: literals(&group.inherited_boolean_condition),
                local: literals(&group.boolean_condition),
            }];
            summary.extend(child_summary);
            summary
        }

        fn combine(&self, mut left: Self::Summary, right: Self::Summary) -> Self::Summary {
            left.extend(right);
            left
        }

        fn join(&self, mut left: Self::Summary, right: Self::Summary) -> Self::Summary {
            left.extend(right);
            left
        }
    }

    #[test]
    fn exact_case_groups_response_names_across_conditions() {
        let schema = Schema::parse_and_validate(
            r#"
            type Query { node: Node }
            interface Node { value: Int }
            type A implements Node { value: Int }
            type B implements Node { value: Int }
            "#,
            "schema.graphql",
        )
        .unwrap();
        let document = ExecutableDocument::parse_and_validate(
            &schema,
            r#"{
                node {
                    ... on A { value }
                    ... on Node { value }
                }
            }"#,
            "query.graphql",
        )
        .unwrap();
        let operation = document.operations.get(None).unwrap();
        let analyzer = Analyzer::new(&schema);

        let syntactic = analyzer
            .operation(&document, operation)
            .mode(AnalysisMode::Syntactic)
            .analyze(&Count)
            .unwrap();
        let exact = analyzer
            .operation(&document, operation)
            .mode(AnalysisMode::ExactCase)
            .analyze(&Count)
            .unwrap();

        assert_eq!(syntactic, FieldCount(3));
        assert_eq!(exact, FieldCount(2));
    }

    #[test]
    fn exact_case_correlates_a_variable_across_child_scopes() {
        let schema = Schema::parse_and_validate(
            "type Query { left: Child right: Child } type Child { value: Int }",
            "schema.graphql",
        )
        .unwrap();
        let document = ExecutableDocument::parse_and_validate(
            &schema,
            r#"
            query Example($show: Boolean!) {
              left { value @include(if: $show) }
              right { value @skip(if: $show) }
            }
            "#,
            "query.graphql",
        )
        .unwrap();
        let operation = document.operations.get(Some("Example")).unwrap();
        let analyzer = Analyzer::new(&schema);

        let syntactic = analyzer
            .operation(&document, operation)
            .mode(AnalysisMode::Syntactic)
            .analyze(&Count)
            .unwrap();
        let exact = analyzer
            .operation(&document, operation)
            .mode(AnalysisMode::ExactCase)
            .analyze(&Count)
            .unwrap();

        assert_eq!(syntactic, FieldCount(4));
        assert_eq!(exact, FieldCount(3));
    }

    #[test]
    fn supplied_variables_prune_conditions() {
        let schema =
            Schema::parse_and_validate("type Query { value: Int }", "schema.graphql").unwrap();
        let document = ExecutableDocument::parse_and_validate(
            &schema,
            "query Example($show: Boolean!) { value @include(if: $show) }",
            "query.graphql",
        )
        .unwrap();
        let operation = document.operations.get(Some("Example")).unwrap();
        let values = apollo_compiler::response::serde_json_bytes::json!({ "show": false })
            .as_object()
            .unwrap()
            .clone();
        let analyzer = Analyzer::new(&schema);

        for mode in [AnalysisMode::Syntactic, AnalysisMode::ExactCase] {
            let summary = analyzer
                .operation(&document, operation)
                .mode(mode)
                .variable_values(valid(&values))
                .analyze(&Count)
                .unwrap();

            assert_eq!(summary, FieldCount(0));
        }
    }

    #[test]
    fn supplied_variables_prune_conditions_in_child_scopes() {
        let schema = Schema::parse_and_validate(
            "type Query { parent: Parent } type Parent { child: Int }",
            "schema.graphql",
        )
        .unwrap();
        let document = ExecutableDocument::parse_and_validate(
            &schema,
            "query Example($show: Boolean!) { parent { child @include(if: $show) } }",
            "query.graphql",
        )
        .unwrap();
        let operation = document.operations.get(Some("Example")).unwrap();
        let values = apollo_compiler::response::serde_json_bytes::json!({ "show": false })
            .as_object()
            .unwrap()
            .clone();
        let analyzer = Analyzer::new(&schema);

        for mode in [AnalysisMode::Syntactic, AnalysisMode::ExactCase] {
            let summary = analyzer
                .operation(&document, operation)
                .mode(mode)
                .variable_values(valid(&values))
                .analyze(&Count)
                .unwrap();

            assert_eq!(summary, FieldCount(1));
        }
    }

    #[test]
    fn missing_supplied_boolean_is_exactly_absent_but_syntactically_widened() {
        let schema =
            Schema::parse_and_validate("type Query { value: Int }", "schema.graphql").unwrap();
        let document = ExecutableDocument::parse_and_validate(
            &schema,
            "query Example($show: Boolean!) { value @include(if: $show) }",
            "query.graphql",
        )
        .unwrap();
        let operation = document.operations.get(Some("Example")).unwrap();
        let values = JsonMap::new();
        let analyzer = Analyzer::new(&schema);

        let syntactic = analyzer
            .operation(&document, operation)
            .mode(AnalysisMode::Syntactic)
            .variable_values(valid(&values))
            .analyze(&Count)
            .unwrap();
        let exact = analyzer
            .operation(&document, operation)
            .mode(AnalysisMode::ExactCase)
            .variable_values(valid(&values))
            .analyze(&Count)
            .unwrap();

        assert_eq!(syntactic, FieldCount(1));
        assert_eq!(exact, FieldCount(0));
    }

    #[test]
    fn complete_missing_null_and_non_boolean_values_select_false() {
        let schema =
            Schema::parse_and_validate("type Query { value: Int }", "schema.graphql").unwrap();
        let document = ExecutableDocument::parse_and_validate(
            &schema,
            "query Example($show: Boolean!) { value @skip(if: $show) }",
            "query.graphql",
        )
        .unwrap();
        let operation = document.operations.get(Some("Example")).unwrap();
        let analyzer = Analyzer::new(&schema);
        let values = [
            JsonMap::new(),
            apollo_compiler::response::serde_json_bytes::json!({ "show": null })
                .as_object()
                .unwrap()
                .clone(),
            apollo_compiler::response::serde_json_bytes::json!({ "show": "invalid" })
                .as_object()
                .unwrap()
                .clone(),
        ];

        for values in &values {
            let summary = analyzer
                .operation(&document, operation)
                .mode(AnalysisMode::ExactCase)
                .variable_values(valid(values))
                .analyze(&Count)
                .unwrap();
            assert_eq!(summary, FieldCount(1));
        }
    }

    #[test]
    fn exact_and_syntactic_groups_keep_inherited_and_local_conditions_distinct() {
        let schema = Schema::parse_and_validate(
            "type Query { parent: Parent } type Parent { child: Int }",
            "schema.graphql",
        )
        .unwrap();
        let document = ExecutableDocument::parse_and_validate(
            &schema,
            "query Example($x: Boolean!) { parent @include(if: $x) { child } }",
            "query.graphql",
        )
        .unwrap();
        let operation = document.operations.get(Some("Example")).unwrap();
        let values = apollo_compiler::response::serde_json_bytes::json!({ "x": true })
            .as_object()
            .unwrap()
            .clone();
        let analyzer = Analyzer::new(&schema);

        let exact = analyzer
            .operation(&document, operation)
            .mode(AnalysisMode::ExactCase)
            .variable_values(valid(&values))
            .analyze(&ContextTrace)
            .unwrap();
        let syntactic = analyzer
            .operation(&document, operation)
            .mode(AnalysisMode::Syntactic)
            .variable_values(valid(&values))
            .analyze(&ContextTrace)
            .unwrap();

        assert_eq!(
            exact,
            [
                ContextEntry {
                    response_name: "parent".into(),
                    inherited: vec![("x".into(), true)],
                    local: vec![],
                },
                ContextEntry {
                    response_name: "child".into(),
                    inherited: vec![("x".into(), true)],
                    local: vec![],
                },
            ]
        );
        assert_eq!(
            syntactic,
            [
                ContextEntry {
                    response_name: "parent".into(),
                    inherited: vec![],
                    local: vec![("x".into(), true)],
                },
                ContextEntry {
                    response_name: "child".into(),
                    inherited: vec![("x".into(), true)],
                    local: vec![],
                },
            ]
        );
    }

    #[test]
    fn syntactic_boolean_alternatives_are_factorized_by_variable() {
        let schema = Schema::parse_and_validate(
            "type Query { included: Int skipped: Int }",
            "schema.graphql",
        )
        .unwrap();
        let document = ExecutableDocument::parse_and_validate(
            &schema,
            r#"
            query Example($show: Boolean!) {
              included @include(if: $show)
              skipped @skip(if: $show)
            }
            "#,
            "query.graphql",
        )
        .unwrap();
        let operation = document.operations.get(Some("Example")).unwrap();
        let analyzer = Analyzer::new(&schema);

        let summary = analyzer
            .operation(&document, operation)
            .mode(AnalysisMode::Syntactic)
            .analyze(&Count)
            .unwrap();

        assert_eq!(summary, FieldCount(1));
    }

    #[test]
    fn syntactic_groups_redundant_parent_types_by_response_name() {
        let schema = Schema::parse_and_validate(
            r#"
            type Query { node: Node }
            interface Node { value: Int }
            interface Left implements Node { value: Int }
            interface Right implements Node { value: Int }
            type A implements Node & Left & Right { value: Int }
            type B implements Node & Left & Right { value: Int }
            "#,
            "schema.graphql",
        )
        .unwrap();
        let document = ExecutableDocument::parse_and_validate(
            &schema,
            "{ node { ... on Left { shared: value } ... on Right { shared: value } } }",
            "query.graphql",
        )
        .unwrap();
        let operation = document.operations.get(None).unwrap();
        let analyzer = Analyzer::new(&schema);

        let summary = analyzer
            .operation(&document, operation)
            .mode(AnalysisMode::Syntactic)
            .analyze(&Count)
            .unwrap();

        assert_eq!(summary, FieldCount(2));
    }

    #[test]
    fn syntactic_merges_equivalent_cumulative_type_conditions() {
        let schema = Schema::parse_and_validate(
            r#"
            type Query { node: Node }
            interface Node { value: Int }
            interface Left implements Node { value: Int }
            interface Right implements Node { value: Int }
            type A implements Node & Left & Right { value: Int }
            type B implements Node { value: Int }
            "#,
            "schema.graphql",
        )
        .unwrap();
        let document = ExecutableDocument::parse_and_validate(
            &schema,
            "{ node { ... on Left { value } ... on Right { value } } }",
            "query.graphql",
        )
        .unwrap();
        let operation = document.operations.get(None).unwrap();

        let summary = Analyzer::new(&schema)
            .operation(&document, operation)
            .mode(AnalysisMode::Syntactic)
            .analyze(&Count)
            .unwrap();

        assert_eq!(summary, FieldCount(2));
    }

    #[test]
    fn syntactic_evaluates_one_product_per_materializable_type_region() {
        let schema = Schema::parse_and_validate(
            r#"
            type Query { node: Node }
            interface Node { value: Int }
            interface Shared implements Node { value: Int }
            type A implements Node & Shared { value: Int }
            type B implements Node & Shared { value: Int }
            type C implements Node { value: Int }
            "#,
            "schema.graphql",
        )
        .unwrap();
        let document = ExecutableDocument::parse_and_validate(
            &schema,
            "{ node { ... on Shared { value } } }",
            "query.graphql",
        )
        .unwrap();
        let operation = document.operations.get(None).unwrap();

        let summary = Analyzer::new(&schema)
            .operation(&document, operation)
            .mode(AnalysisMode::Syntactic)
            .analyze(&SumAlternatives)
            .unwrap();

        assert_eq!(summary, FieldCount(2));
    }

    #[test]
    fn syntactic_preserves_distinct_subset_type_products() {
        let schema = Schema::parse_and_validate(
            r#"
            type Query { node: Node }
            interface Node { value: Int }
            interface Shared implements Node { value: Int }
            type A implements Node & Shared { value: Int }
            type B implements Node & Shared { value: Int }
            type C implements Node { value: Int }
            "#,
            "schema.graphql",
        )
        .unwrap();
        let document = ExecutableDocument::parse_and_validate(
            &schema,
            "{ node { ... on Shared { broad: value } ... on A { narrow: value } } }",
            "query.graphql",
        )
        .unwrap();
        let operation = document.operations.get(None).unwrap();

        let summary = Analyzer::new(&schema)
            .operation(&document, operation)
            .mode(AnalysisMode::Syntactic)
            .analyze(&SumAlternatives)
            .unwrap();

        assert_eq!(summary, FieldCount(4));
    }

    #[test]
    fn alternative_joins_follow_the_lean_right_associated_order() {
        let schema = Schema::parse_and_validate(
            r#"
            type Query { node: Node }
            interface Node { value: Int }
            type A implements Node { value: Int }
            type B implements Node { value: Int }
            type C implements Node { value: Int }
            "#,
            "schema.graphql",
        )
        .unwrap();
        let document = ExecutableDocument::parse_and_validate(
            &schema,
            r#"{
              node {
                ... on A { a: value }
                ... on B { b: value }
                ... on C { c: value }
              }
            }"#,
            "query.graphql",
        )
        .unwrap();
        let operation = document.operations.get(None).unwrap();
        let analyzer = Analyzer::new(&schema);

        for mode in [AnalysisMode::Syntactic, AnalysisMode::ExactCase] {
            let summary = analyzer
                .operation(&document, operation)
                .mode(mode)
                .analyze(&JoinTrace)
                .unwrap();
            assert_eq!(summary, "(a|(b|c))");
        }
    }

    #[test]
    fn syntactic_canonicalizes_boolean_directive_order() {
        let schema =
            Schema::parse_and_validate("type Query { value: Int }", "schema.graphql").unwrap();
        let document = ExecutableDocument::parse_and_validate(
            &schema,
            r#"
            query Example($x: Boolean!, $y: Boolean!) {
              value @include(if: $x) @skip(if: $y)
              value @skip(if: $y) @include(if: $x)
            }
            "#,
            "query.graphql",
        )
        .unwrap();
        let operation = document.operations.get(Some("Example")).unwrap();

        let summary = Analyzer::new(&schema)
            .operation(&document, operation)
            .mode(AnalysisMode::Syntactic)
            .analyze(&Count)
            .unwrap();

        assert_eq!(summary, FieldCount(1));
    }

    #[test]
    fn syntactic_child_scopes_inherit_parent_boolean_conditions() {
        let schema = Schema::parse_and_validate(
            "type Query { parent: Parent } type Parent { child: Int }",
            "schema.graphql",
        )
        .unwrap();
        let document = ExecutableDocument::parse_and_validate(
            &schema,
            "query Example($x: Boolean!) { parent @include(if: $x) { child @skip(if: $x) } }",
            "query.graphql",
        )
        .unwrap();
        let operation = document.operations.get(Some("Example")).unwrap();
        let analyzer = Analyzer::new(&schema);

        let summary = analyzer
            .operation(&document, operation)
            .mode(AnalysisMode::Syntactic)
            .analyze(&Count)
            .unwrap();

        assert_eq!(summary, FieldCount(1));
    }

    #[test]
    fn indexed_possible_types_support_large_schemas_and_preserve_order() {
        use std::fmt::Write as _;

        let mut schema_source =
            String::from("type Query { node: Node } interface Node { id: ID! }\n");
        for index in 0..130 {
            writeln!(schema_source, "type T{index} implements Node {{ id: ID! }}").unwrap();
        }
        schema_source.push_str("union Reversed = T129 | T0\nunion First = T0\n");
        let schema = Schema::parse_and_validate(schema_source, "schema.graphql").unwrap();
        let possible_types = build_possible_types(&schema);
        let node = Name::new("Node").unwrap();
        let reversed = Name::new("Reversed").unwrap();
        let first = Name::new("First").unwrap();

        assert_eq!(possible_types.names(&node).count(), 130);
        assert_eq!(
            possible_types
                .names(&reversed)
                .map(Name::as_str)
                .collect::<Vec<_>>(),
            ["T129", "T0"]
        );

        let narrowed = possible_types.intersection(possible_types.get(&reversed).unwrap(), &first);
        assert_eq!(
            narrowed
                .ordered
                .iter()
                .map(|&index| possible_types.object_name(index).as_str())
                .collect::<Vec<_>>(),
            ["T0"]
        );
    }
}
