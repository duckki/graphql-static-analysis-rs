use super::*;
use apollo_compiler::response::JsonValue;
use apollo_compiler::ExecutableDocument;

fn valid(values: &JsonMap) -> &Valid<JsonMap> {
    Valid::assume_valid_ref(values)
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct FieldCount(usize);

struct Count;

#[test]
fn exact_case_is_the_default_analysis_mode() {
    assert_eq!(AnalysisMode::default(), AnalysisMode::ExactCase);
}

impl Algebra for Count {
    type Summary = FieldCount;

    fn empty(&self) -> Self::Summary {
        FieldCount(0)
    }

    fn field(&self, _group: &CollectedFieldGroup, child_summary: Self::Summary) -> Self::Summary {
        FieldCount(1 + child_summary.0)
    }

    fn combine(&self, left: Self::Summary, right: Self::Summary) -> Self::Summary {
        FieldCount(left.0 + right.0)
    }

    fn join(&self, left: Self::Summary, right: Self::Summary) -> Self::Summary {
        FieldCount(left.0.max(right.0))
    }
}

struct SumAlternatives;

impl Algebra for SumAlternatives {
    type Summary = FieldCount;

    fn empty(&self) -> Self::Summary {
        FieldCount(0)
    }

    fn field(&self, _group: &CollectedFieldGroup, child_summary: Self::Summary) -> Self::Summary {
        FieldCount(1 + child_summary.0)
    }

    fn combine(&self, left: Self::Summary, right: Self::Summary) -> Self::Summary {
        FieldCount(left.0 + right.0)
    }

    fn join(&self, left: Self::Summary, right: Self::Summary) -> Self::Summary {
        FieldCount(left.0 + right.0)
    }
}

struct JoinTrace;

#[derive(Clone, Debug, Eq, PartialEq)]
struct ContextEntry {
    response_name: String,
    inherited: Vec<(String, bool)>,
    local: Vec<(String, bool)>,
}

struct ContextTrace;

impl Algebra for JoinTrace {
    type Summary = String;

    fn empty(&self) -> Self::Summary {
        String::new()
    }

    fn field(&self, group: &CollectedFieldGroup, child_summary: Self::Summary) -> Self::Summary {
        if group.response_name() == "node" {
            child_summary
        } else {
            group.response_name().to_string()
        }
    }

    fn combine(&self, left: Self::Summary, right: Self::Summary) -> Self::Summary {
        match (left.is_empty(), right.is_empty()) {
            (true, _) => right,
            (_, true) => left,
            _ => format!("{left}+{right}"),
        }
    }

    fn join(&self, left: Self::Summary, right: Self::Summary) -> Self::Summary {
        format!("({left}|{right})")
    }
}

impl Algebra for ContextTrace {
    type Summary = Vec<ContextEntry>;

    fn empty(&self) -> Self::Summary {
        Vec::new()
    }

    fn field(&self, group: &CollectedFieldGroup, child_summary: Self::Summary) -> Self::Summary {
        let literals = |condition: &[BooleanLiteral]| {
            condition
                .iter()
                .map(|literal| (literal.variable_name.to_string(), literal.required_value))
                .collect()
        };
        let mut summary = vec![ContextEntry {
            response_name: group.response_name().to_string(),
            inherited: literals(&group.inherited_boolean_condition),
            local: literals(&group.boolean_condition),
        }];
        summary.extend(child_summary);
        summary
    }

    fn combine(&self, mut left: Self::Summary, right: Self::Summary) -> Self::Summary {
        left.extend(right);
        left
    }

    fn join(&self, mut left: Self::Summary, right: Self::Summary) -> Self::Summary {
        left.extend(right);
        left
    }
}

#[test]
fn exact_case_groups_response_names_across_conditions() {
    let schema = Schema::parse_and_validate(
        r#"
        type Query { node: Node }
        interface Node { value: Int }
        type A implements Node { value: Int }
        type B implements Node { value: Int }
        "#,
        "schema.graphql",
    )
    .unwrap();
    let document = ExecutableDocument::parse_and_validate(
        &schema,
        r#"{
            node {
                ... on A { value }
                ... on Node { value }
            }
        }"#,
        "query.graphql",
    )
    .unwrap();
    let operation = document.operations.get(None).unwrap();
    let analyzer = Analyzer::new(&schema);

    let syntactic = analyzer
        .operation(&document, operation)
        .mode(AnalysisMode::Syntactic)
        .analyze(&Count)
        .unwrap();
    let exact = analyzer
        .operation(&document, operation)
        .mode(AnalysisMode::ExactCase)
        .analyze(&Count)
        .unwrap();

    assert_eq!(syntactic, FieldCount(3));
    assert_eq!(exact, FieldCount(2));
}

