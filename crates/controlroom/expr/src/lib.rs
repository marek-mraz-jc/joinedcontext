#![forbid(unsafe_code)]
//! `controlroom-expr`: Grafana server-side expressions (`__expr__` datasource: `math`, `reduce`,
//! `resample`, `threshold`, `classic_conditions`; `sql` warned) and the 13 core data frame
//! transformations of R4 (§3.4, §7.1).

pub mod classic;
pub mod math;
pub mod pipeline;
pub mod reduce;
pub mod resample;
pub mod threshold;
pub mod tx;

pub use classic::{ClassicCondition, eval_classic_conditions};
pub use math::{ExprValue, MathExpr, eval_math_on_vars};
pub use pipeline::{
    ExprNodeSpec, ExprPipelineOutput, evaluate_pipeline, is_expression_datasource_uid,
    parse_duration_ms,
};
pub use reduce::{ReduceMapper, ReduceMode, ReducerId, reduce_frames};
pub use resample::{Upsampler, resample_frames};
pub use threshold::{ThresholdCondition, ThresholdType, eval_threshold_frames};
pub use tx::{TransformationOutcome, apply_transformations};
