use std::collections::BTreeMap;
use std::fs;
use std::hint::black_box;
use std::time::{Duration, Instant};

use apollo_compiler::response::serde_json_bytes::json;
use apollo_compiler::response::JsonMap;
use apollo_compiler::validation::Valid;
use apollo_compiler::{ExecutableDocument, Schema};
use graphql_static_analysis::cost::{CostEstimator, CostModel};
use graphql_static_analysis::max_response_size::MaxResponseSizeEstimator;
use graphql_static_analysis::AnalysisMode;
use serde::Deserialize;

const ABSTRACT_TYPE_COUNT: usize = 80;
const LIST_SIZE: u64 = 10;
const TARGET_SAMPLE: Duration = Duration::from_millis(100);
const SAMPLE_COUNT: usize = 5;
const MAX_CALIBRATION_ITERATIONS: u64 = 1_048_576;
const PATHOLOGICAL_BOOLEAN_MAX_K: usize = 6;

#[derive(Clone, Copy)]
enum Backend {
    ExactCase,
    Syntactic,
}

impl Backend {
    fn mode(self) -> AnalysisMode {
        match self {
            Self::ExactCase => AnalysisMode::ExactCase,
            Self::Syntactic => AnalysisMode::Syntactic,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::ExactCase => "exact-case",
            Self::Syntactic => "syntactic",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "exact-case" => Self::ExactCase,
            "syntactic" => Self::Syntactic,
            _ => panic!("expected exact-case or syntactic"),
        }
    }
}

#[derive(Clone, Copy)]
enum VariableInput {
    Unknown,
    Supplied,
}

impl VariableInput {
    fn name(self) -> &'static str {
        match self {
            Self::Unknown => "without-values",
            Self::Supplied => "with-values",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "without-values" => Self::Unknown,
            "with-values" => Self::Supplied,
            _ => panic!("expected without-values or with-values"),
        }
    }
}

struct Scenario {
    schema: Schema,
    document: ExecutableDocument,
    variables: JsonMap,
}

#[derive(Deserialize)]
struct StudyCorpus {
    #[serde(default)]
    schemas: BTreeMap<String, String>,
    cases: Vec<StudyCase>,
}

#[derive(Deserialize)]
struct StudyCase {
    id: String,
    schema: String,
    query: String,
    variables: JsonMap,
}

fn run_study(path: &str) {
    let corpus: StudyCorpus =
        serde_json::from_str(&fs::read_to_string(path).expect("read study corpus"))
            .expect("parse study corpus");
    for test_case in corpus.cases {
        let schema_source = corpus
            .schemas
            .get(&test_case.schema)
            .unwrap_or(&test_case.schema);
        for backend in [Backend::ExactCase, Backend::Syntactic] {
            let result = (|| {
                let schema = Schema::parse_and_validate(schema_source, "study-schema.graphql")
                    .map_err(|error| format!("schema: {error:?}"))?;
                let document = ExecutableDocument::parse_and_validate(
                    &schema,
                    &test_case.query,
                    "study-query.graphql",
                )
                .map_err(|error| format!("query: {error:?}"))?;
                let schema = schema.into_inner();
                let document = document.into_inner();
                let operation = document
                    .operations
                    .iter()
                    .next()
                    .ok_or_else(|| "query has no operation".to_string())?;
                let model = CostModel::from_schema(&schema)
                    .map_err(|error| format!("cost model: {error}"))?;
                CostEstimator::new(model)
                    .mode(backend.mode())
                    .estimate(
                        &document,
                        operation,
                        Valid::assume_valid_ref(&test_case.variables),
                    )
                    .map_err(|error| format!("estimate: {error}"))
            })();
            let system = format!("graphql-static-analysis-rs-{}", backend.name());
            match result {
                Ok(cost) => println!(
                    "{}",
                    serde_json::json!({
                        "system": system,
                        "version": env!("CARGO_PKG_VERSION"),
                        "case": test_case.id,
                        "status": "ok",
                        "type_cost": cost.type_cost,
                        "field_cost": cost.field_cost,
                    })
                ),
                Err(error) => println!(
                    "{}",
                    serde_json::json!({
                        "system": system,
                        "version": env!("CARGO_PKG_VERSION"),
                        "case": test_case.id,
                        "status": "error",
                        "error": error,
                    })
                ),
            }
        }
    }
}

