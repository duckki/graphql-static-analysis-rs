//! Schema-level IBM cost metadata extraction.

use super::CostError;
use apollo_compiler::ast::Directive;
use apollo_compiler::ast::DirectiveList;
use apollo_compiler::ast::Value;
use apollo_compiler::collections::IndexMap;
use apollo_compiler::schema::ExtendedType;
use apollo_compiler::schema::{self};
use apollo_compiler::Name;
use apollo_compiler::Schema;

#[derive(Clone, Debug, Default)]
pub(super) struct ListSize {
    pub(super) assumed_size: Option<u64>,
    pub(super) slicing_arguments: Vec<Name>,
    pub(super) sized_fields: Vec<Name>,
}

/// Schema-level IBM cost metadata that can be reused across operation estimates.
#[derive(Clone, Debug)]
pub struct CostModel<'schema> {
    pub(super) schema: &'schema Schema,
    pub(super) type_weights: IndexMap<Name, f64>,
    pub(super) field_weights: IndexMap<(Name, Name), f64>,
    pub(super) argument_weights: IndexMap<(Name, Name, Name), f64>,
    pub(super) directive_argument_weights: IndexMap<(Name, Name), f64>,
    pub(super) input_field_weights: IndexMap<(Name, Name), f64>,
    pub(super) list_sizes: IndexMap<(Name, Name), ListSize>,
}

impl<'schema> CostModel<'schema> {
    /// Parses IBM `@cost` and `@listSize` metadata from `schema`.
    pub fn from_schema(schema: &'schema Schema) -> Result<Self, CostError> {
        let mut model = Self {
            schema,
            type_weights: IndexMap::default(),
            field_weights: IndexMap::default(),
            argument_weights: IndexMap::default(),
            directive_argument_weights: IndexMap::default(),
            input_field_weights: IndexMap::default(),
            list_sizes: IndexMap::default(),
        };
        for (directive_name, definition) in &schema.directive_definitions {
            for argument in &definition.arguments {
                if let Some(weight) = parse_weight(
                    &argument.directives,
                    &format!("@{directive_name}({}:)", argument.name),
                )? {
                    model
                        .directive_argument_weights
                        .insert((directive_name.clone(), argument.name.clone()), weight);
                }
            }
        }
        for (type_name, definition) in &schema.types {
            match definition {
                ExtendedType::Scalar(definition) => {
                    model.parse_component_type_weight(type_name, &definition.directives)?;
                }
                ExtendedType::Object(definition) => {
                    model.parse_component_type_weight(type_name, &definition.directives)?;
                    for (field_name, field) in &definition.fields {
                        model.parse_field(type_name, field_name, field)?;
                    }
                }
                ExtendedType::Interface(definition) => {
                    for (field_name, field) in &definition.fields {
                        model.parse_field(type_name, field_name, field)?;
                    }
                }
                ExtendedType::Enum(definition) => {
                    model.parse_component_type_weight(type_name, &definition.directives)?;
                }
                ExtendedType::InputObject(definition) => {
                    for (field_name, field) in &definition.fields {
                        if let Some(weight) =
                            parse_weight(&field.directives, &format!("{type_name}.{field_name}"))?
                        {
                            model
                                .input_field_weights
                                .insert((type_name.clone(), field_name.clone()), weight);
                        }
                    }
                }
                ExtendedType::Union(_) => {}
            }
        }
        Ok(model)
    }

    fn parse_component_type_weight(
        &mut self,
        type_name: &Name,
        directives: &schema::DirectiveList,
    ) -> Result<(), CostError> {
        let coordinate = type_name.to_string();
        if let Some(directive) = directives.get("cost") {
            let weight = parse_weight_directive(directive, &coordinate)?;
            self.type_weights.insert(type_name.clone(), weight);
        }
        Ok(())
    }

    fn parse_field(
        &mut self,
        type_name: &Name,
        field_name: &Name,
        field: &apollo_compiler::ast::FieldDefinition,
    ) -> Result<(), CostError> {
        let coordinate = format!("{type_name}.{field_name}");
        if let Some(weight) = parse_weight(&field.directives, &coordinate)? {
            self.field_weights
                .insert((type_name.clone(), field_name.clone()), weight);
        }
        for argument in &field.arguments {
            if let Some(weight) = parse_weight(
                &argument.directives,
                &format!("{coordinate}({}:)", argument.name),
            )? {
                self.argument_weights.insert(
                    (type_name.clone(), field_name.clone(), argument.name.clone()),
                    weight,
                );
            }
        }
        if let Some(directive) = field.directives.get("listSize") {
            self.list_sizes.insert(
                (type_name.clone(), field_name.clone()),
                parse_list_size(directive, &coordinate)?,
            );
        }
        Ok(())
    }
}

fn parse_weight(directives: &DirectiveList, coordinate: &str) -> Result<Option<f64>, CostError> {
    directives
        .get("cost")
        .map(|directive| parse_weight_directive(directive, coordinate))
        .transpose()
}

fn parse_weight_directive(directive: &Directive, coordinate: &str) -> Result<f64, CostError> {
    let value = directive
        .arguments
        .iter()
        .find(|argument| argument.name == "weight")
        .and_then(|argument| match argument.value.as_ref() {
            Value::String(value) => Some(value.as_str()),
            _ => None,
        })
        .unwrap_or("");
    value
        .parse::<f64>()
        .ok()
        .filter(|weight| weight.is_finite())
        .ok_or_else(|| CostError::InvalidWeight {
            coordinate: coordinate.to_owned(),
            value: value.to_owned(),
        })
}

fn parse_list_size(directive: &Directive, coordinate: &str) -> Result<ListSize, CostError> {
    let mut result = ListSize::default();
    for argument in &directive.arguments {
        match argument.name.as_str() {
            "assumedSize" => {
                let Value::Int(value) = argument.value.as_ref() else {
                    return Err(invalid_list_size(coordinate, "assumedSize"));
                };
                result.assumed_size = value
                    .as_str()
                    .parse::<i64>()
                    .ok()
                    .and_then(|value| u64::try_from(value).ok());
                if result.assumed_size.is_none() {
                    return Err(invalid_list_size(coordinate, "assumedSize"));
                }
            }
            "slicingArguments" => {
                result.slicing_arguments = parse_name_list(&argument.value)
                    .ok_or_else(|| invalid_list_size(coordinate, "slicingArguments"))?;
            }
            "sizedFields" => {
                result.sized_fields = parse_name_list(&argument.value)
                    .ok_or_else(|| invalid_list_size(coordinate, "sizedFields"))?;
            }
            "requireOneSlicingArgument"
                if !matches!(argument.value.as_ref(), Value::Boolean(_)) =>
            {
                return Err(invalid_list_size(coordinate, "requireOneSlicingArgument"));
            }
            _ => {}
        }
    }
    Ok(result)
}

fn invalid_list_size(coordinate: &str, argument: &'static str) -> CostError {
    CostError::InvalidListSize {
        coordinate: coordinate.to_owned(),
        argument,
    }
}

fn parse_name_list(value: &Value) -> Option<Vec<Name>> {
    match value {
        Value::Null => Some(Vec::new()),
        Value::String(value) => Some(vec![Name::new(value).ok()?]),
        Value::List(values) => values
            .iter()
            .map(|value| match value.as_ref() {
                Value::String(value) => Name::new(value).ok(),
                _ => None,
            })
            .collect(),
        _ => None,
    }
}
