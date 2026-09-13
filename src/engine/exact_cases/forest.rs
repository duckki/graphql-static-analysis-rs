//! Supplied-variable evaluation corresponding to Lean ExactCases.CaseForest.
//! Resolve each complete frontier, then join child summaries before the parent transfer.

use super::Engine;
use crate::engine::condition_tree::{Branch, BranchCondition, ConditionTree, NodeId};
use crate::engine::field_group::extend_boolean_condition;
use crate::engine::possible_types::{possible_type_regions, PossibleTypeRegion};
use crate::engine::variables::{BooleanValue, VariableEnvironment};
use crate::engine::{Algebra, BooleanLiteral, CollectedFieldGroup};
use apollo_compiler::collections::IndexMap;
use apollo_compiler::executable::{self, Selection};
use apollo_compiler::{Name, Node};

struct CompleteBooleanAssignment<'tree> {
    variable_name: &'tree Name,
    required_value: bool,
}

/// Arena references to Lean's active trees. A resolved entry retains its fields but
/// no longer contributes branches. Children are inserted immediately after their
/// parent, preserving field occurrence order across successive batched frontiers.
struct CaseForest {
    active_trees: Vec<ActiveTree>,
}

struct ActiveTree {
    node_id: NodeId,
    unresolved: bool,
}

impl CaseForest {
    fn of_condition_tree(tree: &ConditionTree) -> Self {
        Self {
            active_trees: vec![ActiveTree {
                node_id: tree.root(),
                unresolved: true,
            }],
        }
    }

    fn branches<'a, 'tree: 'a>(
        &'a self,
        tree: &'tree ConditionTree,
    ) -> impl Iterator<Item = &'tree Branch> + 'a {
        self.active_trees.iter().flat_map(move |active| {
            if active.unresolved {
                tree.node(active.node_id).branches.as_slice()
            } else {
                &[]
            }
        })
    }

    fn resolve_branches(
        &self,
        tree: &ConditionTree,
        region: &PossibleTypeRegion,
        variables: &VariableEnvironment<'_>,
    ) -> Self {
        let representative = region.ordered[0];
        let mut active_trees = Vec::with_capacity(self.active_trees.len());
        for active in &self.active_trees {
            let node = tree.node(active.node_id);
            // A resolved fieldless tree contributes nothing to later frontiers.
            if !node.fields.is_empty() {
                active_trees.push(ActiveTree {
                    node_id: active.node_id,
                    unresolved: false,
                });
            }
            if !active.unresolved {
                continue;
            }
            for branch in &node.branches {
                let selected = match &branch.condition {
                    // The frontier partition makes membership uniform in a region.
                    BranchCondition::Type(_) => tree
                        .node(branch.body)
                        .condition
                        .possible_types
                        .bits
                        .contains(representative),
                    BranchCondition::Boolean(literal) => {
                        let value = match variables.boolean(&literal.variable_name) {
                            BooleanValue::Known(value) => value,
                            BooleanValue::Missing | BooleanValue::Unknown => false,
                        };
                        literal.required_value == value
                    }
                };
                if selected {
                    active_trees.push(ActiveTree {
                        node_id: branch.body,
                        unresolved: true,
                    });
                }
            }
        }
        Self { active_trees }
    }
}

enum CompleteFieldGroups {
    Empty,
    Single(Vec<Node<executable::Field>>),
    Linear(Vec<Vec<Node<executable::Field>>>),
    Indexed(IndexMap<Name, Vec<Node<executable::Field>>>),
}