#[test]
fn exact_case_correlates_a_variable_across_child_scopes() {
    let schema = Schema::parse_and_validate(
        "type Query { left: Child right: Child } type Child { value: Int }",
        "schema.graphql",
    )
    .unwrap();
    let document = ExecutableDocument::parse_and_validate(
        &schema,
        r#"
        query Example($show: Boolean!) {
          left { value @include(if: $show) }
          right { value @skip(if: $show) }
        }
        "#,
        "query.graphql",
    )
    .unwrap();
    let operation = document.operations.get(Some("Example")).unwrap();
    let analyzer = Analyzer::new(&schema);

    let syntactic = analyzer
        .operation(&document, operation)
        .mode(AnalysisMode::Syntactic)
        .analyze(&Count)
        .unwrap();
    let exact = analyzer
        .operation(&document, operation)
        .mode(AnalysisMode::ExactCase)
        .analyze(&Count)
        .unwrap();

    assert_eq!(syntactic, FieldCount(4));
    assert_eq!(exact, FieldCount(3));
}

#[test]
fn supplied_variables_prune_conditions() {
    let schema = Schema::parse_and_validate("type Query { value: Int }", "schema.graphql").unwrap();
    let document = ExecutableDocument::parse_and_validate(
        &schema,
        "query Example($show: Boolean!) { value @include(if: $show) }",
        "query.graphql",
    )
    .unwrap();
    let operation = document.operations.get(Some("Example")).unwrap();
    let values = apollo_compiler::response::serde_json_bytes::json!({ "show": false })
        .as_object()
        .unwrap()
        .clone();
    let analyzer = Analyzer::new(&schema);

    for mode in [AnalysisMode::Syntactic, AnalysisMode::ExactCase] {
        let summary = analyzer
            .operation(&document, operation)
            .mode(mode)
            .variable_values(valid(&values))
            .analyze(&Count)
            .unwrap();

        assert_eq!(summary, FieldCount(0));
    }
}

#[test]
fn supplied_variables_prune_conditions_in_child_scopes() {
    let schema = Schema::parse_and_validate(
        "type Query { parent: Parent } type Parent { child: Int }",
        "schema.graphql",
    )
    .unwrap();
    let document = ExecutableDocument::parse_and_validate(
        &schema,
        "query Example($show: Boolean!) { parent { child @include(if: $show) } }",
        "query.graphql",
    )
    .unwrap();
    let operation = document.operations.get(Some("Example")).unwrap();
    let values = apollo_compiler::response::serde_json_bytes::json!({ "show": false })
        .as_object()
        .unwrap()
        .clone();
    let analyzer = Analyzer::new(&schema);

    for mode in [AnalysisMode::Syntactic, AnalysisMode::ExactCase] {
        let summary = analyzer
            .operation(&document, operation)
            .mode(mode)
            .variable_values(valid(&values))
            .analyze(&Count)
            .unwrap();

        assert_eq!(summary, FieldCount(1));
    }
}

#[test]
fn missing_supplied_boolean_is_exactly_absent_but_syntactically_widened() {
    let schema = Schema::parse_and_validate("type Query { value: Int }", "schema.graphql").unwrap();
    let document = ExecutableDocument::parse_and_validate(
        &schema,
        "query Example($show: Boolean!) { value @include(if: $show) }",
        "query.graphql",
    )
    .unwrap();
    let operation = document.operations.get(Some("Example")).unwrap();
    let values = JsonMap::new();
    let analyzer = Analyzer::new(&schema);

    let syntactic = analyzer
        .operation(&document, operation)
        .mode(AnalysisMode::Syntactic)
        .variable_values(valid(&values))
        .analyze(&Count)
        .unwrap();
    let exact = analyzer
        .operation(&document, operation)
        .mode(AnalysisMode::ExactCase)
        .variable_values(valid(&values))
        .analyze(&Count)
        .unwrap();

    assert_eq!(syntactic, FieldCount(1));
    assert_eq!(exact, FieldCount(0));
}

