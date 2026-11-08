#![forbid(unsafe_code)]
//! The WASM plugin host (R5, §3.3): a component loads only by its pinned digest, every call runs
//! inside the host limits (sizes, memory, fuel, wall time, plugin and organization concurrency),
//! and the guest reaches the network and secrets only through the host imports.

mod common;

use common::{ORG, Reply, ctx, loopback_guard, mock, q};
use daiya_model::{InstanceId, OrgId, Subject};
use daiya_query::guard::OutboundRequest;
use daiya_query::wasm::WasmHostImports;
use daiya_query::{
    Datasource, DsError, DsFuture, GuardConfig, HealthState, MapSecretResolver, OutboundGuard,
    Query, QueryCtx, ResourceReq, SampleMetricsWasmPlugin, WasmLimits, WasmPluginAdapter,
    WasmPluginGuest, WasmPluginHost,
};
use reqwest::Method;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

/// What a test guest answers.
#[derive(Clone)]
enum Answer {
    Text(String),
    Fail(String),
    Sleep(Duration),
}

impl Answer {
    async fn run(&self) -> Result<String, String> {
        match self {
            Self::Text(t) => Ok(t.clone()),
            Self::Fail(e) => Err(e.clone()),
            Self::Sleep(d) => {
                tokio::time::sleep(*d).await;
                Ok("[]".into())
            }
        }
    }
}

struct Guest {
    bytes: &'static [u8],
    /// After `swap` the guest reports other bytes than it was registered with.
    swap: AtomicBool,
    memory: AtomicUsize,
    fuel: AtomicU64,
    query: Answer,
    health: Answer,
    resource: Answer,
    editor: String,
    import: Result<String, String>,
    export: String,
}

impl Guest {
    fn new(bytes: &'static [u8]) -> Self {
        Self {
            bytes,
            swap: AtomicBool::new(false),
            memory: AtomicUsize::new(1 << 20),
            fuel: AtomicU64::new(1_000),
            query: Answer::Text("[]".into()),
            health: Answer::Text("fine".into()),
            resource: Answer::Text("{\"ok\":true}".into()),
            editor: "{}".into(),
            import: Err("no import".into()),
            export: "{}".into(),
        }
    }
}

impl WasmPluginGuest for Guest {
    fn plugin_id(&self) -> &'static str {
        "wasm:test"
    }
    fn component_bytes(&self) -> &[u8] {
        if self.swap.load(Ordering::SeqCst) {
            b"tampered component"
        } else {
            self.bytes
        }
    }
    fn memory_usage_bytes(&self) -> usize {
        self.memory.load(Ordering::SeqCst)
    }
    fn fuel_consumed(&self) -> u64 {
        self.fuel.load(Ordering::SeqCst)
    }
    fn export_query<'a>(
        &'a self,
        _host: WasmHostImports<'a>,
        _ctx: &'a QueryCtx,
        _queries_json: &'a str,
    ) -> DsFuture<'a, Result<String, String>> {
        Box::pin(async move { self.query.run().await })
    }
    fn export_health<'a>(
        &'a self,
        _host: WasmHostImports<'a>,
        _ctx: &'a QueryCtx,
    ) -> DsFuture<'a, (bool, String)> {
        Box::pin(async move {
            match self.health.run().await {
                Ok(m) => (true, m),
                Err(m) => (false, m),
            }
        })
    }
    fn export_resource<'a>(
        &'a self,
        _host: WasmHostImports<'a>,
        _ctx: &'a QueryCtx,
        _path: &'a str,
        _params_json: &'a str,
    ) -> DsFuture<'a, Result<String, String>> {
        Box::pin(async move { self.resource.run().await })
    }
    fn export_editor_schema(&self) -> String {
        self.editor.clone()
    }
    fn export_import_target(&self, _grafana_target_json: &str) -> Result<String, String> {
        self.import.clone()
    }
    fn export_export_target(&self, _query_json: &str) -> String {
        self.export.clone()
    }
}

fn load(host: &Arc<WasmPluginHost>, guest: Guest) -> (WasmPluginAdapter, Arc<Guest>) {
    load_in(host, guest, ORG)
}

