#![forbid(unsafe_code)]
//! Loki datasource trait slot (R3, §3.2: "Loki as a trait slot only").

use crate::{
    Datasource, DsError, DsFuture, EditorFieldHint, EditorSchema, Health, ImportOutcome, Query,
    QueryCtx, ResourceReq, ResourceResp,
};
use controlroom_model::FrameSet;
use serde_json::{Value, json};

/// Trait slot for Loki LogQL datasource (`expr`, `queryType`, `maxLines` kept on import/export, R3).
#[derive(Debug, Clone, Default)]
pub struct LokiDatasource {
    pub uid: String,
}

impl LokiDatasource {
    #[must_use]
    pub fn new(uid: impl Into<String>) -> Self {
        Self { uid: uid.into() }
    }
}

impl Datasource for LokiDatasource {
    fn kind(&self) -> &'static str {
        "loki"
    }

    fn query<'a>(
        &'a self,
        _ctx: &'a QueryCtx,
        _q: &'a [Query],
    ) -> DsFuture<'a, Result<Vec<FrameSet>, DsError>> {
        Box::pin(async move {
            Err(DsError::NotImplemented(
                "loki datasource is a trait slot only in this deployment".into(),
            ))
        })
    }

    fn health<'a>(&'a self, _ctx: &'a QueryCtx) -> DsFuture<'a, Health> {
        Box::pin(async move {
            Health::error("Loki is not deployed on this installation (trait slot only)")
        })
    }

    fn resource<'a>(
        &'a self,
        _ctx: &'a QueryCtx,
        _r: ResourceReq,
    ) -> DsFuture<'a, Result<ResourceResp, DsError>> {
        Box::pin(async move {
            Err(DsError::NotImplemented(
                "loki datasource is a trait slot only".into(),
            ))
        })
    }

    fn editor(&self) -> EditorSchema {
        EditorSchema {
            kind: "loki".into(),
            json_schema: json!({
                "$schema": "https://json-schema.org/draft-04/schema",
                "type": "object",
                "required": ["expr"],
                "properties": {
                    "expr": { "type": "string" },
                    "queryType": { "type": "string" },
                    "maxLines": { "type": "integer" }
                }
            }),
            field_hints: vec![EditorFieldHint {
                name: "expr".into(),
                label: "LogQL Expression".into(),
                widget: "code".into(),
                resource_path: None,
                required: true,
            }],
        }
    }

    fn import(&self, grafana_target: &Value) -> Result<ImportOutcome, DsError> {
        let obj = grafana_target.as_object().ok_or_else(|| DsError::InvalidQuery {
            ref_id: "A".into(),
            reason: "Loki target must be a JSON object".into(),
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
            warnings: vec!["loki datasource is a trait slot only; target kept for export".into()],
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
