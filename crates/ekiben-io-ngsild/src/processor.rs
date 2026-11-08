//! The `ngsi_ld` processor (E5): the drop-in for the Portal's `branch` + `http` upsert. Each
//! message is kept and carries exactly the metadata the branch leaves today, so the outcome
//! sink and the rejected-records replay work unchanged:
//! - written (all or in part): `jc_refused_<label>` = the refused members as a JSON array of
//!   `{id, message}` (`[]` when none), as the branch's `result_map` formats it;
//! - refused whole (`drop_on`, 400/413/422 by default): `jc_failed` = the problem's `detail`,
//!   else `title`, else the error, and no error flag, as the Portal's `refusals` step leaves it;
//! - still failing after the retries: the error flag, which the stream's `written` output nacks.

use std::sync::Arc;

use async_trait::async_trait;
use ekiben_core::{Batch, ComponentConfig, CoreError, ProcContext, Processor, Registry};

use crate::client::CallEnv;
use crate::writer::{refused_json, Writer, FAILED_KEY, REFUSED_PREFIX};

pub fn register(registry: &Registry) {
    registry.register_processor("ngsi_ld", |conf, ctx| {
        Ok(Arc::new(NgsiLdProcessor::from_config(conf, ctx)?))
    });
}

pub struct NgsiLdProcessor {
    writer: Writer,
    label: String,
    refused_key: String,
}

impl NgsiLdProcessor {
    pub fn from_config(conf: &ComponentConfig, ctx: &ProcContext) -> Result<Self, CoreError> {
        let writer = Writer::from_config(&conf.config, "ngsi_ld", &ctx.org)?;
        let label = if conf.label.is_empty() {
            "ngsi_ld".to_string()
        } else {
            conf.label.clone()
        };
        Ok(Self {
            writer,
            refused_key: format!("{REFUSED_PREFIX}{label}"),
            label,
        })
    }

    pub fn writer(&self) -> &Writer {
        &self.writer
    }
}

#[async_trait]
impl Processor for NgsiLdProcessor {
    fn processor_type(&self) -> &'static str {
        "ngsi_ld"
    }

    fn label(&self) -> &str {
        &self.label
    }

    async fn process_batch(
        &self,
        ctx: &ProcContext,
        mut batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        if batch.is_empty() {
            return Ok(Vec::new());
        }
        let env = CallEnv {
            org: &ctx.org,
            resources: &ctx.resources,
            metrics: ctx.metrics.as_ref(),
            label: &self.label,
            shutdown: &ctx.shutdown_rx,
        };
        let verdicts = self.writer.write(&env, &batch).await;
        for (part, verdict) in batch.iter_mut().zip(verdicts) {
            if let Some(err) = verdict.failed {
                ctx.on_error(part, err);
                continue;
            }
            if let Some(whole) = verdict.whole {
                part.meta_set_str(FAILED_KEY, whole);
                continue;
            }
            part.meta_set_str(self.refused_key.clone(), refused_json(&verdict.refused));
        }
        Ok(vec![batch])
    }
}