fn load_in(
    host: &Arc<WasmPluginHost>,
    guest: Guest,
    org: OrgId,
) -> (WasmPluginAdapter, Arc<Guest>) {
    let guest = Arc::new(guest);
    let digest = host.register_component(guest.clone());
    let adapter = host
        .instantiate(
            org,
            "wasm:test",
            Some(&digest),
            loopback_guard(),
            Arc::new(MapSecretResolver::new()),
            BTreeMap::new(),
        )
        .unwrap();
    (adapter, guest)
}

fn host(limits: WasmLimits) -> Arc<WasmPluginHost> {
    Arc::new(WasmPluginHost::new(limits))
}

fn res(path: &str) -> ResourceReq {
    ResourceReq {
        path: path.into(),
        params: BTreeMap::new(),
    }
}

#[test]
fn the_default_limits_are_those_of_section_3_3() {
    let l = WasmLimits::default();
    assert_eq!(l.memory_bytes, 64 << 20);
    assert_eq!(l.wall_time, Duration::from_secs(5));
    assert_eq!(l.background_wall_time, Duration::from_secs(60));
    assert_eq!(l.fuel, 200_000_000_000);
    assert_eq!(l.request_bytes, 2 << 20);
    assert_eq!(l.response_bytes, 8 << 20);
    assert_eq!(l.per_plugin_concurrency, 16);
    assert_eq!(l.per_org_concurrency, 64);
    assert_eq!(WasmPluginHost::default().limits, l);
}

#[test]
fn a_component_loads_only_by_its_pinned_digest() {
    let h = host(WasmLimits::default());
    let guest = Arc::new(Guest::new(b"component-a"));
    let digest = h.register_component(guest.clone());
    assert_eq!(digest.len(), 64);
    let try_load = |pinned: Option<&str>| {
        h.instantiate(
            ORG,
            "wasm:test",
            pinned,
            loopback_guard(),
            Arc::new(MapSecretResolver::new()),
            BTreeMap::new(),
        )
        .map(|_| ())
    };
    for (pinned, needle) in [
        (None, "missing pinned wasm_digest"),
        (Some("  "), "missing pinned wasm_digest"),
        (Some(&"0".repeat(64)[..]), "unknown WASM component digest"),
        (Some(&digest[..63]), "unknown WASM component digest"),
    ] {
        let err = try_load(pinned).unwrap_err();
        assert!(
            matches!(&err, DsError::Wasm(m) if m.contains(needle)),
            "{pinned:?}: {err}"
        );
    }
    let padded = format!(" {digest} ");
    assert!(try_load(Some(&padded)).is_ok());

    // Bytes that no longer hash to the pinned digest are refused.
    guest.swap.store(true, Ordering::SeqCst);
    let err = try_load(Some(&digest)).unwrap_err();
    assert!(err.to_string().contains("digest mismatch"), "{err}");
    guest.swap.store(false, Ordering::SeqCst);

    // A component over the memory limit does not load.
    let small = host(WasmLimits {
        memory_bytes: 1024,
        ..WasmLimits::default()
    });
    let big = Arc::new(Guest::new(b"component-big"));
    let d = small.register_component(big);
    let err = small
        .instantiate(
            ORG,
            "wasm:test",
            Some(&d),
            loopback_guard(),
            Arc::new(MapSecretResolver::new()),
            BTreeMap::new(),
        )
        .map(|_| ())
        .unwrap_err();
    assert!(err.to_string().contains("linear memory"), "{err}");
}

