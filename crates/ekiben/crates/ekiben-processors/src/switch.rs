use std::sync::Arc;

use async_trait::async_trait;
use ekiben_bloblang::Executor;
use ekiben_core::{
    execute_all, Batch, ComponentConfig, CoreError, Part, PartsView, ProcContext, Processor,
    Registry, SortGroup,
};
use serde_json::Value as JsonValue;

pub fn register(registry: &Registry) {
    registry.register_processor("switch", |conf, ctx| {
        Ok(Arc::new(SwitchProcessor::from_config(conf, ctx)?))
    });
}

struct SwitchCase {
    check: Option<Executor>,
    processors: Vec<Arc<dyn Processor>>,
    fallthrough: bool,
}

/// Implements the `switch` processor (`internal/impl/pure/processor_switch.go`).
pub struct SwitchProcessor {
    cases: Vec<SwitchCase>,
}

impl SwitchProcessor {
    pub fn from_config(conf: &ComponentConfig, ctx: &ProcContext) -> Result<Self, CoreError> {
        let arr = conf
            .config
            .as_array()
            .ok_or_else(|| CoreError::config("switch processor requires an array of cases"))?;

        let mut cases = Vec::with_capacity(arr.len());
        for (i, case_val) in arr.iter().enumerate() {
            let obj = case_val
                .as_object()
                .ok_or_else(|| CoreError::config(format!("switch case '{i}' must be an object")))?;

            let check = match obj.get("check").and_then(JsonValue::as_str) {
                Some(s) if !s.trim().is_empty() => Some(
                    ctx.blobl_env
                        .compile_mapping(s)
                        .map_err(|e| CoreError::config(format!("case '{i}' parse error: {e}")))?,
                ),
                _ => None,
            };

            let fallthrough = obj
                .get("fallthrough")
                .and_then(JsonValue::as_bool)
                .unwrap_or(false);

            let proc_arr = obj
                .get("processors")
                .and_then(JsonValue::as_array)
                .ok_or_else(|| {
                    CoreError::config(format!(
                        "case '{i}' has no processors, in order to have a no-op case use a `noop` processor"
                    ))
                })?;
            if proc_arr.is_empty() {
                return Err(CoreError::config(format!(
                    "case '{i}' has no processors, in order to have a no-op case use a `noop` processor"
                )));
            }

            let mut proc_confs = Vec::with_capacity(proc_arr.len());
            for p in proc_arr {
                proc_confs.push(ComponentConfig::from_json_value(p, "noop", None)?);
            }
            let case_ctx = ctx.into_child("switch", ctx.label.clone(), i.to_string());
            let processors = ctx.registry.build_processor_list(&proc_confs, &case_ctx)?;

            cases.push(SwitchCase {
                check,
                processors,
                fallthrough,
            });
        }

        Ok(Self { cases })
    }
}

fn switch_reorder_from_group(group: &SortGroup, parts: &mut [Part]) {
    parts.sort_by_key(|p| group.get_index(p).unwrap_or(usize::MAX));
}

#[async_trait]
impl Processor for SwitchProcessor {
    fn processor_type(&self) -> &'static str {
        "switch"
    }

    async fn process_batch(
        &self,
        ctx: &ProcContext,
        batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        let (sort_group, mut remaining) = SortGroup::new(batch);
        let mut result: Vec<Part> = Vec::new();
        let mut carry_over: Vec<Part> = Vec::new();

        for (case_idx, switch_case) in self.cases.iter().enumerate() {
            let mut passed = std::mem::take(&mut carry_over);
            let mut failed = Vec::new();

            let test_view = PartsView(&remaining);

            for (j, part) in remaining.iter().enumerate() {
                let matched = match &switch_case.check {
                    None => Ok(true),
                    Some(chk) => chk.query_part(j, &test_view),
                };
                match matched {
                    Ok(true) => passed.push(part.clone()),
                    Ok(false) => failed.push(part.clone()),
                    Err(err) => {
                        let mut errored_part = part.clone();
                        ctx.on_error(
                            &mut errored_part,
                            format!("failed to test case {case_idx}: {err}"),
                        );
                        result.push(errored_part);
                    }
                }
            }

            remaining = failed;

            if !passed.is_empty() {
                let batches = execute_all(ctx, &switch_case.processors, vec![passed]).await?;
                for b in batches {
                    for p in b {
                        if switch_case.fallthrough {
                            carry_over.push(p);
                        } else {
                            result.push(p);
                        }
                    }
                }
            }
        }

        result.extend(carry_over);
        result.extend(remaining);

        if result.len() > 1 {
            switch_reorder_from_group(&sort_group, &mut result);
        }

        if result.is_empty() {
            Ok(Vec::new())
        } else {
            Ok(vec![result])
        }
    }

    async fn close(&self) -> Result<(), CoreError> {
        for c in &self.cases {
            for p in &c.processors {
                p.close().await?;
            }
        }
        Ok(())
    }
}
