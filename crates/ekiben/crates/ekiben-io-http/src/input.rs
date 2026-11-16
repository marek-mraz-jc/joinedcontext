use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use ekiben_core::{
    AckResponder, Batch, ComponentConfig, CoreError, Input, Part, Registry, StreamContext,
};
use serde_json::Value as JsonValue;

use crate::client::HttpClient;
use crate::paginate::{self, PaginateConf, Walk};

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
    /// `stream.enabled`: the body is split by `stream.scanner` (`lines` by default).
    stream: Option<ekiben_io_file::inputs::Scanner>,
    prev_response: Batch,
    queued_stream_parts: Vec<Part>,
    connected: bool,
    /// ekiben extension `paginate` (T-7056): one page per read, the input ends with the walk.
    paginate: Option<Arc<PaginateConf>>,
    walk: Option<Walk>,
    /// The first nack a page got: the run is then incomplete.
    failed: Arc<Mutex<Option<String>>>,
}

impl HttpClientInput {
    pub fn from_config(conf: &ComponentConfig, ctx: &StreamContext) -> Result<Self, CoreError> {
        let client = HttpClient::from_json_config(&conf.config, "GET", &ctx.blobl_env)?;
        let drop_empty_bodies = conf
            .config
            .get("drop_empty_bodies")
            .and_then(JsonValue::as_bool)
            .unwrap_or(true);
        let stream = match conf.config.get("stream").and_then(JsonValue::as_object) {
            Some(s) if s.get("enabled").and_then(JsonValue::as_bool) == Some(true) => {
                Some(ekiben_io_file::inputs::Scanner::lines_by_default(s)?)
            }
            _ => None,
        };

        let paginate = conf
            .config
            .get("paginate")
            .filter(|v| !v.is_null())
            .map(|v| PaginateConf::from_json(v, &ctx.blobl_env))
            .transpose()?;
        if paginate.is_some() && stream.is_some() {
            return Err(CoreError::config(
                "http_client.paginate cannot be combined with stream.enabled",
            ));
        }

        Ok(Self {
            client,
            drop_empty_bodies,
            stream,
            prev_response: Vec::new(),
            queued_stream_parts: Vec::new(),
            connected: false,
            paginate,
            walk: None,
            failed: Arc::new(Mutex::new(None)),
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
        if let Some(conf) = &self.paginate {
            // ponytail: the last page's nack can land after the input ended; the runner awaits
            // each write before the next read, so it is in by then in practice. Upgrade path:
            // hold EndOfInput until the outstanding page acks drained.
            let nacked = || {
                self.failed
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take()
                    .map(|e| CoreError::other(format!("paginate: a page was not delivered: {e}")))
            };
            if let Some(e) = nacked() {
                return Err(e);
            }
            let walk = self.walk.get_or_insert_with(|| Walk::new(conf.clone()));
            let page =
                paginate::next_page(&self.client, walk, &[], &ctx.org, &ctx.resources).await?;
            return match page {
                Some(part) => {
                    let failed = self.failed.clone();
                    let ack = AckResponder::from_fn(move |err| {
                        if let Some(e) = err {
                            failed
                                .lock()
                                .unwrap_or_else(PoisonError::into_inner)
                                .get_or_insert(e.message);
                        }
                    });
                    Ok((vec![part], ack))
                }
                None => Err(nacked().unwrap_or(CoreError::EndOfInput)),
            };
        }

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

        if let Some(scanner) = &self.stream {
            let Some(response) = res_batch.into_iter().next() else {
                return Err(CoreError::Timeout);
            };
            for mut part in scanner.scan(&response.as_bytes_ref(), "")? {
                if self.drop_empty_bodies && part.as_bytes_ref().is_empty() {
                    continue;
                }
                // Each message keeps the response's metadata (status code, headers).
                for (k, v) in response.meta_all() {
                    if part.meta_get(k).is_none() {
                        part.meta_set(k.clone(), v.clone());
                    }
                }
                self.queued_stream_parts.push(part);
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
