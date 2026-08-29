use apollo_compiler::ExecutableDocument;
use apollo_compiler::Schema;
use graphql_static_analysis::max_response_size::MaxResponseSizeEstimator;

fn main() -> Result<(), graphql_static_analysis::AnalysisError> {
    let schema = Schema::parse_and_validate(
        "type Query { users: [User] } type User { name: String }",
        "schema.graphql",
    )
    .expect("valid schema");
    let document = ExecutableDocument::parse_and_validate(
        &schema,
        "query { users { name } }",
        "operation.graphql",
    )
    .expect("valid operation");
    let schema = schema.into_inner();
    let document = document.into_inner();
    let operation = document.operations.iter().next().expect("one operation");

    let estimate =
        MaxResponseSizeEstimator::new(&schema).estimate(&document, operation, 100, None)?;
    println!("maximum response field count: {estimate}");

    Ok(())
}