#[test]
fn complete_missing_null_and_non_boolean_values_select_false() {
    let schema = Schema::parse_and_validate("type Query { value: Int }", "schema.graphql").unwrap();
    let document = ExecutableDocument::parse_and_validate(
        &schema,
        "query Example($show: Boolean!) { value @skip(if: $show) }",
        "query.graphql",
    )
    .unwrap();
    let operation = document.operations.get(Some("Example")).unwrap();
    let analyzer = Analyzer::new(&schema);
    let values = [
        JsonMap::new(),
        apollo_compiler::response::serde_json_bytes::json!({ "show": null })
            .as_object()
            .unwrap()
            .clone(),
        apollo_compiler::response::serde_json_bytes::json!({ "show": "invalid" })
            .as_object()
            .unwrap()
            .clone(),
    ];

    for values in &values {
        let summary = analyzer
            .operation(&document, operation)
            .mode(AnalysisMode::ExactCase)
            .variable_values(valid(values))
            .analyze(&Count)
            .unwrap();
        assert_eq!(summary, FieldCount(1));
    }
}

#[test]
fn exact_and_syntactic_groups_keep_inherited_and_local_conditions_distinct() {
    let schema = Schema::parse_and_validate(
        "type Query { parent: Parent } type Parent { child: Int }",
        "schema.graphql",
    )
    .unwrap();
    let document = ExecutableDocument::parse_and_validate(
        &schema,
        "query Example($x: Boolean!) { parent @include(if: $x) { child } }",
        "query.graphql",
    )
    .unwrap();
    let operation = document.operations.get(Some("Example")).unwrap();
    let values = apollo_compiler::response::serde_json_bytes::json!({ "x": true })
        .as_object()
        .unwrap()
        .clone();
    let analyzer = Analyzer::new(&schema);

    let exact = analyzer
        .operation(&document, operation)
        .mode(AnalysisMode::ExactCase)
        .variable_values(valid(&values))
        .analyze(&ContextTrace)
        .unwrap();
    let syntactic = analyzer
        .operation(&document, operation)
        .mode(AnalysisMode::Syntactic)
        .variable_values(valid(&values))
        .analyze(&ContextTrace)
        .unwrap();

    assert_eq!(
        exact,
        [
            ContextEntry {
                response_name: "parent".into(),
                inherited: vec![("x".into(), true)],
                local: vec![],
            },
            ContextEntry {
                response_name: "child".into(),
                inherited: vec![("x".into(), true)],
                local: vec![],
            },
        ]
    );
    assert_eq!(
        syntactic,
        [
            ContextEntry {
                response_name: "parent".into(),
                inherited: vec![],
                local: vec![("x".into(), true)],
            },
            ContextEntry {
                response_name: "child".into(),
                inherited: vec![("x".into(), true)],
                local: vec![],
            },
        ]
    );
}

#[test]
fn syntactic_boolean_alternatives_are_factorized_by_variable() {
    let schema = Schema::parse_and_validate(
        "type Query { included: Int skipped: Int }",
        "schema.graphql",
    )
    .unwrap();
    let document = ExecutableDocument::parse_and_validate(
        &schema,
        r#"
        query Example($show: Boolean!) {
          included @include(if: $show)
          skipped @skip(if: $show)
        }
        "#,
        "query.graphql",
    )
    .unwrap();
    let operation = document.operations.get(Some("Example")).unwrap();
    let analyzer = Analyzer::new(&schema);

    let summary = analyzer
        .operation(&document, operation)
        .mode(AnalysisMode::Syntactic)
        .analyze(&Count)
        .unwrap();

    assert_eq!(summary, FieldCount(1));
}

