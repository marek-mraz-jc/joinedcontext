// SPDX-License-Identifier: EUPL-1.2
//! HTTP-level tests of the router: health, operational surface, batch and update clauses.

use super::*;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

/// T-2890: a measured size is reused for the TTL; a failure, and a
/// measurement in flight, answer no size for the retry interval; the
/// first miss claims the measurement and a concurrent one does not.
#[test]
fn a_tenant_size_is_measured_once_at_a_time_and_a_failure_is_remembered() {
    use std::time::Duration;
    let mut map = std::collections::HashMap::new();
    let t0 = std::time::Instant::now();
    assert_eq!(
        bytes_cached_or_claim(&mut map, "a", t0),
        None,
        "first miss claims"
    );
    assert_eq!(
        bytes_cached_or_claim(&mut map, "a", t0),
        Some(None),
        "in flight: no second scan"
    );
    assert_eq!(
        bytes_cached_or_claim(&mut map, "b", t0),
        None,
        "another tenant is its own claim"
    );
    // a failure (or a claim that never finished) is retried after the interval
    let later = t0 + TENANT_BYTES_RETRY;
    assert_eq!(
        bytes_cached_or_claim(&mut map, "a", later - Duration::from_millis(1)),
        Some(None)
    );
    assert_eq!(
        bytes_cached_or_claim(&mut map, "a", later),
        None,
        "retried after the interval"
    );
    // a measured size outlives the retry interval up to the TTL
    let v = serde_json::json!({"entities": 1});
    map.insert("a".into(), (t0, Some(v.clone())));
    assert_eq!(bytes_cached_or_claim(&mut map, "a", later), Some(Some(v)));
    assert_eq!(
        bytes_cached_or_claim(&mut map, "a", t0 + TENANT_BYTES_TTL),
        None
    );
}

fn app() -> Router {
    router(AppState::new("antares-test".into()))
}

async fn body_json(resp: Response) -> serde_json::Value {
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    serde_json::from_slice(&bytes).expect("json body")
}

/// The metrics method label is bounded: known HTTP methods pass
/// through, client-minted extension methods collapse to "OTHER" so
/// label cardinality cannot grow without bound.
#[test]
fn metric_method_label_is_bounded() {
    assert_eq!(metric_method("GET"), "GET");
    assert_eq!(metric_method("DELETE"), "DELETE");
    assert_eq!(metric_method("BAZQUX"), "OTHER");
    // methods are case-sensitive tokens (RFC 9110 9.1)
    assert_eq!(metric_method("get"), "OTHER");
}

/// TS 104 176 6.3.2: a verb the broker does not recognise or implement
/// is 501; a recognised verb a resource does not serve stays 405 with
/// its Allow set, and OPTIONS stays 204.
#[tokio::test]
async fn an_unrecognised_verb_is_501_and_an_unserved_one_405() {
    let send = |method: &'static str, uri: &'static str| {
        app().oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .expect("req"),
        )
    };
    for (method, uri) in [
        ("FOO", "/ngsi-ld/v1/entities"),
        ("FOO", "/ngsi-ld/v1/entities/urn:ngsi-ld:X:1"),
        ("TRACE", "/ngsi-ld/v1/entities"),
        ("CONNECT", "/ngsi-ld/v1/entities"),
        ("get", "/ngsi-ld/v1/entities"),
    ] {
        let resp = send(method, uri).await.expect("resp");
        assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED, "{method} {uri}");
    }
    let resp = send("GET", "/ngsi-ld/v1/entityOperations/create")
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert!(resp.headers().get("allow").is_some(), "405 keeps Allow");
    let resp = send("OPTIONS", "/ngsi-ld/v1/entities").await.expect("resp");
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn health_is_up() {
    let resp = app()
        .oneshot(Request::get("/q/health").body(Body::empty()).expect("req"))
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["status"], "UP");
    // Version surface. The commit must be the one this binary was BUILT
    // from, so it is compared against the checkout rather than merely
    // checked for being a non-empty string: a build script that stops
    // rerunning bakes in a hash from an older commit, and /q/health then
    // names a build that is not the one running. Outside a git checkout
    // the build script reports "unknown" and there is nothing to compare.
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
    let head = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok());
    match head {
        Some(h) => assert_eq!(body["commit"], h.trim(), "stale build hash"),
        None => assert_eq!(body["commit"], "unknown"),
    }
}

/// /q/health names the temporal backend next to the store: the store's
/// own mode when one instance serves both seams, `none` for NoTemporal.
/// The deliverable endpoint schemes are the registered bindings and
/// nothing else, so a client can tell what a subscription may name.
#[tokio::test(flavor = "multi_thread")]
async fn health_lists_the_registered_notification_schemes() {
    let st = AppState::new("h".into());
    let (code, body) = health(axum::extract::State(st), axum::http::HeaderMap::new()).await;
    assert_eq!(code, StatusCode::OK);
    let schemes = body.0["notificationSchemes"]
        .as_array()
        .expect("notificationSchemes")
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect::<Vec<_>>();
    assert!(
        schemes.contains(&"http") && schemes.contains(&"https"),
        "{schemes:?}"
    );
    #[cfg(feature = "mqtt")]
    assert!(
        schemes.contains(&"mqtt") && schemes.contains(&"mqtts"),
        "{schemes:?}"
    );
    assert!(
        !schemes.contains(&"ws"),
        "only registered bindings: {schemes:?}"
    );
}

#[tokio::test]
async fn health_names_the_temporal_backend() {
    let body = body_json(
        app()
            .oneshot(Request::get("/q/health").body(Body::empty()).expect("req"))
            .await
            .expect("resp"),
    )
    .await;
    // whichever built-in store the harness composed, both seams name it
    let store = body["store"].as_str().expect("store name").to_owned();
    assert!(store == "memory" || store == "file", "store: {store}");
    assert_eq!(body["temporal"], store);

    let st = AppState::with_drivers(
        "antares-test".into(),
        std::sync::Arc::new(antares_sql::store::any::AnyStore::Mem(
            antares_sql::store::Store::default(),
        )),
        std::sync::Arc::new(antares_store::NoTemporal),
        "memory",
    );
    let body = body_json(
        router(st)
            .oneshot(Request::get("/q/health").body(Body::empty()).expect("req"))
            .await
            .expect("resp"),
    )
    .await;
    assert_eq!(body["store"], "memory");
    assert_eq!(body["temporal"], "none");
}

/// /q/health names the policy engine and the deadline one decision
/// gets, so an operator can tell an allow-all broker from one an addon
/// engine was registered into without reading the process environment.
#[tokio::test]
async fn health_names_the_policy_engine_and_its_timeout() {
    use policy::PolicyEngine as _;
    let body = body_json(
        app()
            .oneshot(Request::get("/q/health").body(Body::empty()).expect("req"))
            .await
            .expect("resp"),
    )
    .await;
    assert_eq!(body["policy"]["engine"], policy::AllowAll.name());
    assert_eq!(
        body["policy"]["timeoutMs"],
        u64::try_from(policy::TIMEOUT.as_millis()).unwrap_or(u64::MAX)
    );

    struct Named;
    impl policy::PolicyEngine for Named {
        fn name(&self) -> &str {
            "named-engine"
        }
        fn decide<'a>(
            &'a self,
            _s: &'a policy::Subject,
            _o: &'a policy::Operation<'a>,
        ) -> policy::DecisionFuture<'a> {
            Box::pin(async { policy::Decision::Allow })
        }
        fn pre_notify(
            &self,
            _s: &policy::Subject,
            _sub: &serde_json::Value,
            _n: &mut serde_json::Value,
        ) -> policy::NotifyDecision {
            policy::NotifyDecision::Deliver
        }
    }
    let st = AppState::new("h".into()).with_policy(std::sync::Arc::new(Named));
    let (_, body) = health(axum::extract::State(st), axum::http::HeaderMap::new()).await;
    assert_eq!(body.0["policy"]["engine"], "named-engine");
}

