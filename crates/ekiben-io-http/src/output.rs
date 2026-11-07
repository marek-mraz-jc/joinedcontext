use std::sync::Arc;

use async_trait::async_trait;
use ekiben_core::{Batch, ComponentConfig, CoreError, Output, Registry, StreamContext};
use serde_json::Value as JsonValue;

use crate::client::HttpClient;

pub fn register(registry: &Registry) {
    registry.register_output("http_client", |conf, ctx| {
        Ok(Arc::new(HttpClientOutput::from_config(conf, ctx)?))
    });
}

/// Implements `http_client` output (`internal/impl/io/output_http_client.go`).
pub struct HttpClientOutput {
    client: Arc<HttpClient>,
    batch_as_multipart: bool,
    propagate_response: bool,
}

impl HttpClientOutput {
    pub fn from_config(conf: &ComponentConfig, ctx: &StreamContext) -> Result<Self, CoreError> {
        let client = HttpClient::from_json_config(&conf.config, "POST", &ctx.blobl_env)?;
        let batch_as_multipart = conf
            .config
            .get("batch_as_multipart")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);
        let propagate_response = conf
            .config
            .get("propagate_response")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);

        Ok(Self {
            client,
            batch_as_multipart,
            propagate_response,
        })
    }
}

#[async_trait]
impl Output for HttpClientOutput {
    fn output_type(&self) -> &'static str {
        "http_client"
    }

    async fn write_batch(&self, ctx: &StreamContext, batch: Batch) -> Result<(), CoreError> {
        if batch.is_empty() {
            return Ok(());
        }

        if batch.len() > 1 && !self.batch_as_multipart {
            for part in batch {
                self.write_batch(ctx, vec![part]).await?;
            }
            return Ok(());
        }

        let res_batch = self
            .client
            .send(
                batch.clone(),
                self.batch_as_multipart,
                &ctx.org,
                &ctx.resources,
            )
            .await?;

        if self.propagate_response {
            if let Some(store) = batch.first().and_then(|p| p.sync_response_store()) {
                store.add(&res_batch);
            }
        }

        Ok(())
    }
}
