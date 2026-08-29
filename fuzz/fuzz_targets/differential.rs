#![no_main]

use graphql_static_analysis_fuzz::lean_oracle::NativeLeanOracle;
use graphql_static_analysis_fuzz::tree_summary::{TreeSummaryInput, LEAN_MODEL_COMMIT};
use libfuzzer_sys::fuzz_target;
use std::sync::{Mutex, OnceLock};

static LEAN_ORACLE: OnceLock<Mutex<NativeLeanOracle>> = OnceLock::new();

fuzz_target!(|data: &[u8]| {
    let input = TreeSummaryInput::from_bytes(data);
    let request_id = input.request_id();
    let payload = input.lean_request();
    let lean = LEAN_ORACLE
        .get_or_init(|| {
            Mutex::new(NativeLeanOracle::from_env_with_model_commit(
                LEAN_MODEL_COMMIT,
            ))
        })
        .lock()
        .expect("lock native Lean oracle")
        .query(&request_id, &payload);
    let mut rust = input.rust_result();
    if std::env::var_os("GRAPHQL_STATIC_ANALYSIS_FUZZ_SENTINEL_MUTANT").is_some() {
        rust.push_str(":sentinel-mutant");
    }
    assert_eq!(
        rust,
        lean,
        "Rust and Lean TreeSummary results disagree\nrequest: {payload}\nquery: {}\nreplay: {}",
        input.query(),
        input.reproduction(),
    );
});
