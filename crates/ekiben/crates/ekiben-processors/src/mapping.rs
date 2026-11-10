use std::sync::Arc;

use async_trait::async_trait;
use ekiben_bloblang::{Executor, Value};
use ekiben_core::{
    Batch, ComponentConfig, CoreError, PartMetaAdapter, PartsView, ProcContext, Processor, Registry,
};

pub fn register(registry: &Registry) {
    for name in ["mapping", "bloblang"] {
        registry.register_processor(name, move |conf, ctx| {
            let src = conf.config.as_str().ok_or_else(|| {
                CoreError::config(format!("processor '{name}' requires a string mapping"))
            })?;
            let exec = ctx.blobl_env.compile_mapping(src)?;
            Ok(Arc::new(MappingProcessor {
                exec,
                is_mutation: false,
                type_name: name,
            }))
        });
    }

    registry.register_processor("mutation", |conf, ctx| {
        let src = conf
            .config
            .as_str()
            .ok_or_else(|| CoreError::config("processor 'mutation' requires a string mapping"))?;
        let exec = ctx.blobl_env.compile_mapping(src)?;
        Ok(Arc::new(MappingProcessor {
            exec,
            is_mutation: true,
            type_name: "mutation",
        }))
    });
}

/// Implements `mapping`, `bloblang`, and `mutation` (`internal/impl/pure/processor_mapping.go`, `processor_mutation.go`).
pub struct MappingProcessor {
    exec: Executor,
    is_mutation: bool,
    type_name: &'static str,
}

#[async_trait]
impl Processor for MappingProcessor {
    fn processor_type(&self) -> &'static str {
        self.type_name
    }

    fn may_fail_batch(&self) -> bool {
        false
    }

    async fn process_batch(
        &self,
        ctx: &ProcContext,
        mut batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        let mut new_batch = Vec::with_capacity(batch.len());
        // Each input part is released once its output exists, unless the mapping reads other
        // parts (`from`, `from_all`): an earlier part's output may share nothing with its input,
        // and holding the whole input batch beside the whole output doubled a large run's peak.
        let release = !self.exec.reads_other_parts();
        for i in 0..batch.len() {
            let mut new_part = batch[i].shallow_copy();
            let res = {
                let view = PartsView(&batch);
                let mut meta_adapter = PartMetaAdapter(&mut new_part);
                if self.is_mutation {
                    // `mutation`: maps directly onto each existing message part (`MapOnto`).
                    let initial_val = batch[i].as_structured_ref().unwrap_or(Value::Nothing);
                    self.exec
                        .mutate_part(i, &view, &mut meta_adapter, Some(initial_val))
                } else {
                    // `mapping` / `bloblang`: the input batch is read-only and `root` starts empty.
                    self.exec.map_part(i, &view, &mut meta_adapter, None)
                }
            };
            let orig_part = if release {
                std::mem::take(&mut batch[i])
            } else {
                batch[i].shallow_copy()
            };
            match res {
                Ok(None) => {
                    // `root = deleted()` -> filter out this message part
                }
                Ok(Some(Value::Nothing)) => {
                    // `root` was never assigned -> keep original payload with updated metadata
                    new_batch.push(new_part);
                }
                Ok(Some(val)) => {
                    new_part.apply_mapped_value(val);
                    new_batch.push(new_part);
                }
                Err(err) => {
                    // On mapping failure (including `throw()`), Bento keeps the original message
                    // content unchanged and attaches the error flag!
                    let mut failed_part = orig_part;
                    ctx.on_error(&mut failed_part, err.to_string());
                    new_batch.push(failed_part);
                }
            }
        }

        if new_batch.is_empty() {
            Ok(Vec::new())
        } else {
            Ok(vec![new_batch])
        }
    }
}

pub fn compile_optional_mapping(
    conf: &ComponentConfig,
    field: &str,
    ctx: &ProcContext,
) -> Result<Option<Executor>, CoreError> {
    let Some(s) = conf.config.get(field).and_then(|v| v.as_str()) else {
        return Ok(None);
    };
    if s.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(ctx.blobl_env.compile_mapping(s)?))
}
