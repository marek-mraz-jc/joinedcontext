//! The connection fields Bento's `aws_s3` input and output share, and the few S3 calls they make
//! (PutObject, ListObjectsV2, GetObject, DeleteObject), signed with SigV4 and sent only through
//! ekiben-io-http's guarded clients, so the organization's outbound rule decides every host.

use std::time::Duration;

use bytes::Bytes;
use ekiben_core::{CoreError, OrgContext, SecretString};
use ekiben_io_http::client::GuardedHttp;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::Method;
use serde_json::{Map, Value as JsonValue};

use crate::sign::{self, Credentials};

/// The most an error answer or a listing page is read, and the default cap of one object.
const MAX_ERROR_BYTES: usize = 64 * 1024;
const MAX_LIST_BYTES: usize = 8 * 1024 * 1024;
pub const DEFAULT_MAX_OBJECT_BYTES: usize = 256 * 1024 * 1024;

pub struct S3Conn {
    pub bucket: String,
    region: String,
    /// `scheme://host[:port]` without a trailing slash.
    endpoint: String,
    path_style: bool,
    creds: Option<Credentials>,
    http: GuardedHttp,
}

fn str_field<'a>(obj: &'a Map<String, JsonValue>, name: &str) -> Option<&'a str> {
    obj.get(name)
        .and_then(JsonValue::as_str)
        .filter(|s| !s.is_empty())
}

