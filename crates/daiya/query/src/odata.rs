#![forbid(unsafe_code)]
//! Built-in OData 4.01 datasource (R3, §3.2):
//! Queries any `jc` module exposing OData 4.01 (such as Antares `/x/views/{view}/odata/v4`,
//! dataroom, or daiya itself) with `$filter`, `$select`, `$orderby`, `$top`, and `$apply`
//! (`aggregate`, `groupby`), exchanging the caller's token for the target's audience.

use crate::guard::{OutboundGuard, OutboundRequest};
use crate::{
    Datasource, DsError, DsFuture, EditorFieldHint, EditorSchema, Health, ImportOutcome, Query,
    QueryCtx, ResourceReq, ResourceResp, TokenExchanger,
};
use daiya_model::{FieldColumn, FieldType, Frame, FrameMeta, FrameSet};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use reqwest::Method;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Built-in OData 4.01 datasource (§3.2, R3).
pub struct ODataDatasource {
    pub uid: String,
    pub service_url: String,
    pub audience: String,
    pub guard: OutboundGuard,
    pub token_exchanger: Arc<dyn TokenExchanger>,
}

impl ODataDatasource {
    #[must_use]
    pub fn new(
        uid: impl Into<String>,
        service_url: impl Into<String>,
        audience: impl Into<String>,
        guard: OutboundGuard,
        token_exchanger: Arc<dyn TokenExchanger>,
    ) -> Self {
        Self {
            uid: uid.into(),
            service_url: service_url.into().trim_end_matches('/').to_owned(),
            audience: audience.into(),
            guard,
            token_exchanger,
        }
    }

    fn build_headers(&self, ctx: &QueryCtx) -> Result<Vec<(String, String)>, DsError> {
        let mut headers = vec![
            ("Accept".to_owned(), "application/json".to_owned()),
            ("OData-Version".to_owned(), "4.01".to_owned()),
        ];
        if let Some(caller_token) = &ctx.subject.bearer_token
            && !caller_token.is_empty()
        {
            let exchanged = self
                .token_exchanger
                .exchange_for_audience(caller_token, &self.audience)?;
            headers.push((
                "Authorization".to_owned(),
                format!("Bearer {}", exchanged.as_str()),
            ));
        }
        Ok(headers)
    }
}

/// OData CSDL `SimpleIdentifier`, ASCII subset: a letter or `_`, then letters, digits or `_`,
/// at most 128 characters.
fn is_odata_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    s.len() <= 128
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn odata_rows_to_frame(ref_id: &str, entity_set: &str, rows: &[Value]) -> Frame {
    let mut col_names: Vec<String> = Vec::new();
    for row in rows {
        if let Some(obj) = row.as_object() {
            for k in obj.keys() {
                if !k.starts_with("@odata.") && !col_names.contains(k) {
                    col_names.push(k.clone());
                }
            }
        }
    }

    let fields = col_names
        .into_iter()
        .map(|name| {
            let values: Vec<Value> = rows
                .iter()
                .map(|r| r.get(&name).cloned().unwrap_or(Value::Null))
                .collect();
            let field_type = values.iter().find(|v| !v.is_null()).map_or(
                FieldType::String,
                |sample| match sample {
                    Value::Number(_) => FieldType::Number,
                    Value::Bool(_) => FieldType::Bool,
                    Value::String(s) if chrono::DateTime::parse_from_rfc3339(s).is_ok() => {
                        FieldType::Time
                    }
                    Value::String(_) => FieldType::String,
                    _ => FieldType::Json,
                },
            );
            FieldColumn {
                name,
                field_type,
                unit: None,
                display_name: None,
                labels: BTreeMap::new(),
                values,
            }
        })
        .collect();

    Frame {
        name: entity_set.to_owned(),
        ref_id: Some(ref_id.to_owned()),
        fields,
        meta: FrameMeta {
            preferred_visualisation_type: Some("table".to_owned()),
            ..FrameMeta::default()
        },
    }
}