/// /q/health reports the bus — `bus: {mode,
/// connected, reconnects}` when the nats wiring installed bus_stats,
/// and the field is ABSENT for bus=local (no stats installed).
#[tokio::test]
async fn health_reports_the_bus_only_when_nats_is_wired() {
    // bus=local: no bus_stats ⇒ no `bus` member at all
    let resp = app()
        .oneshot(Request::get("/q/health").body(Body::empty()).expect("req"))
        .await
        .expect("resp");
    let body = body_json(resp).await;
    assert!(
        body.get("bus").is_none(),
        "bus member must be ABSENT for bus=local: {body}"
    );

    // nats wiring installs the closure ⇒ the live state is reported
    let mut st = AppState::new("antares-test".into());
    st.bus_stats = Some(std::sync::Arc::new(
        || serde_json::json!({"mode": "nats", "connected": true, "reconnects": 2}),
    ));
    let resp = router(st)
        .oneshot(Request::get("/q/health").body(Body::empty()).expect("req"))
        .await
        .expect("resp");
    let body = body_json(resp).await;
    assert_eq!(body["bus"]["mode"], "nats");
    assert_eq!(body["bus"]["connected"], true);
    assert_eq!(body["bus"]["reconnects"], 2);
}

/// /q/ready is READINESS — 200 on a healthy store, 503 the
/// moment the bus reports disconnected (a pod that cannot process must
/// stop receiving traffic while staying alive for liveness).
#[tokio::test]
async fn ready_gates_on_store_and_bus() {
    let resp = app()
        .oneshot(Request::get("/q/ready").body(Body::empty()).expect("req"))
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_json(resp).await["status"], "READY");

    let mut st = AppState::new("antares-test".into());
    st.bus_stats = Some(std::sync::Arc::new(
        || serde_json::json!({"mode": "nats", "connected": false, "reconnects": 3}),
    ));
    let resp = router(st)
        .oneshot(Request::get("/q/ready").body(Body::empty()).expect("req"))
        .await
        .expect("resp");
    assert_eq!(
        resp.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "a disconnected bus must flip readiness"
    );
    assert_eq!(body_json(resp).await["status"], "NOT_READY");
}

/// The operations of `docs/openapi/antares-admin.yaml`, read with a
/// line scan: an OpenAPI path is a two-space-indented key under
/// `paths:`, a method a four-space-indented HTTP verb under it. The
/// document is ours and hand-written, so its layout is fixed.
fn documented_operations() -> Vec<(String, Vec<&'static str>)> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/openapi/antares-admin.yaml"
    );
    let text = std::fs::read_to_string(path).expect("antares-admin.yaml beside the crates");
    let mut ops: Vec<(String, Vec<&'static str>)> = Vec::new();
    let mut in_paths = false;
    for line in text.lines() {
        if line == "paths:" {
            in_paths = true;
        } else if !line.starts_with(' ') && !line.is_empty() {
            in_paths = false;
        }
        if !in_paths {
            continue;
        }
        let key = line.trim_start();
        let indent = line.len() - key.len();
        let Some(key) = key.strip_suffix(':') else {
            continue;
        };
        if indent == 2 && key.starts_with('/') {
            ops.push((key.to_owned(), Vec::new()));
        } else if indent == 4 {
            if let Some(last) = ops.last_mut() {
                if let Some(m) = ["get", "post", "put", "patch", "delete"]
                    .into_iter()
                    .find(|m| *m == key)
                {
                    last.1.push(m);
                }
            }
        }
    }
    ops
}

/// The operational OpenAPI document and the router describe the same
/// surface. Paths: the document lists exactly the admin surface plus
/// the 5.8.1.4 peer wire. Methods, proven from outside on the live
/// router: a documented method is never answered 405, and a method
/// the document omits always is. A route added on one side alone
/// fails here.
#[tokio::test]
async fn operational_openapi_matches_the_router() {
    let ops = documented_operations();
    let mut documented: Vec<&str> = ops.iter().map(|(p, _)| p.as_str()).collect();
    documented.sort_unstable();
    let mut mounted: Vec<String> = Admin::PATHS
        .iter()
        .map(|p| format!("{}{p}", Admin.prefix()))
        .collect();
    mounted.push("/ex/v1/remote-notify".into());
    mounted.sort_unstable();
    assert_eq!(
        documented, mounted,
        "paths in antares-admin.yaml vs the router"
    );

    let app = app();
    for (path, methods) in &ops {
        assert!(!methods.is_empty(), "{path} documents no method");
        let concrete = path.replace("{tenant}", "default").replace("{id}", "x");
        for m in ["get", "post", "put", "patch", "delete"] {
            let resp = app
                .clone()
                .oneshot(
                    // an explicit zero length: the body layer answers
                    // 411 to a length-less write before routing
                    Request::builder()
                        .method(m.to_ascii_uppercase().as_str())
                        .uri(&concrete)
                        .header(axum::http::header::CONTENT_LENGTH, "0")
                        .body(Body::empty())
                        .expect("req"),
                )
                .await
                .expect("resp");
            let refused = resp.status() == StatusCode::METHOD_NOT_ALLOWED;
            assert_eq!(
                refused,
                !methods.contains(&m),
                "{m} {path}: documented={} answered {}",
                methods.contains(&m),
                resp.status()
            );
        }
    }
}

/// A pod without the api role serves ops endpoints ONLY —
/// the NGSI-LD surface must NOT be reachable on it.
#[tokio::test]
async fn ops_router_serves_no_ngsi_ld_surface() {
    let st = AppState::new("antares-test".into());
    let app = ops_router(st);
    let resp = app
        .clone()
        .oneshot(Request::get("/q/health").body(Body::empty()).expect("req"))
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::OK);
    for path in ["/ngsi-ld/v1/entities", "/ngsi-ld/v1/subscriptions"] {
        let resp = app
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).expect("req"))
            .await
            .expect("resp");
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "{path} must not exist on a worker pod"
        );
    }
}

