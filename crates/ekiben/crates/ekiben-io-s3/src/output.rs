//! Bento's `aws_s3` output (`internal/impl/aws/output_s3.go`): each message one object at the
//! interpolated `path`, with the interpolated content headers, tags and the message's metadata
//! as `x-amz-meta-*`. A batch is written part by part; batching to one object is done by the
//! batch policy's `archive` processor, as in Bento.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use ekiben_bloblang::FieldExpression;
use ekiben_core::config::parse_duration_str;
use ekiben_core::{Batch, ComponentConfig, CoreError, Output, PartsView, StreamContext};
use serde_json::Value as JsonValue;

use crate::client::S3Conn;
use crate::sign::uri_encode;

const DEFAULT_PATH: &str = r#"${!count("files")}-${!timestamp_unix_nano()}.txt"#;

pub struct S3Output {
    conn: S3Conn,
    path: FieldExpression,
    /// (header, value) for content_type, content_encoding, cache_control, content_disposition,
    /// content_language and website_redirect_location.
    headers: Vec<(&'static str, FieldExpression)>,
    tags: Vec<(String, FieldExpression)>,
    fixed: Vec<(String, String)>,
    exclude_prefixes: Vec<String>,
    label: String,
}

impl S3Output {
    pub fn from_config(conf: &ComponentConfig, ctx: &StreamContext) -> Result<Self, CoreError> {
        let obj = conf
            .config
            .as_object()
            .ok_or_else(|| CoreError::config("aws_s3 output config must be an object"))?;
        let str_of = |name: &str| obj.get(name).and_then(JsonValue::as_str).unwrap_or("");
        let timeout = match str_of("timeout") {
            "" => Duration::from_secs(5),
            t => parse_duration_str(t)?,
        };
        let conn = S3Conn::from_config("aws_s3 output", obj, timeout)?;
        let path = match str_of("path") {
            "" => DEFAULT_PATH,
            p => p,
        };
        let path = ctx.blobl_env.compile_field(path)?;
        let mut headers = Vec::new();
        for (field, header, default) in [
            ("content_type", "content-type", "application/octet-stream"),
            ("content_encoding", "content-encoding", ""),
            ("cache_control", "cache-control", ""),
            ("content_disposition", "content-disposition", ""),
            ("content_language", "content-language", ""),
            (
                "website_redirect_location",
                "x-amz-website-redirect-location",
                "",
            ),
        ] {
            let v = match str_of(field) {
                "" => default,
                v => v,
            };
            if !v.is_empty() {
                headers.push((header, ctx.blobl_env.compile_field(v)?));
            }
        }
        let mut tags = Vec::new();
        if let Some(t) = obj.get("tags").and_then(JsonValue::as_object) {
            for (k, v) in t {
                let v = v.as_str().ok_or_else(|| {
                    CoreError::config(format!("aws_s3 output: tags.{k} must be a string"))
                })?;
                tags.push((k.clone(), ctx.blobl_env.compile_field(v)?));
            }
        }
        let mut fixed = Vec::new();
        for (field, header) in [
            ("storage_class", "x-amz-storage-class"),
            ("server_side_encryption", "x-amz-server-side-encryption"),
            ("kms_key_id", "x-amz-server-side-encryption-aws-kms-key-id"),
        ] {
            match str_of(field) {
                "" if field == "storage_class" => fixed.push((header.into(), "STANDARD".into())),
                "" => {}
                v => fixed.push((header.into(), v.into())),
            }
        }
        let exclude_prefixes = obj
            .get("metadata")
            .and_then(|m| m.get("exclude_prefixes"))
            .and_then(JsonValue::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(JsonValue::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        Ok(Self {
            conn,
            path,
            headers,
            tags,
            fixed,
            exclude_prefixes,
            label: conf.label.clone(),
        })
    }
}

#[async_trait]
impl Output for S3Output {
    fn output_type(&self) -> &'static str {
        "aws_s3"
    }

    fn label(&self) -> &str {
        &self.label
    }

    // ponytail: parts go one after another; Bento's max_in_flight parallel puts come when a
    // throughput measurement asks for them.
    async fn write_batch(&self, ctx: &StreamContext, mut batch: Batch) -> Result<(), CoreError> {
        for p in &mut batch {
            let _ = p.as_bytes();
        }
        let view = PartsView(&batch);
        for (i, part) in batch.iter().enumerate() {
            let interp = |what: &str, f: &FieldExpression| {
                f.resolve_string(i, &view).map_err(|e| {
                    CoreError::other(format!("aws_s3 output: {what} interpolation error: {e}"))
                })
            };
            let key = interp("path", &self.path)?;
            let key = key.trim_start_matches('/').to_string();
            if key.is_empty() {
                return Err(CoreError::other(
                    "aws_s3 output: path resolved to an empty key",
                ));
            }
            let mut headers = self.fixed.clone();
            for (h, f) in &self.headers {
                headers.push(((*h).to_string(), interp(h, f)?));
            }
            if !self.tags.is_empty() {
                let mut pairs = Vec::new();
                for (k, f) in &self.tags {
                    pairs.push(format!(
                        "{}={}",
                        uri_encode(k, false),
                        uri_encode(&interp("tags", f)?, false)
                    ));
                }
                headers.push(("x-amz-tagging".into(), pairs.join("&")));
            }
            for (k, v) in part.meta_all() {
                if self
                    .exclude_prefixes
                    .iter()
                    .any(|p| k.starts_with(p.as_str()))
                {
                    continue;
                }
                let name = format!("x-amz-meta-{}", k.to_ascii_lowercase());
                let value = v.to_string_coerced();
                // a key or value an HTTP header cannot carry stays off the object
                if reqwest::header::HeaderName::from_bytes(name.as_bytes()).is_ok()
                    && reqwest::header::HeaderValue::from_str(&value).is_ok()
                {
                    headers.push((name, value));
                }
            }
            self.conn
                .put_object(&ctx.org, &key, headers, part.as_bytes_ref())
                .await?;
        }
        Ok(())
    }
}

pub fn build(conf: &ComponentConfig, ctx: &StreamContext) -> Result<Arc<dyn Output>, CoreError> {
    Ok(Arc::new(S3Output::from_config(conf, ctx)?))
}