#[test]
fn syntactic_groups_redundant_parent_types_by_response_name() {
    let schema = Schema::parse_and_validate(
        r#"
        type Query { node: Node }
        interface Node { value: Int }
        interface Left implements Node { value: Int }
        interface Right implements Node { value: Int }
        type A implements Node & Left & Right { value: Int }
        type B implements Node & Left & Right { value: Int }
        "#,
        "schema.graphql",
    )
    .unwrap();
    let document = ExecutableDocument::parse_and_validate(
        &schema,
        "{ node { ... on Left { shared: value } ... on Right { shared: value } } }",
        "query.graphql",
    )
    .unwrap();
    let operation = document.operations.get(None).unwrap();
    let analyzer = Analyzer::new(&schema);

    let summary = analyzer
        .operation(&document, operation)
        .mode(AnalysisMode::Syntactic)
        .analyze(&Count)
        .unwrap();

    assert_eq!(summary, FieldCount(2));
}

#[test]
fn syntactic_merges_equivalent_cumulative_type_conditions() {
    let schema = Schema::parse_and_validate(
        r#"
        type Query { node: Node }
        interface Node { value: Int }
        interface Left implements Node { value: Int }
        interface Right implements Node { value: Int }
        type A implements Node & Left & Right { value: Int }
        type B implements Node { value: Int }
        "#,
        "schema.graphql",
    )
    .unwrap();
    let document = ExecutableDocument::parse_and_validate(
        &schema,
        "{ node { ... on Left { value } ... on Right { value } } }",
        "query.graphql",
    )
    .unwrap();
    let operation = document.operations.get(None).unwrap();

    let summary = Analyzer::new(&schema)
        .operation(&document, operation)
        .mode(AnalysisMode::Syntactic)
        .analyze(&Count)
        .unwrap();

    assert_eq!(summary, FieldCount(2));
}

#[test]
fn syntactic_evaluates_one_product_per_materializable_type_region() {
    let schema = Schema::parse_and_validate(
        r#"
        type Query { node: Node }
        interface Node { value: Int }
        interface Shared implements Node { value: Int }
        type A implements Node & Shared { value: Int }
        type B implements Node & Shared { value: Int }
        type C implements Node { value: Int }
        "#,
        "schema.graphql",
    )
    .unwrap();
    let document = ExecutableDocument::parse_and_validate(
        &schema,
        "{ node { ... on Shared { value } } }",
        "query.graphql",
    )
    .unwrap();
    let operation = document.operations.get(None).unwrap();

    let summary = Analyzer::new(&schema)
        .operation(&document, operation)
        .mode(AnalysisMode::Syntactic)
        .analyze(&SumAlternatives)
        .unwrap();

    assert_eq!(summary, FieldCount(2));
}

#[test]
fn syntactic_preserves_distinct_subset_type_products() {
    let schema = Schema::parse_and_validate(
        r#"
        type Query { node: Node }
        interface Node { value: Int }
        interface Shared implements Node { value: Int }
        type A implements Node & Shared { value: Int }
        type B implements Node & Shared { value: Int }
        type C implements Node { value: Int }
        "#,
        "schema.graphql",
    )
    .unwrap();
    let document = ExecutableDocument::parse_and_validate(
        &schema,
        "{ node { ... on Shared { broad: value } ... on A { narrow: value } } }",
        "query.graphql",
    )
    .unwrap();
    let operation = document.operations.get(None).unwrap();

    let summary = Analyzer::new(&schema)
        .operation(&document, operation)
        .mode(AnalysisMode::Syntactic)
        .analyze(&SumAlternatives)
        .unwrap();

    assert_eq!(summary, FieldCount(4));
}

#[test]
fn alternative_joins_follow_the_lean_right_associated_order() {
    let schema = Schema::parse_and_validate(
        r#"
        type Query { node: Node }
        interface Node { value: Int }
        type A implements Node { value: Int }
        type B implements Node { value: Int }
        type C implements Node { value: Int }
        "#,
        "schema.graphql",
    )
    .unwrap();
    let document = ExecutableDocument::parse_and_validate(
        &schema,
        r#"{
          node {
            ... on A { a: value }
            ... on B { b: value }
            ... on C { c: value }
          }
        }"#,
        "query.graphql",
    )
    .unwrap();
    let operation = document.operations.get(None).unwrap();
    let analyzer = Analyzer::new(&schema);

    for mode in [AnalysisMode::Syntactic, AnalysisMode::ExactCase] {
        let summary = analyzer
            .operation(&document, operation)
            .mode(mode)
            .analyze(&JoinTrace)
            .unwrap();
        assert_eq!(summary, "(a|(b|c))");
    }
}

