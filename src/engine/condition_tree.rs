//! Shared canonical condition-tree extraction.
//!
//! This is the Rust counterpart of Lean's `GraphQL.Theories.ConditionTree`.
//! Source paths are interpreted as cumulative possible-type intersections and
//! canonical Boolean conjunctions. Fields that reach the same cumulative condition
//! therefore share one node even when their source paths use different type names or
//! directive orders. Supplied variables prune Boolean branches known to be inactive;
//! matching, missing, and unresolved literals remain explicit for backend evaluation.

use super::BooleanLiteral;
use super::BooleanValue;
use super::PossibleTypeSet;
use super::PossibleTypesMap;
use super::VariableEnvironment;
use apollo_compiler::ast::DirectiveList;
use apollo_compiler::ast::Value;
use apollo_compiler::collections::HashMap;
use apollo_compiler::collections::HashSet;
use apollo_compiler::collections::IndexMap;
use apollo_compiler::executable::ExecutableDocument;
use apollo_compiler::executable::Selection;
use apollo_compiler::executable::{self};
use apollo_compiler::Name;
use apollo_compiler::Node;
use std::cmp::Ordering;

pub(super) type NodeId = usize;

/// The cumulative semantic identity of one condition-tree node.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(super) struct Condition {
    pub(super) possible_types: PossibleTypeSet,
    pub(super) boolean_condition: Vec<BooleanLiteral>,
}

/// One source-level condition retained on a canonical tree edge.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum BranchCondition {
    Type(Name),
    Boolean(BooleanLiteral),
}

#[derive(Debug)]
pub(super) struct Branch {
    pub(super) condition: BranchCondition,
    pub(super) body: NodeId,
}

#[derive(Debug)]
pub(super) struct ConditionNode {
    pub(super) condition: Condition,
    pub(super) fields: IndexMap<Name, Vec<Node<executable::Field>>>,
    pub(super) branches: Vec<Branch>,
}

/// One selection-set boundary. Nodes are arena-backed so extraction can index
/// cumulative conditions directly without giving up the model's tree structure.
#[derive(Debug)]
pub(super) struct ConditionTree {
    pub(super) nodes: Vec<ConditionNode>,
    by_condition: HashMap<Condition, NodeId>,
}

impl ConditionTree {
    pub(super) fn extract<'selection>(
        document: &ExecutableDocument,
        possible_types: &PossibleTypesMap,
        variables: &VariableEnvironment<'_>,
        parent_type: &Name,
        inherited_boolean_condition: &[BooleanLiteral],
        selection_sets: impl IntoIterator<Item = &'selection [Selection]>,
    ) -> Option<Self> {
        let scope = possible_types.get(parent_type)?.clone();
        if scope.is_empty() {
            return None;
        }

        let root = Condition {
            possible_types: scope,
            boolean_condition: Vec::new(),
        };
        let mut extractor = Extractor {
            document,
            possible_types,
            variables,
            inherited_boolean_condition,
            intersection_cache: HashMap::default(),
            tree: Self::new(root.clone()),
        };
        let mut visited_fragments = HashSet::default();
        for selections in selection_sets {
            extractor.insert_selections(&root, &[], selections, &mut visited_fragments);
        }
        Some(extractor.tree)
    }

    pub(super) fn new(root: Condition) -> Self {
        let mut by_condition = HashMap::default();
        by_condition.insert(root.clone(), 0);
        Self {
            nodes: vec![ConditionNode {
                condition: root,
                fields: IndexMap::default(),
                branches: Vec::new(),
            }],
            by_condition,
        }
    }

    pub(super) fn root(&self) -> NodeId {
        0
    }

    pub(super) fn node(&self, id: NodeId) -> &ConditionNode {
        &self.nodes[id]
    }

    fn node_mut(&mut self, id: NodeId) -> &mut ConditionNode {
        &mut self.nodes[id]
    }

    fn id_for_condition(&self, condition: &Condition) -> Option<NodeId> {
        self.by_condition.get(condition).copied()
    }

    fn push_node(&mut self, condition: Condition) -> NodeId {
        let id = self.nodes.len();
        self.nodes.push(ConditionNode {
            condition: condition.clone(),
            fields: IndexMap::default(),
            branches: Vec::new(),
        });
        let previous = self.by_condition.insert(condition, id);
        debug_assert!(previous.is_none());
        id
    }
}