impl S3Conn {
    /// Reads `bucket`, `region`, `endpoint`, `force_path_style_urls` and `credentials` of `kind`'s
    /// config. Only a static key is taken: the AWS profile, role and instance-metadata chain is
    /// refused, the key and secret come in as `${NAME}` from the pipeline's secretRefs.
    pub fn from_config(
        kind: &str,
        obj: &Map<String, JsonValue>,
        timeout: Duration,
    ) -> Result<Self, CoreError> {
        let bucket = str_field(obj, "bucket")
            .ok_or_else(|| CoreError::config(format!("{kind}: field bucket is required")))?
            .to_string();
        if bucket.contains('/') {
            return Err(CoreError::config(format!(
                "{kind}: bucket '{bucket}' must be a bucket name, without '/'"
            )));
        }
        let region = str_field(obj, "region").unwrap_or("us-east-1").to_string();
        let endpoint = match str_field(obj, "endpoint") {
            Some(e) => {
                let url = url::Url::parse(e).map_err(|err| {
                    CoreError::config(format!(
                        "{kind}: endpoint must be an http(s) URL such as https://minio.example.org:9000: {err}"
                    ))
                })?;
                if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
                    return Err(CoreError::config(format!(
                        "{kind}: endpoint must be an http(s) URL such as https://minio.example.org:9000"
                    )));
                }
                format!("{}://{}", url.scheme(), sign::host_header(&url))
            }
            None => format!("https://s3.{region}.amazonaws.com"),
        };
        let path_style = obj
            .get("force_path_style_urls")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);
        let creds = match obj.get("credentials").and_then(JsonValue::as_object) {
            None => None,
            Some(c) => {
                for (field, set) in [
                    (
                        "profile",
                        c.get("profile")
                            .and_then(JsonValue::as_str)
                            .is_some_and(|s| !s.is_empty()),
                    ),
                    (
                        "role",
                        c.get("role")
                            .and_then(JsonValue::as_str)
                            .is_some_and(|s| !s.is_empty()),
                    ),
                    (
                        "role_external_id",
                        c.get("role_external_id")
                            .and_then(JsonValue::as_str)
                            .is_some_and(|s| !s.is_empty()),
                    ),
                    (
                        "from_ec2_role",
                        c.get("from_ec2_role")
                            .and_then(JsonValue::as_bool)
                            .unwrap_or(false),
                    ),
                ] {
                    if set {
                        return Err(CoreError::config(format!(
                            "{kind}: credentials.{field} is not supported by ekiben (no AWS credential chain); give credentials.id and credentials.secret from a secretRef"
                        )));
                    }
                }
                match (str_field(c, "id"), str_field(c, "secret")) {
                    (None, None) => None,
                    (Some(id), Some(secret)) => Some(Credentials {
                        id: id.to_string(),
                        secret: SecretString::new(secret),
                        token: str_field(c, "token").map(SecretString::new),
                    }),
                    _ => {
                        return Err(CoreError::config(format!(
                            "{kind}: credentials.id and credentials.secret go together"
                        )))
                    }
                }
            }
        };
        Ok(Self {
            bucket,
            region,
            endpoint,
            path_style,
            creds,
            http: GuardedHttp::new(timeout)?,
        })
    }

    /// The URL of `key` (empty for the bucket itself), the key URI-encoded once as S3 signs it.
    fn url(&self, key: &str, query: &str) -> String {
        let (scheme, host) = self.endpoint.split_once("://").unwrap_or(("https", ""));
        let path = sign::uri_encode(key, true);
        let base = if self.path_style {
            format!("{scheme}://{host}/{}/{path}", self.bucket)
        } else {
            format!("{scheme}://{}.{host}/{path}", self.bucket)
        };
        if query.is_empty() {
            base
        } else {
            format!("{base}?{query}")
        }
    }

    async fn call(
        &self,
        org: &OrgContext,
        method: Method,
        key: &str,
        query: &str,
        mut headers: Vec<(String, String)>,
        body: Bytes,
    ) -> Result<reqwest::Response, CoreError> {
        let (url, client) = self.http.for_url(org, &self.url(key, query))?;
        org.admit_http_request().await;
        if let Some(creds) = &self.creds {
            let hash = sign::sha256_hex(&body);
            sign::sign(
                method.as_str(),
                &url,
                &mut headers,
                &hash,
                creds,
                &self.region,
                jiff::Timestamp::now(),
            )?;
        }
        let mut map = HeaderMap::new();
        for (k, v) in headers {
            let name = HeaderName::from_bytes(k.as_bytes())
                .map_err(|_| CoreError::config(format!("aws_s3: '{k}' is not a header name")))?;
            let value = HeaderValue::from_str(&v).map_err(|_| {
                CoreError::config(format!("aws_s3: header {k} has a value HTTP cannot carry"))
            })?;
            map.insert(name, value);
        }
        let resp = client
            .request(method.clone(), url)
            .headers(map)
            .body(body)
            .send()
            .await
            .map_err(|e| CoreError::other(format!("aws_s3: {method} {key}: {e}")))?;
        if resp.status().is_success() {
            return Ok(resp);
        }
        let status = resp.status();
        let text = read_capped(resp, MAX_ERROR_BYTES).await.unwrap_or_default();
        let code = xml_values(&String::from_utf8_lossy(&text), "Code")
            .into_iter()
            .next()
            .unwrap_or_default();
        Err(CoreError::other(format!(
            "aws_s3: {method} s3://{}/{key} answered {status} {code}",
            self.bucket
        )))
    }

    pub async fn put_object(
        &self,
        org: &OrgContext,
        key: &str,
        headers: Vec<(String, String)>,
        body: Bytes,
    ) -> Result<(), CoreError> {
        self.call(org, Method::PUT, key, "", headers, body)
            .await
            .map(drop)
    }

    /// Every key under `prefix` with its size check left to [`Self::get_object`], page by page.
    pub async fn list_keys(
        &self,
        org: &OrgContext,
        prefix: &str,
    ) -> Result<Vec<String>, CoreError> {
        let mut keys = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut query = format!("list-type=2&prefix={}", sign::uri_encode(prefix, false));
            if let Some(t) = &token {
                query.push_str(&format!(
                    "&continuation-token={}",
                    sign::uri_encode(t, false)
                ));
            }
            let resp = self
                .call(org, Method::GET, "", &query, Vec::new(), Bytes::new())
                .await?;
            let body = read_capped(resp, MAX_LIST_BYTES).await?;
            let text = String::from_utf8_lossy(&body);
            keys.extend(xml_values(&text, "Key"));
            let truncated = xml_values(&text, "IsTruncated")
                .first()
                .is_some_and(|v| v == "true");
            token = xml_values(&text, "NextContinuationToken")
                .into_iter()
                .next();
            if !truncated || token.is_none() {
                return Ok(keys);
            }
        }
    }

    /// The object's bytes (refused past `max_bytes`) and its response headers.
    pub async fn get_object(
        &self,
        org: &OrgContext,
        key: &str,
        max_bytes: usize,
    ) -> Result<(Bytes, HeaderMap), CoreError> {
        let resp = self
            .call(org, Method::GET, key, "", Vec::new(), Bytes::new())
            .await?;
        let headers = resp.headers().clone();
        let body = read_capped(resp, max_bytes)
            .await
            .map_err(|e| CoreError::other(format!("aws_s3: s3://{}/{key}: {e}", self.bucket)))?;
        Ok((body, headers))
    }

    pub async fn delete_object(&self, org: &OrgContext, key: &str) -> Result<(), CoreError> {
        self.call(org, Method::DELETE, key, "", Vec::new(), Bytes::new())
            .await
            .map(drop)
    }
}

