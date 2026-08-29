//! Exact-case compatibility-region traversal.
//!
//! This backend mirrors Lean's incremental `ExactCases.CaseCursor`. It processes one
//! condition branch at a time, partitions only the current type region, and preserves
//! type alternatives as factored joins below correlated Boolean decisions.

use super::condition_tree::canonical_boolean_condition;
use super::condition_tree::Branch;
use super::condition_tree::BranchCondition;
use super::condition_tree::ConditionTree;
use super::condition_tree::NodeId;
use super::Algebra;
use super::BooleanLiteral;
use super::BooleanValue;
use super::CollectedFieldGroup;
use super::PossibleTypeRegion;
use super::PossibleTypeSet;
use super::PossibleTypesMap;
use super::VariableEnvironment;
use apollo_compiler::ast::Value;
use apollo_compiler::collections::HashSet;
use apollo_compiler::collections::IndexMap;
use apollo_compiler::collections::IndexSet;
use apollo_compiler::executable::ExecutableDocument;
use apollo_compiler::executable::Operation;
use apollo_compiler::executable::Selection;
use apollo_compiler::executable::{self};
use apollo_compiler::Name;
use apollo_compiler::Node;
use apollo_compiler::Schema;
use std::rc::Rc;

/// Boolean control context shared by every recursive selection-set scope in one case.
/// Symbolic assignments never alter the immutable request values used by condition-tree
/// extraction. Complete requests resolve missing, null, and non-Boolean values as
/// `false`, matching the Lean model's already-coerced directive semantics.
#[derive(Clone)]
enum BooleanEnvironment {
    Symbolic(Option<Rc<BooleanAssignment>>),
    Complete,
}

struct BooleanAssignment {
    variable_name: Name,
    value: bool,
    previous: Option<Rc<Self>>,
}