#[tokio::test]
async fn query_answers_are_checked_against_every_limit() {
    let c = ctx(Subject::anonymous());
    let frames = json!([{ "refId": "A", "frames": [] }]).to_string();

    let h = host(WasmLimits::default());
    let mut g = Guest::new(b"q-ok");
    g.query = Answer::Text(frames.clone());
    let (a, guest) = load(&h, g);
    assert_eq!(a.kind(), "wasm:test");
    let sets = a.query(&c, &q(json!({ "x": 1 }))).await.unwrap();
    assert_eq!(sets[0].ref_id, "A");

    // Memory and fuel are read before each call.
    guest.memory.store(65 << 20, Ordering::SeqCst);
    let err = a.query(&c, &q(json!({}))).await.unwrap_err();
    assert!(err.to_string().contains("linear memory"), "{err}");
    guest.memory.store(1 << 20, Ordering::SeqCst);
    guest.fuel.store(200_000_000_001, Ordering::SeqCst);
    let err = a.query(&c, &q(json!({}))).await.unwrap_err();
    assert!(err.to_string().contains("fuel"), "{err}");
    guest.fuel.store(0, Ordering::SeqCst);

    // Request size.
    let tight = host(WasmLimits {
        request_bytes: 64,
        ..WasmLimits::default()
    });
    let (a, _) = load(&tight, Guest::new(b"q-req"));
    let err = a
        .query(&c, &q(json!({ "big": "x".repeat(100) })))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("request payload"), "{err}");

    // Response size, guest failure and a non-FrameSet answer.
    let tight = host(WasmLimits {
        response_bytes: 8,
        ..WasmLimits::default()
    });
    let mut g = Guest::new(b"q-resp");
    g.query = Answer::Text(frames);
    let (a, _) = load(&tight, g);
    let err = a.query(&c, &q(json!({}))).await.unwrap_err();
    assert!(err.to_string().contains("response payload"), "{err}");
    let mut g = Guest::new(b"q-fail");
    g.query = Answer::Fail("guest trapped".into());
    let (a, _) = load(&h, g);
    assert_eq!(
        a.query(&c, &q(json!({}))).await.unwrap_err(),
        DsError::Wasm("guest trapped".into())
    );
    let mut g = Guest::new(b"q-junk");
    g.query = Answer::Text("{\"not\":\"framesets\"}".into());
    let (a, _) = load(&h, g);
    let err = a.query(&c, &q(json!({}))).await.unwrap_err();
    assert!(err.to_string().contains("invalid FrameSet JSON"), "{err}");

    // Wall time: the smaller of the host limit and the query timeout.
    let fast = host(WasmLimits {
        wall_time: Duration::from_millis(50),
        ..WasmLimits::default()
    });
    let mut g = Guest::new(b"q-slow");
    g.query = Answer::Sleep(Duration::from_secs(5));
    let (a, _) = load(&fast, g);
    let err = a.query(&c, &q(json!({}))).await.unwrap_err();
    assert!(err.to_string().contains("wall-time limit of 50ms"), "{err}");
    let mut short = ctx(Subject::anonymous());
    short.timeout = Duration::from_millis(20);
    let mut g = Guest::new(b"q-slow2");
    g.query = Answer::Sleep(Duration::from_secs(5));
    let (a, _) = load(&h, g);
    let err = a.query(&short, &q(json!({}))).await.unwrap_err();
    assert!(err.to_string().contains("20ms"), "{err}");
}

#[tokio::test]
async fn concurrency_is_capped_per_plugin_and_per_organization() {
    let limits = WasmLimits {
        per_plugin_concurrency: 1,
        per_org_concurrency: 2,
        ..WasmLimits::default()
    };
    let h = host(limits);
    let slow = || {
        let mut g = Guest::new(b"c-slow");
        g.query = Answer::Sleep(Duration::from_millis(300));
        g.health = Answer::Sleep(Duration::from_millis(300));
        g.resource = Answer::Sleep(Duration::from_millis(300));
        g
    };
    let (a, _) = load(&h, slow());
    let c = ctx(Subject::anonymous());
    let queries = q(json!({}));
    let (first, second, health, resource) = tokio::join!(
        a.query(&c, &queries),
        async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            a.query(&c, &queries).await
        },
        async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            a.health(&c).await
        },
        async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            a.resource(&c, res("x")).await
        }
    );
    assert!(first.is_ok());
    let err = second.unwrap_err();
    assert!(
        err.to_string().contains("per-plugin concurrency limit (1)"),
        "{err}"
    );
    assert_eq!(health.status, HealthState::Error);
    assert!(health.message.contains("per-plugin"), "{}", health.message);
    assert!(resource.unwrap_err().to_string().contains("per-plugin"));
    // Free again once the first call ended.
    assert!(a.query(&c, &queries).await.is_ok());

    // Two plugins of one organization share its slots; another organization has its own.
    let mut g2 = slow();
    g2.bytes = b"c-slow-2";
    let (b, _) = load(&h, g2);
    let mut g3 = slow();
    g3.bytes = b"c-slow-3";
    let (other_plugin, _) = load(&h, g3);
    let mut g4 = slow();
    g4.bytes = b"c-slow-4";
    let (foreign, _) = load_in(&h, g4, OrgId::from_u128(7));
    let foreign_ctx = QueryCtx::new(
        OrgId::from_u128(7),
        InstanceId::from_u128(1),
        Subject::anonymous(),
        0,
        1,
    );
    let (r1, r2, r3, r4) = tokio::join!(
        a.query(&c, &queries),
        b.query(&c, &queries),
        async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            other_plugin.query(&c, &queries).await
        },
        async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            foreign.query(&foreign_ctx, &queries).await
        }
    );
    assert!(r1.is_ok() && r2.is_ok());
    let err = r3.unwrap_err();
    assert!(
        err.to_string()
            .contains("per-organization WASM concurrency limit (2)"),
        "{err}"
    );
    assert!(
        r4.is_ok(),
        "another organization is not limited by this one"
    );
}