fn schema_sdl_with_topology(
    object_count: usize,
    abstract_type_count: usize,
    incidences_per_object: usize,
) -> String {
    schema_sdl_with_declared_topology(
        object_count,
        abstract_type_count,
        abstract_type_count,
        incidences_per_object,
    )
}

fn schema_sdl_with_declared_topology(
    object_count: usize,
    declared_abstract_type_count: usize,
    membership_abstract_type_count: usize,
    incidences_per_object: usize,
) -> String {
    assert!(declared_abstract_type_count >= membership_abstract_type_count);
    assert!(membership_abstract_type_count > 0);
    assert!(incidences_per_object > 0 && incidences_per_object <= membership_abstract_type_count);
    let mut output = String::from(
        r#"
directive @cost(weight: String!)
  on ARGUMENT_DEFINITION | ENUM | FIELD_DEFINITION |
     INPUT_FIELD_DEFINITION | OBJECT | SCALAR

type Query { nodes: [Node] }

interface Node { id: ID! }
"#,
    );
    for index in 0..declared_abstract_type_count {
        output.push_str(&format!(
            r#"
interface NodeSubset{index} implements Node {{
  id: ID!
  includedValue: String
  skippedValue: String
}}
"#,
        ));
    }
    for index in 0..object_count {
        let interfaces = (0..incidences_per_object)
            .map(|offset| {
                format!(
                    "NodeSubset{}",
                    (index + offset) % membership_abstract_type_count
                )
            })
            .collect::<Vec<_>>()
            .join(" & ");
        output.push_str(&format!(
            r#"
type NodeType{index} implements Node & {interfaces} {{
  id: ID!
  includedValue: String @cost(weight: "1")
  skippedValue: String @cost(weight: "7")
}}
"#,
        ));
    }
    output
}

