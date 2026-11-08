#![forbid(unsafe_code)]
//! Server-side expression DAG builder, topological sort, and pipeline executor
//! (`GF pkg/expr/graph.go`, `nodes.go`, `commands.go`, `service.go`).

use crate::classic::{ClassicCondition, eval_classic_conditions};
use crate::math::{ExprValue, MathExpr};
use crate::reduce::{ReduceMode, ReducerId, reduce_frames};
use crate::resample::{Upsampler, resample_frames};
use crate::threshold::{ThresholdCondition, ThresholdType, eval_threshold_frames};
use daiya_model::{CrError, CrResult, Frame, FrameMeta, FrameSet, Notice, NoticeSeverity};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap, VecDeque};

/// Checks if a datasource UID identifies Grafana's expression engine (`__expr__` or `-100`).
#[must_use]
pub fn is_expression_datasource_uid(uid: &str) -> bool {
    uid == "__expr__" || uid == "-100"
}

/// Parses a Grafana duration string (`"10s"`, `"5m"`, `"1h"`, `"1d"`, `"500ms"`) into milliseconds.
pub fn parse_duration_ms(raw: &str) -> CrResult<i64> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(CrError::ExpressionError("empty duration".into()));
    }
    if let Some(ms) = s.strip_suffix("ms") {
        let v: i64 = ms
            .trim()
            .parse()
            .map_err(|_| CrError::ExpressionError(format!("invalid duration '{raw}'")))?;
        return Ok(v);
    }
    let (num_str, mult) = if let Some(n) = s.strip_suffix('s') {
        (n, 1_000i64)
    } else if let Some(n) = s.strip_suffix('m') {
        (n, 60_000i64)
    } else if let Some(n) = s.strip_suffix('h') {
        (n, 3_600_000i64)
    } else if let Some(n) = s.strip_suffix('d') {
        (n, 86_400_000i64)
    } else if let Some(n) = s.strip_suffix('w') {
        (n, 7 * 86_400_000i64)
    } else {
        return Err(CrError::ExpressionError(format!(
            "unsupported duration unit in '{raw}'"
        )));
    };
    let n: f64 = num_str
        .trim()
        .parse()
        .map_err(|_| CrError::ExpressionError(format!("invalid duration '{raw}'")))?;
    Ok((n * (mult as f64)) as i64)
}

/// Parsed expression node specification (`CMDNode` in `nodes.go`).
#[derive(Debug, Clone)]
pub enum ExprCommand {
    Math(MathExpr),
    Reduce {
        var_to_reduce: String,
        reducer: ReducerId,
        mode: ReduceMode,
    },
    Resample {
        var_to_resample: String,
        window_ms: i64,
        downsampler: ReducerId,
        upsampler: Upsampler,
    },
    Threshold {
        reference_var: String,
        condition: ThresholdCondition,
    },
    Classic {
        conditions: Vec<ClassicCondition>,
    },
    /// `sql` expression type imports with a named warning (not executed in this run, R4).
    SqlWarning {
        raw_sql: String,
    },
}

impl ExprCommand {
    #[must_use]
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::Math(_) => "math",
            Self::Reduce { .. } => "reduce",
            Self::Resample { .. } => "resample",
            Self::Threshold { .. } => "threshold",
            Self::Classic { .. } => "classic_conditions",
            Self::SqlWarning { .. } => "sql",
        }
    }

    #[must_use]
    pub fn needs_vars(&self) -> Vec<String> {
        match self {
            Self::Math(m) => m.var_names.clone(),
            Self::Reduce { var_to_reduce, .. } => vec![var_to_reduce.clone()],
            Self::Resample {
                var_to_resample, ..
            } => vec![var_to_resample.clone()],
            Self::Threshold { reference_var, .. } => vec![reference_var.clone()],
            Self::Classic { conditions } => {
                conditions.iter().map(|c| c.input_ref_id.clone()).collect()
            }
            Self::SqlWarning { .. } => Vec::new(),
        }
    }
}

/// One expression target node in a panel or query request.
#[derive(Debug, Clone)]
pub struct ExprNodeSpec {
    pub ref_id: String,
    pub hidden: bool,
    pub command: ExprCommand,
}

