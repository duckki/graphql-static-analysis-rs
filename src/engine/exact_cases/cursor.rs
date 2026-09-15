//! Incremental symbolic evaluation corresponding to Lean ExactCases.CaseCursor.
//! Child alternatives stay structural until every parent field transfer has run.

use super::context::{extend_case_condition, CaseCondition};
use super::decision::BooleanDecision;
use super::Engine;
use crate::engine::condition_tree::{Branch, BranchCondition, ConditionTree, NodeId};
use crate::engine::field_group::extend_boolean_condition;
use crate::engine::possible_types::{
    partition_type_region, PossibleTypeRegion, TypeRegionPartition,
};
use crate::engine::variables::{BooleanValue, VariableEnvironment};
use crate::engine::{Algebra, BooleanLiteral, CollectedFieldGroup};
use apollo_compiler::ast::Value;
use apollo_compiler::collections::{HashSet, IndexMap, IndexSet};
use apollo_compiler::executable::{self, Selection};
use apollo_compiler::{Name, Node};
use std::rc::Rc;

/// Boolean control context shared by every recursive selection-set scope in one case.
/// Symbolic assignments never alter the immutable request values used by condition-tree
/// extraction. Complete requests resolve missing, null, and non-Boolean values as
/// `false`, matching the Lean model's already-coerced directive semantics.
#[derive(Clone)]
pub(super) enum BooleanEnvironment {
    Symbolic(Option<Rc<BooleanAssignment>>),
    Complete,
}

pub(super) struct BooleanAssignment {
    variable_name: Name,
    value: bool,
    previous: Option<Rc<Self>>,
}

impl BooleanEnvironment {
    pub(super) fn from_variables(variables: &VariableEnvironment<'_>) -> Self {
        if variables.is_complete() {
            Self::Complete
        } else {
            Self::Symbolic(None)
        }
    }

    fn status_for_variable(
        &self,
        variables: &VariableEnvironment<'_>,
        variable_name: &Name,
    ) -> Option<bool> {
        match self {
            Self::Symbolic(case_values) => {
                let mut assignment = case_values.as_deref();
                while let Some(current) = assignment {
                    if current.variable_name == *variable_name {
                        return Some(current.value);
                    }
                    assignment = current.previous.as_deref();
                }
                None
            }
            Self::Complete => Some(match variables.boolean(variable_name) {
                BooleanValue::Known(value) => value,
                BooleanValue::Missing | BooleanValue::Unknown => false,
            }),
        }
    }

    fn assign(&self, variable_name: &Name, value: bool) -> Self {
        match self {
            Self::Symbolic(case_values) => Self::Symbolic(Some(Rc::new(BooleanAssignment {
                variable_name: variable_name.clone(),
                value,
                previous: case_values.clone(),
            }))),
            Self::Complete => Self::Complete,
        }
    }
}

/// One incrementally resolved selection-set boundary. Activated field chunks are
/// stored newest first; pending branches form a preorder work list.
#[derive(Clone)]
struct CaseCursor<'tree> {
    tree: &'tree ConditionTree,
    named_field_chunks_rev: Rc<NamedFieldChunk>,
    pending_branches: Option<PendingBranches<'tree>>,
}

struct NamedFieldChunk {
    node_id: NodeId,
    previous: Option<Rc<Self>>,
}

#[derive(Clone)]
struct PendingBranches<'tree> {
    branches: &'tree [Branch],
    continuation: Option<Rc<Self>>,
}

fn prepend_pending_branches<'tree>(
    branches: &'tree [Branch],
    rest: Option<PendingBranches<'tree>>,
) -> Option<PendingBranches<'tree>> {
    if branches.is_empty() {
        rest
    } else {
        Some(PendingBranches {
            branches,
            continuation: rest.map(Rc::new),
        })
    }
}

impl<'tree> PendingBranches<'tree> {
    fn branch(&self) -> &'tree Branch {
        &self.branches[0]
    }

    fn rest(&self) -> Option<Self> {
        if self.branches.len() > 1 {
            Some(Self {
                branches: &self.branches[1..],
                continuation: self.continuation.clone(),
            })
        } else {
            self.continuation.as_deref().cloned()
        }
    }
}

impl<'tree> CaseCursor<'tree> {
    fn of_condition_tree(tree: &'tree ConditionTree) -> Self {
        Self {
            tree,
            named_field_chunks_rev: Rc::new(NamedFieldChunk {
                node_id: tree.root(),
                previous: None,
            }),
            pending_branches: prepend_pending_branches(&tree.node(tree.root()).branches, None),
        }
    }

