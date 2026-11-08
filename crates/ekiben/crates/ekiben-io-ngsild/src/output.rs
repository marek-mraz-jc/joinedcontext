//! The `ngsi_ld` output (E5), for streams that end in the write. A message whose entities the
//! broker refused (a 207 member error, a whole refusal) or whose write still failed after its
//! retries is an indexed error of the output's batch (Bento's BatchError), so `fallback` and
//! `reject_errored` dead-letter exactly those messages and the input's ack waits on the rest.

use std::sync::Arc;

use async_trait::async_trait;
use ekiben_core::{Batch, BatchError, ComponentConfig, CoreError, Output, Registry, StreamContext};

use crate::client::CallEnv;
use crate::writer::{error_text, Writer};

pub fn register(registry: &Registry) {
    registry.register_output("ngsi_ld", |conf, ctx| {
        Ok(Arc::new(NgsiLdOutput::from_config(conf, ctx)?))
    });
}

pub struct NgsiLdOutput {
    writer: Writer,
    label: String,
}

impl NgsiLdOutput {
    pub fn from_config(conf: &ComponentConfig, ctx: &StreamContext) -> Result<Self, CoreError> {
        Ok(Self {
            writer: Writer::from_config(&conf.config, "ngsi_ld", &ctx.org)?,
            label: if conf.label.is_empty() {
                "ngsi_ld".to_string()
            } else {
                conf.label.clone()
            },
        })
    }
}

#[async_trait]
impl Output for NgsiLdOutput {
    fn output_type(&self) -> &'static str {
        "ngsi_ld"
    }

    fn label(&self) -> &str {
        &self.label
    }

    async fn write_batch(&self, ctx: &StreamContext, batch: Batch) -> Result<(), CoreError> {
        if batch.is_empty() {
            return Ok(());
        }
        let env = CallEnv {
            org: &ctx.org,
            resources: &ctx.resources,
            metrics: ctx.metrics.as_ref(),
            label: &self.label,
            shutdown: &ctx.shutdown_rx,
        };
        let verdicts = self.writer.write(&env, &batch).await;
        let errors: Vec<(usize, String)> = verdicts
            .iter()
            .enumerate()
            .filter_map(|(i, v)| error_text(v).map(|e| (i, e)))
            .collect();
        let Some((_, first)) = errors.first() else {
            return Ok(());
        };
        let mut err = BatchError::new(batch, first.clone());
        for (i, e) in errors {
            err.failed(i, e);
        }
        Err(CoreError::Batch(err))
    }
}
