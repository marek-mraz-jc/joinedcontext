//! `group_by` processor (`internal/impl/pure/processor_group_by.go`): splits a batch into one batch
//! per group whose `check` a message passes first, runs that group's `processors` on it, and puts
//! the messages no check passes into a last batch of their own.

use std::sync::Arc;

use async_trait::async_trait;
use ekiben_bloblang::Executor;
use ekiben_core::{
    execute_all, Batch, ComponentConfig, CoreError, PartsView, ProcContext, Processor,
};
use serde_json::Value as JsonValue;

struct Group {
    check: Executor,
    processors: Vec<Arc<dyn Processor>>,
}

pub struct GroupByProcessor {
    groups: Vec<Group>,
}

impl GroupByProcessor {
    pub fn from_config(conf: &ComponentConfig, ctx: &ProcContext) -> Result<Self, CoreError> {
        let arr = conf
            .config
            .as_array()
            .ok_or_else(|| CoreError::config("group_by: expected an array of groups"))?;
        let mut groups = Vec::with_capacity(arr.len());
        for (i, item) in arr.iter().enumerate() {
            let parse_err = |e: String| CoreError::config(format!("group '{i}' parse error: {e}"));
            let check = item
                .get("check")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| parse_err("field 'check' is required".to_string()))?;
            let check = ctx
                .blobl_env
                .compile_mapping(check)
                .map_err(|e| parse_err(e.to_string()))?;
            let mut confs = Vec::new();
            if let Some(list) = item.get("processors") {
                let list = list
                    .as_array()
                    .ok_or_else(|| parse_err("field 'processors' must be a list".to_string()))?;
                for p in list {
                    confs.push(ComponentConfig::from_json_value(p, "noop", None)?);
                }
            }
            let group_ctx = ctx.into_child("group_by", ctx.label.clone(), format!("{i}.processors"));
            let processors = ctx.registry.build_processor_list(&confs, &group_ctx)?;
            groups.push(Group { check, processors });
        }
        Ok(Self { groups })
    }
}

#[async_trait]
impl Processor for GroupByProcessor {
    fn processor_type(&self) -> &'static str {
        "group_by"
    }

    async fn process_batch(&self, ctx: &ProcContext, batch: Batch) -> Result<Vec<Batch>, CoreError> {
        if batch.is_empty() {
            return Ok(Vec::new());
        }
        let view = PartsView(&batch);
        let mut grouped: Vec<Batch> = vec![Vec::new(); self.groups.len()];
        let mut groupless = Vec::new();
        for (i, part) in batch.iter().enumerate() {
            let found = self.groups.iter().enumerate().position(|(j, g)| {
                g.check.query_part(i, &view).unwrap_or_else(|e| {
                    tracing::error!("Failed to test group {j}: {e}");
                    false
                })
            });
            match found {
                Some(j) => grouped[j].push(part.clone()),
                None => groupless.push(part.clone()),
            }
        }
        let mut out = Vec::new();
        for (group, msgs) in self.groups.iter().zip(grouped) {
            if !msgs.is_empty() {
                out.extend(execute_all(ctx, &group.processors, vec![msgs]).await?);
            }
        }
        if !groupless.is_empty() {
            out.push(groupless);
        }
        Ok(out)
    }

    async fn close(&self) -> Result<(), CoreError> {
        for group in &self.groups {
            for p in &group.processors {
                p.close().await?;
            }
        }
        Ok(())
    }
}
