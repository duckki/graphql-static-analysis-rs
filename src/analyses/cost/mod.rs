//! `graphql-static-analysis` example implementing the IBM GraphQL Cost Directives estimate.
//!
//! The estimator reports type cost and field cost independently. A missing list-size
//! bound is infinite by default, as required for a conservative static estimate. Use
//! [`CostEstimator::default_list_size`] to choose a finite deployment fallback.

use crate::AnalysisError;

mod estimator;
mod model;

pub use estimator::estimate;
pub use estimator::CostEstimator;
pub use model::CostModel;

/// The two independent costs defined by the IBM proposal.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cost {
    pub type_cost: f64,
    pub field_cost: f64,
}

impl Cost {
    pub const ZERO: Self = Self {
        type_cost: 0.0,
        field_cost: 0.0,
    };

    fn add(self, other: Self) -> Self {
        Self {
            type_cost: self.type_cost + other.type_cost,
            field_cost: self.field_cost + other.field_cost,
        }
    }

    fn max(self, other: Self) -> Self {
        Self {
            type_cost: self.type_cost.max(other.type_cost),
            field_cost: self.field_cost.max(other.field_cost),
        }
    }
}

#[derive(Clone, Debug, thiserror::Error, PartialEq)]
pub enum CostError {
    #[error(transparent)]
    Analysis(#[from] AnalysisError),

    #[error("invalid IBM @cost weight `{value}` at `{coordinate}`")]
    InvalidWeight { coordinate: String, value: String },

    #[error("invalid IBM @listSize argument `{argument}` at `{coordinate}`")]
    InvalidListSize {
        coordinate: String,
        argument: &'static str,
    },
}
