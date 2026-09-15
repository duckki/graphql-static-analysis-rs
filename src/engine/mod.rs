//! The `graphql-static-analysis` engine follows the condition-tree-summary split used
//! by the Lean model:
//! selection conditions are framework-owned, while analyses only define how to
//! summarize a collected response-name group and how to compose simultaneous and
//! alternative summaries.

use apollo_compiler::executable::ExecutableDocument;
use apollo_compiler::executable::Operation;
use apollo_compiler::executable::{self};
use apollo_compiler::response::JsonMap;
use apollo_compiler::validation::Valid;
use apollo_compiler::Name;
use apollo_compiler::Node;
use apollo_compiler::Schema;

mod condition_tree;
mod exact_cases;
mod field_group;
mod possible_types;
mod syntactic;
mod variables;

use possible_types::{build_possible_types, PossibleTypesMap};
use variables::VariableEnvironment;

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
/// it does not compare summary values or require `join` to be idempotent. Without
/// request variables, exact-case factoring additionally assumes that delaying
/// `combine` and `field` across a `join` remains a sound upper bound. With supplied
/// variables, ExactCase batches active type conditions and joins their regions in
/// right-associated order. These two schedules need not produce identical terms
/// for an algebra whose `join` is non-associative.
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

#[cfg(test)]
mod tests;