#[test]
fn supplied_exact_cases_batch_overlapping_type_regions_before_joining() {
    let schema = Schema::parse_and_validate(
        r#"
        type Query { node: Node }
        interface Node { value: Int }
        interface I { value: Int }
        interface J { value: Int }
        type A implements Node & I & J { value: Int }
        type B implements Node & I { value: Int }
        type C implements Node & J { value: Int }
        type D implements Node { value: Int }
        "#,
        "schema.graphql",
    )
    .unwrap();
    let document = ExecutableDocument::parse_and_validate(
        &schema,
        "{ node { base: value ... on I { i: value } ... on J { j: value } } }",
        "query.graphql",
    )
    .unwrap();
    let operation = document.operations.get(None).unwrap();
    let analyzer = Analyzer::new(&schema);
    let analysis = analyzer.operation(&document, operation);

    // An empty supplied map still selects the closed-request evaluator. Its
    // joinMap schedule deliberately differs from the symbolic binary cursor.
    let supplied = analysis
        .variable_values(valid(&JsonMap::default()))
        .analyze(&JoinTrace)
        .unwrap();
    assert_eq!(supplied, "(base+i+j|(base+i|(base+j|base)))");
    let symbolic = analyzer
        .operation(&document, operation)
        .analyze(&JoinTrace)
        .unwrap();
    assert_eq!(symbolic, "((base+i+j|base+i)|(base+j|base))");
}

#[test]
fn symbolic_child_alternatives_keep_parent_field_transfers_separate() {
    struct WrappedFieldTrace;
    impl Algebra for WrappedFieldTrace {
        type Summary = String;

        fn empty(&self) -> Self::Summary {
            String::new()
        }

        fn field(&self, group: &CollectedFieldGroup, children: Self::Summary) -> Self::Summary {
            format!("{}{{{children}}}", group.response_name())
        }

        fn combine(&self, left: Self::Summary, right: Self::Summary) -> Self::Summary {
            JoinTrace.combine(left, right)
        }

        fn join(&self, left: Self::Summary, right: Self::Summary) -> Self::Summary {
            JoinTrace.join(left, right)
        }
    }

    fn assert_schedules(schema: &str, query: &str, symbolic: &str, complete: &str) {
        let schema = Schema::parse_and_validate(schema, "schema.graphql").unwrap();
        let document =
            ExecutableDocument::parse_and_validate(&schema, query, "query.graphql").unwrap();
        let operation = document.operations.get(None).unwrap();
        let analyzer = Analyzer::new(&schema);
        assert_eq!(
            analyzer
                .operation(&document, operation)
                .analyze(&WrappedFieldTrace)
                .unwrap(),
            symbolic
        );
        assert_eq!(
            analyzer
                .operation(&document, operation)
                .variable_values(valid(&JsonMap::default()))
                .analyze(&WrappedFieldTrace)
                .unwrap(),
            complete
        );
    }

    assert_schedules(
        "type Query { node: Node } interface Node { value: Int } \
         type A implements Node { value: Int } type B implements Node { value: Int }",
        "{ node { ... on A { a: value } ... on B { b: value } } }",
        "(node{a{}}|node{b{}})",
        "node{(a{}|b{})}",
    );
    // Covariant field outputs exercise the separate child-type join boundary.
    assert_schedules(
        "type Query { parent: Parent } \
         interface Parent { child: Child } \
         type AParent implements Parent { child: AChild } \
         type BParent implements Parent { child: BChild } \
         interface Child { value: Int } \
         type AChild implements Child { value: Int } \
         type BChild implements Child { value: Int }",
        "{ parent { child { value } } }",
        "(parent{child{value{}}}|parent{child{value{}}})",
        "parent{child{(value{}|value{})}}",
    );
}