#[tokio::test]
async fn health_resource_and_target_exports() {
    let c = ctx(Subject::anonymous());
    let h = host(WasmLimits {
        wall_time: Duration::from_millis(50),
        response_bytes: 64,
        request_bytes: 64,
        ..WasmLimits::default()
    });
    let mut g = Guest::new(b"e-1");
    g.health = Answer::Fail("upstream down".into());
    g.resource = Answer::Fail("no such path".into());
    g.editor = json!({ "kind": "wasm:test", "jsonSchema": { "type": "object" }, "fieldHints": [] })
        .to_string();
    g.import = Ok(json!({ "query": { "refId": "B", "model": {} }, "warnings": ["w"] }).to_string());
    g.export = "{\"refId\":\"B\"}".into();
    let (a, _) = load(&h, g);
    let health = a.health(&c).await;
    assert_eq!(health.status, HealthState::Error);
    assert_eq!(health.message, "upstream down");
    assert_eq!(
        a.resource(&c, res("x")).await.unwrap_err(),
        DsError::Wasm("no such path".into())
    );
    let err = a
        .resource(
            &c,
            ResourceReq {
                path: "p".repeat(70),
                params: BTreeMap::new(),
            },
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("request payload"), "{err}");
    assert_eq!(a.editor().kind, "wasm:test");
    let imported = a.import(&json!({ "refId": "B" })).unwrap();
    assert_eq!(imported.warnings, vec!["w".to_owned()]);
    let err = a.import(&json!({ "big": "x".repeat(100) })).unwrap_err();
    assert!(err.to_string().contains("request payload"), "{err}");
    assert_eq!(
        a.export(&Query {
            ref_id: "B".into(),
            hide: false,
            model: json!({})
        }),
        json!({ "refId": "B" })
    );

    let mut g = Guest::new(b"e-2");
    g.health = Answer::Sleep(Duration::from_secs(5));
    g.resource = Answer::Sleep(Duration::from_secs(5));
    g.editor = "not json".into();
    g.import = Ok("not json".into());
    g.export = "not json".into();
    let (a, _) = load(&h, g);
    let health = a.health(&c).await;
    assert_eq!(health.message, "WASM health check timed out");
    assert!(
        a.resource(&c, res("x"))
            .await
            .unwrap_err()
            .to_string()
            .contains("timed out")
    );
    // A guest schema that does not parse falls back to a bare object form of the plugin kind.
    let editor = a.editor();
    assert_eq!(editor.kind, "wasm:test");
    assert_eq!(editor.json_schema, json!({ "type": "object" }));
    assert!(a.import(&json!({})).is_err());
    assert_eq!(
        a.export(&Query {
            ref_id: "A".into(),
            hide: false,
            model: json!({})
        }),
        Value::Null
    );

    let mut g = Guest::new(b"e-3");
    g.resource = Answer::Text("x".repeat(100));
    g.import = Ok("y".repeat(100));
    let (a, _) = load(&h, g);
    assert!(
        a.resource(&c, res("x"))
            .await
            .unwrap_err()
            .to_string()
            .contains("too large")
    );
    assert!(
        a.import(&json!({}))
            .unwrap_err()
            .to_string()
            .contains("too large")
    );
    let mut g = Guest::new(b"e-4");
    g.resource = Answer::Text("{oops".into());
    g.import = Err("guest refused".into());
    let (a, _) = load(&h, g);
    assert!(a.resource(&c, res("x")).await.is_err());
    assert_eq!(
        a.import(&json!({})).unwrap_err(),
        DsError::Wasm("guest refused".into())
    );
    let mut g = Guest::new(b"e-5");
    g.health = Answer::Text("all good".into());
    let (a, _) = load(&h, g);
    let health = a.health(&c).await;
    assert_eq!(
        (health.status, health.message.as_str()),
        (HealthState::Ok, "all good")
    );
    assert_eq!(
        a.resource(&c, res("x")).await.unwrap().data,
        json!({ "ok": true })
    );
}

