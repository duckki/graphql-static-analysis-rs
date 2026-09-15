//! Persistent case assignments corresponding to ExactCases.Internal.extendBooleanCondition.

use crate::engine::BooleanLiteral;
use apollo_compiler::Name;
use std::rc::Rc;

pub(super) type CaseCondition = Option<Rc<CaseConditionEntry>>;

pub(super) struct CaseConditionEntry {
    pub(super) literal: BooleanLiteral,
    pub(super) previous: CaseCondition,
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

pub(super) fn extend_case_condition(
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