#[test]
fn supplied_exact_frontiers_preserve_nested_field_order_and_boolean_context() {
    let schema = Schema::parse_and_validate(
        r#"
        type Query { node: Node }
        interface Node { value: Int }
        interface I { value: Int }
        interface J { value: Int }
        type A implements Node & I & J { value: Int }
        type B implements Node & I { value: Int }
        type C implements Node & J { value: Int }
        type D implements Node { value: Int }
        "#,
        "schema.graphql",
    )
    .unwrap();
    let document = ExecutableDocument::parse_and_validate(
        &schema,
        r#"query($x: Boolean!, $y: Boolean!) {
          node {
            base: value
            ... on I { i: value ... @include(if: $x) { ix: value } }
            ... on J { j: value @include(if: $y) }
          }
        }"#,
        "query.graphql",
    )
    .unwrap();
    let operation = document.operations.get(None).unwrap();
    let values = JsonMap::from_iter([
        ("x".into(), JsonValue::Bool(true)),
        ("y".into(), JsonValue::Bool(true)),
    ]);
    let analyzer = Analyzer::new(&schema);
    let analysis = analyzer
        .operation(&document, operation)
        .variable_values(valid(&values));
    assert_eq!(
        analysis.analyze(&JoinTrace).unwrap(),
        "(base+i+ix+j|(base+i+ix|(base+j|base)))"
    );
    let contexts = analysis.analyze(&ContextTrace).unwrap();
    let base_contexts = contexts
        .iter()
        .filter(|entry| entry.response_name == "base")
        .map(|entry| (entry.inherited.clone(), entry.local.clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        base_contexts,
        vec![
            (vec![("x".into(), true), ("y".into(), true)], vec![]),
            (vec![("x".into(), true)], vec![]),
            (vec![("y".into(), true)], vec![]),
            (vec![], vec![]),
        ]
    );
}

#[test]
fn syntactic_canonicalizes_boolean_directive_order() {
    let schema = Schema::parse_and_validate("type Query { value: Int }", "schema.graphql").unwrap();
    let document = ExecutableDocument::parse_and_validate(
        &schema,
        r#"
        query Example($x: Boolean!, $y: Boolean!) {
          value @include(if: $x) @skip(if: $y)
          value @skip(if: $y) @include(if: $x)
        }
        "#,
        "query.graphql",
    )
    .unwrap();
    let operation = document.operations.get(Some("Example")).unwrap();

    let summary = Analyzer::new(&schema)
        .operation(&document, operation)
        .mode(AnalysisMode::Syntactic)
        .analyze(&Count)
        .unwrap();

    assert_eq!(summary, FieldCount(1));
}

#[test]
fn syntactic_child_scopes_inherit_parent_boolean_conditions() {
    let schema = Schema::parse_and_validate(
        "type Query { parent: Parent } type Parent { child: Int }",
        "schema.graphql",
    )
    .unwrap();
    let document = ExecutableDocument::parse_and_validate(
        &schema,
        "query Example($x: Boolean!) { parent @include(if: $x) { child @skip(if: $x) } }",
        "query.graphql",
    )
    .unwrap();
    let operation = document.operations.get(Some("Example")).unwrap();
    let analyzer = Analyzer::new(&schema);

    let summary = analyzer
        .operation(&document, operation)
        .mode(AnalysisMode::Syntactic)
        .analyze(&Count)
        .unwrap();

    assert_eq!(summary, FieldCount(1));
}

#[test]
fn indexed_possible_types_support_large_schemas_and_preserve_order() {
    use std::fmt::Write as _;

    let mut schema_source = String::from("type Query { node: Node } interface Node { id: ID! }\n");
    for index in 0..130 {
        writeln!(schema_source, "type T{index} implements Node {{ id: ID! }}").unwrap();
    }
    schema_source.push_str("union Reversed = T129 | T0\nunion First = T0\n");
    let schema = Schema::parse_and_validate(schema_source, "schema.graphql").unwrap();
    let possible_types = build_possible_types(&schema);
    let node = Name::new("Node").unwrap();
    let reversed = Name::new("Reversed").unwrap();
    let first = Name::new("First").unwrap();

    assert_eq!(possible_types.names(&node).count(), 130);
    assert_eq!(
        possible_types
            .names(&reversed)
            .map(Name::as_str)
            .collect::<Vec<_>>(),
        ["T129", "T0"]
    );

    let narrowed = possible_types.intersection(possible_types.get(&reversed).unwrap(), &first);
    assert_eq!(
        narrowed
            .ordered
            .iter()
            .map(|&index| possible_types.object_name(index).as_str())
            .collect::<Vec<_>>(),
        ["T0"]
    );
}