async fn read_capped(mut resp: reqwest::Response, max: usize) -> Result<Bytes, CoreError> {
    let mut out = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| CoreError::other(format!("aws_s3: reading the answer: {e}")))?
    {
        if out.len() + chunk.len() > max {
            return Err(CoreError::other(format!(
                "answer is over the {max} bytes ekiben reads (max_object_bytes)"
            )));
        }
        out.extend_from_slice(&chunk);
    }
    Ok(Bytes::from(out))
}

/// The text of every `<tag>` element, XML entities decoded; S3's answers nest no element of the
/// same name, so a flat scan is exact for Key, IsTruncated, NextContinuationToken and Code.
fn xml_values(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let Some(end) = after.find(&close) else { break };
        out.push(
            after[..end]
                .replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace("&quot;", "\"")
                .replace("&apos;", "'")
                .replace("&amp;", "&"),
        );
        rest = &after[end + close.len()..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn conn(v: JsonValue) -> Result<S3Conn, CoreError> {
        S3Conn::from_config("aws_s3", v.as_object().unwrap(), Duration::from_secs(5))
    }

    #[test]
    fn urls_follow_bentos_addressing() {
        let c = conn(json!({"bucket": "raw", "endpoint": "http://minio:9000", "force_path_style_urls": true})).unwrap();
        assert_eq!(
            c.url("a b/c$.json", ""),
            "http://minio:9000/raw/a%20b/c%24.json"
        );
        let c = conn(json!({"bucket": "raw", "region": "eu-central-1"})).unwrap();
        assert_eq!(
            c.url("k", "list-type=2"),
            "https://raw.s3.eu-central-1.amazonaws.com/k?list-type=2"
        );
    }

    #[test]
    fn the_aws_credential_chain_and_half_keys_are_refused() {
        for field in [
            json!({"profile": "default"}),
            json!({"role": "arn:aws:iam::1:role/x"}),
            json!({"from_ec2_role": true}),
            json!({"role_external_id": "x"}),
        ] {
            let e = conn(json!({"bucket": "b", "credentials": field}))
                .err()
                .unwrap()
                .to_string();
            assert!(e.contains("no AWS credential chain"), "{e}");
        }
        let e = conn(json!({"bucket": "b", "credentials": {"id": "AK"}}))
            .err()
            .unwrap()
            .to_string();
        assert!(e.contains("go together"), "{e}");
        assert!(conn(json!({"bucket": "b/c"})).is_err());
        assert!(conn(json!({"bucket": "b", "endpoint": "ftp://x"})).is_err());
    }

    #[test]
    fn listing_fields_are_read_with_entities_decoded() {
        let xml = "<ListBucketResult><IsTruncated>true</IsTruncated><Contents><Key>a&amp;b.csv</Key></Contents><Contents><Key>c.csv</Key></Contents><NextContinuationToken>t1</NextContinuationToken></ListBucketResult>";
        assert_eq!(xml_values(xml, "Key"), ["a&b.csv", "c.csv"]);
        assert_eq!(xml_values(xml, "NextContinuationToken"), ["t1"]);
    }

    #[tokio::test]
    async fn a_private_endpoint_is_refused_before_any_connection() {
        let c = conn(json!({"bucket": "b", "endpoint": "http://10.0.0.5:9000", "force_path_style_urls": true})).unwrap();
        let org = OrgContext::new(ekiben_core::OrgId("o".into()), Default::default());
        let e = c
            .put_object(&org, "k", Vec::new(), Bytes::from_static(b"x"))
            .await
            .err()
            .unwrap();
        assert!(matches!(e, CoreError::AccessDenied { .. }), "{e}");
    }
}