impl Datasource for ODataDatasource {
    fn kind(&self) -> &'static str {
        "odata"
    }

    fn query<'a>(
        &'a self,
        ctx: &'a QueryCtx,
        queries: &'a [Query],
    ) -> DsFuture<'a, Result<Vec<FrameSet>, DsError>> {
        Box::pin(async move {
            let headers = self.build_headers(ctx)?;
            let mut out = Vec::with_capacity(queries.len());

            for q in queries {
                if q.hide {
                    continue;
                }
                let entity_set = q
                    .model
                    .get("entitySet")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| DsError::InvalidQuery {
                        ref_id: q.ref_id.clone(),
                        reason: "missing 'entitySet' in OData query".into(),
                    })?;
                // The entity set becomes a path segment: only an OData simple identifier, so a
                // query cannot walk to another path of the service (`../`, `?`, `#`, `/`).
                if !is_odata_identifier(entity_set) {
                    return Err(DsError::InvalidQuery {
                        ref_id: q.ref_id.clone(),
                        reason: "'entitySet' must be an OData simple identifier".into(),
                    });
                }

                let mut params: Vec<String> = Vec::new();
                for (json_key, odata_key) in [
                    ("$filter", "$filter"),
                    ("filter", "$filter"),
                    ("$select", "$select"),
                    ("select", "$select"),
                    ("$orderby", "$orderby"),
                    ("orderby", "$orderby"),
                    ("$apply", "$apply"),
                    ("apply", "$apply"),
                ] {
                    if let Some(val) = q
                        .model
                        .get(json_key)
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    {
                        params.push(format!(
                            "{odata_key}={}",
                            utf8_percent_encode(val, NON_ALPHANUMERIC)
                        ));
                    }
                }
                if let Some(top) = q.model.get("$top").or_else(|| q.model.get("top"))
                    && let Some(n) = top.as_u64()
                {
                    params.push(format!("$top={n}"));
                }

                let qs = params.join("&");
                let url = if qs.is_empty() {
                    format!("{}/{entity_set}", self.service_url)
                } else {
                    format!("{}/{entity_set}?{qs}", self.service_url)
                };

                let resp = self
                    .guard
                    .send(OutboundRequest {
                        method: Method::GET,
                        url,
                        headers: headers.clone(),
                        body: None,
                        timeout: Some(ctx.timeout),
                    })
                    .await?;

                if resp.status >= 400 {
                    let msg = String::from_utf8_lossy(&resp.body);
                    return Err(DsError::Upstream {
                        status: resp.status,
                        message: msg.chars().take(200).collect(),
                    });
                }

                let payload: Value =
                    serde_json::from_slice(&resp.body).map_err(|e| DsError::Upstream {
                        status: 502,
                        message: format!("invalid OData JSON response: {e}"),
                    })?;

                let rows = payload
                    .get("value")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();

                let frame = odata_rows_to_frame(&q.ref_id, entity_set, &rows);
                out.push(FrameSet::new(q.ref_id.clone(), vec![frame]));
            }

            Ok(out)
        })
    }

    fn health<'a>(&'a self, ctx: &'a QueryCtx) -> DsFuture<'a, Health> {
        Box::pin(async move {
            let Ok(headers) = self.build_headers(ctx) else {
                return Health::error("token exchange failed");
            };
            let url = format!("{}/$metadata", self.service_url);
            match self
                .guard
                .send(OutboundRequest {
                    method: Method::GET,
                    url,
                    headers,
                    body: None,
                    timeout: Some(ctx.timeout),
                })
                .await
            {
                Ok(resp) if resp.status == 200 => Health::ok("OData 4.01 service reachable"),
                Ok(resp) => Health::error(format!("OData $metadata returned HTTP {}", resp.status)),
                Err(e) => Health::error(format!("OData service unreachable: {e}")),
            }
        })
    }

    fn resource<'a>(
        &'a self,
        ctx: &'a QueryCtx,
        _r: ResourceReq,
    ) -> DsFuture<'a, Result<ResourceResp, DsError>> {
        Box::pin(async move {
            let headers = self.build_headers(ctx)?;
            let resp = self
                .guard
                .send(OutboundRequest {
                    method: Method::GET,
                    url: self.service_url.clone(),
                    headers,
                    body: None,
                    timeout: Some(ctx.timeout),
                })
                .await?;
            let data: Value =
                serde_json::from_slice(&resp.body).unwrap_or_else(|_| json!({ "value": [] }));
            Ok(ResourceResp { data })
        })
    }

    fn editor(&self) -> EditorSchema {
        EditorSchema {
            kind: "odata".into(),
            json_schema: json!({
                "$schema": "https://json-schema.org/draft-04/schema",
                "type": "object",
                "required": ["entitySet"],
                "properties": {
                    "entitySet": { "type": "string" },
                    "$filter": { "type": "string" },
                    "$select": { "type": "string" },
                    "$orderby": { "type": "string" },
                    "$apply": { "type": "string", "description": "aggregate / groupby transformation" },
                    "$top": { "type": "integer" }
                }
            }),
            field_hints: vec![
                EditorFieldHint {
                    name: "entitySet".into(),
                    label: "Entity Set".into(),
                    widget: "select".into(),
                    resource_path: Some("sets".into()),
                    required: true,
                },
                EditorFieldHint {
                    name: "$filter".into(),
                    label: "$filter".into(),
                    widget: "text".into(),
                    resource_path: None,
                    required: false,
                },
                EditorFieldHint {
                    name: "$apply".into(),
                    label: "$apply (groupby/aggregate)".into(),
                    widget: "text".into(),
                    resource_path: None,
                    required: false,
                },
            ],
        }
    }

    fn import(&self, grafana_target: &Value) -> Result<ImportOutcome, DsError> {
        let obj = grafana_target
            .as_object()
            .ok_or_else(|| DsError::InvalidQuery {
                ref_id: "A".into(),
                reason: "OData target must be a JSON object".into(),
            })?;
        let ref_id = obj
            .get("refId")
            .and_then(Value::as_str)
            .unwrap_or("A")
            .to_owned();
        let hide = obj.get("hide").and_then(Value::as_bool).unwrap_or(false);
        Ok(ImportOutcome {
            query: Query {
                ref_id,
                hide,
                model: Value::Object(obj.clone()),
            },
            warnings: Vec::new(),
        })
    }

    fn export(&self, q: &Query) -> Value {
        let mut map = q.model.as_object().cloned().unwrap_or_default();
        map.insert("refId".to_owned(), Value::String(q.ref_id.clone()));
        if q.hide {
            map.insert("hide".to_owned(), Value::Bool(true));
        }
        Value::Object(map)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DenyTokenExchanger;
    use crate::guard::GuardConfig;
    use daiya_model::{InstanceId, OrgId, Subject};
    use parking_lot::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// One-request-per-connection HTTP server on 127.0.0.1 that records each request head and
    /// answers `status` with `body`.
    async fn mock(status: u16, body: &'static str) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = vec![0u8; 8192];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                log.lock()
                    .push(String::from_utf8_lossy(&buf[..n]).into_owned());
                let resp = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            }
        });
        (format!("http://127.0.0.1:{}/odata/v4", addr.port()), seen)
    }

    fn ds(url: &str) -> ODataDatasource {
        ODataDatasource::new(
            "od",
            format!("{url}/"),
            "dataroom",
            OutboundGuard::new(GuardConfig {
                allow_hosts: vec!["127.0.0.1".into()],
                allow_http: true,
                allow_loopback_for_tests: true,
                ..GuardConfig::default()
            }),
            Arc::new(DenyTokenExchanger),
        )
    }

    fn ctx() -> QueryCtx {
        QueryCtx::new(
            OrgId::from_u128(1),
            InstanceId::from_u128(1),
            Subject::authenticated("u", vec![OrgId::from_u128(1)], vec![]),
            0,
            60_000,
        )
    }

    fn q(model: Value) -> Query {
        Query {
            ref_id: "A".into(),
            hide: false,
            model,
        }
    }

    #[tokio::test]
    async fn query_sends_encoded_options_and_types_columns() {
        let (url, seen) = mock(
            200,
            r#"{"@odata.context":"x","value":[{"@odata.id":"r1","name":"a","n":1,"ok":true,"at":"2026-01-01T00:00:00Z","o":{"k":1}},{"name":"b","n":null,"extra":"e"}]}"#,
        )
        .await;
        let d = ds(&url);
        let out = d
            .query(
                &ctx(),
                &[
                    q(json!({"entitySet": "Stations", "$filter": "name eq 'a b'", "select": "name,n", "$orderby": "n desc", "$apply": "groupby((name))", "$top": 5})),
                    Query { ref_id: "H".into(), hide: true, model: json!({"entitySet": "Stations"}) },
                ],
            )
            .await
            .unwrap();
        let req = seen.lock()[0].clone();
        let line = req.lines().next().unwrap();
        assert_eq!(
            line,
            "GET /odata/v4/Stations?$filter=name%20eq%20%27a%20b%27&$select=name%2Cn&$orderby=n%20desc&$apply=groupby%28%28name%29%29&$top=5 HTTP/1.1"
        );
        assert!(req.to_ascii_lowercase().contains("odata-version: 4.01"));
        // No caller token on the subject: no Authorization header is sent.
        assert!(!req.to_ascii_lowercase().contains("authorization"));
        assert_eq!(out.len(), 1, "hidden query is skipped");
        let frame = &out[0].frames[0];
        assert_eq!(frame.name, "Stations");
        // serde_json `preserve_order` is on (the jc binary unifies it anyway): columns come out
        // in response order; columns first seen in later rows go last.
        let cols: Vec<_> = frame
            .fields
            .iter()
            .map(|f| (f.name.as_str(), f.field_type))
            .collect();
        assert_eq!(
            cols,
            vec![
                ("name", FieldType::String),
                ("n", FieldType::Number),
                ("ok", FieldType::Bool),
                ("at", FieldType::Time),
                ("o", FieldType::Json),
                ("extra", FieldType::String),
            ]
        );
        assert_eq!(frame.fields[5].values, vec![Value::Null, json!("e")]);
    }

    #[tokio::test]
    async fn entity_set_cannot_leave_the_service_path() {
        let (url, seen) = mock(200, r#"{"value":[]}"#).await;
        let d = ds(&url);
        for bad in [
            "../admin",
            "Sets/../../x",
            "Sets?$expand=*",
            "Sets#x",
            "a b",
            "1abc",
            "",
            "Sets('x')/Export",
        ] {
            let err = d
                .query(&ctx(), &[q(json!({"entitySet": bad}))])
                .await
                .unwrap_err();
            assert!(matches!(err, DsError::InvalidQuery { .. }), "{bad}: {err}");
        }
        assert!(
            seen.lock().is_empty(),
            "no request may leave for a refused entity set"
        );
        assert!(
            d.query(&ctx(), &[q(json!({"entitySet": "_Sets_2"}))])
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn upstream_errors_bad_json_health_and_token_exchange() {
        let (url, _) = mock(500, "boom").await;
        let err = ds(&url)
            .query(&ctx(), &[q(json!({"entitySet": "S"}))])
            .await
            .unwrap_err();
        assert!(
            matches!(err, DsError::Upstream { status: 500, .. }),
            "{err}"
        );
        assert!(ds(&url).health(&ctx()).await.message.contains("500"));

        let (url, seen) = mock(200, "not json").await;
        let err = ds(&url)
            .query(&ctx(), &[q(json!({"entitySet": "S"}))])
            .await
            .unwrap_err();
        assert!(
            matches!(err, DsError::Upstream { status: 502, .. }),
            "{err}"
        );
        let health = ds(&url).health(&ctx()).await;
        assert!(
            seen.lock()
                .last()
                .unwrap()
                .starts_with("GET /odata/v4/$metadata ")
        );
        assert!(health.message.contains("reachable"), "{}", health.message);
        assert_eq!(
            ds(&url)
                .resource(
                    &ctx(),
                    ResourceReq {
                        path: "sets".into(),
                        params: BTreeMap::new()
                    }
                )
                .await
                .unwrap()
                .data,
            json!({"value": []})
        );

        // A caller token is never forwarded as is: the default exchanger refuses.
        let mut c = ctx();
        c.subject.bearer_token = Some("caller-jwt".into());
        let err = ds(&url)
            .query(&c, &[q(json!({"entitySet": "S"}))])
            .await
            .unwrap_err();
        assert!(matches!(err, DsError::TokenExchange { .. }), "{err}");
        assert!(ds(&url).health(&c).await.message.contains("token exchange"));
        assert!(!seen.lock().iter().any(|r| r.contains("caller-jwt")));
    }

    #[test]
    fn editor_import_and_export_round_trip() {
        let d = ds("http://127.0.0.1:1/odata/v4");
        assert_eq!(d.kind(), "odata");
        assert_eq!(d.service_url, "http://127.0.0.1:1/odata/v4");
        assert_eq!(d.editor().json_schema["required"], json!(["entitySet"]));
        let target = json!({"refId": "B", "hide": true, "entitySet": "S", "$top": 3});
        let imported = d.import(&target).unwrap();
        assert_eq!(imported.query.ref_id, "B");
        assert!(imported.query.hide);
        assert_eq!(d.export(&imported.query), target);
        assert!(d.import(&json!("S")).is_err());
        let missing = d.import(&json!({"entitySet": "S"})).unwrap();
        assert_eq!(missing.query.ref_id, "A");
    }
}
