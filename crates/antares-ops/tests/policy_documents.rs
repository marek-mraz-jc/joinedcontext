// SPDX-License-Identifier: EUPL-1.2
//! `/q/policy` and `PolicyEngine::load` (ADR-0020 amendment): a Tenant's rule
//! document written, replaced, deleted, reloaded into the engine on another
//! pod, refused without losing the previous one, bounded, absent without an
//! engine that reads documents, and behind the admin credential.

use antares_api::admin_auth::AdminAuth;
use antares_api::policy::{
    AllowAll, Decision, DecisionFuture, NotifyDecision, Operation, PolicyEngine, Subject,
};
use antares_api::AppState;
use antares_ops::views::rule_documents::{MAX_DOCUMENT_BYTES, MAX_DOCUMENT_DEPTH};
use antares_ops::views::Reload;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

/// An engine that keeps each Tenant's document and refuses one that says
/// `"refuse": true`, naming it in the reason.
#[derive(Default)]
struct Rules(Mutex<HashMap<String, Value>>);

impl Rules {
    fn held(&self, tenant: &str) -> Option<Value> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(tenant)
            .cloned()
    }
}

impl PolicyEngine for Rules {
    fn name(&self) -> &'static str {
        "rules"
    }
    fn decide<'a>(&'a self, _s: &'a Subject, _op: &'a Operation<'a>) -> DecisionFuture<'a> {
        Box::pin(std::future::ready(Decision::Allow))
    }
    fn pre_notify(&self, _s: &Subject, _sub: &Value, _n: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
    fn loads_documents(&self) -> bool {
        true
    }
    fn load(&self, tenant: &str, document: &Value) -> Result<(), String> {
        if document.get("refuse") == Some(&json!(true)) {
            return Err(format!("refused: {document}"));
        }
        let mut held = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if document.is_null() {
            held.remove(tenant);
        } else {
            held.insert(tenant.to_owned(), document.clone());
        }
        Ok(())
    }
}

const SECRET: &str = "rule-documents-are-written-with-the-admin-secret";