#[tokio::test]
async fn host_imports_go_through_the_guard_and_the_declared_secret_refs() {
    let m = mock(vec![("/data", Reply::json(200, &json!({ "v": 1 })))]).await;
    let guard = loopback_guard();
    let mut store = MapSecretResolver::new();
    store.insert(ORG, "plugin-key", "k-123");
    store.insert(ORG, "other-key", "nope");
    let declared: BTreeMap<String, String> =
        [("apiKey".to_owned(), "plugin-key".to_owned())].into();
    let imports = WasmHostImports {
        org_id: ORG,
        guard: &guard,
        secrets: &store,
        allowed_secret_refs: &declared,
    };
    let resp = imports
        .http_out(OutboundRequest {
            method: Method::GET,
            url: format!("{}/data", m.base),
            headers: vec![],
            body: None,
            timeout: None,
        })
        .await
        .unwrap();
    assert_eq!(resp.status, 200);
    // The guard still decides: a host outside its allow-list is refused.
    let err = imports
        .http_out(OutboundRequest {
            method: Method::GET,
            url: "https://evil.example/x".into(),
            headers: vec![],
            body: None,
            timeout: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(err, DsError::GuardRefused(_)), "{err}");

    assert_eq!(imports.resolve_secret("apiKey").unwrap().as_str(), "k-123");
    assert_eq!(
        imports.resolve_secret("plugin-key").unwrap().as_str(),
        "k-123"
    );
    let err = imports.resolve_secret("other-key").unwrap_err();
    assert!(err.to_string().contains("other-key"), "{err}");
    assert!(!err.to_string().contains("nope"));
}

/// A guest that reports no memory or fuel of its own: the host assumes the defaults.
struct Plain;

impl WasmPluginGuest for Plain {
    fn plugin_id(&self) -> &'static str {
        "wasm:plain"
    }
    fn component_bytes(&self) -> &[u8] {
        b"plain component"
    }
    fn export_query<'a>(
        &'a self,
        _host: WasmHostImports<'a>,
        _ctx: &'a QueryCtx,
        _queries_json: &'a str,
    ) -> DsFuture<'a, Result<String, String>> {
        Box::pin(async move { Ok("[]".to_owned()) })
    }
    fn export_health<'a>(
        &'a self,
        _host: WasmHostImports<'a>,
        _ctx: &'a QueryCtx,
    ) -> DsFuture<'a, (bool, String)> {
        Box::pin(async move { (true, String::new()) })
    }
    fn export_resource<'a>(
        &'a self,
        _host: WasmHostImports<'a>,
        _ctx: &'a QueryCtx,
        _path: &'a str,
        _params_json: &'a str,
    ) -> DsFuture<'a, Result<String, String>> {
        Box::pin(async move { Ok("null".to_owned()) })
    }
    fn export_editor_schema(&self) -> String {
        String::new()
    }
    fn export_import_target(&self, _grafana_target_json: &str) -> Result<String, String> {
        Err(String::new())
    }
    fn export_export_target(&self, _query_json: &str) -> String {
        String::new()
    }
}