struct Extractor<'a> {
    document: &'a ExecutableDocument,
    possible_types: &'a PossibleTypesMap,
    variables: &'a VariableEnvironment<'a>,
    inherited_boolean_condition: &'a [BooleanLiteral],
    // Path minimization revisits the same pure intersections while checking candidate
    // paths. Reuse their canonical sets without making the shared analyzer mutable.
    intersection_cache: HashMap<(PossibleTypeSet, Name), PossibleTypeSet>,
    tree: ConditionTree,
}

impl Extractor<'_> {
    fn insert_selections(
        &mut self,
        current_condition: &Condition,
        source_path: &[(BranchCondition, Condition)],
        selections: &[Selection],
        visited_fragments: &mut HashSet<Name>,
    ) {
        for selection in selections {
            match selection {
                Selection::Field(field) => {
                    let Some(next_branches) =
                        branch_conditions_for_directives(&field.directives, self.variables)
                    else {
                        continue;
                    };
                    let Some(next_path) = self.path_for_branches(current_condition, &next_branches)
                    else {
                        continue;
                    };
                    let target = next_path
                        .last()
                        .map(|item| item.1.clone())
                        .unwrap_or_else(|| current_condition.clone());
                    let mut full_path = source_path.to_vec();
                    full_path.extend(next_path);
                    self.insert_field(&full_path, target, field.clone());
                }
                Selection::InlineFragment(fragment) => {
                    let mut next_branches = fragment
                        .type_condition
                        .as_ref()
                        .map(|name| vec![BranchCondition::Type(name.clone())])
                        .unwrap_or_default();
                    let Some(directive_path) =
                        branch_conditions_for_directives(&fragment.directives, self.variables)
                    else {
                        continue;
                    };
                    next_branches.extend(directive_path);
                    let Some(next_path) = self.path_for_branches(current_condition, &next_branches)
                    else {
                        continue;
                    };
                    let next_condition = next_path
                        .last()
                        .map(|item| item.1.clone())
                        .unwrap_or_else(|| current_condition.clone());
                    let mut full_path = source_path.to_vec();
                    full_path.extend(next_path);
                    self.insert_selections(
                        &next_condition,
                        &full_path,
                        &fragment.selection_set.selections,
                        visited_fragments,
                    );
                }
                Selection::FragmentSpread(spread) => {
                    if !visited_fragments.insert(spread.fragment_name.clone()) {
                        continue;
                    }
                    if let Some(fragment) = self.document.fragments.get(&spread.fragment_name) {
                        let mut next_branches =
                            vec![BranchCondition::Type(fragment.selection_set.ty.clone())];
                        if let Some(directive_path) =
                            branch_conditions_for_directives(&spread.directives, self.variables)
                        {
                            next_branches.extend(directive_path);
                            if let Some(next_path) =
                                self.path_for_branches(current_condition, &next_branches)
                            {
                                let next_condition = next_path
                                    .last()
                                    .map(|item| item.1.clone())
                                    .unwrap_or_else(|| current_condition.clone());
                                let mut full_path = source_path.to_vec();
                                full_path.extend(next_path);
                                self.insert_selections(
                                    &next_condition,
                                    &full_path,
                                    &fragment.selection_set.selections,
                                    visited_fragments,
                                );
                            }
                        }
                    }
                    visited_fragments.remove(&spread.fragment_name);
                }
            }
        }
    }

    fn condition_for_branch(
        &mut self,
        start: &Condition,
        branch: &BranchCondition,
    ) -> Option<Condition> {
        match branch {
            BranchCondition::Type(type_name) => {
                let key = (start.possible_types.clone(), type_name.clone());
                let possible_types = match self.intersection_cache.get(&key) {
                    Some(possible_types) => possible_types.clone(),
                    None => {
                        let possible_types = self
                            .possible_types
                            .intersection(&start.possible_types, type_name);
                        self.intersection_cache.insert(key, possible_types.clone());
                        possible_types
                    }
                };
                (!possible_types.is_empty()).then(|| Condition {
                    possible_types,
                    boolean_condition: start.boolean_condition.clone(),
                })
            }
            BranchCondition::Boolean(literal) => {
                let mut candidate = start.boolean_condition.clone();
                candidate.push(literal.clone());
                let candidate = canonical_boolean_condition(candidate)?;
                let boolean_condition = candidate
                    .into_iter()
                    .filter(|item| !self.inherited_boolean_condition.contains(item))
                    .collect::<Vec<_>>();
                let mut global = self.inherited_boolean_condition.to_vec();
                global.extend(boolean_condition.iter().cloned());
                canonical_boolean_condition(global)?;
                Some(Condition {
                    possible_types: start.possible_types.clone(),
                    boolean_condition,
                })
            }
        }
    }

    fn condition_for_branches(
        &mut self,
        start: &Condition,
        branches: &[BranchCondition],
    ) -> Option<Condition> {
        branches
            .iter()
            .try_fold(start.clone(), |condition, branch| {
                self.condition_for_branch(&condition, branch)
            })
    }

    fn path_for_branches(
        &mut self,
        start: &Condition,
        branches: &[BranchCondition],
    ) -> Option<Vec<(BranchCondition, Condition)>> {
        let mut condition = start.clone();
        let mut path = Vec::with_capacity(branches.len());
        for branch in branches {
            condition = self.condition_for_branch(&condition, branch)?;
            path.push((branch.clone(), condition.clone()));
        }
        Some(path)
    }

    fn shrink_branches(
        &mut self,
        start: &Condition,
        target: &Condition,
        source: &[BranchCondition],
    ) -> Vec<BranchCondition> {
        let mut retained = Vec::new();
        for (index, branch) in source.iter().enumerate() {
            let mut candidate = retained.clone();
            candidate.extend_from_slice(&source[index + 1..]);
            if self.condition_for_branches(start, &candidate).as_ref() != Some(target) {
                retained.push(branch.clone());
            }
        }

        if retained
            .iter()
            .any(|branch| matches!(branch, BranchCondition::Type(_)))
            && target.possible_types.ordered.len() == 1
        {
            let object_name = self
                .possible_types
                .object_name(target.possible_types.ordered[0])
                .clone();
            let mut singleton = vec![BranchCondition::Type(object_name)];
            singleton.extend(retained.iter().filter_map(|branch| match branch {
                BranchCondition::Boolean(literal) => {
                    Some(BranchCondition::Boolean(literal.clone()))
                }
                BranchCondition::Type(_) => None,
            }));
            if self.condition_for_branches(start, &singleton).as_ref() == Some(target) {
                return singleton;
            }
        }
        retained
    }

    fn deepest_existing_prefix(
        &self,
        start: &Condition,
        path: &[(BranchCondition, Condition)],
    ) -> (NodeId, Condition, usize) {
        let mut best = (
            self.tree
                .id_for_condition(start)
                .expect("the path start must already exist"),
            start.clone(),
            0,
        );
        for (index, (_branch, condition)) in path.iter().enumerate() {
            if let Some(id) = self.tree.id_for_condition(condition) {
                best = (id, condition.clone(), index + 1);
            }
        }
        best
    }

    fn insert_field(
        &mut self,
        source_path: &[(BranchCondition, Condition)],
        target: Condition,
        field: Node<executable::Field>,
    ) {
        if let Some(id) = self.tree.id_for_condition(&target) {
            self.add_field(id, field);
            return;
        }

        let root = self.tree.node(self.tree.root()).condition.clone();
        let (_source_id, source_condition, source_length) =
            self.deepest_existing_prefix(&root, source_path);
        let remaining_path = &source_path[source_length..];
        let remaining = remaining_path
            .iter()
            .map(|(branch, _condition)| branch.clone())
            .collect::<Vec<_>>();
        let shrunk = self.shrink_branches(&source_condition, &target, &remaining);
        let retained = if shrunk == remaining {
            remaining_path.to_vec()
        } else {
            self.path_for_branches(&source_condition, &shrunk)
                .filter(|path| path.last().map(|item| &item.1) == Some(&target))
                .unwrap_or_else(|| remaining_path.to_vec())
        };
        let (mut parent, _parent_condition, retained_length) =
            self.deepest_existing_prefix(&source_condition, &retained);
        let missing = erase_path_cycles(&retained[retained_length..]);

        if missing.is_empty() {
            self.add_field(parent, field);
            return;
        }

        for (branch_condition, condition) in &missing {
            let child = self.tree.push_node(condition.clone());
            self.tree.node_mut(parent).branches.push(Branch {
                condition: branch_condition.clone(),
                body: child,
            });
            parent = child;
        }
        self.add_field(parent, field);
    }

    fn add_field(&mut self, id: NodeId, field: Node<executable::Field>) {
        self.tree
            .node_mut(id)
            .fields
            .entry(field.response_key().clone())
            .or_default()
            .push(field);
    }
}

