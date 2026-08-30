//! Shared GraphQL schema, structural operation grammar, and request values.

use super::TreeSummaryInput;
use apollo_compiler::response::serde_json_bytes::json;
use apollo_compiler::response::JsonMap;
use std::fmt::Write as _;

pub(super) const SCHEMA: &str = r#"
    directive @tag(flag: Boolean) on FIELD | INLINE_FRAGMENT

    interface Animal { name: String id: String friend: Animal friends: [Animal] }
    interface I { name: String id: String friend: Animal friends: [Animal] }
    interface J { name: String id: String friend: Animal friends: [Animal] }
    interface K { name: String id: String friend: Animal friends: [Animal] }

    type Dog implements Animal & I & J & K {
        name: String id: String friend: Animal friends: [Animal]
    }
    type Cat implements Animal & I & K {
        name: String id: String friend: Animal friends: [Animal]
    }
    type Fox implements Animal & J {
        name: String id: String friend: Animal friends: [Animal]
    }

    type Query { animals: [Animal] }
"#;

impl TreeSummaryInput {
    pub fn query(&self) -> String {
        self.query_with_fragments(false)
    }

    pub(super) fn query_with_fragments(&self, named_fragments: bool) -> String {
        let selections = if self.structural {
            let mut cursor = ByteCursor::new(&self.data[usize::min(7, self.data.len())..]);
            let depth = 1 + cursor.next() % 3;
            structural_selection_set(depth, &mut cursor)
        } else {
            legacy_selection_set(self.family)
        };
        let mut variables = Vec::new();
        collect_variables(&selections, &mut variables);
        let defaults = match self.default_case {
            1 => " = false",
            2 => " = true",
            _ => "",
        };
        let definitions = if variables.is_empty() {
            String::new()
        } else {
            format!(
                "({})",
                variables
                    .iter()
                    .map(|name| format!("${name}: Boolean!{defaults}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let body = selections
            .iter()
            .map(SelectionSpec::render)
            .collect::<Vec<_>>()
            .join(" ");
        if named_fragments {
            format!(
                "query Case{definitions} {{ animals {{ ...Generated ...Generated }} }} \
                 fragment Generated on Animal {{ {body} ...RustOnly }} \
                 fragment RustOnly on Animal {{ rustOnlyName: name @tag(flag: true) }}"
            )
        } else {
            format!("query Case{definitions} {{ animals {{ {body} }} }}")
        }
    }
}

#[derive(Clone)]
enum InputSpec {
    Variable(&'static str),
    Boolean(bool),
}

#[derive(Clone)]
enum DirectiveSpec {
    Include(InputSpec),
    Skip(InputSpec),
}

#[derive(Clone)]
enum SelectionSpec {
    Field {
        response_name: &'static str,
        field_name: &'static str,
        directives: Vec<DirectiveSpec>,
        children: Vec<Self>,
    },
    InlineFragment {
        type_condition: Option<&'static str>,
        directives: Vec<DirectiveSpec>,
        children: Vec<Self>,
    },
}

impl SelectionSpec {
    fn render(&self) -> String {
        match self {
            Self::Field {
                response_name,
                field_name,
                directives,
                children,
            } => {
                let alias = if response_name == field_name {
                    String::new()
                } else {
                    format!("{response_name}: ")
                };
                let directives = render_directives(directives);
                let children = if children.is_empty() {
                    String::new()
                } else {
                    format!(
                        " {{ {} }}",
                        children
                            .iter()
                            .map(Self::render)
                            .collect::<Vec<_>>()
                            .join(" ")
                    )
                };
                format!("{alias}{field_name}{directives}{children}")
            }
            Self::InlineFragment {
                type_condition,
                directives,
                children,
            } => {
                let condition = type_condition
                    .map(|name| format!(" on {name}"))
                    .unwrap_or_default();
                format!(
                    "...{condition}{} {{ {} }}",
                    render_directives(directives),
                    children
                        .iter()
                        .map(Self::render)
                        .collect::<Vec<_>>()
                        .join(" ")
                )
            }
        }
    }
}

fn render_directives(directives: &[DirectiveSpec]) -> String {
    let mut rendered = String::new();
    for directive in directives {
        let (name, value) = match directive {
            DirectiveSpec::Include(value) => ("include", value),
            DirectiveSpec::Skip(value) => ("skip", value),
        };
        let value = match value {
            InputSpec::Variable(name) => format!("${name}"),
            InputSpec::Boolean(value) => value.to_string(),
        };
        write!(rendered, " @{name}(if: {value})").unwrap();
    }
    rendered
}

fn collect_variables(selections: &[SelectionSpec], variables: &mut Vec<&'static str>) {
    for selection in selections {
        let (directives, children) = match selection {
            SelectionSpec::Field {
                directives,
                children,
                ..
            }
            | SelectionSpec::InlineFragment {
                directives,
                children,
                ..
            } => (directives, children),
        };
        for directive in directives {
            let value = match directive {
                DirectiveSpec::Include(value) | DirectiveSpec::Skip(value) => value,
            };
            if let InputSpec::Variable(name) = value {
                if !variables.contains(name) {
                    variables.push(name);
                }
            }
        }
        collect_variables(children, variables);
    }
}

struct ByteCursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> ByteCursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn next(&mut self) -> u8 {
        let value = self.bytes.get(self.offset).copied().unwrap_or(0);
        self.offset = self.offset.saturating_add(1);
        value
    }
}

fn structural_selection_set(depth: u8, cursor: &mut ByteCursor<'_>) -> Vec<SelectionSpec> {
    let count = 1 + cursor.next() % 3;
    (0..count)
        .map(|_| structural_selection(depth, cursor))
        .collect()
}

fn structural_selection(depth: u8, cursor: &mut ByteCursor<'_>) -> SelectionSpec {
    let tag = cursor.next();
    let kind = if depth == 0 { tag % 2 } else { tag % 4 };
    match kind {
        0 | 1 => {
            let field_name = if kind == 0 { "name" } else { "id" };
            SelectionSpec::Field {
                response_name: scalar_response_name(field_name, cursor.next()),
                field_name,
                directives: directive_specs(cursor.next()),
                children: Vec::new(),
            }
        }
        2 => {
            let field_name = if cursor.next().is_multiple_of(2) {
                "friend"
            } else {
                "friends"
            };
            SelectionSpec::Field {
                response_name: composite_response_name(field_name, cursor.next()),
                field_name,
                directives: directive_specs(cursor.next()),
                children: structural_selection_set(depth - 1, cursor),
            }
        }
        _ => SelectionSpec::InlineFragment {
            type_condition: type_condition(cursor.next()),
            directives: directive_specs(cursor.next()),
            children: structural_selection_set(depth - 1, cursor),
        },
    }
}

fn scalar_response_name(field_name: &'static str, token: u8) -> &'static str {
    match (field_name, token % 3) {
        ("name", 0) => "name",
        ("name", 1) => "nameLabel",
        ("name", _) => "nameValue",
        ("id", 0) => "id",
        ("id", 1) => "identifier",
        _ => "idValue",
    }
}

fn composite_response_name(field_name: &'static str, token: u8) -> &'static str {
    match (field_name, token % 2) {
        ("friend", 0) => "friend",
        ("friend", _) => "node",
        ("friends", 0) => "friends",
        _ => "nodes",
    }
}

fn type_condition(token: u8) -> Option<&'static str> {
    // Every generated structural condition overlaps every other one through `Dog`.
    // Disjoint object alternatives remain covered by the legacy seed families.
    match token % 5 {
        0 => None,
        1 => Some("Animal"),
        2 => Some("I"),
        3 => Some("J"),
        _ => Some("K"),
    }
}

fn directive_specs(token: u8) -> Vec<DirectiveSpec> {
    use DirectiveSpec::{Include, Skip};
    use InputSpec::{Boolean, Variable};
    match token % 16 {
        0 => vec![],
        1 => vec![Include(Variable("x"))],
        2 => vec![Skip(Variable("x"))],
        3 => vec![Include(Variable("y"))],
        4 => vec![Skip(Variable("y"))],
        5 => vec![Include(Boolean(true))],
        6 => vec![Include(Boolean(false))],
        7 => vec![Skip(Boolean(true))],
        8 => vec![Skip(Boolean(false))],
        9 => vec![Include(Variable("x")), Skip(Variable("y"))],
        10 => vec![Include(Variable("y")), Skip(Variable("x"))],
        11 => vec![Include(Variable("x")), Skip(Variable("x"))],
        12 => vec![Include(Variable("y")), Skip(Variable("y"))],
        13 => vec![Include(Boolean(true)), Skip(Variable("x"))],
        14 => vec![Include(Variable("x")), Skip(Boolean(false))],
        _ => vec![Include(Boolean(false)), Skip(Boolean(true))],
    }
}

fn legacy_selection_set(family: u8) -> Vec<SelectionSpec> {
    use DirectiveSpec::{Include, Skip};
    use InputSpec::Variable;
    let field = |response_name, field_name, directives| SelectionSpec::Field {
        response_name,
        field_name,
        directives,
        children: Vec::new(),
    };
    let inline = |type_condition, directives, children| SelectionSpec::InlineFragment {
        type_condition,
        directives,
        children,
    };
    match family {
        0 => vec![field("name", "name", vec![])],
        1 => vec![field("name", "name", vec![Skip(Variable("x"))])],
        2 => vec![field("name", "name", vec![Include(Variable("x"))])],
        3 => vec![
            field("included", "name", vec![Include(Variable("x"))]),
            field("skipped", "id", vec![Skip(Variable("x"))]),
        ],
        4 => vec![
            field("nameLabel", "name", vec![]),
            field("nameLabel", "name", vec![Include(Variable("x"))]),
        ],
        5 => vec![
            field("a", "name", vec![Include(Variable("x"))]),
            field("b", "id", vec![Include(Variable("y"))]),
        ],
        6 => vec![inline(
            None,
            vec![Include(Variable("x"))],
            vec![inline(
                None,
                vec![Skip(Variable("y"))],
                vec![field("name", "name", vec![])],
            )],
        )],
        7 => vec![
            inline(Some("I"), vec![], vec![field("i", "name", vec![])]),
            inline(Some("J"), vec![], vec![field("j", "id", vec![])]),
        ],
        8 => vec![
            field("nameLabel", "name", vec![]),
            inline(
                Some("Dog"),
                vec![],
                vec![field("nameLabel", "name", vec![])],
            ),
        ],
        9 => vec![
            field("nameLabel", "name", vec![]),
            inline(
                None,
                vec![Include(Variable("x"))],
                vec![inline(
                    None,
                    vec![Include(Variable("y"))],
                    vec![field("nameLabel", "name", vec![])],
                )],
            ),
        ],
        10 => vec![
            inline(
                Some("I"),
                vec![Include(Variable("x"))],
                vec![field("i", "name", vec![])],
            ),
            inline(
                Some("J"),
                vec![Skip(Variable("y"))],
                vec![field("j", "id", vec![])],
            ),
        ],
        _ => vec![
            inline(Some("Dog"), vec![], vec![field("dog", "name", vec![])]),
            inline(Some("Cat"), vec![], vec![field("cat", "id", vec![])]),
        ],
    }
}

pub(super) fn variable_values(variable_case: u8) -> Option<JsonMap> {
    match variable_case {
        0 => None,
        1 => Some(JsonMap::new()),
        2 => Some(
            json!({ "x": false, "y": false })
                .as_object()
                .unwrap()
                .clone(),
        ),
        3 => Some(json!({ "x": true, "y": true }).as_object().unwrap().clone()),
        4 => Some(
            json!({ "x": false, "y": true })
                .as_object()
                .unwrap()
                .clone(),
        ),
        5 => Some(
            json!({ "x": true, "y": false })
                .as_object()
                .unwrap()
                .clone(),
        ),
        6 => Some(json!({ "x": null, "y": null }).as_object().unwrap().clone()),
        7 => Some(json!({ "x": true }).as_object().unwrap().clone()),
        8 => Some(json!({ "y": true }).as_object().unwrap().clone()),
        _ => Some(
            json!({ "x": "non-boolean", "y": 1 })
                .as_object()
                .unwrap()
                .clone(),
        ),
    }
}
