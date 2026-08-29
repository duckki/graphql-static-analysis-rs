//! Bounded TreeSummary input decoding and deterministic case matrices.

use graphql_static_analysis::AnalysisMode;
use std::fmt::Write as _;

const MAX_INPUT_BYTES: usize = 64;
const FAMILY_COUNT: u8 = 12;
const VARIABLE_CASE_COUNT: u8 = 10;
const MODE_COUNT: u8 = 2;
pub(super) const OBSERVATION_COUNT: u8 = 4;
const DEFAULT_CASE_COUNT: u8 = 3;

pub const LEAN_MODEL_COMMIT: &str = "c354cdb9a4296f46f0fb78871ed2500c32a5fec8";

// Depth, selection count, and node tokens. These are coverage seeds, not the grammar's
// complete input space; libFuzzer may mutate every token and append more nodes.
const STRUCTURAL_SHAPES: &[&[u8]] = &[
    &[0, 0, 0, 0],
    &[0, 2, 0, 1, 1, 1, 2, 2],
    &[1, 1, 2, 0, 9, 1, 0, 1],
    &[1, 2, 3, 1, 0, 1, 0, 1, 3, 2, 10, 0, 0, 2],
    &[2, 1, 2, 1, 11, 2, 0, 1, 1, 0, 3, 4, 9, 1, 0, 2],
    &[2, 2, 3, 5, 1, 1, 0, 0, 3, 6, 2, 1, 1, 7, 0, 2, 14],
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TreeSummaryInput {
    pub(super) data: Vec<u8>,
    pub family: u8,
    pub variable_case: u8,
    pub mode: u8,
    pub list_size: u8,
    pub observation: u8,
    pub default_case: u8,
    pub structural: bool,
}

impl TreeSummaryInput {
    pub fn from_bytes(data: &[u8]) -> Self {
        let data = data
            .iter()
            .copied()
            .take(MAX_INPUT_BYTES)
            .collect::<Vec<_>>();
        let byte = |index| data.get(index).copied().unwrap_or(0);
        Self {
            family: byte(0) % FAMILY_COUNT,
            variable_case: byte(1) % VARIABLE_CASE_COUNT,
            mode: byte(2) % MODE_COUNT,
            list_size: byte(3) % 5,
            observation: byte(4) % OBSERVATION_COUNT,
            default_case: byte(5) % DEFAULT_CASE_COUNT,
            structural: byte(6) % 2 == 1,
            data,
        }
    }

    pub fn from_seed(mut seed: u32) -> Self {
        let mut bytes = [0_u8; 48];
        for byte in &mut bytes {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            *byte = seed as u8;
        }
        Self::from_bytes(&bytes)
    }

    pub fn exhaustive_cases() -> impl Iterator<Item = Self> {
        let mut cases = Vec::new();
        for family in 0..FAMILY_COUNT {
            for variable_case in 0..VARIABLE_CASE_COUNT {
                for mode in 0..MODE_COUNT {
                    for list_size in 0..5 {
                        for observation in 0..OBSERVATION_COUNT {
                            for default_case in 0..DEFAULT_CASE_COUNT {
                                cases.push(Self::from_bytes(&[
                                    family,
                                    variable_case,
                                    mode,
                                    list_size,
                                    observation,
                                    default_case,
                                ]));
                            }
                        }
                    }
                }
            }
        }
        for shape in STRUCTURAL_SHAPES {
            for variable_case in 0..VARIABLE_CASE_COUNT {
                for mode in 0..MODE_COUNT {
                    for observation in 0..OBSERVATION_COUNT {
                        for default_case in 0..DEFAULT_CASE_COUNT {
                            let mut data =
                                vec![0, variable_case, mode, 2, observation, default_case, 1];
                            data.extend_from_slice(shape);
                            cases.push(Self::from_bytes(&data));
                        }
                    }
                }
            }
        }
        cases.into_iter()
    }

    /// A deterministic corpus that removes header combinations ignored by the
    /// Rust-only target while retaining every legacy family, variable state, list
    /// bound, default state, structural seed, and a broad sample of generated trees.
    pub fn coverage_cases() -> impl Iterator<Item = Self> {
        let mut cases = Vec::new();
        for family in 0..FAMILY_COUNT {
            for variable_case in 0..VARIABLE_CASE_COUNT {
                for list_size in 0..5 {
                    for default_case in 0..DEFAULT_CASE_COUNT {
                        cases.push(Self::from_bytes(&[
                            family,
                            variable_case,
                            0,
                            list_size,
                            0,
                            default_case,
                            0,
                        ]));
                    }
                }
            }
        }
        for shape in STRUCTURAL_SHAPES {
            for variable_case in 0..VARIABLE_CASE_COUNT {
                for list_size in 0..5 {
                    for default_case in 0..DEFAULT_CASE_COUNT {
                        let mut data = vec![0, variable_case, 0, list_size, 0, default_case, 1];
                        data.extend_from_slice(shape);
                        cases.push(Self::from_bytes(&data));
                    }
                }
            }
        }
        // Keep enough generated trees to exercise the ordered Split/Join combinations
        // used by ExactCase without making the coverage job depend on a mutable fuzz
        // corpus. The xorshift generator makes this set stable across runs.
        cases.extend((0..2_048).map(Self::from_seed));
        cases.into_iter()
    }

    pub fn bytes(&self) -> &[u8] {
        &self.data
    }

    pub fn request_id(&self) -> String {
        let hex = encode_hex(&self.data);
        if hex.is_empty() {
            "empty".to_string()
        } else {
            hex
        }
    }

    pub fn lean_request(&self) -> String {
        let bytes = if self.data.is_empty() {
            "-".to_string()
        } else {
            self.data
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(",")
        };
        format!("TS2 {} {bytes}", self.request_id())
    }

    pub fn mode(&self) -> AnalysisMode {
        match self.mode {
            0 => AnalysisMode::ExactCase,
            _ => AnalysisMode::Syntactic,
        }
    }

    pub fn reproduction(&self) -> String {
        format!("--input-hex {}", encode_hex(&self.data))
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(encoded, "{byte:02x}").unwrap();
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoder_is_bounded() {
        let input = TreeSummaryInput::from_bytes(&[255; MAX_INPUT_BYTES + 10]);
        assert_eq!(input.bytes().len(), MAX_INPUT_BYTES);
    }

    #[test]
    fn deterministic_matrix_sizes_are_stable() {
        assert_eq!(TreeSummaryInput::exhaustive_cases().count(), 15_840);
        assert_eq!(TreeSummaryInput::coverage_cases().count(), 4_748);
    }
}
