#![forbid(unsafe_code)]
//! Sample third-party WASM datasource plugin implementing `jc:controlroom/datasource@0.1.0` (R5).

use crate::wasm::{WasmHostImports, WasmPluginGuest};
use crate::{
    DsFuture, EditorFieldHint, EditorSchema, ImportOutcome, Query, QueryCtx,
};
use controlroom_model::{FieldColumn, Frame, FrameMeta, FrameSet};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Sample third-party datasource plugin (`wasm:sample-json`) implementing all 6 exports of
/// `jc:controlroom/datasource@0.1.0` (`query`, `health`, `resource`, `editor-schema`,
/// `import-target`, `export-target`) and using only `http-out` and `secret` host imports (R5).
#[derive(Debug, Clone, Default)]
pub struct SampleMetricsWasmPlugin {
    pub simulated_memory_bytes: usize,
    pub simulated_fuel: u64,
}

impl SampleMetricsWasmPlugin {
    pub const COMPONENT_MAGIC: &'static [u8] =
        b"\0asm\x0d\0\x01\0jc:controlroom/datasource@0.1.0:sample-json";

    #[must_use]
    pub const fn new() -> Self {
        Self {
            simulated_memory_bytes: 4 << 20, // 4 MiB (well within 64 MiB limit)
            simulated_fuel: 50_000,
        }
    }
}

impl WasmPluginGuest for SampleMetricsWasmPlugin {
    fn plugin_id(&self) -> &'static str {
        "wasm:sample-json"
    }

    fn component_bytes(&self) -> &[u8] {
        Self::COMPONENT_MAGIC
    }

    fn memory_usage_bytes(&self) -> usize {
        self.simulated_memory_bytes
    }

    fn fuel_consumed(&self) -> u64 {
        self.simulated_fuel
    }

    fn export_query<'a>(
        &'a self,
        host: WasmHostImports<'a>,
        ctx: &'a QueryCtx,
        queries_json: &'a str,
    ) -> DsFuture<'a, Result<String, String>> {
        Box::pin(async move {
            let queries: Vec<Query> =
                serde_json::from_str(queries_json).map_err(|e| e.to_string())?;

            // If a secretRef is configured, resolve it via host `secret` import (never returned in frames)
            if !host.allowed_secret_refs.is_empty() {
                let _resolved = host
                    .resolve_secret("apiKey")
                    .map_err(|e| e.to_string())?;
            }

            let mut out = Vec::with_capacity(queries.len());
            for q in queries {
                let metric = q
                    .model
                    .get("metric")
                    .and_then(Value::as_str)
                    .unwrap_or("sample_metric");
                let scale = q
                    .model
                    .get("scale")
                    .and_then(Value::as_f64)
                    .unwrap_or(1.0);

                let frame = Frame {
                    name: metric.to_owned(),
                    ref_id: Some(q.ref_id.clone()),
                    fields: vec![
                        FieldColumn::time("Time", vec![ctx.from_ms, ctx.to_ms]),
                        FieldColumn::numbers(
                            metric,
                            BTreeMap::new(),
                            vec![Some(10.0 * scale), Some(20.0 * scale)],
                        ),
                    ],
                    meta: FrameMeta::default(),
                };
                out.push(FrameSet::new(q.ref_id, vec![frame]));
            }
            serde_json::to_string(&out).map_err(|e| e.to_string())
        })
    }

    fn export_health<'a>(
        &'a self,
        _host: WasmHostImports<'a>,
        _ctx: &'a QueryCtx,
    ) -> DsFuture<'a, (bool, String)> {
        Box::pin(async move { (true, "sample WASM datasource healthy".to_owned()) })
    }

    fn export_resource<'a>(
        &'a self,
        _host: WasmHostImports<'a>,
        _ctx: &'a QueryCtx,
        path: &'a str,
        _params_json: &'a str,
    ) -> DsFuture<'a, Result<String, String>> {
        Box::pin(async move {
            let items = match path {
                "metrics" => json!(["cpu_usage", "memory_usage", "request_rate"]),
                _ => json!([]),
            };
            Ok(items.to_string())
        })
    }

    fn export_editor_schema(&self) -> String {
        let schema = EditorSchema {
            kind: "wasm:sample-json".into(),
            json_schema: json!({
                "type": "object",
                "required": ["metric"],
                "properties": {
                    "metric": { "type": "string" },
                    "scale": { "type": "number" }
                }
            }),
            field_hints: vec![EditorFieldHint {
                name: "metric".into(),
                label: "Metric Name".into(),
                widget: "select".into(),
                resource_path: Some("metrics".into()),
                required: true,
            }],
        };
        serde_json::to_string(&schema).unwrap_or_default()
    }

    fn export_import_target(&self, grafana_target_json: &str) -> Result<String, String> {
        let val: Value = serde_json::from_str(grafana_target_json).map_err(|e| e.to_string())?;
        let ref_id = val
            .get("refId")
            .and_then(Value::as_str)
            .unwrap_or("A")
            .to_owned();
        let hide = val.get("hide").and_then(Value::as_bool).unwrap_or(false);
        let outcome = ImportOutcome {
            query: Query {
                ref_id,
                hide,
                model: val,
            },
            warnings: Vec::new(),
        };
        serde_json::to_string(&outcome).map_err(|e| e.to_string())
    }

    fn export_export_target(&self, query_json: &str) -> String {
        let Ok(q) = serde_json::from_str::<Query>(query_json) else {
            return "{}".to_owned();
        };
        let mut map = q.model.as_object().cloned().unwrap_or_default();
        map.insert("refId".to_owned(), Value::String(q.ref_id));
        Value::Object(map).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guard::GuardConfig;
    use crate::wasm::WasmPluginHost;
    use crate::{Datasource, MapSecretResolver, OutboundGuard};
    use controlroom_model::{InstanceId, OrgId, Subject};
    use std::sync::Arc;

    #[tokio::test]
    async fn wasm_host_enforces_pinned_digest_and_limits() {
        let host = Arc::new(WasmPluginHost::default());
        let plugin = Arc::new(SampleMetricsWasmPlugin::new());
        let pinned_digest = host.register_component(plugin);

        let org = OrgId::new();
        let inst = InstanceId::new();
        let guard = OutboundGuard::new(GuardConfig::default());
        let secrets = Arc::new(MapSecretResolver::new());

        // 1. Unknown digest is refused
        let bad = host.instantiate(
            org,
            "wasm:sample-json",
            Some("0000000000000000000000000000000000000000000000000000000000000000"),
            guard.clone(),
            secrets.clone(),
            BTreeMap::new(),
        );
        assert!(bad.is_err());

        // 2. Missing digest is refused
        let missing = host.instantiate(
            org,
            "wasm:sample-json",
            None,
            guard.clone(),
            secrets.clone(),
            BTreeMap::new(),
        );
        assert!(missing.is_err());

        // 3. Matching pinned digest loads and executes
        let adapter = host
            .instantiate(
                org,
                "wasm:sample-json",
                Some(&pinned_digest),
                guard,
                secrets,
                BTreeMap::new(),
            )
            .expect("pinned digest loads");

        let ctx = QueryCtx::new(org, inst, Subject::anonymous(), 1000, 2000);
        let q = adapter
            .import(&json!({"refId": "A", "metric": "cpu_usage", "scale": 2.5}))
            .unwrap()
            .query;
        let frames = adapter.query(&ctx, &[q]).await.expect("query succeeds");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].frames[0].fields[1].f64_at(0), Some(25.0));
    }
}
