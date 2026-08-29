use graphql_static_analysis_fuzz::lean_oracle::NativeLeanOracle;
use graphql_static_analysis_fuzz::tree_summary::{TreeSummaryInput, LEAN_MODEL_COMMIT};
use std::env;
use std::path::PathBuf;

struct Options {
    oracle: PathBuf,
    input: Option<Vec<u8>>,
    seed: u32,
    cases: usize,
    exhaustive: bool,
    mode: Option<u8>,
    observation: Option<u8>,
    variable_case: Option<u8>,
}

fn main() {
    let options = options();
    let mut inputs = if let Some(bytes) = options.input {
        vec![TreeSummaryInput::from_bytes(&bytes)]
    } else if options.exhaustive {
        TreeSummaryInput::exhaustive_cases().collect()
    } else {
        (0..options.cases)
            .map(|offset| TreeSummaryInput::from_seed(options.seed.wrapping_add(offset as u32)))
            .collect()
    };
    if let Some(mode) = options.mode {
        inputs.retain(|input| input.mode == mode);
    }
    if let Some(observation) = options.observation {
        inputs.retain(|input| input.observation == observation);
    }
    if let Some(variable_case) = options.variable_case {
        inputs.retain(|input| input.variable_case == variable_case);
    }

    let mut oracle = NativeLeanOracle::new_with_model_commit(&options.oracle, LEAN_MODEL_COMMIT);
    for (index, input) in inputs.iter().enumerate() {
        let request_id = input.request_id();
        let payload = input.lean_request();
        let lean = oracle.query(&request_id, &payload);
        let rust = input.rust_result();
        if rust != lean {
            eprintln!("TreeSummary disagreement at case {index}");
            eprintln!("request: {payload}");
            eprintln!("query: {}", input.query());
            eprintln!("Rust: {rust}");
            eprintln!("Lean: {lean}");
            eprintln!("replay: {}", input.reproduction());
            std::process::exit(1);
        }
    }
    println!("{} TreeSummary Lean/Rust cases agreed", inputs.len());
}

fn options() -> Options {
    let mut arguments = env::args().skip(1);
    let mut oracle = None;
    let mut input = None;
    let mut seed = 1;
    let mut cases = 100;
    let mut exhaustive = false;
    let mut mode = None;
    let mut observation = None;
    let mut variable_case = None;
    while let Some(argument) = arguments.next() {
        let mut value = || {
            arguments
                .next()
                .unwrap_or_else(|| usage(&format!("missing value for {argument}")))
        };
        match argument.as_str() {
            "--lean-oracle" => oracle = Some(PathBuf::from(value())),
            "--input-hex" => input = Some(decode_hex(&value())),
            "--seed" => seed = value().parse().unwrap_or_else(|_| usage("invalid seed")),
            "--cases" => cases = value().parse().unwrap_or_else(|_| usage("invalid cases")),
            "--exhaustive" => exhaustive = true,
            "--mode" => {
                mode = Some(match value().as_str() {
                    "exact" => 0,
                    "syntactic" => 1,
                    _ => usage("--mode must be exact or syntactic"),
                })
            }
            "--observation" => {
                observation = Some(match value().as_str() {
                    "max" => 0,
                    "cases" => 1,
                    "trace" => 2,
                    "cost" => 3,
                    _ => usage("--observation must be max, cases, trace, or cost"),
                })
            }
            "--variable-case" => {
                let parsed = value()
                    .parse()
                    .unwrap_or_else(|_| usage("invalid variable case"));
                if parsed > 9 {
                    usage("--variable-case must be between 0 and 9");
                }
                variable_case = Some(parsed);
            }
            "--help" | "-h" => usage(""),
            _ => usage(&format!("unknown option: {argument}")),
        }
    }
    Options {
        oracle: oracle.unwrap_or_else(|| usage("--lean-oracle is required")),
        input,
        seed,
        cases,
        exhaustive,
        mode,
        observation,
        variable_case,
    }
}

fn decode_hex(value: &str) -> Vec<u8> {
    if value.len() & 1 != 0 {
        usage("--input-hex must contain an even number of digits");
    }
    (0..value.len())
        .step_by(2)
        .map(|offset| {
            u8::from_str_radix(&value[offset..offset + 2], 16)
                .unwrap_or_else(|_| usage("--input-hex must be hexadecimal"))
        })
        .collect()
}

fn usage(error: &str) -> ! {
    if !error.is_empty() {
        eprintln!("error: {error}\n");
    }
    eprintln!(
        "usage: differential --lean-oracle PATH [--input-hex HEX | --seed N --cases N | --exhaustive] [--mode exact|syntactic] [--observation max|cases|trace|cost] [--variable-case 0..9]"
    );
    std::process::exit(2)
}
