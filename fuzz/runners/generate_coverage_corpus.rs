use graphql_static_analysis_fuzz::tree_summary::TreeSummaryInput;
use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let output = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .expect("usage: generate_coverage_corpus OUTPUT_DIRECTORY");
    fs::create_dir_all(&output).expect("create coverage corpus directory");
    let mut count = 0;
    for (index, input) in TreeSummaryInput::coverage_cases().enumerate() {
        fs::write(output.join(format!("case-{index:04}")), input.bytes())
            .expect("write coverage corpus case");
        count += 1;
    }
    println!("wrote {count} deterministic TreeSummary coverage cases");
}