#[tokio::test]
async fn default_memory_and_fuel_fit_the_default_limits_but_not_tight_ones() {
    let ok = Arc::new(WasmPluginHost::default());
    let digest = ok.register_component(Arc::new(Plain));
    let load = |h: &Arc<WasmPluginHost>| {
        h.instantiate(
            ORG,
            "wasm:plain",
            Some(&digest),
            loopback_guard(),
            Arc::new(MapSecretResolver::new()),
            BTreeMap::new(),
        )
    };
    let a = load(&ok).unwrap();
    assert!(
        a.query(&ctx(Subject::anonymous()), &q(json!({})))
            .await
            .unwrap()
            .is_empty()
    );
    // Defaults: 1 MiB of memory, 1 000 000 units of fuel.
    let tight = Arc::new(WasmPluginHost::new(WasmLimits {
        fuel: 999_999,
        ..WasmLimits::default()
    }));
    tight.register_component(Arc::new(Plain));
    let a = load(&tight).unwrap();
    let err = a
        .query(&ctx(Subject::anonymous()), &q(json!({})))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("fuel 1000000"), "{err}");
    let small = Arc::new(WasmPluginHost::new(WasmLimits {
        memory_bytes: (1 << 20) - 1,
        ..WasmLimits::default()
    }));
    small.register_component(Arc::new(Plain));
    assert!(load(&small).is_err());
}

#[tokio::test]
async fn the_sample_plugin_runs_inside_the_host() {
    let h = Arc::new(WasmPluginHost::default());
    let plugin = SampleMetricsWasmPlugin::new();
    assert_eq!(plugin.plugin_id(), "wasm:sample-json");
    assert_eq!(
        plugin.component_bytes(),
        SampleMetricsWasmPlugin::COMPONENT_MAGIC
    );
    let digest = h.register_component(Arc::new(plugin));
    let guard = OutboundGuard::new(GuardConfig::default());
    let mut store = MapSecretResolver::new();
    store.insert(ORG, "sample-key", "s");
    let declared: BTreeMap<String, String> =
        [("apiKey".to_owned(), "sample-key".to_owned())].into();
    let adapter = h
        .instantiate(
            ORG,
            "wasm:sample-json",
            Some(&digest),
            guard.clone(),
            Arc::new(store),
            declared.clone(),
        )
        .unwrap();
    let c = ctx(Subject::anonymous());
    let sets = adapter
        .query(&c, &q(json!({ "metric": "cpu_usage", "scale": 0.5 })))
        .await
        .unwrap();
    let frame = &sets[0].frames[0];
    assert_eq!(frame.name, "cpu_usage");
    assert_eq!(
        frame.fields[0].values,
        vec![json!(c.from_ms), json!(c.to_ms)]
    );
    assert_eq!(frame.fields[1].f64_at(1), Some(10.0));
    let sets = adapter.query(&c, &q(json!({}))).await.unwrap();
    assert_eq!(sets[0].frames[0].name, "sample_metric");
    assert_eq!(adapter.health(&c).await.status, HealthState::Ok);
    assert_eq!(
        adapter.resource(&c, res("metrics")).await.unwrap().data,
        json!(["cpu_usage", "memory_usage", "request_rate"])
    );
    assert_eq!(
        adapter.resource(&c, res("other")).await.unwrap().data,
        json!([])
    );
    let editor = adapter.editor();
    assert_eq!(editor.kind, "wasm:sample-json");
    assert_eq!(
        editor.field_hints[0].resource_path.as_deref(),
        Some("metrics")
    );
    let imported = adapter
        .import(&json!({ "metric": "m", "hide": true }))
        .unwrap();
    assert_eq!(imported.query.ref_id, "A");
    assert!(imported.query.hide);
    assert_eq!(
        adapter.export(&imported.query),
        json!({ "metric": "m", "hide": true, "refId": "A" })
    );

    // A declared secret the organization does not hold fails the query, without a value.
    let missing = h
        .instantiate(
            ORG,
            "wasm:sample-json",
            Some(&digest),
            guard,
            Arc::new(MapSecretResolver::new()),
            declared,
        )
        .unwrap();
    let err = missing.query(&c, &q(json!({}))).await.unwrap_err();
    assert!(
        matches!(&err, DsError::Wasm(m) if m.contains("sample-key")),
        "{err}"
    );

    // The guest's own exports refuse what is not JSON.
    let guest = SampleMetricsWasmPlugin::default();
    assert!(guest.export_import_target("{").is_err());
    let empty_refs = BTreeMap::new();
    let imports = WasmHostImports {
        org_id: ORG,
        guard: &loopback_guard(),
        secrets: &MapSecretResolver::new(),
        allowed_secret_refs: &empty_refs,
    };
    assert!(guest.export_query(imports, &c, "not json").await.is_err());
    assert_eq!(guest.export_export_target("{"), "{}");
}
