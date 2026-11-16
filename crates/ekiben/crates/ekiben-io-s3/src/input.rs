//! Bento's `aws_s3` input (`internal/impl/aws/input_s3.go`) without SQS: lists every object under
//! `prefix` once, reads each (up to `max_object_bytes`), splits it with `scanner` and ends. A
//! schedule re-lists the bucket; `delete_objects` removes an object once its messages are acked.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use ekiben_core::{AckResponder, Batch, ComponentConfig, CoreError, Input, StreamContext};
use ekiben_io_file::inputs::Scanner;
use serde_json::Value as JsonValue;

use crate::client::{S3Conn, DEFAULT_MAX_OBJECT_BYTES};

pub struct S3Input {
    conn: Arc<S3Conn>,
    prefix: String,
    scanner: Scanner,
    delete_objects: bool,
    max_object_bytes: usize,
    keys: Option<VecDeque<String>>,
    label: String,
}

impl S3Input {
    pub fn from_config(conf: &ComponentConfig) -> Result<Self, CoreError> {
        let obj = conf
            .config
            .as_object()
            .ok_or_else(|| CoreError::config("aws_s3 input config must be an object"))?;
        if obj
            .get("sqs")
            .and_then(|s| s.get("url"))
            .and_then(JsonValue::as_str)
            .is_some_and(|u| !u.is_empty())
        {
            return Err(CoreError::config(
                "aws_s3 input: sqs is not supported by ekiben; list the bucket on a schedule instead",
            ));
        }
        let max_object_bytes = match obj.get("max_object_bytes") {
            None => DEFAULT_MAX_OBJECT_BYTES,
            Some(v) => v
                .as_u64()
                .filter(|n| *n > 0 && *n as usize <= DEFAULT_MAX_OBJECT_BYTES)
                .ok_or_else(|| {
                    CoreError::config(format!(
                        "aws_s3 input: max_object_bytes must be 1 to {DEFAULT_MAX_OBJECT_BYTES}, e.g. 10485760"
                    ))
                })? as usize,
        };
        Ok(Self {
            conn: Arc::new(S3Conn::from_config(
                "aws_s3 input",
                obj,
                Duration::from_secs(30),
            )?),
            prefix: obj
                .get("prefix")
                .and_then(JsonValue::as_str)
                .unwrap_or("")
                .to_string(),
            scanner: Scanner::from_config(obj)?,
            delete_objects: obj
                .get("delete_objects")
                .and_then(JsonValue::as_bool)
                .unwrap_or(false),
            max_object_bytes,
            keys: None,
            label: conf.label.clone(),
        })
    }
}

#[async_trait]
impl Input for S3Input {
    fn input_type(&self) -> &'static str {
        "aws_s3"
    }

    fn label(&self) -> &str {
        &self.label
    }

    async fn read_batch(
        &mut self,
        ctx: &StreamContext,
    ) -> Result<(Batch, AckResponder), CoreError> {
        if self.keys.is_none() {
            self.keys = Some(self.conn.list_keys(&ctx.org, &self.prefix).await?.into());
        }
        loop {
            let Some(key) = self.keys.as_mut().and_then(VecDeque::pop_front) else {
                return Err(CoreError::EndOfInput);
            };
            if key.ends_with('/') {
                continue;
            }
            let (body, headers) = self
                .conn
                .get_object(&ctx.org, &key, self.max_object_bytes)
                .await?;
            let mut parts = self.scanner.scan(&body, &key)?;
            let header = |name: &str| {
                headers
                    .get(name)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string()
            };
            let modified = header("last-modified");
            let modified_unix = chrono::DateTime::parse_from_rfc2822(&modified)
                .map(|t| t.timestamp().to_string())
                .unwrap_or_default();
            for p in &mut parts {
                p.meta_set_str("s3_key", key.clone());
                p.meta_set_str("s3_bucket", self.conn.bucket.clone());
                p.meta_set_str("s3_last_modified", modified.clone());
                p.meta_set_str("s3_last_modified_unix", modified_unix.clone());
                p.meta_set_str("s3_content_type", header("content-type"));
                p.meta_set_str("s3_content_encoding", header("content-encoding"));
                for (name, value) in &headers {
                    if let (Some(meta), Ok(v)) =
                        (name.as_str().strip_prefix("x-amz-meta-"), value.to_str())
                    {
                        p.meta_set_str(meta.to_string(), v.to_string());
                    }
                }
            }
            if parts.is_empty() {
                continue;
            }
            let ack = if self.delete_objects {
                let conn = Arc::clone(&self.conn);
                let org = Arc::clone(&ctx.org);
                AckResponder::from_fn(move |err| {
                    if err.is_some() {
                        return;
                    }
                    if let Ok(rt) = tokio::runtime::Handle::try_current() {
                        rt.spawn(async move {
                            if let Err(e) = conn.delete_object(&org, &key).await {
                                tracing::warn!("aws_s3 input: delete after ack failed: {e}");
                            }
                        });
                    }
                })
            } else {
                AckResponder::noop()
            };
            return Ok((parts, ack));
        }
    }
}
