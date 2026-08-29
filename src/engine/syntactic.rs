//! Syntactic condition-tree traversal.
//!
//! Every syntactic branch is summarized once. Type conditions partition the runtime
//! scope into symbolic regions, and one representative evaluates each materializable
//! branch product.

use super::condition_tree::canonical_boolean_condition;
use super::condition_tree::BranchCondition;
use super::condition_tree::ConditionNode;
use super::condition_tree::ConditionTree;
use super::condition_tree::NodeId;
use super::possible_type_regions;
use super::Algebra;
use super::BooleanLiteral;
use super::BooleanValue;
use super::CollectedFieldGroup;
use super::PossibleTypeRegion;
use super::PossibleTypeSet;
use super::PossibleTypesMap;
use super::VariableEnvironment;
use apollo_compiler::collections::IndexMap;
use apollo_compiler::collections::IndexSet;
use apollo_compiler::executable::ExecutableDocument;
use apollo_compiler::executable::Operation;
use apollo_compiler::executable::Selection;
use apollo_compiler::Name;
use apollo_compiler::Schema;

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
    .summarize_scope(
        operation.selection_set.ty.clone(),
        Vec::new(),
        [operation.selection_set.selections.as_slice()],
    )
}

struct Engine<'a, 'possible_types, A> {
    schema: &'a Schema,
    document: &'a ExecutableDocument,
    algebra: &'a A,
    variables: VariableEnvironment<'a>,
    possible_types: &'possible_types PossibleTypesMap,
}

struct TypeBranchSummary<S> {
    possible_types: PossibleTypeSet,
    summary: S,
}

struct BooleanAlternatives<S> {
    when_false: Option<S>,
    when_true: Option<S>,
}

impl<S> Default for BooleanAlternatives<S> {
    fn default() -> Self {
        Self {
            when_false: None,
            when_true: None,
        }
    }
}

