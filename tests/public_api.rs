use apollo_compiler::ExecutableDocument;
use apollo_compiler::Schema;
use graphql_static_analysis::max_response_size::estimate;
use graphql_static_analysis::AnalysisMode;

#[test]
fn maximum_response_size_is_available_through_the_public_api() {
    let schema =
        Schema::parse_and_validate("type Query { greeting: String }", "schema.graphql").unwrap();
    let document =
        ExecutableDocument::parse_and_validate(&schema, "query { greeting }", "operation.graphql")
            .unwrap();
    let schema = schema.into_inner();
    let document = document.into_inner();
    let operation = document.operations.iter().next().unwrap();

    assert_eq!(
        estimate(
            &schema,
            &document,
            operation,
            AnalysisMode::Syntactic,
            100,
            None,
        )
        .unwrap(),
        1
    );
}