impl ExprNodeSpec {
    /// Parses a Grafana `__expr__` target JSON object into an [`ExprNodeSpec`] (`nodes.go:buildCMDNode`).
    pub fn from_json(target: &Value, max_depth: usize) -> CrResult<Self> {
        let obj = target.as_object().ok_or_else(|| {
            CrError::ExpressionError("expression target must be an object".into())
        })?;
        let ref_id = obj
            .get("refId")
            .and_then(Value::as_str)
            .unwrap_or("A")
            .to_owned();
        let hidden = obj.get("hide").and_then(Value::as_bool).unwrap_or(false);
        let cmd_type = obj.get("type").and_then(Value::as_str).ok_or_else(|| {
            CrError::ExpressionError(format!("no expression command type in query '{ref_id}'"))
        })?;

        let command = match cmd_type {
            "math" => {
                let expr_str = obj
                    .get("expression")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        CrError::ExpressionError(format!(
                            "math command '{ref_id}' is missing 'expression'"
                        ))
                    })?;
                ExprCommand::Math(MathExpr::parse_with_max_depth(expr_str, max_depth)?)
            }
            "reduce" => {
                let raw_var = obj
                    .get("expression")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        CrError::ExpressionError(format!(
                            "reduce command '{ref_id}' is missing 'expression'"
                        ))
                    })?;
                let var_to_reduce = raw_var.trim_start_matches('$').to_owned();
                let reducer_str = obj.get("reducer").and_then(Value::as_str).ok_or_else(|| {
                    CrError::ExpressionError(format!(
                        "reduce command '{ref_id}' is missing 'reducer'"
                    ))
                })?;
                let reducer = ReducerId::parse(reducer_str)?;
                let mut mode = ReduceMode::Strict;
                if let Some(settings) = obj.get("settings").and_then(Value::as_object) {
                    match settings.get("mode").and_then(Value::as_str).unwrap_or("") {
                        "" => mode = ReduceMode::Strict,
                        "dropNN" => mode = ReduceMode::DropNonNumber,
                        "replaceNN" => {
                            let val = settings
                                .get("replaceWithValue")
                                .and_then(Value::as_f64)
                                .ok_or_else(|| {
                                    CrError::ExpressionError(
                                        "setting replaceWithValue must be specified when mode is 'replaceNN'".into(),
                                    )
                                })?;
                            mode = ReduceMode::ReplaceNonNumber(val);
                        }
                        other => {
                            return Err(CrError::ExpressionError(format!(
                                "reducer mode '{other}' is not supported. Supported only: [dropNN,replaceNN]"
                            )));
                        }
                    }
                }
                ExprCommand::Reduce {
                    var_to_reduce,
                    reducer,
                    mode,
                }
            }
            "resample" => {
                let raw_var = obj
                    .get("expression")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        CrError::ExpressionError(format!(
                            "resample command '{ref_id}' is missing 'expression'"
                        ))
                    })?;
                let var_to_resample = raw_var.trim_start_matches('$').to_owned();
                let window_str = obj.get("window").and_then(Value::as_str).ok_or_else(|| {
                    CrError::ExpressionError(format!(
                        "resample command '{ref_id}' is missing 'window'"
                    ))
                })?;
                let window_ms = parse_duration_ms(window_str)?;
                let ds_str = obj
                    .get("downsampler")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        CrError::ExpressionError(format!(
                            "resample command '{ref_id}' is missing 'downsampler'"
                        ))
                    })?;
                let us_str = obj
                    .get("upsampler")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        CrError::ExpressionError(format!(
                            "resample command '{ref_id}' is missing 'upsampler'"
                        ))
                    })?;
                ExprCommand::Resample {
                    var_to_resample,
                    window_ms,
                    downsampler: ReducerId::parse(ds_str)?,
                    upsampler: Upsampler::parse(us_str)?,
                }
            }
            "threshold" => {
                let raw_var = obj
                    .get("expression")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        CrError::ExpressionError(format!(
                            "threshold command '{ref_id}' is missing 'expression'"
                        ))
                    })?;
                let reference_var = raw_var.trim_start_matches('$').to_owned();
                let conds = obj
                    .get("conditions")
                    .and_then(Value::as_array)
                    .ok_or_else(|| {
                        CrError::ExpressionError(
                            "threshold expression requires 'conditions' array".into(),
                        )
                    })?;
                if conds.len() != 1 {
                    return Err(CrError::ExpressionError(
                        "threshold expression requires exactly one condition".into(),
                    ));
                }
                let first = &conds[0];
                let eval_obj = first
                    .get("evaluator")
                    .and_then(Value::as_object)
                    .ok_or_else(|| {
                        CrError::ExpressionError("threshold condition missing 'evaluator'".into())
                    })?;
                let eval_type = ThresholdType::parse(
                    eval_obj.get("type").and_then(Value::as_str).unwrap_or(""),
                )?;
                let params: Vec<f64> = eval_obj
                    .get("params")
                    .and_then(Value::as_array)
                    .map(|arr| arr.iter().filter_map(Value::as_f64).collect())
                    .unwrap_or_default();

                let unload_evaluator =
                    if let Some(u_obj) = first.get("unloadEvaluator").and_then(Value::as_object) {
                        let uty = ThresholdType::parse(
                            u_obj.get("type").and_then(Value::as_str).unwrap_or(""),
                        )?;
                        let uparams: Vec<f64> = u_obj
                            .get("params")
                            .and_then(Value::as_array)
                            .map(|arr| arr.iter().filter_map(Value::as_f64).collect())
                            .unwrap_or_default();
                        Some((uty, uparams))
                    } else {
                        None
                    };

                ExprCommand::Threshold {
                    reference_var,
                    condition: ThresholdCondition {
                        evaluator_type: eval_type,
                        params,
                        unload_evaluator,
                        loaded_fingerprints: BTreeSet::new(),
                    },
                }
            }
            "classic_conditions" => {
                let conds_arr =
                    obj.get("conditions")
                        .and_then(Value::as_array)
                        .ok_or_else(|| {
                            CrError::ExpressionError(
                                "classic_conditions missing 'conditions' array".into(),
                            )
                        })?;
                let mut conditions = Vec::with_capacity(conds_arr.len());
                for (idx, c_val) in conds_arr.iter().enumerate() {
                    let op = c_val
                        .get("operator")
                        .and_then(|o| o.get("type"))
                        .and_then(Value::as_str)
                        .unwrap_or("and");
                    if idx > 0 && op != "and" && op != "or" && op != "logic-or" {
                        return Err(CrError::ExpressionError(format!(
                            "condition {} operator must be `and`, `or` or `logic-or`",
                            idx + 1
                        )));
                    }
                    let input_ref_id = c_val
                        .get("query")
                        .and_then(|q| q.get("params"))
                        .and_then(Value::as_array)
                        .and_then(|a| a.first())
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .ok_or_else(|| {
                            CrError::ExpressionError(format!(
                                "condition {} is missing the query RefID argument",
                                idx + 1
                            ))
                        })?
                        .to_owned();
                    let reducer = c_val
                        .get("reducer")
                        .and_then(|r| r.get("type"))
                        .and_then(Value::as_str)
                        .unwrap_or("avg")
                        .to_owned();
                    let eval_obj = c_val
                        .get("evaluator")
                        .and_then(Value::as_object)
                        .ok_or_else(|| {
                            CrError::ExpressionError("classic condition missing evaluator".into())
                        })?;
                    let evaluator_type = eval_obj
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("gt")
                        .to_owned();
                    let evaluator_params: Vec<f64> = eval_obj
                        .get("params")
                        .and_then(Value::as_array)
                        .map(|arr| arr.iter().filter_map(Value::as_f64).collect())
                        .unwrap_or_default();
                    conditions.push(ClassicCondition {
                        input_ref_id,
                        reducer,
                        evaluator_type,
                        evaluator_params,
                        operator: op.to_owned(),
                    });
                }
                ExprCommand::Classic { conditions }
            }
            "sql" => {
                let raw_sql = obj
                    .get("expression")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                ExprCommand::SqlWarning { raw_sql }
            }
            other => {
                return Err(CrError::ExpressionError(format!(
                    "'{other}' is not a recognized expression type"
                )));
            }
        };

        Ok(Self {
            ref_id,
            hidden,
            command,
        })
    }
}

