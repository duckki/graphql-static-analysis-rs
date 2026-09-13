use graphql_static_analysis_fuzz::lean_oracle::NativeLeanOracle;
use graphql_static_analysis_fuzz::tree_summary::{TreeSummaryInput, LEAN_MODEL_COMMIT};

fn main() {
    let mut arguments = std::env::args().skip(1);
    let oracle_path = arguments
        .next()
        .expect("usage: schedule_alignment LEAN_ORACLE [--input-hex HEX]");
    let inputs: Box<dyn Iterator<Item = TreeSummaryInput>> = match arguments.next() {
        None => Box::new(
            TreeSummaryInput::exhaustive_cases()
                .filter(|input| input.observation == 0)
                .chain((1..=2_000).map(TreeSummaryInput::from_seed))
                .filter(|input| input.mode == 0),
        ),
        Some(flag) => {
            assert_eq!(flag, "--input-hex");
            let hex = arguments.next().expect("missing hexadecimal input");
            assert!(
                hex.is_ascii() && hex.len().is_multiple_of(2),
                "invalid hex input"
            );
            let bytes = (0..hex.len())
                .step_by(2)
                .map(|offset| u8::from_str_radix(&hex[offset..offset + 2], 16).unwrap())
                .collect::<Vec<_>>();
            let input = TreeSummaryInput::from_bytes(&bytes);
            assert_eq!(input.mode, 0, "schedule audit requires ExactCase mode");
            Box::new(std::iter::once(input))
        }
    };
    assert!(arguments.next().is_none(), "unexpected extra argument");
    let mut oracle = NativeLeanOracle::new_with_model_commit(oracle_path, LEAN_MODEL_COMMIT);
    let mut count = 0;
    for input in inputs {
        let lean = oracle.query(&input.request_id(), &input.lean_schedule_request());
        let rust = input.rust_schedule_result();
        assert_eq!(
            rust,
            lean,
            "ExactCase schedule disagreement: {}\n{}",
            input.reproduction(),
            input.query()
        );
        count += 1;
    }
    println!("{count} ExactCase Lean/Rust schedules agreed");
}