    fn skip_branch(&self, rest: Option<PendingBranches<'tree>>) -> Self {
        Self {
            tree: self.tree,
            named_field_chunks_rev: Rc::clone(&self.named_field_chunks_rev),
            pending_branches: rest,
        }
    }

    fn select_branch(&self, body: NodeId, rest: Option<PendingBranches<'tree>>) -> Self {
        let named_field_chunks_rev = if self.tree.node(body).fields.is_empty() {
            Rc::clone(&self.named_field_chunks_rev)
        } else {
            Rc::new(NamedFieldChunk {
                node_id: body,
                previous: Some(Rc::clone(&self.named_field_chunks_rev)),
            })
        };
        Self {
            tree: self.tree,
            named_field_chunks_rev,
            pending_branches: prepend_pending_branches(&self.tree.node(body).branches, rest),
        }
    }

    fn resolve_boolean_branch(
        &self,
        branch: &'tree Branch,
        rest: Option<PendingBranches<'tree>>,
        literal: &BooleanLiteral,
        value: bool,
    ) -> Self {
        if literal.required_value == value {
            self.select_branch(branch.body, rest)
        } else {
            self.skip_branch(rest)
        }
    }

    fn fields_by_response_name(&self) -> IndexMap<Name, Vec<Node<executable::Field>>> {
        let mut capacity = 0;
        let mut chunk = Some(self.named_field_chunks_rev.as_ref());
        while let Some(current) = chunk {
            capacity += self.tree.node(current.node_id).fields.len();
            chunk = current.previous.as_deref();
        }

        fn append_fields(
            tree: &ConditionTree,
            chunk: &NamedFieldChunk,
            output: &mut IndexMap<Name, Vec<Node<executable::Field>>>,
        ) {
            if let Some(previous) = chunk.previous.as_deref() {
                append_fields(tree, previous, output);
            }
            for (response_name, fields) in &tree.node(chunk.node_id).fields {
                output
                    .entry(response_name.clone())
                    .or_default()
                    .extend(fields.iter().cloned());
            }
        }

        let mut fields_by_response_name =
            IndexMap::with_capacity_and_hasher(capacity, Default::default());
        append_fields(
            self.tree,
            self.named_field_chunks_rev.as_ref(),
            &mut fields_by_response_name,
        );
        fields_by_response_name
    }
}

