//! Exact-case compatibility-region traversal.
//!
//! Without request variables, Lean's incremental `ExactCases.CaseCursor` preserves
//! type alternatives as factored joins below correlated Boolean decisions. Supplied
//! variables use `ExactCases.CaseForest`: resolve the entire active frontier for each
//! compatibility region, then fold completed fields directly into summaries.

mod context;
mod cursor;
mod decision;
mod forest;

use super::possible_types::PossibleTypesMap;
use super::variables::VariableEnvironment;
use super::Algebra;
use apollo_compiler::collections::{HashSet, IndexSet};
use apollo_compiler::executable::{ExecutableDocument, Operation};
use apollo_compiler::Schema;
use cursor::BooleanEnvironment;

pub(super) fn summarize<A: Algebra>(
    schema: &Schema,
    document: &ExecutableDocument,
    operation: &Operation,
    algebra: &A,
    variables: VariableEnvironment<'_>,
    possible_types: &PossibleTypesMap,
) -> A::Summary {
    Engine {
        schema,
        document,
        algebra,
        variables,
        possible_types,
    }
    .summarize_operation(operation)
}

struct Engine<'a, 'possible_types, A> {
    schema: &'a Schema,
    document: &'a ExecutableDocument,
    algebra: &'a A,
    variables: VariableEnvironment<'a>,
    possible_types: &'possible_types PossibleTypesMap,
}

impl<A: Algebra> Engine<'_, '_, A> {
    fn summarize_operation(&self, operation: &Operation) -> A::Summary {
        if self.variables.is_complete() {
            return self.summarize_complete_scope(
                operation.selection_set.ty.clone(),
                &[],
                [operation.selection_set.selections.as_slice()],
            );
        }

        let mut variable_names = IndexSet::default();
        let mut visited_fragments = HashSet::default();
        self.collect_boolean_variables(
            &operation.selection_set.selections,
            &mut visited_fragments,
            &mut variable_names,
        );
        let variable_order = variable_names.into_iter().collect::<Vec<_>>();
        let environment = BooleanEnvironment::from_variables(&self.variables);
        self.summarize_scope(
            operation.selection_set.ty.clone(),
            &[],
            [operation.selection_set.selections.as_slice()],
            &variable_order,
            &environment,
        )
        .compact(&|left, right| self.algebra.join(left, right))
        .collapse(self.algebra)
    }
}
