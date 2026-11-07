use std::sync::Arc;

use async_trait::async_trait;
use ekiben_bloblang::Value;
use ekiben_core::{Batch, ComponentConfig, CoreError, ProcContext, Processor, Registry};
use serde_json::Value as JsonValue;

use crate::client::{extract_status_code_from_err, HttpClient};

pub fn register(registry: &Registry) {
    registry.register_processor("http", |conf, ctx| {
        Ok(Arc::new(HttpProcessor::from_config(conf, ctx)?))
    });
}

/// Implements the `http` processor (`internal/impl/io/processor_http.go`).
pub struct HttpProcessor {
    client: Arc<HttpClient>,
    batch_as_multipart: bool,
    parallel: bool,
}

impl HttpProcessor {
    pub fn from_config(conf: &ComponentConfig, ctx: &ProcContext) -> Result<Self, CoreError> {
        let client = HttpClient::from_json_config(&conf.config, "POST", &ctx.blobl_env)?;
        let batch_as_multipart = conf
            .config
            .get("batch_as_multipart")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);
        let parallel = conf
            .config
            .get("parallel")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);

        Ok(Self {
            client,
            batch_as_multipart,
            parallel,
        })
    }
}

#[async_trait]
impl Processor for HttpProcessor {
    fn processor_type(&self) -> &'static str {
        "http"
    }

    async fn process_batch(
        &self,
        ctx: &ProcContext,
        batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        if batch.is_empty() {
            return Ok(Vec::new());
        }

        if self.batch_as_multipart || batch.len() == 1 {
            match self
                .client
                .send(
                    batch.clone(),
                    self.batch_as_multipart,
                    &ctx.org,
                    &ctx.resources,
                )
                .await
            {
                Ok(res_batch) => {
                    let mut out = Vec::with_capacity(res_batch.len());
                    for (i, mut rp) in res_batch.into_iter().enumerate() {
                        let mut base = if i < batch.len() {
                            batch[i].shallow_copy()
                        } else {
                            batch[0].shallow_copy()
                        };
                        base.set_bytes(rp.as_bytes());
                        for (k, v) in rp.meta_all() {
                            base.meta_set(k.clone(), v.clone());
                        }
                        out.push(base);
                    }
                    return Ok(vec![out]);
                }
                Err(CoreError::AccessDenied { org, reason }) => {
                    return Err(CoreError::AccessDenied { org, reason });
                }
                Err(CoreError::QuotaExceeded { org, reason }) => {
                    return Err(CoreError::QuotaExceeded { org, reason });
                }
                Err(err) => {
                    let err_str = err.to_string();
                    let code_opt = extract_status_code_from_err(&err_str);
                    let mut out = batch;
                    for p in &mut out {
                        if let Some(code) = code_opt {
                            p.meta_set("http_status_code", Value::Int(code));
                        }
                        ctx.on_error(p, err_str.clone());
                    }
                    return Ok(vec![out]);
                }
            }
        }

        if !self.parallel {
            let mut out = Vec::with_capacity(batch.len());
            for part in batch {
                match self
                    .client
                    .send(vec![part.clone()], false, &ctx.org, &ctx.resources)
                    .await
                {
                    Ok(res_parts) => {
                        for mut rp in res_parts {
                            let mut base = part.shallow_copy();
                            base.set_bytes(rp.as_bytes());
                            for (k, v) in rp.meta_all() {
                                base.meta_set(k.clone(), v.clone());
                            }
                            out.push(base);
                        }
                    }
                    Err(CoreError::AccessDenied { org, reason }) => {
                        return Err(CoreError::AccessDenied { org, reason });
                    }
                    Err(CoreError::QuotaExceeded { org, reason }) => {
                        return Err(CoreError::QuotaExceeded { org, reason });
                    }
                    Err(err) => {
                        let err_str = err.to_string();
                        let mut err_part = part.shallow_copy();
                        if let Some(code) = extract_status_code_from_err(&err_str) {
                            err_part.meta_set("http_status_code", Value::Int(code));
                        }
                        ctx.on_error(&mut err_part, err_str);
                        out.push(err_part);
                    }
                }
            }
            return Ok(vec![out]);
        }

        // Parallel per-part execution
        let mut futs = Vec::with_capacity(batch.len());
        for part in &batch {
            let client = self.client.clone();
            let p = part.clone();
            let org = ctx.org.clone();
            let res_mgr = ctx.resources.clone();
            futs.push(async move { client.send(vec![p], false, &org, &res_mgr).await });
        }

        let results = futures::future::join_all(futs).await;
        let mut out = Vec::with_capacity(batch.len());
        for (i, res) in results.into_iter().enumerate() {
            let mut base = batch[i].shallow_copy();
            match res {
                Ok(mut res_parts) if !res_parts.is_empty() => {
                    let mut rp = res_parts.remove(0);
                    base.set_bytes(rp.as_bytes());
                    for (k, v) in rp.meta_all() {
                        base.meta_set(k.clone(), v.clone());
                    }
                    out.push(base);
                }
                Ok(_) => {
                    ctx.on_error(&mut base, "empty HTTP response");
                    out.push(base);
                }
                Err(CoreError::AccessDenied { org, reason }) => {
                    return Err(CoreError::AccessDenied { org, reason });
                }
                Err(CoreError::QuotaExceeded { org, reason }) => {
                    return Err(CoreError::QuotaExceeded { org, reason });
                }
                Err(err) => {
                    let err_str = err.to_string();
                    if let Some(code) = extract_status_code_from_err(&err_str) {
                        base.meta_set("http_status_code", Value::Int(code));
                    }
                    ctx.on_error(&mut base, err_str);
                    out.push(base);
                }
            }
        }
        Ok(vec![out])
    }
}