fn branch_conditions_for_directives(
    directives: &DirectiveList,
    variables: &VariableEnvironment<'_>,
) -> Option<Vec<BranchCondition>> {
    let mut path = Vec::new();
    for directive in &directives.0 {
        let required_value = match directive.name.as_str() {
            "include" => true,
            "skip" => false,
            _ => continue,
        };
        let value = directive
            .arguments
            .iter()
            .find(|argument| argument.name == "if")
            .map(|argument| argument.value.as_ref());
        let variable_name = match value {
            Some(Value::Boolean(actual)) if *actual == required_value => continue,
            Some(Value::Boolean(_)) | Some(Value::Null) | None => return None,
            Some(Value::Variable(variable_name)) => variable_name,
            Some(_) => return None,
        };
        if matches!(
            variables.boolean(variable_name),
            BooleanValue::Known(actual) if actual != required_value
        ) {
            return None;
        }
        path.push(BranchCondition::Boolean(BooleanLiteral {
            variable_name: variable_name.clone(),
            required_value,
        }));
    }
    Some(path)
}

pub(super) fn canonical_boolean_condition(
    mut condition: Vec<BooleanLiteral>,
) -> Option<Vec<BooleanLiteral>> {
    condition.sort_by(|left, right| {
        left.variable_name.cmp(&right.variable_name).then_with(|| {
            match (left.required_value, right.required_value) {
                (true, false) => Ordering::Less,
                (false, true) => Ordering::Greater,
                _ => Ordering::Equal,
            }
        })
    });
    for adjacent in condition.windows(2) {
        if adjacent[0].variable_name == adjacent[1].variable_name
            && adjacent[0].required_value != adjacent[1].required_value
        {
            return None;
        }
    }
    condition.dedup_by(|right, left| right.variable_name == left.variable_name);
    Some(condition)
}

