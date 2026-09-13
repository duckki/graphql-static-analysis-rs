//! Lean ExactCases.Internal.BooleanDecision: deferred joins and correlated splits.

use crate::engine::Algebra;
use apollo_compiler::Name;

/// Lean's lazy Boolean decision tree. `Join` preserves type and child-parent
/// alternatives without constructing a Cartesian product with unrelated Boolean
/// supports.
#[derive(Clone)]
pub(super) enum BooleanDecision<S> {
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

    pub(super) fn map_owned<T>(self, transform: &impl Fn(S) -> T) -> BooleanDecision<T> {
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

    pub(super) fn zip_with_owned<T: Clone, U>(
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

    // Only use while compacting the completed operation. A Leaf can still have
    // pending parent field transfers during recursive child evaluation.
    fn join_cases(self, right: Self, join: &impl Fn(S, S) -> S) -> Self {
        match (self, right) {
            (Self::Leaf(left), Self::Leaf(right)) => Self::Leaf(join(left, right)),
            (left, right) => Self::Join {
                left: Box::new(left),
                right: Box::new(right),
            },
        }
    }

    pub(super) fn compact(self, join: &impl Fn(S, S) -> S) -> Self {
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

    pub(super) fn collapse<A: Algebra<Summary = S>>(self, algebra: &A) -> S {
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

#[cfg(test)]
mod tests {
    use super::*;

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
