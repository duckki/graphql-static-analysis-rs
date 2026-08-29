use std::hint::black_box;
use std::time::{Duration, Instant};

use apollo_compiler::response::serde_json_bytes::json;
use apollo_compiler::response::JsonMap;
use apollo_compiler::{ExecutableDocument, Schema};
use graphql_static_analysis::max_response_size::MaxResponseSizeEstimator;
use graphql_static_analysis::AnalysisMode;

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

fn schema_sdl(object_count: usize) -> String {
    let mut output = String::from(
        r#"
type Query { nodes: [Node] }

interface Node { id: ID! }
"#,
    );
    for index in 0..ABSTRACT_TYPE_COUNT {
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
        let interfaces = (0..4)
            .map(|offset| format!("NodeSubset{}", (index + offset) % ABSTRACT_TYPE_COUNT))
            .collect::<Vec<_>>()
            .join(" & ");
        output.push_str(&format!(
            r#"
type NodeType{index} implements Node & {interfaces} {{
  id: ID!
  includedValue: String
  skippedValue: String
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
    let schema =
        Schema::parse_and_validate(schema_sdl(object_count), "benchmark-schema.graphql").unwrap();
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
        VariableInput::Supplied => Some(&scenario.variables),
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
    samples.sort_unstable();
    let total_ns = samples[samples.len() / 2];
    println!(
        "{},{},{},{},{},{},{},{},{},{}",
        backend.name(),
        variable_input.name(),
        object_count,
        ABSTRACT_TYPE_COUNT,
        query_spreads,
        result,
        iterations,
        total_ns,
        total_ns / iterations,
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
    samples.sort_unstable();
    let total_ns = samples[samples.len() / 2];
    println!(
        "{},{},{},{},{},{},{},{},{}",
        backend.name(),
        variable_input.name(),
        variable_count,
        2 * variable_count,
        result,
        iterations,
        total_ns,
        total_ns / iterations,
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
            "backend,variables,boolean_variables_per_region,total_boolean_variables,response_size,iterations,median_total_ns,median_ns_per_op,checksum"
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
        "backend,variables,object_types,abstract_types,query_spreads,response_size,iterations,median_total_ns,median_ns_per_op,checksum"
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
