use apollo_compiler::request::coerce_variable_values;
use apollo_compiler::response::JsonMap;
use apollo_compiler::ExecutableDocument;
use apollo_compiler::Schema;
use graphql_static_analysis::cost::CostEstimator;
use graphql_static_analysis::cost::CostModel;

fn main() -> Result<(), graphql_static_analysis::cost::CostError> {
    let schema = Schema::parse_and_validate(
        r#"
            directive @cost(weight: String!) on FIELD_DEFINITION
            type Query { book: Book @cost(weight: "2") }
            type Book { title: String @cost(weight: "1") }
        "#,
        "schema.graphql",
    )
    .expect("valid schema");
    let document = ExecutableDocument::parse_and_validate(
        &schema,
        "query { book { title } }",
        "operation.graphql",
    )
    .expect("valid operation");
    let operation = document.operations.iter().next().expect("one operation");
    let variables =
        coerce_variable_values(&schema, operation, &JsonMap::new()).expect("valid variable values");

    let model = CostModel::from_schema(&schema)?;
    let cost = CostEstimator::new(model).estimate(&document, operation, &variables)?;
    println!(
        "type cost: {}, field cost: {}",
        cost.type_cost, cost.field_cost
    );

    Ok(())
}
