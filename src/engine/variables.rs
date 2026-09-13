//! Immutable request values and operation defaults used by extraction and traversal.

use apollo_compiler::ast::Value;
use apollo_compiler::executable::Operation;
use apollo_compiler::response::{JsonMap, JsonValue};
use apollo_compiler::Name;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum BooleanValue {
    Missing,
    Known(bool),
    Unknown,
}

pub(super) struct VariableEnvironment<'a> {
    operation: &'a Operation,
    supplied: Option<&'a JsonMap>,
}

impl<'a> VariableEnvironment<'a> {
    pub(super) fn new(operation: &'a Operation, supplied: Option<&'a JsonMap>) -> Self {
        Self {
            operation,
            supplied,
        }
    }

    pub(super) fn boolean(&self, name: &Name) -> BooleanValue {
        let Some(supplied) = self.supplied else {
            return BooleanValue::Unknown;
        };
        if let Some(value) = supplied.get(name.as_str()) {
            return match value {
                JsonValue::Bool(value) => BooleanValue::Known(*value),
                JsonValue::Null => BooleanValue::Missing,
                _ => BooleanValue::Missing,
            };
        }
        match self
            .operation
            .variables
            .iter()
            .find(|definition| definition.name == *name)
            .and_then(|definition| definition.default_value.as_deref())
        {
            Some(Value::Boolean(value)) => BooleanValue::Known(*value),
            _ => BooleanValue::Missing,
        }
    }

    pub(super) fn is_complete(&self) -> bool {
        self.supplied.is_some()
    }
}