impl BooleanEnvironment {
    fn from_variables(variables: &VariableEnvironment<'_>) -> Self {
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

/// Lean's lazy Boolean decision tree. `Join` preserves type and child-parent
/// alternatives without constructing a Cartesian product with unrelated Boolean
/// supports.
#[derive(Clone)]
enum BooleanDecision<S> {
    Leaf(S),
    Split {
        variable_name: Name,
        when_false: Box<Self>,
        when_true: Box<Self>,
    },
    Join {
        left: Box<Self>,
        right: Box<Self>,
    },
}

impl<S: Clone> BooleanDecision<S> {
    fn map<T>(&self, transform: &impl Fn(&S) -> T) -> BooleanDecision<T> {
        match self {
            Self::Leaf(summary) => BooleanDecision::Leaf(transform(summary)),
            Self::Split {
                variable_name,
                when_false,
                when_true,
            } => BooleanDecision::Split {
                variable_name: variable_name.clone(),
                when_false: Box::new(when_false.map(transform)),
                when_true: Box::new(when_true.map(transform)),
            },
            Self::Join { left, right } => BooleanDecision::Join {
                left: Box::new(left.map(transform)),
                right: Box::new(right.map(transform)),
            },
        }
    }

    fn map_owned<T>(self, transform: &impl Fn(S) -> T) -> BooleanDecision<T> {
        match self {
            Self::Leaf(summary) => BooleanDecision::Leaf(transform(summary)),
            Self::Split {
                variable_name,
                when_false,
                when_true,
            } => BooleanDecision::Split {
                variable_name,
                when_false: Box::new(when_false.map_owned(transform)),
                when_true: Box::new(when_true.map_owned(transform)),
            },
            Self::Join { left, right } => BooleanDecision::Join {
                left: Box::new(left.map_owned(transform)),
                right: Box::new(right.map_owned(transform)),
            },
        }
    }

    fn restrict(&self, selected_variable: &Name, selected_value: bool) -> Self {
        match self {
            Self::Leaf(summary) => Self::Leaf(summary.clone()),
            Self::Split {
                variable_name,
                when_false,
                when_true,
            } if variable_name == selected_variable => {
                if selected_value {
                    when_true.restrict(selected_variable, selected_value)
                } else {
                    when_false.restrict(selected_variable, selected_value)
                }
            }
            Self::Split {
                variable_name,
                when_false,
                when_true,
            } => Self::Split {
                variable_name: variable_name.clone(),
                when_false: Box::new(when_false.restrict(selected_variable, selected_value)),
                when_true: Box::new(when_true.restrict(selected_variable, selected_value)),
            },
            Self::Join { left, right } => Self::Join {
                left: Box::new(left.restrict(selected_variable, selected_value)),
                right: Box::new(right.restrict(selected_variable, selected_value)),
            },
        }
    }

    fn zip_with<T: Clone, U>(
        &self,
        other: &BooleanDecision<T>,
        variable_order: &[Name],
        operation: &impl Fn(&S, &T) -> U,
    ) -> BooleanDecision<U> {
        match (self, other) {
            (Self::Leaf(left), right) => right.map(&|right| operation(left, right)),
            (left, BooleanDecision::Leaf(right)) => left.map(&|left| operation(left, right)),
            (Self::Join { left, right }, other) => BooleanDecision::Join {
                left: Box::new(left.zip_with(other, variable_order, operation)),
                right: Box::new(right.zip_with(other, variable_order, operation)),
            },
            (left, BooleanDecision::Join { left: first, right }) => BooleanDecision::Join {
                left: Box::new(left.zip_with(first, variable_order, operation)),
                right: Box::new(left.zip_with(right, variable_order, operation)),
            },
            (
                Self::Split {
                    variable_name: left_variable,
                    when_false: left_false,
                    when_true: left_true,
                },
                BooleanDecision::Split {
                    variable_name: right_variable,
                    when_false: right_false,
                    when_true: right_true,
                },
            ) if left_variable == right_variable => BooleanDecision::Split {
                variable_name: left_variable.clone(),
                when_false: Box::new(left_false.zip_with(right_false, variable_order, operation)),
                when_true: Box::new(left_true.zip_with(right_true, variable_order, operation)),
            },
            (
                Self::Split {
                    variable_name: left_variable,
                    when_false: left_false,
                    when_true: left_true,
                },
                right @ BooleanDecision::Split {
                    variable_name: right_variable,
                    ..
                },
            ) if variable_index(variable_order, left_variable)
                < variable_index(variable_order, right_variable) =>
            {
                BooleanDecision::Split {
                    variable_name: left_variable.clone(),
                    when_false: Box::new(left_false.zip_with(
                        &right.restrict(left_variable, false),
                        variable_order,
                        operation,
                    )),
                    when_true: Box::new(left_true.zip_with(
                        &right.restrict(left_variable, true),
                        variable_order,
                        operation,
                    )),
                }
            }
            (
                left @ Self::Split { .. },
                BooleanDecision::Split {
                    variable_name: right_variable,
                    when_false: right_false,
                    when_true: right_true,
                },
            ) => BooleanDecision::Split {
                variable_name: right_variable.clone(),
                when_false: Box::new(left.restrict(right_variable, false).zip_with(
                    right_false,
                    variable_order,
                    operation,
                )),
                when_true: Box::new(left.restrict(right_variable, true).zip_with(
                    right_true,
                    variable_order,
                    operation,
                )),
            },
        }
    }

    fn zip_with_owned<T: Clone, U>(
        self,
        other: BooleanDecision<T>,
        variable_order: &[Name],
        operation: &impl Fn(S, T) -> U,
    ) -> BooleanDecision<U> {
        match (self, other) {
            (Self::Leaf(left), BooleanDecision::Leaf(right)) => {
                BooleanDecision::Leaf(operation(left, right))
            }
            (left, right) => left.zip_with(&right, variable_order, &|left, right| {
                operation(left.clone(), right.clone())
            }),
        }
    }

    fn join_cases(self, right: Self, join: &impl Fn(S, S) -> S) -> Self {
        match (self, right) {
            (Self::Leaf(left), Self::Leaf(right)) => Self::Leaf(join(left, right)),
            (left, right) => Self::Join {
                left: Box::new(left),
                right: Box::new(right),
            },
        }
    }

    fn compact(self, join: &impl Fn(S, S) -> S) -> Self {
        match self {
            Self::Leaf(summary) => Self::Leaf(summary),
            Self::Split {
                variable_name,
                when_false,
                when_true,
            } => Self::Split {
                variable_name,
                when_false: Box::new(when_false.compact(join)),
                when_true: Box::new(when_true.compact(join)),
            },
            Self::Join { left, right } => left.compact(join).join_cases(right.compact(join), join),
        }
    }

    fn collapse<A: Algebra<Summary = S>>(self, algebra: &A) -> S {
        match self {
            Self::Leaf(summary) => summary,
            Self::Split {
                when_false,
                when_true,
                ..
            } => algebra.join(
                (*when_false).collapse(algebra),
                (*when_true).collapse(algebra),
            ),
            Self::Join { left, right } => {
                algebra.join((*left).collapse(algebra), (*right).collapse(algebra))
            }
        }
    }
}

fn variable_index(variable_order: &[Name], variable_name: &Name) -> usize {
    variable_order
        .iter()
        .position(|candidate| candidate == variable_name)
        .unwrap_or(variable_order.len())
}

enum TypeRegionPartition {
    Selected,
    Rejected,
    Split {
        selected: PossibleTypeRegion,
        rejected: PossibleTypeRegion,
    },
}

fn partition_type_region(
    region: &PossibleTypeRegion,
    allowed: &PossibleTypeSet,
) -> TypeRegionPartition {
    let selected_count = region
        .ordered
        .iter()
        .filter(|&&index| allowed.bits.contains(index))
        .count();
    if selected_count == 0 {
        return TypeRegionPartition::Rejected;
    }
    if selected_count == region.ordered.len() {
        return TypeRegionPartition::Selected;
    }

    let rejected_count = region.ordered.len() - selected_count;
    let mut remainder = region.ordered.clone();
    if selected_count <= rejected_count {
        let mut selected = Vec::with_capacity(selected_count);
        remainder.retain(|&index| {
            if allowed.bits.contains(index) {
                selected.push(index);
                false
            } else {
                true
            }
        });
        TypeRegionPartition::Split {
            selected: PossibleTypeRegion { ordered: selected },
            rejected: PossibleTypeRegion { ordered: remainder },
        }
    } else {
        let mut rejected = Vec::with_capacity(rejected_count);
        remainder.retain(|&index| {
            if allowed.bits.contains(index) {
                true
            } else {
                rejected.push(index);
                false
            }
        });
        TypeRegionPartition::Split {
            selected: PossibleTypeRegion { ordered: remainder },
            rejected: PossibleTypeRegion { ordered: rejected },
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

type CaseCondition = Option<Rc<CaseConditionEntry>>;

struct CaseConditionEntry {
    literal: BooleanLiteral,
    previous: CaseCondition,
}

struct CompleteBooleanAssignment<'tree> {
    variable_name: &'tree Name,
    required_value: bool,
}

fn case_condition_value(condition: &CaseCondition, variable_name: &Name) -> Option<bool> {
    let mut existing = condition.as_deref();
    while let Some(current) = existing {
        if current.literal.variable_name == *variable_name {
            return Some(current.literal.required_value);
        }
        existing = current.previous.as_deref();
    }
    None
}

fn extend_case_condition(
    condition: &CaseCondition,
    variable_name: &Name,
    required_value: bool,
) -> CaseCondition {
    if let Some(existing) = case_condition_value(condition, variable_name) {
        debug_assert_eq!(existing, required_value);
        return condition.clone();
    }
    Some(Rc::new(CaseConditionEntry {
        literal: BooleanLiteral {
            variable_name: variable_name.clone(),
            required_value,
        },
        previous: condition.clone(),
    }))
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

enum CompleteFieldGroups {
    Empty,
    Single(Vec<Node<executable::Field>>),
    Linear(Vec<Vec<Node<executable::Field>>>),
    Indexed(IndexMap<Name, Vec<Node<executable::Field>>>),
}

fn complete_field_groups(tree: &ConditionTree, node_ids: &[NodeId]) -> CompleteFieldGroups {
    let group_capacity = node_ids
        .iter()
        .map(|&node_id| tree.node(node_id).fields.len())
        .sum();
    let mut single_name = None;
    let mut single_fields = Vec::new();
    let mut linear: Option<Vec<Vec<Node<executable::Field>>>> = None;
    let mut indexed: Option<IndexMap<Name, Vec<Node<executable::Field>>>> = None;

    for &node_id in node_ids {
        for (response_name, fields) in &tree.node(node_id).fields {
            if let Some(groups) = &mut indexed {
                groups
                    .entry(response_name.clone())
                    .or_default()
                    .extend(fields.iter().cloned());
            } else if let Some(groups) = &mut linear {
                if let Some(collected) = groups
                    .iter_mut()
                    .find(|collected| collected[0].response_key() == response_name)
                {
                    collected.extend(fields.iter().cloned());
                } else {
                    groups.push(fields.to_vec());
                }
            } else if single_name
                .as_ref()
                .is_none_or(|name| *name == response_name)
            {
                single_name.get_or_insert(response_name);
                single_fields.extend(fields.iter().cloned());
            } else if group_capacity <= 8 {
                let mut groups = Vec::with_capacity(group_capacity);
                groups.push(std::mem::take(&mut single_fields));
                groups.push(fields.to_vec());
                linear = Some(groups);
            } else {
                let mut groups =
                    IndexMap::with_capacity_and_hasher(group_capacity, Default::default());
                groups.insert(
                    single_name.take().unwrap().clone(),
                    std::mem::take(&mut single_fields),
                );
                groups.insert(response_name.clone(), fields.to_vec());
                indexed = Some(groups);
            }
        }
    }

    if let Some(groups) = indexed {
        CompleteFieldGroups::Indexed(groups)
    } else if let Some(groups) = linear {
        CompleteFieldGroups::Linear(groups)
    } else if single_name.is_some() {
        CompleteFieldGroups::Single(single_fields)
    } else {
        CompleteFieldGroups::Empty
    }
}

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

    fn summarize_complete_scope<'selection>(
        &self,
        parent_type: Name,
        inherited_boolean_condition: &[BooleanLiteral],
        selection_sets: impl IntoIterator<Item = &'selection [Selection]>,
    ) -> A::Summary {
        let Some(tree) = ConditionTree::extract(
            self.document,
            self.possible_types,
            &self.variables,
            &parent_type,
            inherited_boolean_condition,
            selection_sets,
        ) else {
            return self.algebra.empty();
        };
        let scope = tree.node(tree.root()).condition.possible_types.clone();
        let mut selected_field_nodes = Vec::with_capacity(tree.nodes.len().min(16));
        selected_field_nodes.push(tree.root());
        let mut case_condition = Vec::with_capacity(tree.nodes.len().min(8));
        self.summarize_complete_decision(
            &tree,
            &mut selected_field_nodes,
            prepend_pending_branches(&tree.node(tree.root()).branches, None),
            &PossibleTypeRegion::from(&scope),
            inherited_boolean_condition,
            &mut case_condition,
        )
    }

    fn summarize_complete_decision<'tree>(
        &self,
        tree: &'tree ConditionTree,
        selected_field_nodes: &mut Vec<NodeId>,
        mut pending_branches: Option<PendingBranches<'tree>>,
        possible_types: &PossibleTypeRegion,
        inherited_boolean_condition: &[BooleanLiteral],
        case_condition: &mut Vec<CompleteBooleanAssignment<'tree>>,
    ) -> A::Summary {
        loop {
            let Some(pending) = &pending_branches else {
                return self.summarize_complete_field_groups(
                    tree,
                    selected_field_nodes,
                    possible_types,
                    inherited_boolean_condition,
                    case_condition,
                );
            };
            let branch = pending.branch();
            let rest = pending.rest();

            match &branch.condition {
                BranchCondition::Type(_) => {
                    let body = tree.node(branch.body);
                    let allowed = &body.condition.possible_types;
                    match partition_type_region(possible_types, allowed) {
                        TypeRegionPartition::Selected => {
                            if !body.fields.is_empty() {
                                selected_field_nodes.push(branch.body);
                            }
                            pending_branches = prepend_pending_branches(&body.branches, rest);
                        }
                        TypeRegionPartition::Rejected => {
                            pending_branches = rest;
                        }
                        TypeRegionPartition::Split { selected, rejected } => {
                            let selected_field_count = selected_field_nodes.len();
                            let selected_condition_count = case_condition.len();
                            if !body.fields.is_empty() {
                                selected_field_nodes.push(branch.body);
                            }
                            let selected = self.summarize_complete_decision(
                                tree,
                                selected_field_nodes,
                                prepend_pending_branches(&body.branches, rest.clone()),
                                &selected,
                                inherited_boolean_condition,
                                case_condition,
                            );
                            selected_field_nodes.truncate(selected_field_count);
                            case_condition.truncate(selected_condition_count);
                            let rejected = self.summarize_complete_decision(
                                tree,
                                selected_field_nodes,
                                rest,
                                &rejected,
                                inherited_boolean_condition,
                                case_condition,
                            );
                            return self.algebra.join(selected, rejected);
                        }
                    }
                }
                BranchCondition::Boolean(literal) => {
                    let existing = case_condition
                        .iter()
                        .find(|current| *current.variable_name == literal.variable_name)
                        .map(|current| current.required_value);
                    let value = existing.unwrap_or_else(|| {
                        let value = match self.variables.boolean(&literal.variable_name) {
                            BooleanValue::Known(value) => value,
                            BooleanValue::Missing | BooleanValue::Unknown => false,
                        };
                        case_condition.push(CompleteBooleanAssignment {
                            variable_name: &literal.variable_name,
                            required_value: value,
                        });
                        value
                    });
                    if literal.required_value == value {
                        let body = tree.node(branch.body);
                        if !body.fields.is_empty() {
                            selected_field_nodes.push(branch.body);
                        }
                        pending_branches = prepend_pending_branches(&body.branches, rest);
                    } else {
                        pending_branches = rest;
                    }
                }
            }
        }
    }

    fn summarize_complete_field_groups(
        &self,
        tree: &ConditionTree,
        selected_field_nodes: &[NodeId],
        possible_types: &PossibleTypeRegion,
        inherited_boolean_condition: &[BooleanLiteral],
        case_condition: &[CompleteBooleanAssignment<'_>],
    ) -> A::Summary {
        let field_groups = complete_field_groups(tree, selected_field_nodes);
        if matches!(field_groups, CompleteFieldGroups::Empty) {
            return self.algebra.empty();
        }
        let mut extended_condition =
            Vec::with_capacity(inherited_boolean_condition.len() + case_condition.len());
        extended_condition.extend_from_slice(inherited_boolean_condition);
        extended_condition.extend(case_condition.iter().map(|assignment| BooleanLiteral {
            variable_name: assignment.variable_name.clone(),
            required_value: assignment.required_value,
        }));
        let inherited_boolean_condition = canonical_boolean_condition(extended_condition)
            .unwrap_or_else(|| inherited_boolean_condition.to_vec());
        let possible_types = possible_types
            .ordered
            .iter()
            .map(|&index| self.possible_types.object_name(index).clone())
            .collect::<Vec<_>>();

        match field_groups {
            CompleteFieldGroups::Single(fields) => self.summarize_complete_field_group_iter(
                std::iter::once(fields),
                possible_types,
                inherited_boolean_condition,
            ),
            CompleteFieldGroups::Linear(groups) => self.summarize_complete_field_group_iter(
                groups.into_iter().rev(),
                possible_types,
                inherited_boolean_condition,
            ),
            CompleteFieldGroups::Indexed(groups) => self.summarize_complete_field_group_iter(
                groups.into_values().rev(),
                possible_types,
                inherited_boolean_condition,
            ),
            CompleteFieldGroups::Empty => unreachable!(),
        }
    }

    fn summarize_complete_field_group_iter(
        &self,
        mut groups: impl Iterator<Item = Vec<Node<executable::Field>>>,
        possible_types: Vec<Name>,
        inherited_boolean_condition: Vec<BooleanLiteral>,
    ) -> A::Summary {
        let first_fields = groups.next().unwrap();
        let mut group = CollectedFieldGroup {
            possible_types,
            inherited_boolean_condition,
            boolean_condition: Vec::new(),
            fields: first_fields,
        };
        let mut summary = self.summarize_complete_field_group(&group);
        for fields in groups {
            group.fields = fields;
            let left = self.summarize_complete_field_group(&group);
            summary = self.algebra.combine(left, summary);
        }
        summary
    }

    fn summarize_complete_field_group(&self, group: &CollectedFieldGroup) -> A::Summary {
        let children = self.summarize_complete_children(group);
        self.algebra.field(group, children)
    }

    fn summarize_complete_children(&self, group: &CollectedFieldGroup) -> A::Summary {
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
        let child_inherited_boolean_condition = group.child_inherited_boolean_condition();
        let mut child_parent_types = child_parent_types.into_iter().rev();
        let Some(child_parent_type) = child_parent_types.next() else {
            return self.algebra.empty();
        };
        let mut summary = self.summarize_complete_scope(
            child_parent_type,
            &child_inherited_boolean_condition,
            group
                .fields
                .iter()
                .map(|field| field.selection_set.selections.as_slice()),
        );
        for child_parent_type in child_parent_types {
            let left = self.summarize_complete_scope(
                child_parent_type,
                &child_inherited_boolean_condition,
                group
                    .fields
                    .iter()
                    .map(|field| field.selection_set.selections.as_slice()),
            );
            summary = self.algebra.join(left, summary);
        }
        summary
    }

    fn summarize_scope<'selection>(
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
                            return selected.join_cases(rejected, &|left, right| {
                                self.algebra.join(left, right)
                            });
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
        let mut extended_condition = inherited_boolean_condition.to_vec();
        let mut selected = case_condition.as_deref();
        while let Some(current) = selected {
            extended_condition.push(current.literal.clone());
            selected = current.previous.as_deref();
        }
        let extended_condition = canonical_boolean_condition(extended_condition)
            .unwrap_or_else(|| inherited_boolean_condition.to_vec());
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
        if group
            .fields
            .iter()
            .all(|field| field.selection_set.selections.is_empty())
        {
            return BooleanDecision::Leaf(self.algebra.empty());
        }

        let mut child_parent_types = IndexSet::default();
        for runtime_type in &group.possible_types {
            for field in &group.fields {
                if let Ok(definition) = self.schema.type_field(runtime_type, &field.name) {
                    child_parent_types.insert(definition.ty.inner_named_type().clone());
                }
            }
        }
        let child_inherited_boolean_condition = group.child_inherited_boolean_condition();
        let decisions = child_parent_types.into_iter().map(|child_parent_type| {
            self.summarize_scope(
                child_parent_type,
                &child_inherited_boolean_condition,
                group
                    .fields
                    .iter()
                    .map(|field| field.selection_set.selections.as_slice()),
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
            joined = decision.join_cases(joined, &|left, right| self.algebra.join(left, right));
        }
        joined
    }

    fn collect_boolean_variables(
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
    use super::super::condition_tree::Condition;
    use super::super::condition_tree::ConditionNode;
    use super::*;

    fn possible_types(indices: &[usize], object_count: usize) -> PossibleTypeSet {
        let mut bits = fixedbitset::FixedBitSet::with_capacity(object_count);
        for &index in indices {
            bits.insert(index);
        }
        PossibleTypeSet(std::sync::Arc::new(super::super::PossibleTypeSetData {
            bits,
            ordered: indices.to_vec(),
            fingerprint: super::super::possible_type_fingerprint(indices),
        }))
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

    #[test]
    fn compact_combines_only_completed_join_leaves() {
        let decision = BooleanDecision::Join {
            left: Box::new(BooleanDecision::Leaf(2_u64)),
            right: Box::new(BooleanDecision::Split {
                variable_name: Name::new("x").unwrap(),
                when_false: Box::new(BooleanDecision::Leaf(3)),
                when_true: Box::new(BooleanDecision::Leaf(5)),
            }),
        };
        let compacted = decision.compact(&|left, right| left.max(right).to_owned());

        assert!(matches!(compacted, BooleanDecision::Join { .. }));
    }
}