/// Output of evaluating a combined datasource + expression pipeline.
#[derive(Debug, Clone)]
pub struct ExprPipelineOutput {
    pub frame_sets: Vec<FrameSet>,
    pub warnings: Vec<String>,
}

/// Builds the dependency DAG of expression nodes, verifies no cycles or illegal edges exist
/// (`graph.go:buildGraphEdges`), and executes all expression nodes in topological order.
pub fn evaluate_pipeline(
    datasource_results: &[FrameSet],
    hidden_ds_refs: &BTreeSet<String>,
    expr_nodes: &[ExprNodeSpec],
    from_ms: i64,
    to_ms: i64,
    max_depth: usize,
) -> CrResult<ExprPipelineOutput> {
    if expr_nodes.len() > max_depth {
        return Err(CrError::ExpressionError(format!(
            "expression pipeline has {} nodes, exceeding maximum depth {max_depth}",
            expr_nodes.len()
        )));
    }

    let mut vars: HashMap<String, Vec<ExprValue>> = HashMap::new();
    let mut ds_ref_set: BTreeSet<String> = BTreeSet::new();

    for fs in datasource_results {
        ds_ref_set.insert(fs.ref_id.clone());
        let mut vals = Vec::new();
        for frame in &fs.frames {
            vals.extend(ExprValue::from_frame(frame));
        }
        if vals.is_empty() {
            vals.push(ExprValue::NoData);
        }
        vars.insert(fs.ref_id.clone(), vals);
    }

    let mut expr_by_ref: HashMap<String, &ExprNodeSpec> = HashMap::new();
    for node in expr_nodes {
        if expr_by_ref.insert(node.ref_id.clone(), node).is_some()
            || ds_ref_set.contains(&node.ref_id)
        {
            return Err(CrError::ExpressionError(format!(
                "multiple queries using the same RefId '{}' is not allowed",
                node.ref_id
            )));
        }
    }

    // Validate edges and compute in-degrees for topological sort (`graph.go:349-413`)
    let mut in_degree: HashMap<String, usize> = HashMap::new();
    let mut dependents: HashMap<String, Vec<String>> = HashMap::new();

    for node in expr_nodes {
        in_degree.entry(node.ref_id.clone()).or_insert(0);
        for needed in node.command.needs_vars() {
            if needed == node.ref_id {
                return Err(CrError::ExpressionError(format!(
                    "expression '{}' cannot reference itself. Must be query or another expression",
                    node.ref_id
                )));
            }
            let is_ds = ds_ref_set.contains(&needed);
            let needed_expr = expr_by_ref.get(&needed).copied();
            if !is_ds && needed_expr.is_none() {
                return Err(CrError::ExpressionError(format!(
                    "unable to find dependent node '{needed}'"
                )));
            }
            if matches!(node.command, ExprCommand::Classic { .. }) && !is_ds {
                return Err(CrError::ExpressionError(format!(
                    "only data source queries may be inputs to a classic condition, {needed} is an Expression"
                )));
            }
            if let Some(ne) = needed_expr {
                if matches!(ne.command, ExprCommand::Classic { .. }) {
                    return Err(CrError::ExpressionError(format!(
                        "classic conditions may not be the input for other expressions, but {needed} is the input for {}",
                        node.ref_id
                    )));
                }
                if matches!(ne.command, ExprCommand::SqlWarning { .. }) {
                    return Err(CrError::ExpressionError(format!(
                        "sql expressions can not be the input for other expressions, but {needed} is the input for {}",
                        node.ref_id
                    )));
                }
                *in_degree.entry(node.ref_id.clone()).or_insert(0) += 1;
                dependents
                    .entry(needed)
                    .or_default()
                    .push(node.ref_id.clone());
            }
        }
    }

    // Stabilized Kahn topological sort preserving original declaration order
    let mut queue: VecDeque<String> = VecDeque::new();
    for node in expr_nodes {
        if in_degree.get(&node.ref_id).copied().unwrap_or(0) == 0 {
            queue.push_back(node.ref_id.clone());
        }
    }

    let mut ordered_refs = Vec::with_capacity(expr_nodes.len());
    while let Some(r) = queue.pop_front() {
        ordered_refs.push(r.clone());
        if let Some(deps) = dependents.get(&r) {
            for dep in deps {
                if let Some(deg) = in_degree.get_mut(dep) {
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push_back(dep.clone());
                    }
                }
            }
        }
    }

    if ordered_refs.len() != expr_nodes.len() {
        return Err(CrError::ExpressionError(
            "cyclic dependency detected in server-side expressions".into(),
        ));
    }

    let mut warnings = Vec::new();
    let mut expr_frames: HashMap<String, Vec<Frame>> = HashMap::new();

    for ref_id in &ordered_refs {
        let node = expr_by_ref[ref_id];
        match &node.command {
            ExprCommand::Math(m) => {
                let vals = m.eval(ref_id, &vars)?;
                let frames = vals.iter().map(|v| v.to_frame(ref_id)).collect();
                vars.insert(ref_id.clone(), vals);
                expr_frames.insert(ref_id.clone(), frames);
            }
            ExprCommand::Reduce {
                var_to_reduce,
                reducer,
                mode,
            } => {
                let inputs = vars.get(var_to_reduce).map_or(&[][..], Vec::as_slice);
                let vals = reduce_frames(ref_id, var_to_reduce, *reducer, *mode, inputs)?;
                let frames = vals.iter().map(|v| v.to_frame(ref_id)).collect();
                vars.insert(ref_id.clone(), vals);
                expr_frames.insert(ref_id.clone(), frames);
            }
            ExprCommand::Resample {
                var_to_resample,
                window_ms,
                downsampler,
                upsampler,
            } => {
                let inputs = vars.get(var_to_resample).map_or(&[][..], Vec::as_slice);
                let vals = resample_frames(
                    ref_id,
                    *window_ms,
                    *downsampler,
                    *upsampler,
                    from_ms,
                    to_ms,
                    inputs,
                )?;
                let frames = vals.iter().map(|v| v.to_frame(ref_id)).collect();
                vars.insert(ref_id.clone(), vals);
                expr_frames.insert(ref_id.clone(), frames);
            }
            ExprCommand::Threshold {
                reference_var,
                condition,
            } => {
                let inputs = vars.get(reference_var).map_or(&[][..], Vec::as_slice);
                let vals = eval_threshold_frames(ref_id, condition, inputs)?;
                let frames = vals.iter().map(|v| v.to_frame(ref_id)).collect();
                vars.insert(ref_id.clone(), vals);
                expr_frames.insert(ref_id.clone(), frames);
            }
            ExprCommand::Classic { conditions } => {
                let vals = eval_classic_conditions(ref_id, conditions, &vars)?;
                let frames = vals.iter().map(|v| v.to_frame(ref_id)).collect();
                vars.insert(ref_id.clone(), vals);
                expr_frames.insert(ref_id.clone(), frames);
            }
            ExprCommand::SqlWarning { raw_sql } => {
                let msg = format!(
                    "sql expression '{ref_id}' is not enabled in this run (query: {raw_sql})"
                );
                warnings.push(msg.clone());
                let frame = Frame {
                    name: ref_id.clone(),
                    ref_id: Some(ref_id.clone()),
                    fields: Vec::new(),
                    meta: FrameMeta {
                        notices: vec![Notice {
                            severity: NoticeSeverity::Warning,
                            text: msg,
                        }],
                        ..FrameMeta::default()
                    },
                };
                vars.insert(ref_id.clone(), vec![ExprValue::NoData]);
                expr_frames.insert(ref_id.clone(), vec![frame]);
            }
        }
    }

    // Collect non-hidden datasource results and non-hidden expression results (`transform.go:88-99`)
    let mut out_sets = Vec::new();
    for fs in datasource_results {
        if !hidden_ds_refs.contains(&fs.ref_id) {
            out_sets.push(fs.clone());
        }
    }
    for node in expr_nodes {
        if !node.hidden
            && let Some(frames) = expr_frames.remove(&node.ref_id)
        {
            out_sets.push(FrameSet::new(node.ref_id.clone(), frames));
        }
    }

    Ok(ExprPipelineOutput {
        frame_sets: out_sets,
        warnings,
    })
}