/// One pod: `engine` attached before the view table is installed, as the
/// broker's `main` does, over `store` when given (another pod's).
async fn pod(
    engine: Option<Arc<dyn PolicyEngine>>,
    auth: bool,
    store: Option<&AppState>,
) -> Result<(AppState, Reload, Router), String> {
    let mut state = AppState::new("antares-test".into());
    if let Some(other) = store {
        state.store = other.store.clone();
    }
    antares_api::wire(&mut state).await;
    if let Some(e) = engine {
        state = state.with_policy(e);
    }
    if auth {
        state = state.with_admin_auth(AdminAuth::new(SECRET, false)?);
    }
    let reload = antares_ops::views::install(&state).await?;
    let app = antares_api::router(
        state
            .clone()
            .with_surfaces(vec![Box::new(antares_api::Admin)])?,
    );
    Ok((state, reload, app))
}

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    body: Option<String>,
    bearer: Option<&str>,
) -> (StatusCode, Value) {
    let bytes = body.unwrap_or_default();
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("Content-Length", bytes.len());
    if !bytes.is_empty() {
        req = req.header("Content-Type", "application/json");
    }
    if let Some(token) = bearer {
        req = req.header("Authorization", format!("Bearer {token}"));
    }
    let Ok(resp) = app
        .clone()
        .oneshot(req.body(Body::from(bytes)).unwrap_or_default())
        .await;
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn put(app: &Router, tenant: &str, doc: &Value) -> (StatusCode, Value) {
    send(
        app,
        Method::PUT,
        &format!("/q/policy/{tenant}"),
        Some(doc.to_string()),
        None,
    )
    .await
}

async fn get(app: &Router, uri: &str) -> (StatusCode, Value) {
    send(app, Method::GET, uri, None, None).await
}

async fn health_documents(app: &Router) -> Value {
    let (s, body) = get(app, "/q/health").await;
    assert_eq!(s, StatusCode::OK);
    body.pointer("/policy/documents")
        .cloned()
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_document_is_put_replaced_and_deleted_and_the_engine_follows() {
    let rules = Arc::new(Rules::default());
    let (_state, _reload, app) = pod(Some(rules.clone()), false, None).await.expect("pod");
    let (s, body) = get(&app, "/q/policy").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body, json!({"loaded": [], "refused": []}));

    let first = json!({"allow": ["Car"]});
    assert_eq!(put(&app, "city", &first).await.0, StatusCode::CREATED);
    assert_eq!(rules.held("city"), Some(first.clone()));
    let (s, stored) = get(&app, "/q/policy/city").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        stored,
        json!({"tenant": "city", "revision": 1, "document": first})
    );

    let second = json!({"allow": ["Car", "Bus"]});
    assert_eq!(put(&app, "city", &second).await.0, StatusCode::NO_CONTENT);
    assert_eq!(rules.held("city"), Some(second.clone()));
    assert_eq!(get(&app, "/q/policy/city").await.1["revision"], 2);
    let listed = json!({"loaded": [{"tenant": "city", "revision": 2}], "refused": []});
    assert_eq!(get(&app, "/q/policy").await.1, listed);
    assert_eq!(health_documents(&app).await, listed);

    let (s, _) = send(&app, Method::DELETE, "/q/policy/city", None, None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(rules.held("city"), None, "the engine dropped the document");
    assert_eq!(get(&app, "/q/policy/city").await.0, StatusCode::NOT_FOUND);
    let (s, _) = send(&app, Method::DELETE, "/q/policy/city", None, None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(
        health_documents(&app).await,
        json!({"loaded": [], "refused": []})
    );
}

/// Another pod over the same store takes the document on its next reload,
/// and drops it once it is deleted there.
#[tokio::test(flavor = "multi_thread")]
async fn a_document_written_on_one_pod_reaches_the_engine_of_another() {
    let (here, there) = (Arc::new(Rules::default()), Arc::new(Rules::default()));
    let (state, _reload, app) = pod(Some(here), false, None).await.expect("pod");
    let (_s, reload, _app) = pod(Some(there.clone()), false, Some(&state))
        .await
        .expect("pod");
    let doc = json!({"allow": ["Car"]});
    assert_eq!(put(&app, "city", &doc).await.0, StatusCode::CREATED);
    assert_eq!(there.held("city"), None, "not before its reload");
    reload.now().await.expect("reload");
    assert_eq!(there.held("city"), Some(doc));
    let (s, _) = send(&app, Method::DELETE, "/q/policy/city", None, None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    reload.now().await.expect("reload");
    assert_eq!(there.held("city"), None);
}

/// A PUT the engine refuses is not stored; a stored revision it refuses on
/// reload keeps the previous one deciding and is named by tenant and
/// revision, never with the document or the engine's reason.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_document_keeps_the_previous_one() {
    let rules = Arc::new(Rules::default());
    let (state, reload, app) = pod(Some(rules.clone()), false, None).await.expect("pod");
    let good = json!({"allow": ["Car"]});
    assert_eq!(put(&app, "city", &good).await.0, StatusCode::CREATED);

    let (s, body) = put(&app, "city", &json!({"refuse": true})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["detail"]
        .as_str()
        .is_some_and(|d| d.contains("refused")));
    assert_eq!(rules.held("city"), Some(good.clone()));
    assert_eq!(get(&app, "/q/policy/city").await.1["revision"], 1);

    // Written past the PUT, as another version or a hand edit may leave it.
    let hidden = json!({"refuse": true, "secret": "sk-do-not-show"});
    state
        .put_policy_doc(
            "city",
            json!({"tenant": "city", "revision": 7, "document": hidden}),
        )
        .await
        .expect("stored");
    state
        .put_policy_doc("town", json!({"tenant": "town", "revision": 1}))
        .await
        .expect("stored");
    state
        .put_policy_doc(
            "village",
            json!({"tenant": "village", "revision": 1, "document": null}),
        )
        .await
        .expect("stored");
    reload.now().await.expect("reload");
    assert_eq!(rules.held("city"), Some(good), "the previous one decides");
    assert_eq!(rules.held("town"), None, "no document, nothing loaded");
    let report = health_documents(&app).await;
    assert_eq!(
        report,
        json!({"loaded": [], "refused": [
            {"tenant": "city", "revision": 7},
            {"tenant": "town", "revision": 1},
            {"tenant": "village", "revision": 1},
        ]})
    );
    let (_, health) = get(&app, "/q/health").await;
    assert!(!health.to_string().contains("sk-do-not-show"));
    // An unchanged refused revision is not asked again on the next reload.
    reload.now().await.expect("reload");
    assert_eq!(health_documents(&app).await, report);
    // A stored document with no tenant is skipped.
    state
        .put_policy_doc("x", json!({"revision": 1, "document": {}}))
        .await
        .expect("stored");
    reload.now().await.expect("reload");
    assert_eq!(health_documents(&app).await, report);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_document_is_bounded_and_named_for_a_client_tenant() {
    let rules = Arc::new(Rules::default());
    let (_state, _reload, app) = pod(Some(rules.clone()), false, None).await.expect("pod");
    let big = format!("\"{}\"", "x".repeat(MAX_DOCUMENT_BYTES));
    let (s, _) = send(&app, Method::PUT, "/q/policy/city", Some(big), None).await;
    assert_eq!(s, StatusCode::PAYLOAD_TOO_LARGE);
    let deep = (0..MAX_DOCUMENT_DEPTH).fold(json!(0), |v, _| json!([v]));
    let cases = [
        ("too deep", "city", deep.to_string()),
        ("null", "city", "null".to_owned()),
        ("no JSON", "city", "{".to_owned()),
        ("a reserved tenant", "antares-policy", "{}".to_owned()),
        ("a tenant outside the grammar", "a.b", "{}".to_owned()),
    ];
    for (what, tenant, body) in cases {
        let uri = format!("/q/policy/{tenant}");
        let (s, _) = send(&app, Method::PUT, &uri, Some(body), None).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{what}");
        assert_eq!(rules.held(tenant), None, "{what} reached the engine");
    }
    for method in [Method::GET, Method::DELETE] {
        let (s, _) = send(&app, method, "/q/policy/antares-views", None, None).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
    }
}

/// `AllowAll` and a broker without an engine read no documents: every
/// `/q/policy` route answers 404, `/q/health` lists none.
#[tokio::test(flavor = "multi_thread")]
async fn without_an_engine_that_reads_documents_policy_is_404() {
    for engine in [None, Some(Arc::new(AllowAll) as Arc<dyn PolicyEngine>)] {
        let (_state, reload, app) = pod(engine, false, None).await.expect("pod");
        for (method, uri, body) in [
            (Method::GET, "/q/policy", None),
            (Method::GET, "/q/policy/city", None),
            (Method::PUT, "/q/policy/city", Some("{}".to_owned())),
            (Method::DELETE, "/q/policy/city", None),
        ] {
            let (s, _) = send(&app, method.clone(), uri, body, None).await;
            assert_eq!(s, StatusCode::NOT_FOUND, "{method} {uri}");
        }
        reload.now().await.expect("reload");
        assert_eq!(health_documents(&app).await, Value::Null);
    }
    assert!(
        AllowAll.load("city", &json!({})).is_err(),
        "the default refuses"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn policy_sits_behind_the_admin_secret() {
    let rules = Arc::new(Rules::default());
    let (_state, _reload, app) = pod(Some(rules.clone()), true, None).await.expect("pod");
    let doc = Some(json!({"allow": ["Car"]}).to_string());
    for bearer in [None, Some("not-the-admin-secret")] {
        for (method, uri, body) in [
            (Method::GET, "/q/policy", None),
            (Method::GET, "/q/policy/city", None),
            (Method::PUT, "/q/policy/city", doc.clone()),
            (Method::DELETE, "/q/policy/city", None),
        ] {
            let (s, _) = send(&app, method.clone(), uri, body, bearer).await;
            assert_eq!(s, StatusCode::UNAUTHORIZED, "{method} {uri} {bearer:?}");
        }
    }
    assert_eq!(rules.held("city"), None);
    let (s, _) = send(&app, Method::PUT, "/q/policy/city", doc, Some(SECRET)).await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, _) = send(&app, Method::GET, "/q/policy", None, Some(SECRET)).await;
    assert_eq!(s, StatusCode::OK);
}

/// T-5717: a space that 600 Pipelines write holds 600 derived write grants
/// (PL-20), about 140 KiB and 10,000 values; the 64 KiB bound refused the
/// slovensko-podujatia document at 302 and left every newer pipeline 403.
#[tokio::test(flavor = "multi_thread")]
async fn a_document_of_six_hundred_pipeline_grants_is_taken() {
    let rules = Arc::new(Rules::default());
    let (_state, _reload, app) = pod(Some(rules.clone()), false, None).await.expect("pod");
    let policies: Vec<Value> = (0..600)
        .map(|n| {
            json!({
                "id": format!("slovensko/pl-podujatia-source-{n:03}-w-1"),
                "effect": "permit",
                "assignee": {"kind": "serviceAccount", "id": format!("pl-podujatia-source-{n:03}")},
                "operations": ["5.6.8", "5.6.7", "5.7.2"],
                "information": [{"entities": [{"type": "Event"}]}],
            })
        })
        .collect();
    let doc = json!({ "policies": policies });
    assert!(doc.to_string().len() > 128 * 1024);
    assert_eq!(put(&app, "city", &doc).await.0, StatusCode::CREATED);
    assert_eq!(rules.held("city"), Some(doc));
}
