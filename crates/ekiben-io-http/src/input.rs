use std::sync::Arc;

use async_trait::async_trait;
use ekiben_core::{
    AckResponder, Batch, ComponentConfig, CoreError, Input, Part, Registry, StreamContext,
};
use serde_json::Value as JsonValue;

use crate::client::HttpClient;

pub fn register(registry: &Registry) {
    registry.register_input("http_client", |conf, ctx| {
        Ok(Box::new(HttpClientInput::from_config(conf, ctx)?))
    });
}

/// Implements `http_client` input (`internal/impl/io/input_http_client.go`),
/// including `prev_response` reference for pagination interpolations in `url` and `headers`.
pub struct HttpClientInput {
    client: Arc<HttpClient>,
    drop_empty_bodies: bool,
    stream_lines: bool,
    prev_response: Batch,
    queued_stream_parts: Vec<Part>,
    connected: bool,
}

impl HttpClientInput {
    pub fn from_config(conf: &ComponentConfig, ctx: &StreamContext) -> Result<Self, CoreError> {
        let client = HttpClient::from_json_config(&conf.config, "GET", &ctx.blobl_env)?;
        let drop_empty_bodies = conf
            .config
            .get("drop_empty_bodies")
            .and_then(JsonValue::as_bool)
            .unwrap_or(true);
        let stream_lines = conf
            .config
            .get("stream")
            .and_then(|s| s.get("enabled"))
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);

        Ok(Self {
            client,
            drop_empty_bodies,
            stream_lines,
            prev_response: Vec::new(),
            queued_stream_parts: Vec::new(),
            connected: false,
        })
    }
}

#[async_trait]
impl Input for HttpClientInput {
    fn input_type(&self) -> &'static str {
        "http_client"
    }

    async fn connect(&mut self, _ctx: &StreamContext) -> Result<(), CoreError> {
        self.connected = true;
        Ok(())
    }

    async fn read_batch(
        &mut self,
        ctx: &StreamContext,
    ) -> Result<(Batch, AckResponder), CoreError> {
        if !self.queued_stream_parts.is_empty() {
            let p = self.queued_stream_parts.remove(0);
            return Ok((vec![p], AckResponder::noop()));
        }

        let res_batch = self
            .client
            .send(self.prev_response.clone(), false, &ctx.org, &ctx.resources)
            .await?;

        if res_batch.is_empty() {
            return Err(CoreError::Timeout);
        }

        if self.stream_lines {
            let mut first = res_batch.into_iter().next().unwrap();
            let bytes = first.as_bytes();
            for line in bytes.split(|&b| b == b'\n') {
                if self.drop_empty_bodies && line.is_empty() {
                    continue;
                }
                let mut lp = first.shallow_copy();
                lp.set_bytes(bytes::Bytes::copy_from_slice(line));
                self.queued_stream_parts.push(lp);
            }
            if self.queued_stream_parts.is_empty() {
                return Err(CoreError::EndOfInput);
            }
            let next_part = self.queued_stream_parts.remove(0);
            self.prev_response = vec![next_part.shallow_copy()];
            return Ok((vec![next_part], AckResponder::noop()));
        }

        if res_batch.len() == 1 && self.drop_empty_bodies && res_batch[0].as_bytes_ref().is_empty()
        {
            return Err(CoreError::Timeout);
        }

        self.prev_response = res_batch.iter().map(Part::shallow_copy).collect();
        Ok((res_batch, AckResponder::noop()))
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    async fn close(&mut self) -> Result<(), CoreError> {
        self.connected = false;
        Ok(())
    }
}
