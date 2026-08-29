#![doc = include_str!("../README.md")]
#![deny(unreachable_pub)]

mod analyses;
mod engine;

pub use analyses::cost;
pub use analyses::max_response_size;
pub use engine::Algebra;
pub use engine::Analysis;
pub use engine::AnalysisError;
pub use engine::AnalysisMode;
pub use engine::Analyzer;
pub use engine::BooleanLiteral;
pub use engine::CollectedFieldGroup;