fn erase_path_cycles(path: &[(BranchCondition, Condition)]) -> Vec<(BranchCondition, Condition)> {
    let mut kept = Vec::new();
    for edge in path {
        if let Some(index) = kept
            .iter()
            .position(|(_branch, condition)| condition == &edge.1)
        {
            kept.truncate(index + 1);
        } else {
            kept.push(edge.clone());
        }
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::build_possible_types;
    use apollo_compiler::response::serde_json_bytes::json;
    use apollo_compiler::ExecutableDocument;
    use apollo_compiler::Schema;

    #[test]
    fn extraction_prunes_only_known_inactive_boolean_branches() {
        let schema =
            Schema::parse_and_validate("type Query { value: Int }", "schema.graphql").unwrap();
        let document = ExecutableDocument::parse_and_validate(
            &schema,
            "query Example($show: Boolean!) { value @include(if: $show) }",
            "query.graphql",
        )
        .unwrap();
        let operation = document.operations.get(Some("Example")).unwrap();
        let possible_types = build_possible_types(&schema);

        for (value, expected_nodes) in [(false, 1), (true, 2)] {
            let values = json!({ "show": value }).as_object().unwrap().clone();
            let variables = VariableEnvironment::new(operation, Some(&values));
            let tree = ConditionTree::extract(
                &document,
                &possible_types,
                &variables,
                &operation.selection_set.ty,
                &[],
                [operation.selection_set.selections.as_slice()],
            )
            .unwrap();

            assert_eq!(tree.nodes.len(), expected_nodes);
        }
    }
}
