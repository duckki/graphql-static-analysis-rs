//! Common field/output/context operations corresponding to TreeSummary.Core.

use super::condition_tree::canonical_boolean_condition;
use super::{BooleanLiteral, CollectedFieldGroup};
use apollo_compiler::collections::IndexSet;
use apollo_compiler::executable::{self, Selection};
use apollo_compiler::{Name, Node, Schema};

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
        extend_boolean_condition(
            &self.inherited_boolean_condition,
            self.boolean_condition.iter().cloned(),
        )
    }
}

/// Lean's Internal.extendBooleanCondition: canonicalize a case extension while
/// retaining the inherited context if the extension is contradictory.
pub(super) fn extend_boolean_condition(
    inherited: &[BooleanLiteral],
    additional: impl IntoIterator<Item = BooleanLiteral>,
) -> Vec<BooleanLiteral> {
    let additional = additional.into_iter();
    let mut condition = Vec::with_capacity(inherited.len() + additional.size_hint().0);
    condition.extend_from_slice(inherited);
    condition.extend(additional);
    canonical_boolean_condition(condition).unwrap_or_else(|| inherited.to_vec())
}

impl CollectedFieldGroup {
    pub(super) fn has_child_selections(&self) -> bool {
        self.child_selection_sets()
            .any(|selections| !selections.is_empty())
    }

    /// Lean's mergedSelectionSet, kept as slices so every occurrence contributes
    /// its children without allocating a flattened executable selection list.
    pub(super) fn child_selection_sets(&self) -> impl Iterator<Item = &[Selection]> + Clone {
        self.fields
            .iter()
            .map(|field| field.selection_set.selections.as_slice())
    }

    /// Corresponds to TreeSummary.childParentTypes. Preserve runtime/occurrence
    /// traversal and output-type deduplication order in both evaluation schedules.
    /// The existing all-occurrence lookup is retained in this organizational change.
    pub(super) fn child_parent_types(&self, schema: &Schema) -> IndexSet<Name> {
        let mut parent_types = IndexSet::default();
        for runtime_type in &self.possible_types {
            for field in &self.fields {
                if let Ok(definition) = schema.type_field(runtime_type, &field.name) {
                    parent_types.insert(definition.ty.inner_named_type().clone());
                }
            }
        }
        parent_types
    }
}
