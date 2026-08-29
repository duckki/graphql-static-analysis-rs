#![no_main]

use graphql_static_analysis_fuzz::tree_summary::TreeSummaryInput;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    TreeSummaryInput::from_bytes(data).exercise_rust_paths();
});