impl<A: Algebra> Engine<'_, '_, A> {
    fn summarize_scope<'selection>(
        &self,
        parent_type: Name,
        inherited_boolean_condition: Vec<BooleanLiteral>,
        selection_sets: impl IntoIterator<Item = &'selection [Selection]>,
    ) -> A::Summary {
        let Some(tree) = ConditionTree::extract(
            self.document,
            self.possible_types,
            &self.variables,
            &parent_type,
            &inherited_boolean_condition,
            selection_sets,
        ) else {
            return self.algebra.empty();
        };
        self.summarize_tree(&tree, tree.root(), &inherited_boolean_condition)
            .unwrap_or_else(|| self.algebra.empty())
    }

    fn summarize_tree(
        &self,
        tree: &ConditionTree,
        node_id: NodeId,
        inherited_boolean_condition: &[BooleanLiteral],
    ) -> Option<A::Summary> {
        let node = tree.node(node_id);
        let fields = self.summarize_fields(node, inherited_boolean_condition);

        let mut type_branches = Vec::new();
        let mut known_boolean = None;
        let mut unknown_booleans: IndexMap<Name, BooleanAlternatives<A::Summary>> =
            IndexMap::default();

        for branch in &node.branches {
            match &branch.condition {
                BranchCondition::Type(_) => {
                    let Some(summary) =
                        self.summarize_tree(tree, branch.body, inherited_boolean_condition)
                    else {
                        continue;
                    };
                    type_branches.push(TypeBranchSummary {
                        possible_types: tree.node(branch.body).condition.possible_types.clone(),
                        summary,
                    });
                }
                BranchCondition::Boolean(literal) => {
                    match self.variables.boolean(&literal.variable_name) {
                        BooleanValue::Known(value) if value == literal.required_value => {
                            if let Some(summary) =
                                self.summarize_tree(tree, branch.body, inherited_boolean_condition)
                            {
                                known_boolean = self.combine_optional(known_boolean, Some(summary));
                            }
                        }
                        BooleanValue::Missing | BooleanValue::Unknown => {
                            if let Some(summary) =
                                self.summarize_tree(tree, branch.body, inherited_boolean_condition)
                            {
                                let alternatives = unknown_booleans
                                    .entry(literal.variable_name.clone())
                                    .or_default();
                                let slot = if literal.required_value {
                                    &mut alternatives.when_true
                                } else {
                                    &mut alternatives.when_false
                                };
                                *slot = self.combine_optional(slot.take(), Some(summary));
                            }
                        }
                        BooleanValue::Known(_) => {}
                    }
                }
            }
        }

        let type_cases = self.summarize_type_cases(&node.condition.possible_types, &type_branches);
        let unknown_boolean = unknown_booleans
            .into_values()
            .fold(None, |product, alternatives| {
                let alternative =
                    self.join_optional(alternatives.when_false, alternatives.when_true);
                self.combine_optional(product, alternative)
            });
        let boolean_cases = self.combine_optional(known_boolean, unknown_boolean);

        self.combine_optional(fields, self.combine_optional(type_cases, boolean_cases))
    }

    fn summarize_fields(
        &self,
        node: &ConditionNode,
        inherited_boolean_condition: &[BooleanLiteral],
    ) -> Option<A::Summary> {
        let mut child_boolean_condition = inherited_boolean_condition.to_vec();
        child_boolean_condition.extend(node.condition.boolean_condition.iter().cloned());
        canonical_boolean_condition(child_boolean_condition)
            .expect("extracted condition-tree nodes are feasible");
        node.fields
            .iter()
            .fold(None, |summary, (_response_name, fields)| {
                let group = CollectedFieldGroup {
                    possible_types: node
                        .condition
                        .possible_types
                        .ordered
                        .iter()
                        .map(|&index| self.possible_types.object_name(index).clone())
                        .collect(),
                    inherited_boolean_condition: inherited_boolean_condition.to_vec(),
                    boolean_condition: node.condition.boolean_condition.clone(),
                    fields: fields.clone(),
                };
                let child_summary = self.summarize_children(&group);
                self.combine_optional(summary, Some(self.algebra.field(&group, child_summary)))
            })
    }

    fn summarize_children(&self, group: &CollectedFieldGroup) -> A::Summary {
        if group
            .fields
            .iter()
            .all(|field| field.selection_set.selections.is_empty())
        {
            return self.algebra.empty();
        }

        let mut child_parent_types = IndexSet::default();
        for runtime_type in &group.possible_types {
            for field in &group.fields {
                if let Ok(definition) = self.schema.type_field(runtime_type, &field.name) {
                    child_parent_types.insert(definition.ty.inner_named_type().clone());
                }
            }
        }

        child_parent_types
            .into_iter()
            .rev()
            .fold(None, |summary, child_parent_type| {
                let child = self.summarize_scope(
                    child_parent_type,
                    group.child_inherited_boolean_condition(),
                    group
                        .fields
                        .iter()
                        .map(|field| field.selection_set.selections.as_slice()),
                );
                self.join_optional(Some(child), summary)
            })
            .unwrap_or_else(|| self.algebra.empty())
    }

    fn summarize_type_cases(
        &self,
        scope: &PossibleTypeSet,
        branches: &[TypeBranchSummary<A::Summary>],
    ) -> Option<A::Summary> {
        if branches.is_empty() {
            return None;
        }
        let regions = possible_type_regions(
            &PossibleTypeRegion::from(scope),
            branches
                .iter()
                .filter(|branch| !branch.possible_types.is_empty())
                .map(|branch| &branch.possible_types),
        );
        regions.into_iter().rev().fold(None, |cases, region| {
            let runtime_type = *region
                .ordered
                .first()
                .expect("possible-type regions are nonempty");
            let product = branches
                .iter()
                .rev()
                .filter(|branch| branch.possible_types.bits.contains(runtime_type))
                .fold(None, |product, branch| {
                    self.combine_optional(Some(branch.summary.clone()), product)
                });
            self.join_optional(product, cases)
        })
    }

    fn combine_optional(
        &self,
        left: Option<A::Summary>,
        right: Option<A::Summary>,
    ) -> Option<A::Summary> {
        match (left, right) {
            (None, right) => right,
            (left, None) => left,
            (Some(left), Some(right)) => Some(self.algebra.combine(left, right)),
        }
    }

    fn join_optional(
        &self,
        left: Option<A::Summary>,
        right: Option<A::Summary>,
    ) -> Option<A::Summary> {
        match (left, right) {
            (None, right) => right,
            (left, None) => left,
            (Some(left), Some(right)) => Some(self.algebra.join(left, right)),
        }
    }
}
