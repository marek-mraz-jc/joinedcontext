//! `group_by_value` processor (`internal/impl/pure/processor_group_by_value.go`): splits a batch into
//! one batch per value of the interpolated `value`, in the order each value first appears.

use std::collections::HashMap;

use async_trait::async_trait;
use ekiben_bloblang::FieldExpression;
use ekiben_core::{Batch, ComponentConfig, CoreError, PartsView, ProcContext, Processor};
use serde_json::Value as JsonValue;

pub struct GroupByValueProcessor {
    value: FieldExpression,
}

impl GroupByValueProcessor {
    pub fn from_config(conf: &ComponentConfig, ctx: &ProcContext) -> Result<Self, CoreError> {
        let value = conf
            .config
            .get("value")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| CoreError::config("field 'value' is required"))?;
        let value = ctx
            .blobl_env
            .compile_field(value)
            .map_err(|e| CoreError::config(format!("failed to parse value expression: {e}")))?;
        Ok(Self { value })
    }
}

#[async_trait]
impl Processor for GroupByValueProcessor {
    fn processor_type(&self) -> &'static str {
        "group_by_value"
    }

    async fn process_batch(&self, ctx: &ProcContext, batch: Batch) -> Result<Vec<Batch>, CoreError> {
        let view = PartsView(&batch);
        let mut out: Vec<Batch> = Vec::new();
        let mut index: HashMap<String, usize> = HashMap::new();
        for (i, part) in batch.iter().enumerate() {
            let mut part = part.clone();
            // An interpolation error groups the message under "" and flags it, as Bento does.
            let key = self.value.resolve_string(i, &view).unwrap_or_else(|e| {
                tracing::error!("Group value interpolation error: {e}");
                ctx.on_error(&mut part, format!("group value interpolation error: {e}"));
                String::new()
            });
            match index.get(&key) {
                Some(&g) => out[g].push(part),
                None => {
                    index.insert(key, out.len());
                    out.push(vec![part]);
                }
            }
        }
        Ok(out)
    }
}
