#![forbid(unsafe_code)]
//! Built-in OData 4.01 datasource (R3, §3.2):
//! Queries any `jc` module exposing OData 4.01 (such as Antares `/x/views/{view}/odata/v4`,
//! dataroom, or controlroom itself) with `$filter`, `$select`, `$orderby`, `$top`, and `$apply`
//! (`aggregate`, `groupby`), exchanging the caller's token for the target's audience.

use crate::guard::{OutboundGuard, OutboundRequest};
use crate::{
    Datasource, DsError, DsFuture, EditorFieldHint, EditorSchema, Health, ImportOutcome, Query,
    QueryCtx, ResourceReq, ResourceResp, TokenExchanger,
};
use controlroom_model::{FieldColumn, FieldType, Frame, FrameMeta, FrameSet};
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