/// 5.6.2.4: "no existing Entity whose id (URI), and where specified
/// type, is equivalent … an error of type ResourceNotFound shall be
/// raised" — the optional ?type selector (4.17) narrows the target.
#[tokio::test]
async fn clause_5_6_2_type_selector_gates_the_update() {
    let app = app();
    let entity = serde_json::json!({"id": "urn:ngsi-ld:B:sel", "type": "Building",
        "name": {"type": "Property", "value": "x"}});
    let body = entity.to_string();
    let resp = app
        .clone()
        .oneshot(
            Request::post("/ngsi-ld/v1/entities")
                .header("Content-Type", "application/json")
                .header("Content-Length", body.len())
                .body(Body::from(body))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::CREATED);
    let patch = |ty: &str| {
        let frag = serde_json::json!({"name": {"type": "Property", "value": "y"}}).to_string();
        Request::patch(format!(
            "/ngsi-ld/v1/entities/urn:ngsi-ld:B:sel/attrs?type={ty}"
        ))
        .header("Content-Type", "application/json")
        .header("Content-Length", frag.len())
        .body(Body::from(frag))
        .expect("req")
    };
    // wrong type: the target is "not known" under this selector → 404
    let resp = app.clone().oneshot(patch("Vehicle")).await.expect("resp");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    // matching type: update proceeds
    let resp = app.clone().oneshot(patch("Building")).await.expect("resp");
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    // and the wrong-type attempt must NOT have written anything
    let resp = app
        .oneshot(
            Request::get("/ngsi-ld/v1/entities/urn:ngsi-ld:B:sel")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    let doc = body_json(resp).await;
    assert_eq!(doc["name"]["value"], "y");
}

/// 5.6.3.4: append honours the ?type selector the same way — a
/// mismatching selector means the entity is not known (404); overwrite
/// vs noOverwrite semantics are 5.5.8's merge_instance_sets.
#[tokio::test]
async fn clause_5_6_3_type_selector_gates_the_append() {
    let app = app();
    let entity = serde_json::json!({"id": "urn:ngsi-ld:B:app", "type": "Building"});
    let body = entity.to_string();
    let resp = app
        .clone()
        .oneshot(
            Request::post("/ngsi-ld/v1/entities")
                .header("Content-Type", "application/json")
                .header("Content-Length", body.len())
                .body(Body::from(body))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::CREATED);
    let append = |ty: &str| {
        let frag = serde_json::json!({"name": {"type": "Property", "value": "z"}}).to_string();
        Request::post(format!(
            "/ngsi-ld/v1/entities/urn:ngsi-ld:B:app/attrs?type={ty}"
        ))
        .header("Content-Type", "application/json")
        .header("Content-Length", frag.len())
        .body(Body::from(frag))
        .expect("req")
    };
    let resp = app.clone().oneshot(append("Vehicle")).await.expect("resp");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND, "wrong type selector");
    let resp = app.clone().oneshot(append("Building")).await.expect("resp");
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = app
        .oneshot(
            Request::get("/ngsi-ld/v1/entities/urn:ngsi-ld:B:app")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    let doc = body_json(resp).await;
    assert_eq!(doc["name"]["value"], "z");
}

/// 5.6.4.4: "If the target Attribute is scope, then an error of type
/// BadRequestData shall be raised"; the ?type selector narrows the
/// target entity (404 on mismatch).
#[tokio::test]
async fn clause_5_6_4_scope_target_and_type_selector() {
    let app = app();
    let entity = serde_json::json!({"id": "urn:ngsi-ld:B:pu", "type": "Building",
        "scope": "/Madrid", "name": {"type": "Property", "value": "x"}});
    let body = entity.to_string();
    let resp = app
        .clone()
        .oneshot(
            Request::post("/ngsi-ld/v1/entities")
                .header("Content-Type", "application/json")
                .header("Content-Length", body.len())
                .body(Body::from(body))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::CREATED);
    let patch = |attr: &str, q: &str| {
        let frag = serde_json::json!({"value": "y"}).to_string();
        Request::patch(format!(
            "/ngsi-ld/v1/entities/urn:ngsi-ld:B:pu/attrs/{attr}{q}"
        ))
        .header("Content-Type", "application/json")
        .header("Content-Length", frag.len())
        .body(Body::from(frag))
        .expect("req")
    };
    // the scope pseudo-attribute is not partially updatable
    let resp = app.clone().oneshot(patch("scope", "")).await.expect("resp");
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "scope target is 400"
    );
    // type selector gates the partial update
    let resp = app
        .clone()
        .oneshot(patch("name", "?type=Vehicle"))
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let resp = app
        .clone()
        .oneshot(patch("name", "?type=Building"))
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = app
        .oneshot(
            Request::get("/ngsi-ld/v1/entities/urn:ngsi-ld:B:pu")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    let doc = body_json(resp).await;
    assert_eq!(doc["name"]["value"], "y");
    assert_eq!(doc["scope"], "/Madrid", "scope untouched");
}

/// 5.6.5.4: the ?type selector narrows the delete target — mismatch
/// means the entity is not known (404) and nothing is deleted.
#[tokio::test]
async fn clause_5_6_5_type_selector_gates_the_delete() {
    let app = app();
    let entity = serde_json::json!({"id": "urn:ngsi-ld:B:del", "type": "Building",
        "name": {"type": "Property", "value": "x"}});
    let body = entity.to_string();
    let resp = app
        .clone()
        .oneshot(
            Request::post("/ngsi-ld/v1/entities")
                .header("Content-Type", "application/json")
                .header("Content-Length", body.len())
                .body(Body::from(body))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::CREATED);
    let del = |q: &str| {
        Request::delete(format!(
            "/ngsi-ld/v1/entities/urn:ngsi-ld:B:del/attrs/name{q}"
        ))
        .body(Body::empty())
        .expect("req")
    };
    let resp = app
        .clone()
        .oneshot(del("?type=Vehicle"))
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    // the attribute must still exist after the gated attempt
    let resp = app
        .clone()
        .oneshot(
            Request::get("/ngsi-ld/v1/entities/urn:ngsi-ld:B:del")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(body_json(resp).await["name"]["value"], "x");
    let resp = app
        .clone()
        .oneshot(del("?type=Building"))
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = app
        .oneshot(
            Request::get("/ngsi-ld/v1/entities/urn:ngsi-ld:B:del")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert!(
        body_json(resp).await.get("name").is_none(),
        "attribute deleted under the matching selector"
    );
}

/// 5.6.6.4: the ?type selector narrows the delete-entity target — a
/// mismatch means the entity is not known (404) and nothing is deleted.
#[tokio::test]
async fn clause_5_6_6_type_selector_gates_entity_delete() {
    let app = app();
    let entity = serde_json::json!({"id": "urn:ngsi-ld:B:edel", "type": "Building"});
    let body = entity.to_string();
    let resp = app
        .clone()
        .oneshot(
            Request::post("/ngsi-ld/v1/entities")
                .header("Content-Type", "application/json")
                .header("Content-Length", body.len())
                .body(Body::from(body))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::CREATED);
    let del = |q: &str| {
        Request::delete(format!("/ngsi-ld/v1/entities/urn:ngsi-ld:B:edel{q}"))
            .body(Body::empty())
            .expect("req")
    };
    let resp = app
        .clone()
        .oneshot(del("?type=Vehicle"))
        .await
        .expect("resp");
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "wrong selector is 404"
    );
    let resp = app
        .clone()
        .oneshot(
            Request::get("/ngsi-ld/v1/entities/urn:ngsi-ld:B:edel")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "entity survives the gated delete"
    );
    let resp = app
        .clone()
        .oneshot(del("?type=Building"))
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = app
        .oneshot(
            Request::get("/ngsi-ld/v1/entities/urn:ngsi-ld:B:edel")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// 5.6.7.4: "If the input Array is empty or contains a null value in
/// any of its items an error of type BadRequestData shall be raised" —
/// the WHOLE request fails, nothing is created.
#[tokio::test]
async fn clause_5_6_7_null_item_fails_the_whole_batch() {
    let app = app();
    let body = r#"[{"id":"urn:ngsi-ld:V:nb1","type":"Vehicle"}, null]"#;
    let resp = app
        .clone()
        .oneshot(
            Request::post("/ngsi-ld/v1/entityOperations/create")
                .header("Content-Type", "application/json")
                .header("Content-Length", body.len())
                .body(Body::from(body))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    // and the non-null sibling must NOT have been created
    let resp = app
        .oneshot(
            Request::get("/ngsi-ld/v1/entities/urn:ngsi-ld:V:nb1")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    // the empty array is 400 too
    let resp = crate::tests::app()
        .oneshot(
            Request::post("/ngsi-ld/v1/entityOperations/create")
                .header("Content-Type", "application/json")
                .header("Content-Length", 2)
                .body(Body::from("[]"))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

/// 5.6.11.4: an upsert onto an existing Temporal Evolution adds the
/// instances AND unions new Entity Type names; a deletion-null instance
/// (the 4.5.7 representation) is legal temporal input.
#[tokio::test]
async fn clause_5_6_11_temporal_upsert_type_union_and_null_instances() {
    let app = app();
    let post = |body: String| {
        Request::post("/ngsi-ld/v1/temporal/entities")
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("req")
    };
    let first = serde_json::json!({
        "id": "urn:ngsi-ld:V:tu", "type": "Vehicle",
        "speed": [{"type": "Property", "value": 1,
                   "observedAt": "2026-01-01T00:00:00Z"}]
    });
    let resp = app
        .clone()
        .oneshot(post(first.to_string()))
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::CREATED);
    // second upsert: new type name + a deleted-instance representation
    let second = serde_json::json!({
        "id": "urn:ngsi-ld:V:tu", "type": ["Vehicle", "Truck"],
        "speed": [{"type": "Property", "value": "urn:ngsi-ld:null",
                   "observedAt": "2026-01-02T00:00:00Z"}]
    });
    let resp = app
        .clone()
        .oneshot(post(second.to_string()))
        .await
        .expect("resp");
    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "deletion-null instances are legal temporal input (4.5.7/5.5.4)"
    );
    let resp = app
        .oneshot(
            Request::get("/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:V:tu?timerel=before&timeAt=2030-01-01T00:00:00Z")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::OK);
    let doc = body_json(resp).await;
    let types: Vec<&str> = doc["type"]
        .as_array()
        .map(|a| a.iter().filter_map(serde_json::Value::as_str).collect())
        .unwrap_or_else(|| vec![doc["type"].as_str().unwrap_or_default()]);
    assert!(
        types.contains(&"Vehicle") && types.contains(&"Truck"),
        "type names are unioned: {:?}",
        doc["type"]
    );
    // both instances present — the deletion representation included
    let speed = doc["speed"].as_array().expect("speed instances");
    assert_eq!(speed.len(), 2, "history keeps both instances: {doc}");
}

/// 5.6.19.4: replacing scope is BadRequestData; the ?type selector
/// narrows the target (404 on mismatch, attribute untouched).
#[tokio::test]
async fn clause_5_6_19_replace_attr_scope_and_type_selector() {
    let app = app();
    let entity = serde_json::json!({"id": "urn:ngsi-ld:B:ra", "type": "Building",
        "scope": "/Madrid", "name": {"type": "Property", "value": "x"}});
    let body = entity.to_string();
    let resp = app
        .clone()
        .oneshot(
            Request::post("/ngsi-ld/v1/entities")
                .header("Content-Type", "application/json")
                .header("Content-Length", body.len())
                .body(Body::from(body))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::CREATED);
    let put = |attr: &str, q: &str| {
        let frag = serde_json::json!({"type": "Property", "value": "y"}).to_string();
        Request::put(format!(
            "/ngsi-ld/v1/entities/urn:ngsi-ld:B:ra/attrs/{attr}{q}"
        ))
        .header("Content-Type", "application/json")
        .header("Content-Length", frag.len())
        .body(Body::from(frag))
        .expect("req")
    };
    let resp = app.clone().oneshot(put("scope", "")).await.expect("resp");
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "scope target is 400"
    );
    let resp = app
        .clone()
        .oneshot(put("name", "?type=Vehicle"))
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let resp = app
        .clone()
        .oneshot(put("name", "?type=Building"))
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = app
        .oneshot(
            Request::get("/ngsi-ld/v1/entities/urn:ngsi-ld:B:ra")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    let doc = body_json(resp).await;
    assert_eq!(doc["name"]["value"], "y");
    assert_eq!(doc["scope"], "/Madrid", "scope untouched");
}

/// 5.5.11.1: in Batch Create the FIRST occurrence creates the entity,
/// any subsequent instance of the same id is reported as an error
/// (already exists). 5.5.11.4: in Batch Delete the first occurrence
/// deletes, subsequent ones report an error (does not exist).
#[tokio::test]
async fn clause_5_5_11_duplicate_ids_in_create_and_delete() {
    let app = app();
    let batch = serde_json::json!([
        {"id": "urn:ngsi-ld:Building:c-dup", "type": "Building",
         "speed": {"type": "Property", "value": 1}},
        {"id": "urn:ngsi-ld:Building:c-dup", "type": "Building",
         "speed": {"type": "Property", "value": 2}}
    ]);
    let resp = app
        .clone()
        .oneshot(
            Request::post("/ngsi-ld/v1/entityOperations/create")
                .header("Content-Type", "application/json")
                .header("Content-Length", batch.to_string().len())
                .body(Body::from(batch.to_string()))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(
        resp.status(),
        StatusCode::MULTI_STATUS,
        "one ok + one error"
    );
    let body = body_json(resp).await;
    assert_eq!(
        body["success"],
        serde_json::json!(["urn:ngsi-ld:Building:c-dup"])
    );
    assert_eq!(body["errors"].as_array().map(Vec::len), Some(1));
    assert!(
        body["errors"][0].to_string().contains("AlreadyExists"),
        "second occurrence is an already-exists error: {body}"
    );
    // the FIRST occurrence created the entity
    let resp = app
        .clone()
        .oneshot(
            Request::get("/ngsi-ld/v1/entities/urn:ngsi-ld:Building:c-dup")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    let doc = body_json(resp).await;
    assert_eq!(doc["speed"]["value"], 1, "first occurrence wins the create");
    // batch delete with the id twice: first deletes, second errors
    let del = serde_json::json!(["urn:ngsi-ld:Building:c-dup", "urn:ngsi-ld:Building:c-dup"]);
    let resp = app
        .clone()
        .oneshot(
            Request::post("/ngsi-ld/v1/entityOperations/delete")
                .header("Content-Type", "application/json")
                .header("Content-Length", del.to_string().len())
                .body(Body::from(del.to_string()))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::MULTI_STATUS);
    let body = body_json(resp).await;
    assert_eq!(
        body["success"],
        serde_json::json!(["urn:ngsi-ld:Building:c-dup"])
    );
    assert!(
        body["errors"][0].to_string().contains("ResourceNotFound"),
        "second delete occurrence is a not-found error: {body}"
    );
    // and the entity is really gone
    let resp = app
        .oneshot(
            Request::get("/ngsi-ld/v1/entities/urn:ngsi-ld:Building:c-dup")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// 5.5.11.0: "All Entities and Attributes in the batch will get the same
/// modifiedAt timestamp, so it makes sense to distinguish them via the
/// observedAt temporal property." One batch is one instant. Reading the
/// clock per entity spreads a large create over several milliseconds, and
/// a Context Consumer that filters or pages on modifiedAt then sees the
/// batch split in two — half of it before its own cursor, half after.
#[tokio::test]
async fn clause_5_5_11_0_one_batch_create_carries_one_timestamp() {
    let app = app();
    // Enough documents that per-entity stamping cannot stay inside one
    // millisecond: the spread is what the clause forbids, not the count.
    let batch: Vec<serde_json::Value> = (0..500)
        .map(|i| {
            serde_json::json!({
                "id": format!("urn:ngsi-ld:Building:ts-{i}"),
                "type": "Building",
                "speed": {"type": "Property", "value": i},
                "name": {"type": "Property", "value": "x".repeat(64)},
                "near": {"type": "Relationship",
                         "object": "urn:ngsi-ld:Building:other"},
            })
        })
        .collect();
    let body = serde_json::Value::Array(batch).to_string();
    let resp = app
        .clone()
        .oneshot(
            Request::post("/ngsi-ld/v1/entityOperations/create")
                .header("Content-Type", "application/json")
                .header("Content-Length", body.len())
                .body(Body::from(body))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::CREATED);
    let resp = app
        .oneshot(
            Request::get("/ngsi-ld/v1/entities?type=Building&options=sysAttrs&limit=1000")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::OK);
    let list = body_json(resp).await;
    let ents = list.as_array().expect("entity array");
    assert_eq!(ents.len(), 500, "every entity was created");
    // Entity level and Attribute level: stamp_new writes the same instant
    // into both, so one batch may produce exactly one value overall.
    let mut stamps: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for e in ents {
        let o = e.as_object().expect("entity object");
        for key in ["createdAt", "modifiedAt"] {
            stamps.insert(
                o.get(key)
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_else(|| panic!("entity {key} is served under sysAttrs"))
                    .to_owned(),
            );
        }
        for (k, v) in o {
            if matches!(
                k.as_str(),
                "id" | "type" | "@context" | "createdAt" | "modifiedAt"
            ) {
                continue;
            }
            for key in ["createdAt", "modifiedAt"] {
                stamps.insert(
                    v.get(key)
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_else(|| panic!("attribute {k} carries {key}"))
                        .to_owned(),
                );
            }
        }
    }
    assert_eq!(
        stamps.len(),
        1,
        "one batch, one timestamp; got {} distinct: {:?}",
        stamps.len(),
        stamps
    );
}

/// 4.6.6: duplicate instances of one Entity in a batch array "shall come
/// in chronological order" — the broker applies them sequentially, so
/// the LAST occurrence's state wins, never the first.
#[tokio::test]
async fn batch_duplicate_instances_apply_in_array_order() {
    let app = app();
    let batch = serde_json::json!([
        {"id": "urn:ngsi-ld:Building:dup", "type": "Building",
         "speed": {"type": "Property", "value": 1},
         "old": {"type": "Property", "value": true}},
        {"id": "urn:ngsi-ld:Building:dup", "type": "Building",
         "speed": {"type": "Property", "value": 2}}
    ]);
    let resp = app
        .clone()
        .oneshot(
            Request::post("/ngsi-ld/v1/entityOperations/upsert")
                .header("Content-Type", "application/json")
                .header("Content-Length", batch.to_string().len())
                .body(Body::from(batch.to_string()))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert!(
        resp.status() == StatusCode::CREATED || resp.status() == StatusCode::NO_CONTENT,
        "upsert with duplicates succeeds: {}",
        resp.status()
    );
    let resp = app
        .clone()
        .oneshot(
            Request::get("/ngsi-ld/v1/entities/urn:ngsi-ld:Building:dup")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::OK);
    let doc = body_json(resp).await;
    assert_eq!(doc["speed"]["value"], 2, "later instance wins");
    assert_ne!(doc["speed"]["value"], 1, "first instance must not survive");
    // default upsert is REPLACE: the second instance replaced the first
    // wholesale, so the first-only attribute is gone too
    assert!(
        doc.get("old").is_none(),
        "replace semantics: first instance's attrs must not linger"
    );
}

/// 4.6.4 Supported Content: "implementations shall preserve the
/// representation of the content of the values provided by the context
/// information providers and return the original content" — the
/// script-injection characters < > " ' = ; ( ) are stored and served
/// verbatim, never HTML/unicode-escaped and never rejected.
#[tokio::test]
async fn dangerous_content_preserved_verbatim() {
    let app = app();
    let payload = "<script>alert('x')</script> \"quoted\" = ; ( )";
    let entity = serde_json::json!({
        "id": "urn:ngsi-ld:Building:content",
        "type": "Building",
        "note": {"type": "Property", "value": payload}
    });
    let resp = app
        .clone()
        .oneshot(
            Request::post("/ngsi-ld/v1/entities")
                .header("Content-Type", "application/json")
                .header("Content-Length", entity.to_string().len())
                .body(Body::from(entity.to_string()))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::CREATED, "content never rejected");
    let resp = app
        .clone()
        .oneshot(
            Request::get("/ngsi-ld/v1/entities/urn:ngsi-ld:Building:content")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::OK);
    let raw = resp.into_body().collect().await.expect("body").to_bytes();
    let text = std::str::from_utf8(&raw).expect("utf-8");
    // original content, not an escaped rendering of it
    assert!(!text.contains("&lt;"), "no HTML escaping");
    assert!(!text.contains("\\u003c"), "no unicode escaping");
    let doc: serde_json::Value = serde_json::from_str(text).expect("json");
    assert_eq!(doc["note"]["value"], payload, "value returned verbatim");
}

/// 4.6.1 Supported text encodings: UTF-8 JSON accepted and exposed;
/// a non-UTF-8 body is not valid JSON → InvalidRequest 400.
#[tokio::test]
async fn utf8_encoding_accepted_and_non_utf8_rejected() {
    let app = app();
    // multibyte UTF-8 round-trips byte-exact
    let entity = serde_json::json!({
        "id": "urn:ngsi-ld:Building:utf8",
        "type": "Building",
        "label": {"type": "Property", "value": "žltý kôň — 100 €"}
    });
    let resp = app
        .clone()
        .oneshot(
            Request::post("/ngsi-ld/v1/entities")
                .header("Content-Type", "application/json")
                .header("Content-Length", entity.to_string().len())
                .body(Body::from(entity.to_string()))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::CREATED);
    let resp = app
        .clone()
        .oneshot(
            Request::get("/ngsi-ld/v1/entities/urn:ngsi-ld:Building:utf8")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::OK);
    let raw = resp.into_body().collect().await.expect("body").to_bytes();
    let text = std::str::from_utf8(&raw).expect("response is valid UTF-8");
    assert!(!text.contains('\u{FFFD}'), "no mojibake in output");
    let doc: serde_json::Value = serde_json::from_str(text).expect("json");
    assert_eq!(doc["label"]["value"], "žltý kôň — 100 €");

    // invalid UTF-8 byte in the body → InvalidRequest, not BadRequestData
    let mut bad = b"{\"id\": \"urn:ngsi-ld:Building:b\", \"type\": \"".to_vec();
    bad.extend_from_slice(&[0xFF, 0xFE]);
    bad.extend_from_slice(b"\"}");
    let resp = app
        .clone()
        .oneshot(
            Request::post("/ngsi-ld/v1/entities")
                .header("Content-Type", "application/json")
                .header("Content-Length", bad.len())
                .body(Body::from(bad.clone()))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let err = body_json(resp).await;
    assert_eq!(
        err["type"],
        "https://uri.etsi.org/ngsi-ld/errors/InvalidRequest"
    );
    assert_ne!(
        err["type"], "https://uri.etsi.org/ngsi-ld/errors/BadRequestData",
        "syntactic (encoding) failure is InvalidRequest, not BadRequestData"
    );
}

#[tokio::test]
async fn prefer_ngsild_version_downgrades_and_203s() {
    // 6.3.6/6.3.21 + 4.3.6.8: Prefer: ngsi-ld=1.4 on a retrieve of an
    // entity holding a 1.8-era attribute type → amended payload,
    // Preference-Applied echo, 203 Non-Authoritative.
    let app = app();
    let entity = serde_json::json!({
        "id": "urn:ngsi-ld:Building:pref1",
        "type": "Building",
        "spec": {"type": "JsonProperty", "json": {"k": 1}}
    });
    let resp = app
        .clone()
        .oneshot(
            Request::post("/ngsi-ld/v1/entities")
                .header("Content-Type", "application/json")
                .header("Content-Length", (entity.to_string()).len())
                .body(Body::from(entity.to_string()))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = app
        .clone()
        .oneshot(
            Request::get("/ngsi-ld/v1/entities/urn:ngsi-ld:Building:pref1")
                .header("Prefer", "ngsi-ld=1.4")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::NON_AUTHORITATIVE_INFORMATION);
    assert_eq!(
        resp.headers()
            .get("Preference-Applied")
            .map(|v| v.to_str().unwrap()),
        Some("ngsi-ld=1.4")
    );
    let doc = body_json(resp).await;
    assert_eq!(
        doc["spec"],
        serde_json::json!({"type": "Property", "value": {"k": 1}})
    );

    // Native-version preference: applied header, payload untouched, 200.
    let resp = app
        .clone()
        .oneshot(
            Request::get("/ngsi-ld/v1/entities/urn:ngsi-ld:Building:pref1")
                .header("Prefer", "ngsi-ld=1.9")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("Preference-Applied")
            .map(|v| v.to_str().unwrap()),
        Some("ngsi-ld=1.9")
    );
}

/// 6.3.4 answers an over-long URI with a bare 414 and an over-large body
/// with 413. Both are preconditions on the request itself, so they must
/// not be spent on — or masked by — the 5.5.10 tenant lookup.
#[tokio::test]
async fn the_bounds_wall_answers_before_the_tenant_lookup() {
    let app = app();
    let long = "x".repeat(crate::bounds::MAX_URI_BYTES + 1);
    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("/ngsi-ld/v1/entities?type=T&idPattern={long}"))
                .header("NGSILD-Tenant", "ghost")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(
        resp.status(),
        StatusCode::URI_TOO_LONG,
        "the URI precondition decides, not the unknown tenant"
    );

    let big = vec![b'a'; *crate::bounds::MAX_BODY_BYTES + 1];
    let resp = app
        .oneshot(
            Request::post("/ngsi-ld/v1/entities")
                .header("Content-Type", "application/json")
                .header("NGSILD-Tenant", "ghost")
                .header("Content-Length", big.len())
                .body(Body::from(big))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn bounds_wall_rejects_spec_shaped() {
    // Every cap answers with the spec error, before any parse.
    let app = app();

    // JSON nesting > 64 → 400 BadRequestData
    let deep = format!(
        r#"{{"id":"urn:x:1","type":"T","a":{}1{}}}"#,
        "[".repeat(70),
        "]".repeat(70)
    );
    let resp = app
        .clone()
        .oneshot(
            Request::post("/ngsi-ld/v1/entities")
                .header("Content-Type", "application/json")
                .header("Content-Length", (deep).len())
                .body(Body::from(deep))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // URI too long → bare 414
    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("/ngsi-ld/v1/entities?q={}", "a".repeat(9000)))
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::URI_TOO_LONG);

    // body over 4 MiB → bare 413
    let resp = app
        .clone()
        .oneshot(
            Request::post("/ngsi-ld/v1/entities")
                .header("Content-Type", "application/json")
                .header(
                    "Content-Length",
                    ("x".repeat(*bounds::MAX_BODY_BYTES + 1)).len(),
                )
                .body(Body::from("x".repeat(*bounds::MAX_BODY_BYTES + 1)))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);

    // limit above the ceiling → 403 TooManyResults
    let resp = app
        .clone()
        .oneshot(
            Request::get("/ngsi-ld/v1/entities?type=T&limit=99999")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let doc = body_json(resp).await;
    assert!(doc["type"]
        .as_str()
        .expect("type")
        .ends_with("TooManyResults"));

    // batch above the item cap → 400
    let big: Vec<serde_json::Value> = (0..*bounds::MAX_BATCH_ITEMS + 1)
        .map(|i| serde_json::json!({"id": format!("urn:b:{i}"), "type": "T"}))
        .collect();
    let resp = app
        .clone()
        .oneshot(
            Request::post("/ngsi-ld/v1/entityOperations/create")
                .header("Content-Type", "application/json")
                .header(
                    "Content-Length",
                    (serde_json::to_vec(&big).expect("json")).len(),
                )
                .body(Body::from(serde_json::to_vec(&big).expect("json")))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // joinLevel above the cap → 400
    let resp = app
        .clone()
        .oneshot(
            Request::get("/ngsi-ld/v1/entities?type=T&join=inline&joinLevel=99")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

/// TS 104 179 5.2.3/5.2.4: a Sealed Attribute is an Attribute with an
/// `ngsildproof` sub-property (entityIdSealed, entityTypeSealed, value =
/// the W3C proof). Nested under a Property it survives create, a merge
/// that adds a second Sealed Attribute, and retrieval, untouched: a
/// relaying broker altering it breaks the proof (INT-4).
#[tokio::test]
async fn ts104179_5_2_3_nested_sealed_attribute_round_trips_through_merge() {
    let id = "urn:ngsi-ld:Car:sealed1";
    let proof = |member: &str, value: &str| {
        serde_json::json!({
            "type": "Property",
            "entityIdSealed": id,
            "entityTypeSealed": "Car",
            "value": {
                "type": "DataIntegrityProof",
                "cryptosuite": "eddsa-rdfc-2022",
                "created": "2026-10-10T17:00:00Z",
                "verificationMethod": format!("did:web:a.example#{member}"),
                "proofPurpose": "assertionMethod",
                "proofValue": value
            }
        })
    };
    let color = proof(
        "color",
        "z3FXQjecWufY46yg5abdVZsXqLhxhueuSoZgNSARiKBk9czhSePTF",
    );
    let parked = proof(
        "parkedAt",
        "z58DAdFfa9SkqZMVPxAQpic7ndSayn1PzZs6ZjWp1CktyGesjuTSw",
    );
    let send = |method: &str, uri: &str, doc: serde_json::Value| {
        let body = doc.to_string();
        Request::builder()
            .method(method)
            .uri(uri)
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("req")
    };
    let app = app();
    let created = serde_json::json!({
        "id": id, "type": "Car",
        "color": {"type": "Property", "value": "red", "ngsildproof": color}
    });
    let resp = app
        .clone()
        .oneshot(send("POST", "/ngsi-ld/v1/entities", created))
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::CREATED);
    let merge = serde_json::json!({
        "parkedAt": {"type": "Property", "value": "P1", "ngsildproof": parked}
    });
    let resp = app
        .clone()
        .oneshot(send("PATCH", &format!("/ngsi-ld/v1/entities/{id}"), merge))
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("/ngsi-ld/v1/entities/{id}?options=normalized"))
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::OK);
    let doc = body_json(resp).await;
    for (attribute, sealed) in [("color", &color), ("parkedAt", &parked)] {
        let served = &doc[attribute]["ngsildproof"];
        assert_eq!(
            served, sealed,
            "{attribute}: the Sealed Attribute changed: {doc}"
        );
        assert_eq!(
            served["value"].to_string(),
            sealed["value"].to_string(),
            "{attribute}: the proof is not byte-equal"
        );
        assert_eq!(served["entityTypeSealed"], "Car", "the original term");
    }
    assert_eq!(doc["color"]["value"], "red");
    assert_eq!(doc["parkedAt"]["value"], "P1");
}

/// TS 104 175 10.4.4.4 (062_05_02, 062_07_01, 062_07_02): the ?type
/// selector narrows the target Entity, and without ?datasetId an Attribute
/// with no default instance is not found.
#[tokio::test]
async fn retrieve_attr_type_selector_and_default_instance_10_4_4() {
    let app = app();
    let entity = serde_json::json!({
        "id": "urn:ngsi-ld:Vehicle:r1", "type": "Vehicle",
        "name": {"type": "Property", "value": "Bus"},
        "speed": [
            {"type": "Property", "value": 55, "datasetId": "urn:ngsi-ld:Property:gps"},
            {"type": "Property", "value": 54, "datasetId": "urn:ngsi-ld:Property:speedometer"}
        ]
    });
    let resp = app
        .clone()
        .oneshot(
            Request::post("/ngsi-ld/v1/entities")
                .header("Content-Type", "application/json")
                .header("Content-Length", entity.to_string().len())
                .body(Body::from(entity.to_string()))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::CREATED);
    let get = |path: &str| {
        let app = app.clone();
        let uri = format!("/ngsi-ld/v1/entities/urn:ngsi-ld:Vehicle:r1/attrs/{path}");
        async move {
            let resp = app
                .oneshot(Request::get(uri).body(Body::empty()).expect("req"))
                .await
                .expect("resp");
            let status = resp.status();
            (status, body_json(resp).await)
        }
    };
    let (status, doc) = get("name?type=Vehicle").await;
    assert_eq!(status, StatusCode::OK, "{doc}");
    assert_eq!(doc["value"], "Bus");
    let (status, doc) = get("name?type=Building").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{doc}");
    assert!(
        doc["type"]
            .as_str()
            .is_some_and(|t| t.ends_with("ResourceNotFound")),
        "{doc}"
    );
    let (status, doc) = get("speed").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "no default instance: {doc}");
    assert!(
        doc["type"]
            .as_str()
            .is_some_and(|t| t.ends_with("ResourceNotFound")),
        "{doc}"
    );
    let (status, doc) = get("speed/value").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "no default instance: {doc}");
    let (status, doc) = get("speed?datasetId=urn:ngsi-ld:Property:speedometer").await;
    assert_eq!(status, StatusCode::OK, "{doc}");
    assert_eq!(doc["value"], 54);
}

#[tokio::test]
async fn preadoptions_attr_get_value_options_head() {
    // 2.0 pre-adoptions: #14 GET .../attrs/{attrId},
    // #15 .../value, #58 HEAD, #59 OPTIONS with the route's Allow set.
    let app = app();
    let entity = serde_json::json!({
        "id": "urn:ngsi-ld:Building:h3", "type": "Building",
        "name": {"type": "Property", "value": "Hala"}
    });
    let resp = app
        .clone()
        .oneshot(
            Request::post("/ngsi-ld/v1/entities")
                .header("Content-Type", "application/json")
                .header("Content-Length", (entity.to_string()).len())
                .body(Body::from(entity.to_string()))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = app
        .clone()
        .oneshot(
            Request::get("/ngsi-ld/v1/entities/urn:ngsi-ld:Building:h3/attrs/name")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::OK);
    let doc = body_json(resp).await;
    assert_eq!(doc["type"], "Property");
    assert_eq!(doc["value"], "Hala");

    let resp = app
        .clone()
        .oneshot(
            Request::get("/ngsi-ld/v1/entities/urn:ngsi-ld:Building:h3/attrs/name/value")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_json(resp).await, serde_json::json!("Hala"));

    // absent attribute → 404 ResourceNotFound
    let resp = app
        .clone()
        .oneshot(
            Request::get("/ngsi-ld/v1/entities/urn:ngsi-ld:Building:h3/attrs/nope")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // #59: OPTIONS answers 204 + Allow computed from the route
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/ngsi-ld/v1/entities")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let allow = resp
        .headers()
        .get("allow")
        .expect("Allow")
        .to_str()
        .expect("str");
    assert!(
        allow.contains("GET") && allow.contains("POST"),
        "Allow: {allow}"
    );

    // #58: HEAD serves like GET, no body needed
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("HEAD")
                .uri("/ngsi-ld/v1/entities/urn:ngsi-ld:Building:h3")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::OK);
}

/// `information[].entities ×
/// (propertyNames + relationshipNames)` was expanded into an in-memory Vec
/// before any SQL ran, with no cardinality cap — a 4 MiB body produced on
/// the order of 10^10 objects and OOM-killed the process. Capped at the
/// validation boundary so it is a 400, not a dead pod: the request's
/// content is what is refused, and 5.9.2.4 raises BadRequestData for it.
#[tokio::test]
async fn registration_cardinality_is_capped_before_expansion() {
    let app = app();
    let entities: Vec<serde_json::Value> = (0..600)
        .map(|i| serde_json::json!({"id": format!("urn:e:{i}")}))
        .collect();
    let props: Vec<String> = (0..600).map(|i| format!("p{i}")).collect();
    let reg = serde_json::json!({
        "type": "ContextSourceRegistration",
        "endpoint": "http://peer.example/ngsi-ld/v1",
        "information": [{"entities": entities, "propertyNames": props}]
    });
    let resp = app
        .clone()
        .oneshot(
            Request::post("/ngsi-ld/v1/csourceRegistrations")
                .header("Content-Type", "application/json")
                .header("Content-Length", (reg.to_string()).len())
                .body(Body::from(reg.to_string()))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "an oversized registration body is BadRequestData, not a query \
         the client is told to narrow"
    );

    // a registration of ordinary size is untouched by the cap
    let reg = serde_json::json!({
        "type": "ContextSourceRegistration",
        "endpoint": "http://peer.example/ngsi-ld/v1",
        "information": [{"entities": [{"type": "Vehicle"}],
                         "propertyNames": ["speed", "heading"]}]
    });
    let resp = app
        .clone()
        .oneshot(
            Request::post("/ngsi-ld/v1/csourceRegistrations")
                .header("Content-Type", "application/json")
                .header("Content-Length", (reg.to_string()).len())
                .body(Body::from(reg.to_string()))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn tolerant_reader_echoes_unknown_members() {
    // Unknown members of Subscription/Registration documents are
    // stored and echoed, never rejected or stripped — a member added by a
    // future spec version flows through a broker that predates it.
    let app = app();
    let sub = serde_json::json!({
        "id": "urn:ngsi-ld:Subscription:tol1", "type": "Subscription",
        "entities": [{"type": "Building"}],
        "notification": {"endpoint": {"uri": "http://localhost:1/x"}},
        "futureMember": {"nested": [1, 2, 3]}
    });
    let resp = app
        .clone()
        .oneshot(
            Request::post("/ngsi-ld/v1/subscriptions")
                .header("Content-Type", "application/json")
                .header("Content-Length", (sub.to_string()).len())
                .body(Body::from(sub.to_string()))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::CREATED);
    let resp = app
        .clone()
        .oneshot(
            Request::get("/ngsi-ld/v1/subscriptions/urn:ngsi-ld:Subscription:tol1")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::OK);
    let doc = body_json(resp).await;
    assert_eq!(
        doc["futureMember"],
        serde_json::json!({"nested": [1, 2, 3]})
    );

    let reg = serde_json::json!({
        "id": "urn:ngsi-ld:ContextSourceRegistration:tol1",
        "type": "ContextSourceRegistration",
        "information": [{"entities": [{"type": "Building"}]}],
        "endpoint": "http://localhost:1/csr",
        "futureMember": "kept"
    });
    let resp = app
        .clone()
        .oneshot(
            Request::post("/ngsi-ld/v1/csourceRegistrations")
                .header("Content-Type", "application/json")
                .header("Content-Length", (reg.to_string()).len())
                .body(Body::from(reg.to_string()))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::CREATED, "registration create");
    let resp = app
        .clone()
        .oneshot(
            Request::get(
                "/ngsi-ld/v1/csourceRegistrations/urn:ngsi-ld:ContextSourceRegistration:tol1",
            )
            .body(Body::empty())
            .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::OK);
    let doc = body_json(resp).await;
    assert_eq!(doc["futureMember"], "kept");
}

#[tokio::test]
async fn entity_create_retrieve_delete_roundtrip() {
    let app = app();
    let entity = serde_json::json!({
        "id": "urn:ngsi-ld:Building:rt1",
        "type": "Building",
        "name": {"type": "Property", "value": "Eiffel Tower"}
    });
    let resp = app
        .clone()
        .oneshot(
            Request::post("/ngsi-ld/v1/entities")
                .header("Content-Type", "application/json")
                .header("Content-Length", (entity.to_string()).len())
                .body(Body::from(entity.to_string()))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::CREATED);
    assert_eq!(
        resp.headers().get("Location").map(|v| v.to_str().unwrap()),
        Some("/ngsi-ld/v1/entities/urn:ngsi-ld:Building:rt1")
    );

    // duplicate → 409
    let resp = app
        .clone()
        .oneshot(
            Request::post("/ngsi-ld/v1/entities")
                .header("Content-Type", "application/json")
                .header("Content-Length", (entity.to_string()).len())
                .body(Body::from(entity.to_string()))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    let resp = app
        .clone()
        .oneshot(
            Request::get("/ngsi-ld/v1/entities/urn:ngsi-ld:Building:rt1")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.headers().contains_key("Link"));
    let body = body_json(resp).await;
    assert_eq!(body["name"]["value"], "Eiffel Tower");
    assert!(body.get("createdAt").is_none(), "sysAttrs off by default");

    let resp = app
        .clone()
        .oneshot(
            Request::delete("/ngsi-ld/v1/entities/urn:ngsi-ld:Building:rt1")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .oneshot(
            Request::get("/ngsi-ld/v1/entities/urn:ngsi-ld:Building:rt1")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn query_requires_filter_and_unknown_param_is_400() {
    let resp = app()
        .oneshot(
            Request::get("/ngsi-ld/v1/entities")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let resp = app()
        .oneshot(
            Request::get("/ngsi-ld/v1/entities?invalidParams=x&type=Building")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp).await;
    assert_eq!(
        body["type"],
        "https://uri.etsi.org/ngsi-ld/errors/InvalidRequest"
    );
}

#[tokio::test]
async fn unsupported_media_type_and_accept() {
    let resp = app()
        .oneshot(
            Request::post("/ngsi-ld/v1/entities")
                .header("Content-Type", "text/plain")
                .header("Content-Length", ("x").len())
                .body(Body::from("x"))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

    let resp = app()
        .oneshot(
            Request::get("/ngsi-ld/v1/entities?type=Building")
                .header("Accept", "text/csv")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::NOT_ACCEPTABLE);
}

#[tokio::test]
async fn tenant_header_is_echoed_and_validated() {
    // 5.5.10: an unknown tenant on a non-create op is NonexistentTenant
    // 404 — and 6.3.14 still requires the tenant header on the response.
    let resp = app()
        .oneshot(
            Request::get("/ngsi-ld/v1/entities?type=Building")
                .header("NGSILD-Tenant", "city-01")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        resp.headers()
            .get("NGSILD-Tenant")
            .map(|v| v.to_str().expect("ascii")),
        Some("city-01")
    );
    // once the tenant exists (implicit creation), the query echoes on 200
    let app = app();
    let entity = serde_json::json!({"id": "urn:ngsi-ld:B:t1", "type": "Building"});
    let body = entity.to_string();
    let resp = app
        .clone()
        .oneshot(
            Request::post("/ngsi-ld/v1/entities")
                .header("Content-Type", "application/json")
                .header("Content-Length", body.len())
                .header("NGSILD-Tenant", "city-01")
                .body(Body::from(body))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::CREATED);
    let resp = app
        .oneshot(
            Request::get("/ngsi-ld/v1/entities?type=Building")
                .header("NGSILD-Tenant", "city-01")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("NGSILD-Tenant")
            .map(|v| v.to_str().expect("ascii")),
        Some("city-01")
    );
}

/// 5.5.10 / 6.3.14: the `snap-` tenant namespace is the broker's own —
/// the snapshot code mints `snap-index` (the synth-tenant reverse index)
/// and `snap-<uuid>` (one per snapshot). A client NGSILD-Tenant inside
/// that namespace would let the caller read and delete other tenants'
/// snapshot bookkeeping, so it is an invalid tenant value: 400
/// BadRequestData, and nothing reaches the store.
#[tokio::test]
async fn internal_tenant_namespace_is_reserved() {
    let state = AppState::new("antares-test".into());
    let app = router(state.clone());
    let idx = antares_model::TenantId::new_internal("snap-index").expect("tenant");

    // create (implicit tenant creation) must not mint the internal tenant
    let entity = serde_json::json!({"id": "urn:ngsi-ld:B:si", "type": "Building"});
    let body = entity.to_string();
    let resp = app
        .clone()
        .oneshot(
            Request::post("/ngsi-ld/v1/entities")
                .header("Content-Type", "application/json")
                .header("Content-Length", body.len())
                .header("NGSILD-Tenant", "snap-index")
                .body(Body::from(body))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        body_json(resp).await["type"],
        "https://uri.etsi.org/ngsi-ld/errors/BadRequestData"
    );
    assert!(
        !state.store.tenant_exists(&idx).await.expect("store"),
        "refused request must not create the internal tenant"
    );
    assert!(
        state
            .store
            .list(&idx, antares_store::Kind::Entity)
            .await
            .expect("store")
            .is_empty(),
        "refused request must not write into the internal tenant"
    );

    // reads of the index are refused too, whatever the resource
    for path in [
        "/ngsi-ld/v1/entities?type=Building",
        "/ngsi-ld/v1/subscriptions",
        "/ngsi-ld/v1/snapshots",
    ] {
        let resp = app
            .clone()
            .oneshot(
                Request::get(path)
                    .header("NGSILD-Tenant", "snap-index")
                    .body(Body::empty())
                    .expect("req"),
            )
            .await
            .expect("resp");
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{path}");
    }
    // a per-snapshot synthetic tenant is equally off-limits, and so is the
    // distributed-subscription inbound index, whose own record claims it
    // is reserved while nothing enforced it
    for tenant in ["snap-0123456789abcdef", "distsub-index"] {
        let resp = app
            .clone()
            .oneshot(
                Request::get("/ngsi-ld/v1/entities?type=Building")
                    .header("NGSILD-Tenant", tenant)
                    .body(Body::empty())
                    .expect("req"),
            )
            .await
            .expect("resp");
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{tenant}");
    }
}

/// The reserved namespace is the `snap-` prefix only: a tenant that
/// merely contains "snap" is an ordinary 5.5.10 tenant.
#[tokio::test]
async fn tenant_containing_snap_is_not_reserved() {
    let app = app();
    let entity = serde_json::json!({"id": "urn:ngsi-ld:B:st", "type": "Building"});
    let body = entity.to_string();
    let resp = app
        .clone()
        .oneshot(
            Request::post("/ngsi-ld/v1/entities")
                .header("Content-Type", "application/json")
                .header("Content-Length", body.len())
                .header("NGSILD-Tenant", "snapshots-team")
                .body(Body::from(body))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::CREATED);
    let resp = app
        .oneshot(
            Request::get("/ngsi-ld/v1/entities?type=Building")
                .header("NGSILD-Tenant", "snapshots-team")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("NGSILD-Tenant")
            .map(|v| v.to_str().expect("ascii")),
        Some("snapshots-team")
    );
}

mod erasure;
mod history;
mod temporal_window;
mod validation;