fn complete_field_groups(
    tree: &ConditionTree,
    node_ids: impl Iterator<Item = NodeId> + Clone,
) -> CompleteFieldGroups {
    let group_capacity = node_ids
        .clone()
        .map(|node_id| tree.node(node_id).fields.len())
        .sum();
    let mut single_name = None;
    let mut single_fields = Vec::new();
    let mut linear: Option<Vec<Vec<Node<executable::Field>>>> = None;
    let mut indexed: Option<IndexMap<Name, Vec<Node<executable::Field>>>> = None;

    for node_id in node_ids {
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

impl<A: Algebra> Engine<'_, '_, A> {
    pub(super) fn summarize_complete_scope<'selection>(
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
        self.summarize_complete_forest(
            &tree,
            CaseForest::of_condition_tree(&tree),
            &PossibleTypeRegion::from(&scope),
            inherited_boolean_condition,
            &mut Vec::with_capacity(tree.nodes.len().min(8)),
        )
    }

    fn summarize_complete_forest<'tree>(
        &self,
        tree: &'tree ConditionTree,
        mut forest: CaseForest,
        possible_types: &PossibleTypeRegion,
        inherited_boolean_condition: &[BooleanLiteral],
        case_condition: &mut Vec<CompleteBooleanAssignment<'tree>>,
    ) -> A::Summary {
        loop {
            if forest.branches(tree).next().is_none() {
                return self.summarize_complete_field_groups(
                    tree,
                    &forest,
                    possible_types,
                    inherited_boolean_condition,
                    case_condition,
                );
            }

            // Record every Boolean seen at this frontier before choosing a region,
            // including implicit false values and branches rejected by that region.
            for branch in forest.branches(tree) {
                if let BranchCondition::Boolean(literal) = &branch.condition {
                    if !case_condition
                        .iter()
                        .any(|current| *current.variable_name == literal.variable_name)
                    {
                        let value = match self.variables.boolean(&literal.variable_name) {
                            BooleanValue::Known(value) => value,
                            BooleanValue::Missing | BooleanValue::Unknown => false,
                        };
                        case_condition.push(CompleteBooleanAssignment {
                            variable_name: &literal.variable_name,
                            required_value: value,
                        });
                    }
                }
            }

            let regions = {
                let mut conditions = forest
                    .branches(tree)
                    .filter(|branch| matches!(branch.condition, BranchCondition::Type(_)))
                    .map(|branch| &tree.node(branch.body).condition.possible_types);
                conditions.next().map(|first| {
                    possible_type_regions(possible_types, std::iter::once(first).chain(conditions))
                })
            };
            let Some(regions) = regions else {
                // A Boolean-only frontier needs no type partition or scope copy.
                forest = forest.resolve_branches(tree, possible_types, &self.variables);
                continue;
            };
            if regions.len() == 1 {
                // Uniform type frontiers also advance without a recursive frame
                // or a synthetic alternative join.
                forest = forest.resolve_branches(tree, possible_types, &self.variables);
                continue;
            }

            let condition_count = case_condition.len();
            let mut regions = regions.into_iter().rev();
            let Some(region) = regions.next() else {
                return self.algebra.empty();
            };
            let mut summary = self.summarize_complete_forest(
                tree,
                forest.resolve_branches(tree, &region, &self.variables),
                &region,
                inherited_boolean_condition,
                case_condition,
            );
            case_condition.truncate(condition_count);
            for region in regions {
                let left = self.summarize_complete_forest(
                    tree,
                    forest.resolve_branches(tree, &region, &self.variables),
                    &region,
                    inherited_boolean_condition,
                    case_condition,
                );
                case_condition.truncate(condition_count);
                // Lean's joinMap is right-associated, even for non-associative
                // algebras. Do not retain the old cursor's binary split nesting.
                summary = self.algebra.join(left, summary);
            }
            return summary;
        }
    }

    fn summarize_complete_field_groups(
        &self,
        tree: &ConditionTree,
        forest: &CaseForest,
        possible_types: &PossibleTypeRegion,
        inherited_boolean_condition: &[BooleanLiteral],
        case_condition: &[CompleteBooleanAssignment<'_>],
    ) -> A::Summary {
        let field_groups = complete_field_groups(
            tree,
            forest.active_trees.iter().map(|active| active.node_id),
        );
        if matches!(field_groups, CompleteFieldGroups::Empty) {
            return self.algebra.empty();
        }
        let inherited_boolean_condition = extend_boolean_condition(
            inherited_boolean_condition,
            case_condition.iter().map(|assignment| BooleanLiteral {
                variable_name: assignment.variable_name.clone(),
                required_value: assignment.required_value,
            }),
        );
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
        if !group.has_child_selections() {
            return self.algebra.empty();
        }

        let child_parent_types = group.child_parent_types(self.schema);
        let child_inherited_boolean_condition = group.child_inherited_boolean_condition();
        let mut child_parent_types = child_parent_types.into_iter().rev();
        let Some(child_parent_type) = child_parent_types.next() else {
            return self.algebra.empty();
        };
        let mut summary = self.summarize_complete_scope(
            child_parent_type,
            &child_inherited_boolean_condition,
            group.child_selection_sets(),
        );
        for child_parent_type in child_parent_types {
            let left = self.summarize_complete_scope(
                child_parent_type,
                &child_inherited_boolean_condition,
                group.child_selection_sets(),
            );
            summary = self.algebra.join(left, summary);
        }
        summary
    }
}