impl<A: Algebra> Engine<'_, '_, A> {
    pub(super) fn summarize_scope<'selection>(
        &self,
        parent_type: Name,
        inherited_boolean_condition: &[BooleanLiteral],
        selection_sets: impl IntoIterator<Item = &'selection [Selection]>,
        variable_order: &[Name],
        environment: &BooleanEnvironment,
    ) -> BooleanDecision<A::Summary> {
        let Some(tree) = ConditionTree::extract(
            self.document,
            self.possible_types,
            &self.variables,
            &parent_type,
            inherited_boolean_condition,
            selection_sets,
        ) else {
            return BooleanDecision::Leaf(self.algebra.empty());
        };
        let scope = tree.node(tree.root()).condition.possible_types.clone();
        self.summarize_decision(
            &CaseCursor::of_condition_tree(&tree),
            &PossibleTypeRegion::from(&scope),
            inherited_boolean_condition,
            &None,
            variable_order,
            environment,
        )
    }

    fn summarize_decision(
        &self,
        cursor: &CaseCursor<'_>,
        possible_types: &PossibleTypeRegion,
        inherited_boolean_condition: &[BooleanLiteral],
        case_condition: &CaseCondition,
        variable_order: &[Name],
        environment: &BooleanEnvironment,
    ) -> BooleanDecision<A::Summary> {
        let mut cursor = cursor.clone();
        let mut case_condition = case_condition.clone();
        let environment = environment.clone();

        loop {
            let Some(pending) = &cursor.pending_branches else {
                return self.summarize_field_groups(
                    &cursor,
                    possible_types,
                    inherited_boolean_condition,
                    &case_condition,
                    variable_order,
                    &environment,
                );
            };
            let branch = pending.branch();
            let rest = pending.rest();

            match &branch.condition {
                BranchCondition::Type(_) => {
                    let allowed = &cursor.tree.node(branch.body).condition.possible_types;
                    match partition_type_region(possible_types, allowed) {
                        TypeRegionPartition::Selected => {
                            cursor = cursor.select_branch(branch.body, rest);
                        }
                        TypeRegionPartition::Rejected => {
                            cursor = cursor.skip_branch(rest);
                        }
                        TypeRegionPartition::Split { selected, rejected } => {
                            let selected_cursor = cursor.select_branch(branch.body, rest.clone());
                            let selected = self.summarize_decision(
                                &selected_cursor,
                                &selected,
                                inherited_boolean_condition,
                                &case_condition,
                                variable_order,
                                &environment,
                            );
                            let rejected_cursor = cursor.skip_branch(rest);
                            let rejected = self.summarize_decision(
                                &rejected_cursor,
                                &rejected,
                                inherited_boolean_condition,
                                &case_condition,
                                variable_order,
                                &environment,
                            );
                            // Parent field transfers still need to map over each
                            // alternative. Lean retains this join until the completed
                            // operation boundary, even when both children are leaves.
                            return BooleanDecision::Join {
                                left: Box::new(selected),
                                right: Box::new(rejected),
                            };
                        }
                    }
                }
                BranchCondition::Boolean(literal) => {
                    if let Some(value) =
                        environment.status_for_variable(&self.variables, &literal.variable_name)
                    {
                        case_condition =
                            extend_case_condition(&case_condition, &literal.variable_name, value);
                        cursor = cursor.resolve_boolean_branch(branch, rest, literal, value);
                    } else {
                        let when_false_condition =
                            extend_case_condition(&case_condition, &literal.variable_name, false);
                        let when_false_environment =
                            environment.assign(&literal.variable_name, false);
                        let when_false_cursor =
                            cursor.resolve_boolean_branch(branch, rest.clone(), literal, false);
                        let when_false = self.summarize_decision(
                            &when_false_cursor,
                            possible_types,
                            inherited_boolean_condition,
                            &when_false_condition,
                            variable_order,
                            &when_false_environment,
                        );

                        let when_true_condition =
                            extend_case_condition(&case_condition, &literal.variable_name, true);
                        let when_true_environment =
                            environment.assign(&literal.variable_name, true);
                        let when_true_cursor =
                            cursor.resolve_boolean_branch(branch, rest, literal, true);
                        let when_true = self.summarize_decision(
                            &when_true_cursor,
                            possible_types,
                            inherited_boolean_condition,
                            &when_true_condition,
                            variable_order,
                            &when_true_environment,
                        );

                        return BooleanDecision::Split {
                            variable_name: literal.variable_name.clone(),
                            when_false: Box::new(when_false),
                            when_true: Box::new(when_true),
                        };
                    }
                }
            }
        }
    }

    fn summarize_field_groups(
        &self,
        cursor: &CaseCursor<'_>,
        possible_types: &PossibleTypeRegion,
        inherited_boolean_condition: &[BooleanLiteral],
        case_condition: &CaseCondition,
        variable_order: &[Name],
        environment: &BooleanEnvironment,
    ) -> BooleanDecision<A::Summary> {
        let extended_condition = extend_boolean_condition(
            inherited_boolean_condition,
            std::iter::successors(case_condition.as_deref(), |entry| entry.previous.as_deref())
                .map(|entry| entry.literal.clone()),
        );
        let region_names = possible_types
            .ordered
            .iter()
            .map(|&index| self.possible_types.object_name(index).clone())
            .collect::<Vec<_>>();
        let decisions = cursor
            .fields_by_response_name()
            .into_values()
            .map(|fields| {
                let group = CollectedFieldGroup {
                    possible_types: region_names.clone(),
                    inherited_boolean_condition: extended_condition.clone(),
                    boolean_condition: Vec::new(),
                    fields,
                };
                self.summarize_children(&group, variable_order, environment)
                    .map_owned(&|child_summary| self.algebra.field(&group, child_summary))
            });
        self.combine_decisions(decisions, variable_order)
    }

    fn summarize_children(
        &self,
        group: &CollectedFieldGroup,
        variable_order: &[Name],
        environment: &BooleanEnvironment,
    ) -> BooleanDecision<A::Summary> {
        if !group.has_child_selections() {
            return BooleanDecision::Leaf(self.algebra.empty());
        }

        let child_parent_types = group.child_parent_types(self.schema);
        let child_inherited_boolean_condition = group.child_inherited_boolean_condition();
        let decisions = child_parent_types.into_iter().map(|child_parent_type| {
            self.summarize_scope(
                child_parent_type,
                &child_inherited_boolean_condition,
                group.child_selection_sets(),
                variable_order,
                environment,
            )
        });
        self.join_decisions(decisions)
    }

    fn combine_decisions<I>(
        &self,
        decisions: I,
        variable_order: &[Name],
    ) -> BooleanDecision<A::Summary>
    where
        I: DoubleEndedIterator<Item = BooleanDecision<A::Summary>>,
    {
        let mut decisions = decisions.rev();
        let Some(mut combined) = decisions.next() else {
            return BooleanDecision::Leaf(self.algebra.empty());
        };
        for decision in decisions {
            combined = decision.zip_with_owned(combined, variable_order, &|left, right| {
                self.algebra.combine(left, right)
            });
        }
        combined
    }

    fn join_decisions<I>(&self, decisions: I) -> BooleanDecision<A::Summary>
    where
        I: DoubleEndedIterator<Item = BooleanDecision<A::Summary>>,
    {
        let mut decisions = decisions.rev();
        let Some(mut joined) = decisions.next() else {
            return BooleanDecision::Leaf(self.algebra.empty());
        };
        for decision in decisions {
            joined = BooleanDecision::Join {
                left: Box::new(decision),
                right: Box::new(joined),
            };
        }
        joined
    }

    pub(super) fn collect_boolean_variables(
        &self,
        selections: &[Selection],
        visited_fragments: &mut HashSet<Name>,
        output: &mut IndexSet<Name>,
    ) {
        for selection in selections {
            for directive in &selection.directives().0 {
                if !matches!(directive.name.as_str(), "include" | "skip") {
                    continue;
                }
                if let Some(Value::Variable(name)) = directive
                    .arguments
                    .iter()
                    .find(|argument| argument.name == "if")
                    .map(|argument| argument.value.as_ref())
                {
                    output.insert(name.clone());
                }
            }
            match selection {
                Selection::Field(field) => self.collect_boolean_variables(
                    &field.selection_set.selections,
                    visited_fragments,
                    output,
                ),
                Selection::InlineFragment(fragment) => self.collect_boolean_variables(
                    &fragment.selection_set.selections,
                    visited_fragments,
                    output,
                ),
                Selection::FragmentSpread(spread) => {
                    if visited_fragments.insert(spread.fragment_name.clone()) {
                        if let Some(fragment) = self.document.fragments.get(&spread.fragment_name) {
                            self.collect_boolean_variables(
                                &fragment.selection_set.selections,
                                visited_fragments,
                                output,
                            );
                        }
                        visited_fragments.remove(&spread.fragment_name);
                    }
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::condition_tree::Condition;
    use crate::engine::condition_tree::ConditionNode;
    use crate::engine::possible_types::PossibleTypeSet;

    fn possible_types(indices: &[usize], object_count: usize) -> PossibleTypeSet {
        let names = (0..object_count)
            .map(|i| Name::new(&format!("Type{i}")).unwrap())
            .collect::<Vec<_>>();
        let by_name = names
            .iter()
            .enumerate()
            .map(|(i, name)| (name.clone(), i))
            .collect();
        PossibleTypeSet::from_names(indices.iter().map(|&i| &names[i]), &by_name, object_count)
    }

    fn type_branch(
        tree: &mut ConditionTree,
        name: &str,
        indices: &[usize],
        object_count: usize,
    ) -> Branch {
        let possible_types = possible_types(indices, object_count);
        let body = tree.nodes.len();
        tree.nodes.push(ConditionNode {
            condition: Condition {
                possible_types,
                boolean_condition: Vec::new(),
            },
            fields: IndexMap::default(),
            branches: Vec::new(),
        });
        Branch {
            condition: BranchCondition::Type(Name::new(name).unwrap()),
            body,
        }
    }

    #[test]
    fn selected_branch_children_are_scheduled_before_later_siblings() {
        let object_count = 2;
        let mut tree = ConditionTree::new(Condition {
            possible_types: possible_types(&[0, 1], object_count),
            boolean_condition: Vec::new(),
        });
        let first = type_branch(&mut tree, "First", &[0], object_count);
        let nested = type_branch(&mut tree, "Nested", &[0], object_count);
        tree.nodes[first.body].branches.push(nested);
        let second = type_branch(&mut tree, "Second", &[1], object_count);
        let root = tree.root();
        tree.nodes[root].branches.push(first);
        tree.nodes[root].branches.push(second);

        let cursor = CaseCursor::of_condition_tree(&tree);
        let (first_body, rest) = {
            let first = cursor.pending_branches.as_ref().unwrap();
            (first.branch().body, first.rest())
        };
        let cursor = cursor.select_branch(first_body, rest);
        let mut pending_bodies = Vec::new();
        let mut pending = cursor.pending_branches.clone();
        while let Some(current) = pending {
            pending_bodies.push(current.branch().body);
            pending = current.rest();
        }

        assert_eq!(pending_bodies, [2, 3]);
    }
}
