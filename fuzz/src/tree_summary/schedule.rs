//! Order-sensitive alignment audit, separate from the four stable fuzz observations.

use super::operation::{variable_values, SCHEMA};
use super::TreeSummaryInput;
use apollo_compiler::validation::Valid;
use apollo_compiler::{ExecutableDocument, Schema};
use graphql_static_analysis::{Algebra, Analyzer, CollectedFieldGroup};

impl TreeSummaryInput {
    pub fn rust_schedule_result(&self) -> String {
        let schema = Schema::parse_and_validate(SCHEMA, "schema.graphql").unwrap();
        let document =
            ExecutableDocument::parse_and_validate(&schema, self.query(), "case.graphql").unwrap();
        let operation = document.operations.get(Some("Case")).unwrap();
        let variables = variable_values(self.variable_case);
        let analyzer = Analyzer::new(&schema);
        let analysis = analyzer.operation(&document, operation).mode(self.mode());
        let result = match &variables {
            Some(values) => analysis
                .variable_values(Valid::assume_valid_ref(values))
                .analyze(&Schedule),
            None => analysis.analyze(&Schedule),
        }
        .unwrap();
        format!("schedule:{}", result.join("&"))
    }

    pub fn lean_schedule_request(&self) -> String {
        self.lean_request().replacen("TS2 ", "TS2S ", 1)
    }
}

struct Schedule;

impl Algebra for Schedule {
    // Append normalizes combine's identity and associativity. Join remains an
    // explicit binary term; fields and their recursive children retain their order.
    type Summary = Vec<String>;

    fn empty(&self) -> Self::Summary {
        Vec::new()
    }

    fn field(&self, group: &CollectedFieldGroup, children: Self::Summary) -> Self::Summary {
        let mut types = group
            .possible_types
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        types.sort_unstable();
        let mut booleans = group
            .child_inherited_boolean_condition()
            .iter()
            .map(|literal| {
                format!(
                    "{}={}",
                    literal.variable_name,
                    u8::from(literal.required_value)
                )
            })
            .collect::<Vec<_>>();
        booleans.sort_unstable();
        let mut fields = group
            .fields()
            .iter()
            .map(|field| field.name.to_string())
            .collect::<Vec<_>>();
        fields.sort_unstable();
        vec![format!(
            "{}<{}>[{}]#{}{{{}}}",
            group.response_name(),
            types.join("+"),
            booleans.join("+"),
            fields.join("+"),
            children.join("&")
        )]
    }

    fn combine(&self, mut left: Self::Summary, right: Self::Summary) -> Self::Summary {
        left.extend(right);
        left
    }

    fn join(&self, left: Self::Summary, right: Self::Summary) -> Self::Summary {
        vec![format!("({}|{})", left.join("&"), right.join("&"))]
    }
}