fn operation_source(query_spreads: usize) -> String {
    let mut output = String::from(
        r#"query Benchmark($includeBranch: Boolean!, $skipBranch: Boolean!) {
  nodes {
"#,
    );
    for index in 0..query_spreads {
        if index % 2 == 0 {
            output.push_str(&format!(
                "    ... on NodeSubset{index} @include(if: $includeBranch) {{\n      sharedIncluded: includedValue\n    }}\n"
            ));
        } else {
            output.push_str(&format!(
                "    ... on NodeSubset{index} @skip(if: $skipBranch) {{\n      sharedSkipped: skippedValue\n    }}\n"
            ));
        }
    }
    output.push_str("  }\n}\n");
    output
}

fn scenario(object_count: usize, query_spreads: usize) -> Scenario {
    scenario_with_topology(object_count, ABSTRACT_TYPE_COUNT, 4, query_spreads)
}

fn scenario_with_topology(
    object_count: usize,
    abstract_type_count: usize,
    incidences_per_object: usize,
    query_spreads: usize,
) -> Scenario {
    assert!(query_spreads <= abstract_type_count);
    let schema = Schema::parse_and_validate(
        schema_sdl_with_topology(object_count, abstract_type_count, incidences_per_object),
        "benchmark-schema.graphql",
    )
    .unwrap();
    let document = ExecutableDocument::parse_and_validate(
        &schema,
        operation_source(query_spreads),
        "benchmark-query.graphql",
    )
    .unwrap();
    let variables = json!({
        "includeBranch": true,
        "skipBranch": true,
    })
    .as_object()
    .unwrap()
    .clone();
    Scenario {
        schema: schema.into_inner(),
        document: document.into_inner(),
        variables,
    }
}

fn scenario_with_unused_abstract_types(
    object_count: usize,
    declared_abstract_type_count: usize,
    membership_abstract_type_count: usize,
    incidences_per_object: usize,
    query_spreads: usize,
) -> Scenario {
    assert!(query_spreads <= membership_abstract_type_count);
    let schema = Schema::parse_and_validate(
        schema_sdl_with_declared_topology(
            object_count,
            declared_abstract_type_count,
            membership_abstract_type_count,
            incidences_per_object,
        ),
        "benchmark-schema.graphql",
    )
    .unwrap();
    let document = ExecutableDocument::parse_and_validate(
        &schema,
        operation_source(query_spreads),
        "benchmark-query.graphql",
    )
    .unwrap();
    let variables = json!({
        "includeBranch": true,
        "skipBranch": true,
    })
    .as_object()
    .unwrap()
    .clone();
    Scenario {
        schema: schema.into_inner(),
        document: document.into_inner(),
        variables,
    }
}

fn structure_scenario(nesting_depth: usize, response_fan_in: usize) -> Scenario {
    assert!(response_fan_in > 0);
    let mut schema_source = String::from("type Query { root: Level0 }\n");
    for level in 0..nesting_depth {
        schema_source.push_str(&format!(
            "type Level{level} {{ child: Level{} }}\n",
            level + 1,
        ));
    }
    schema_source.push_str(&format!(
        "type Level{nesting_depth} {{ value: String @cost(weight: \"1\") }}\n"
    ));
    schema_source.push_str(
        "directive @cost(weight: String!) on ARGUMENT_DEFINITION | ENUM | FIELD_DEFINITION | INPUT_FIELD_DEFINITION | OBJECT | SCALAR\n",
    );
    let schema = Schema::parse_and_validate(schema_source, "structure-schema.graphql").unwrap();

    let mut operation = String::from("query Benchmark { root {");
    for _ in 0..nesting_depth {
        operation.push_str(" child {");
    }
    for _ in 0..response_fan_in {
        operation.push_str(" shared: value");
    }
    for _ in 0..nesting_depth {
        operation.push_str(" }");
    }
    operation.push_str(" } }");
    let document =
        ExecutableDocument::parse_and_validate(&schema, operation, "structure-query.graphql")
            .unwrap();
    Scenario {
        schema: schema.into_inner(),
        document: document.into_inner(),
        variables: JsonMap::new(),
    }
}

fn pathological_boolean_scenario(variable_count: usize) -> Scenario {
    let schema = Schema::parse_and_validate(
        r#"
            type Query { subject: Subject }
            union Subject = Left | Right
            type Left { value: String }
            type Right { value: String }
        "#,
        "pathological-boolean-schema.graphql",
    )
    .unwrap();

    let variable_names = ["left", "right"]
        .into_iter()
        .flat_map(|region| (0..variable_count).map(move |index| format!("{region}{index}")))
        .collect::<Vec<_>>();
    let definitions = variable_names
        .iter()
        .map(|name| format!("${name}: Boolean!"))
        .collect::<Vec<_>>()
        .join(", ");
    let fields = |region: &str| {
        (0..variable_count)
            .map(|index| format!("{region}Value{index}: value @include(if: ${region}{index})"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let operation = format!(
        "query Benchmark({definitions}) {{\n  subject {{\n    ... on Left {{\n{}\n    }}\n    ... on Right {{\n{}\n    }}\n  }}\n}}",
        fields("left"),
        fields("right"),
    );
    let document = ExecutableDocument::parse_and_validate(
        &schema,
        operation,
        "pathological-boolean-query.graphql",
    )
    .unwrap();
    let variables = variable_names
        .into_iter()
        .map(|name| (name.into(), true.into()))
        .collect();

    Scenario {
        schema: schema.into_inner(),
        document: document.into_inner(),
        variables,
    }
}

fn estimate(
    scenario: &Scenario,
    estimator: &MaxResponseSizeEstimator<'_>,
    variable_input: VariableInput,
) -> u64 {
    let operation = scenario.document.operations.get(Some("Benchmark")).unwrap();
    let variables = match variable_input {
        VariableInput::Unknown => None,
        VariableInput::Supplied => Some(Valid::assume_valid_ref(&scenario.variables)),
    };
    estimator
        .estimate(&scenario.document, operation, LIST_SIZE, variables)
        .unwrap()
}

fn expected(backend: Backend, variable_input: VariableInput) -> u64 {
    match (backend, variable_input) {
        (Backend::ExactCase, VariableInput::Unknown) => 21,
        (Backend::ExactCase, VariableInput::Supplied) => 11,
        (Backend::Syntactic, VariableInput::Unknown) => 41,
        (Backend::Syntactic, VariableInput::Supplied) => 21,
    }
}

fn run_iterations(
    scenario: &Scenario,
    estimator: &MaxResponseSizeEstimator<'_>,
    variable_input: VariableInput,
    iterations: u64,
) -> u64 {
    let mut checksum = 0_u64;
    for _ in 0..iterations {
        checksum = checksum.wrapping_add(black_box(estimate(
            black_box(scenario),
            black_box(estimator),
            black_box(variable_input),
        )));
    }
    black_box(checksum)
}

fn timed(
    scenario: &Scenario,
    estimator: &MaxResponseSizeEstimator<'_>,
    variable_input: VariableInput,
    iterations: u64,
) -> (Duration, u64) {
    let start = Instant::now();
    let checksum = run_iterations(scenario, estimator, variable_input, iterations);
    (start.elapsed(), checksum)
}

fn calibration_iterations(
    scenario: &Scenario,
    estimator: &MaxResponseSizeEstimator<'_>,
    variable_input: VariableInput,
) -> u64 {
    let mut iterations = 1;
    loop {
        let (elapsed, _) = timed(scenario, estimator, variable_input, iterations);
        if elapsed >= TARGET_SAMPLE || iterations >= MAX_CALIBRATION_ITERATIONS {
            return iterations;
        }
        iterations *= 2;
    }
}

fn benchmark(
    scenario: &Scenario,
    object_count: usize,
    query_spreads: usize,
    backend: Backend,
    variable_input: VariableInput,
) {
    let estimator = MaxResponseSizeEstimator::new(&scenario.schema).mode(backend.mode());
    let result = estimate(scenario, &estimator, variable_input);
    assert_eq!(result, expected(backend, variable_input));
    run_iterations(scenario, &estimator, variable_input, 2);
    let iterations = calibration_iterations(scenario, &estimator, variable_input);
    let mut samples = Vec::with_capacity(SAMPLE_COUNT);
    let mut checksum = 0_u64;
    for _ in 0..SAMPLE_COUNT {
        let (elapsed, value) = timed(scenario, &estimator, variable_input, iterations);
        samples.push(elapsed.as_nanos() as u64);
        checksum = checksum.wrapping_add(value);
    }
    let mut sorted_samples = samples.clone();
    sorted_samples.sort_unstable();
    let total_ns = sorted_samples[sorted_samples.len() / 2];
    println!(
        "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
        backend.name(),
        variable_input.name(),
        object_count,
        ABSTRACT_TYPE_COUNT,
        query_spreads,
        result,
        iterations,
        total_ns,
        total_ns / iterations,
        samples[0],
        samples[1],
        samples[2],
        samples[3],
        samples[4],
        checksum,
    );
}

fn benchmark_pathological_booleans(
    scenario: &Scenario,
    variable_count: usize,
    backend: Backend,
    variable_input: VariableInput,
) {
    let estimator = MaxResponseSizeEstimator::new(&scenario.schema).mode(backend.mode());
    let result = estimate(scenario, &estimator, variable_input);
    assert_eq!(result, variable_count as u64 + 1);
    run_iterations(scenario, &estimator, variable_input, 2);
    let iterations = calibration_iterations(scenario, &estimator, variable_input);
    let mut samples = Vec::with_capacity(SAMPLE_COUNT);
    let mut checksum = 0_u64;
    for _ in 0..SAMPLE_COUNT {
        let (elapsed, value) = timed(scenario, &estimator, variable_input, iterations);
        samples.push(elapsed.as_nanos() as u64);
        checksum = checksum.wrapping_add(value);
    }
    let mut sorted_samples = samples.clone();
    sorted_samples.sort_unstable();
    let total_ns = sorted_samples[sorted_samples.len() / 2];
    println!(
        "{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
        backend.name(),
        variable_input.name(),
        variable_count,
        2 * variable_count,
        result,
        iterations,
        total_ns,
        total_ns / iterations,
        samples[0],
        samples[1],
        samples[2],
        samples[3],
        samples[4],
        checksum,
    );
}

fn estimate_cost(scenario: &Scenario, estimator: &CostEstimator<'_>) -> (f64, f64) {
    let operation = scenario.document.operations.get(Some("Benchmark")).unwrap();
    let cost = estimator
        .estimate(
            &scenario.document,
            operation,
            Valid::assume_valid_ref(&scenario.variables),
        )
        .unwrap();
    (cost.type_cost, cost.field_cost)
}

fn run_cost_iterations(scenario: &Scenario, estimator: &CostEstimator<'_>, iterations: u64) -> u64 {
    let mut checksum = 0_u64;
    for _ in 0..iterations {
        let (type_cost, field_cost) = estimate_cost(black_box(scenario), black_box(estimator));
        checksum = checksum.wrapping_add((type_cost + field_cost) as u64);
    }
    black_box(checksum)
}

fn timed_cost(
    scenario: &Scenario,
    estimator: &CostEstimator<'_>,
    iterations: u64,
) -> (Duration, u64) {
    let start = Instant::now();
    let checksum = run_cost_iterations(scenario, estimator, iterations);
    (start.elapsed(), checksum)
}

fn cost_calibration_iterations(scenario: &Scenario, estimator: &CostEstimator<'_>) -> u64 {
    let mut iterations = 1;
    loop {
        let (elapsed, _) = timed_cost(scenario, estimator, iterations);
        if elapsed >= TARGET_SAMPLE || iterations >= MAX_CALIBRATION_ITERATIONS {
            return iterations;
        }
        iterations *= 2;
    }
}

fn benchmark_cost(
    scenario: &Scenario,
    object_count: usize,
    abstract_type_count: usize,
    incidences_per_object: usize,
    query_spreads: usize,
    backend: Backend,
    expected: Option<(f64, f64)>,
) {
    let estimator = CostEstimator::new(CostModel::from_schema(&scenario.schema).unwrap())
        .mode(backend.mode())
        .default_list_size(1);
    let (type_cost, field_cost) = estimate_cost(scenario, &estimator);
    if let Some(expected) = expected {
        assert_eq!((type_cost, field_cost), expected);
    }
    run_cost_iterations(scenario, &estimator, 2);
    let iterations = cost_calibration_iterations(scenario, &estimator);
    let mut samples = Vec::with_capacity(SAMPLE_COUNT);
    let mut checksum = 0_u64;
    for _ in 0..SAMPLE_COUNT {
        let (elapsed, value) = timed_cost(scenario, &estimator, iterations);
        samples.push(elapsed.as_nanos() as u64);
        checksum = checksum.wrapping_add(value);
    }
    let mut sorted_samples = samples.clone();
    sorted_samples.sort_unstable();
    let total_ns = sorted_samples[sorted_samples.len() / 2];
    println!(
        "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
        backend.name(),
        object_count,
        abstract_type_count,
        incidences_per_object,
        query_spreads,
        type_cost,
        field_cost,
        iterations,
        total_ns,
        total_ns / iterations,
        samples[0],
        samples[1],
        samples[2],
        samples[3],
        samples[4],
        checksum,
    );
}

fn expected_cost(backend: Backend) -> (f64, f64) {
    match backend {
        Backend::ExactCase => (2.0, 2.0),
        Backend::Syntactic => (2.0, 3.0),
    }
}

fn benchmark_cost_structure(
    scenario: &Scenario,
    nesting_depth: usize,
    response_fan_in: usize,
    backend: Backend,
) {
    let estimator = CostEstimator::new(CostModel::from_schema(&scenario.schema).unwrap())
        .mode(backend.mode())
        .default_list_size(1);
    let (type_cost, field_cost) = estimate_cost(scenario, &estimator);
    run_cost_iterations(scenario, &estimator, 2);
    let iterations = cost_calibration_iterations(scenario, &estimator);
    let mut samples = Vec::with_capacity(SAMPLE_COUNT);
    let mut checksum = 0_u64;
    for _ in 0..SAMPLE_COUNT {
        let (elapsed, value) = timed_cost(scenario, &estimator, iterations);
        samples.push(elapsed.as_nanos() as u64);
        checksum = checksum.wrapping_add(value);
    }
    let mut sorted_samples = samples.clone();
    sorted_samples.sort_unstable();
    let total_ns = sorted_samples[sorted_samples.len() / 2];
    println!(
        "{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
        backend.name(),
        nesting_depth,
        response_fan_in,
        type_cost,
        field_cost,
        iterations,
        total_ns,
        total_ns / iterations,
        samples[0],
        samples[1],
        samples[2],
        samples[3],
        samples[4],
        checksum,
    );
}

fn profile(
    object_count: usize,
    query_spreads: usize,
    backend: Backend,
    variable_input: VariableInput,
    iterations: u64,
) {
    let scenario = scenario(object_count, query_spreads);
    let estimator = MaxResponseSizeEstimator::new(&scenario.schema).mode(backend.mode());
    assert_eq!(
        estimate(&scenario, &estimator, variable_input),
        expected(backend, variable_input),
    );
    println!(
        "checksum={}",
        run_iterations(&scenario, &estimator, variable_input, iterations),
    );
}

fn profile_pathological_booleans(
    variable_count: usize,
    backend: Backend,
    variable_input: VariableInput,
    iterations: u64,
) {
    let scenario = pathological_boolean_scenario(variable_count);
    let estimator = MaxResponseSizeEstimator::new(&scenario.schema).mode(backend.mode());
    assert_eq!(
        estimate(&scenario, &estimator, variable_input),
        variable_count as u64 + 1,
    );
    println!(
        "checksum={}",
        run_iterations(&scenario, &estimator, variable_input, iterations),
    );
}

fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    if args.get(1).map(String::as_str) == Some("study") {
        run_study(&args[2]);
        return;
    }
    if args.get(1).map(String::as_str) == Some("profile") {
        profile(
            args[2].parse().unwrap(),
            args[3].parse().unwrap(),
            Backend::parse(&args[4]),
            VariableInput::parse(&args[5]),
            args[6].parse().unwrap(),
        );
        return;
    }
    if args.get(1).map(String::as_str) == Some("profile-pathological-booleans") {
        profile_pathological_booleans(
            args[2].parse().unwrap(),
            Backend::parse(&args[3]),
            VariableInput::parse(&args[4]),
            args[5].parse().unwrap(),
        );
        return;
    }
    if args.get(1).map(String::as_str) == Some("pathological-booleans") {
        println!(
            "backend,variables,boolean_variables_per_region,total_boolean_variables,response_size,iterations,median_total_ns,median_ns_per_op,sample_0_total_ns,sample_1_total_ns,sample_2_total_ns,sample_3_total_ns,sample_4_total_ns,checksum"
        );
        for variable_count in 1..=PATHOLOGICAL_BOOLEAN_MAX_K {
            let scenario = pathological_boolean_scenario(variable_count);
            for backend in [Backend::ExactCase, Backend::Syntactic] {
                for variable_input in [VariableInput::Unknown, VariableInput::Supplied] {
                    benchmark_pathological_booleans(
                        &scenario,
                        variable_count,
                        backend,
                        variable_input,
                    );
                }
            }
        }
        return;
    }
    if args.get(1).map(String::as_str) == Some("cost-topology-point") {
        let object_count = args[2].parse().unwrap();
        let abstract_type_count = args[3].parse().unwrap();
        let incidences_per_object = args[4].parse().unwrap();
        let query_spreads = args[5].parse().unwrap();
        let backend = Backend::parse(&args[6]);
        println!(
            "backend,object_types,abstract_types,incidences_per_object,query_spreads,type_cost,field_cost,iterations,median_total_ns,median_ns_per_op,sample_0_total_ns,sample_1_total_ns,sample_2_total_ns,sample_3_total_ns,sample_4_total_ns,checksum"
        );
        let scenario = scenario_with_topology(
            object_count,
            abstract_type_count,
            incidences_per_object,
            query_spreads,
        );
        benchmark_cost(
            &scenario,
            object_count,
            abstract_type_count,
            incidences_per_object,
            query_spreads,
            backend,
            None,
        );
        return;
    }
    if args.get(1).map(String::as_str) == Some("cost-unused-abstract-point") {
        let object_count = args[2].parse().unwrap();
        let declared_abstract_type_count = args[3].parse().unwrap();
        let membership_abstract_type_count = args[4].parse().unwrap();
        let incidences_per_object = args[5].parse().unwrap();
        let query_spreads = args[6].parse().unwrap();
        let backend = Backend::parse(&args[7]);
        println!(
            "backend,object_types,abstract_types,incidences_per_object,query_spreads,type_cost,field_cost,iterations,median_total_ns,median_ns_per_op,sample_0_total_ns,sample_1_total_ns,sample_2_total_ns,sample_3_total_ns,sample_4_total_ns,checksum"
        );
        let scenario = scenario_with_unused_abstract_types(
            object_count,
            declared_abstract_type_count,
            membership_abstract_type_count,
            incidences_per_object,
            query_spreads,
        );
        benchmark_cost(
            &scenario,
            object_count,
            declared_abstract_type_count,
            incidences_per_object,
            query_spreads,
            backend,
            None,
        );
        return;
    }
    if args.get(1).map(String::as_str) == Some("cost-structure-point") {
        let nesting_depth = args[2].parse().unwrap();
        let response_fan_in = args[3].parse().unwrap();
        let backend = Backend::parse(&args[4]);
        println!(
            "backend,nesting_depth,response_fan_in,type_cost,field_cost,iterations,median_total_ns,median_ns_per_op,sample_0_total_ns,sample_1_total_ns,sample_2_total_ns,sample_3_total_ns,sample_4_total_ns,checksum"
        );
        let scenario = structure_scenario(nesting_depth, response_fan_in);
        benchmark_cost_structure(&scenario, nesting_depth, response_fan_in, backend);
        return;
    }
    if args.get(1).map(String::as_str) == Some("cost-point") {
        let object_count = args[2].parse().unwrap();
        let query_spreads = args[3].parse().unwrap();
        let backend = Backend::parse(&args[4]);
        println!(
            "backend,object_types,abstract_types,incidences_per_object,query_spreads,type_cost,field_cost,iterations,median_total_ns,median_ns_per_op,sample_0_total_ns,sample_1_total_ns,sample_2_total_ns,sample_3_total_ns,sample_4_total_ns,checksum"
        );
        benchmark_cost(
            &scenario(object_count, query_spreads),
            object_count,
            ABSTRACT_TYPE_COUNT,
            4,
            query_spreads,
            backend,
            Some(expected_cost(backend)),
        );
        return;
    }
    if matches!(
        args.get(1).map(String::as_str),
        Some("cost-schema-size" | "cost-query-size" | "cost-endpoints")
    ) {
        let axis = args[1].as_str();
        let points = match axis {
            "cost-schema-size" => (1..=10).map(|scale| (scale * 1024, 8)).collect(),
            "cost-query-size" => (1..=10).map(|scale| (1024, scale * 8)).collect(),
            "cost-endpoints" => vec![(1024, 8), (10_240, 8), (1024, 80)],
            _ => unreachable!(),
        };
        println!(
            "backend,object_types,abstract_types,incidences_per_object,query_spreads,type_cost,field_cost,iterations,median_total_ns,median_ns_per_op,sample_0_total_ns,sample_1_total_ns,sample_2_total_ns,sample_3_total_ns,sample_4_total_ns,checksum"
        );
        for (object_count, query_spreads) in points {
            let scenario = scenario(object_count, query_spreads);
            for backend in [Backend::ExactCase, Backend::Syntactic] {
                benchmark_cost(
                    &scenario,
                    object_count,
                    ABSTRACT_TYPE_COUNT,
                    4,
                    query_spreads,
                    backend,
                    Some(expected_cost(backend)),
                );
            }
        }
        return;
    }
    let axis = args.get(1).map(String::as_str).unwrap_or("schema-size");
    let points = match axis {
        "schema-size" => (1..=10).map(|scale| (scale * 1024, 8)).collect::<Vec<_>>(),
        "query-size" => (1..=10).map(|scale| (1024, scale * 8)).collect::<Vec<_>>(),
        "endpoints" => vec![(1024, 8), (10_240, 8), (1024, 80)],
        _ => panic!(
            "expected schema-size, query-size, endpoints, pathological-booleans, profile, or profile-pathological-booleans"
        ),
    };
    println!(
        "backend,variables,object_types,abstract_types,query_spreads,response_size,iterations,median_total_ns,median_ns_per_op,sample_0_total_ns,sample_1_total_ns,sample_2_total_ns,sample_3_total_ns,sample_4_total_ns,checksum"
    );
    for (object_count, query_spreads) in points {
        let scenario = scenario(object_count, query_spreads);
        for backend in [Backend::ExactCase, Backend::Syntactic] {
            for variable_input in [VariableInput::Unknown, VariableInput::Supplied] {
                benchmark(
                    &scenario,
                    object_count,
                    query_spreads,
                    backend,
                    variable_input,
                );
            }
        }
    }
}
