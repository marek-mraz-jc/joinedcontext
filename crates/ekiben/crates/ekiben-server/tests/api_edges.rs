//! The Streams API's refusals and error answers, route by route: malformed bodies and ids, lint
//! errors, foreign organizations, the quota API's admin rule, resource kinds, a refused save, and
//! what `/ready` and `/metrics` show to whom.

use std::sync::Arc;
use std::time::Duration;

use ekiben_core::{
    AckResponder, Batch, CoreError, EnvAccess, OrgId, OrgQuotas, ResourceManager, StreamContext,
};
use ekiben_server::{
    build_router, ServerAuthConfig, ServerState, StreamManager, StreamPersistence,
};
use ekiben_testkit::full_registry;
use futures::future::BoxFuture;
use reqwest::StatusCode;
use serde_json::{json, Value};
use tokio::net::TcpListener;

const ADMIN: &str = "admin-edges-token";

const RESIDENT: &str =
    "input:\n  generate:\n    interval: 1h\n    mapping: 'root = {}'\noutput:\n  drop: {}\n";

const ONCE: &str =
    "input:\n  generate:\n    count: 2\n    mapping: 'root = {}'\noutput:\n  drop: {}\n";

const MISSING_VAR: &str = "input:\n  generate:\n    interval: 1h\n    mapping: 'root = \"${EKIBEN_VAR_NOT_SET_T7583}\"'\noutput:\n  drop: {}\n";

/// An input whose connect never finishes: its stream is active and not ready.
struct Stalls;

#[async_trait::async_trait]
impl ekiben_core::Input for Stalls {
    fn input_type(&self) -> &'static str {
        "stalls"
    }
    async fn connect(&mut self, _ctx: &StreamContext) -> Result<(), CoreError> {
        std::future::pending::<()>().await;
        Ok(())
    }
    async fn read_batch(
        &mut self,
        _ctx: &StreamContext,
    ) -> Result<(Batch, AckResponder), CoreError> {
        Err(CoreError::EndOfInput)
    }
}

/// Refuses every save.
struct Refuses;

impl StreamPersistence for Refuses {
    fn saved<'a>(
        &'a self,
        _: &'a OrgId,
        _: &'a str,
        _: &'a Value,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async { Err("database down".to_string()) })
    }
    fn deleted<'a>(&'a self, _: &'a OrgId, _: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
    fn resource_saved<'a>(
        &'a self,
        _: &'a OrgId,
        _: &'a str,
        _: &'a str,
        _: &'a str,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async { Err("database down".to_string()) })
    }
}

fn manager(resources: ResourceManager) -> StreamManager {
    let reg = full_registry();
    reg.register_input("stalls", |_, _| Ok(Box::new(Stalls)));
    StreamManager::new(reg, resources, None, EnvAccess::AllowListed)
}

async fn serve_manager(auth: ServerAuthConfig, manager: StreamManager) -> String {
    let app = build_router(ServerState::new(manager, auth));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

async fn serve(auth: ServerAuthConfig) -> String {
    serve_manager(auth, manager(ResourceManager::new())).await
}

fn admin_and_org() -> ServerAuthConfig {
    ServerAuthConfig {
        admin_bearer_token: Some(ADMIN.to_string()),
        org_tokens: vec![("tok-a".to_string(), OrgId::new("org-a"))],
        ..Default::default()
    }
}

async fn text(res: reqwest::Response) -> (StatusCode, String) {
    let status = res.status();
    (status, res.text().await.unwrap())
}

#[tokio::test]
async fn a_token_in_both_the_list_and_the_file_acts_for_the_listed_organization() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tokens");
    std::fs::write(&path, ekiben_server::tokens::token_line("org-b", "tok-a")).unwrap();
    let mut auth = admin_and_org();
    auth.org_token_file = Some(Arc::new(ekiben_server::OrgTokenFile::load(&path).unwrap()));
    let base = serve(auth).await;
    let c = reqwest::Client::new();
    let res = c
        .post(format!("{base}/streams/s"))
        .bearer_auth("tok-a")
        .body(RESIDENT)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let mine = c
        .get(format!("{base}/streams/s"))
        .bearer_auth(ADMIN)
        .header("x-jc-org", "org-a")
        .send()
        .await
        .unwrap();
    assert_eq!(mine.status(), StatusCode::OK);
}

#[tokio::test]
async fn an_id_past_256_characters_is_refused() {
    let base = serve(ServerAuthConfig::default()).await;
    let long = "s".repeat(257);
    let (status, body) = text(
        reqwest::Client::new()
            .post(format!("{base}/streams/{long}"))
            .body(RESIDENT)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        (status, body.as_str()),
        (StatusCode::BAD_REQUEST, "invalid stream id length")
    );
}

#[tokio::test]
async fn version_and_endpoints_answer_their_maps() {
    let base = serve(ServerAuthConfig::default()).await;
    let c = reqwest::Client::new();
    let v: Value = c
        .get(format!("{base}/version"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
    assert!(v["built"].is_string());
    let e: Value = c
        .get(format!("{base}/bento/endpoints"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(e["/streams/{id}/stats"].is_string());
    assert_eq!(e.as_object().unwrap().len(), 10);
}

#[tokio::test]
async fn a_lowercase_bearer_is_accepted_and_a_bad_org_header_is_refused() {
    let base = serve(admin_and_org()).await;
    let c = reqwest::Client::new();
    let res = c
        .get(format!("{base}/streams"))
        .header("authorization", format!("bearer {ADMIN}"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let (status, body) = text(
        c.get(format!("{base}/streams"))
            .bearer_auth(ADMIN)
            .header("x-jc-org", "bad org")
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        (status, body.as_str()),
        (StatusCode::BAD_REQUEST, "invalid X-Jc-Org header")
    );
    // An empty header means the default organization.
    let res = c
        .get(format!("{base}/streams"))
        .bearer_auth(ADMIN)
        .header("x-jc-org", "")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[test]
fn debug_of_the_auth_config_redacts_every_token() {
    let auth = admin_and_org();
    let debug = format!("{auth:?}");
    assert!(
        !debug.contains(ADMIN) && !debug.contains("tok-a"),
        "{debug}"
    );
    assert!(
        debug.contains("[REDACTED]") && debug.contains("org-a"),
        "{debug}"
    );
}

#[tokio::test]
async fn post_validates_the_id_the_body_and_the_lints() {
    let base = serve(ServerAuthConfig::default()).await;
    let c = reqwest::Client::new();
    let post = |path: String, body: &'static str| c.post(format!("{base}{path}")).body(body).send();
    let (status, body) = text(post("/streams/bad%20id".into(), RESIDENT).await.unwrap()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("disallowed characters"), "{body}");
    let (status, body) = text(post("/streams/s".into(), "input: [\n").await.unwrap()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.starts_with("Error: "), "{body}");
    let (status, _) = text(post("/streams/s".into(), "input: 5\n").await.unwrap()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let res = post("/streams/s".into(), MISSING_VAR).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let lints: Value = res.json().await.unwrap();
    assert_eq!(
        lints["lint_errors"][0],
        "environment variable 'EKIBEN_VAR_NOT_SET_T7583' was not defined"
    );
    assert_eq!(lints["lint_errors"], lints["linting_errors"]);
    // Chilled: the lint is ignored and the stream starts.
    let res = post("/streams/s?chilled=true".into(), MISSING_VAR)
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let (status, body) = text(
        post("/streams/s?chilled=true".into(), MISSING_VAR)
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        (status, body.as_str()),
        (StatusCode::BAD_REQUEST, "Stream already exists")
    );
}

#[tokio::test]
async fn a_run_once_post_or_put_answers_the_run_summary() {
    let base = serve(ServerAuthConfig::default()).await;
    let c = reqwest::Client::new();
    let summary: Value = c
        .post(format!("{base}/streams/job?run=once"))
        .body(ONCE)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(summary["success"], true);
    assert_eq!(summary["messages_in"], 2);
    let summary: Value = c
        .put(format!("{base}/streams/job?run=once"))
        .body(ONCE)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(summary["messages_out"], 2);
    let status: Value = c
        .get(format!("{base}/streams/job"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["active"], false);
    assert_eq!(status["last_run"]["success"], true);
}

#[tokio::test]
async fn put_validates_like_post_and_answers_404_for_an_unknown_stream() {
    let base = serve(ServerAuthConfig::default()).await;
    let c = reqwest::Client::new();
    let put = |path: &str, body: &'static str| c.put(format!("{base}{path}")).body(body).send();
    let (status, _) = text(put("/streams/bad%20id", RESIDENT).await.unwrap()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = text(put("/streams/s", "input: [\n").await.unwrap()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let res = put("/streams/s", MISSING_VAR).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert!(res.json::<Value>().await.unwrap()["lint_errors"].is_array());
    let (status, body) = text(put("/streams/s", RESIDENT).await.unwrap()).await;
    assert_eq!(
        (status, body.as_str()),
        (StatusCode::NOT_FOUND, "Stream not found")
    );
    assert_eq!(
        c.post(format!("{base}/streams/s"))
            .body(RESIDENT)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        put("/streams/s", RESIDENT).await.unwrap().status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn patch_delete_and_stats_answer_404_or_their_result() {
    let base = serve(ServerAuthConfig::default()).await;
    let c = reqwest::Client::new();
    let (status, _) = text(
        c.patch(format!("{base}/streams/s"))
            .body("a: [\n")
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = text(
        c.patch(format!("{base}/streams/s"))
            .body("{}")
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        c.delete(format!("{base}/streams/s"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        c.get(format!("{base}/streams/s/stats"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    c.post(format!("{base}/streams/s"))
        .body(RESIDENT)
        .send()
        .await
        .unwrap();
    let res = c
        .patch(format!("{base}/streams/s"))
        .body("{\"input\": {\"generate\": {\"interval\": \"2h\"}}}")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let got: Value = c
        .get(format!("{base}/streams/s"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(got["config"]["input"]["generate"]["interval"], "2h");
    let stats: Value = c
        .get(format!("{base}/streams/s/stats"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(stats.is_object());
    assert_eq!(
        c.delete(format!("{base}/streams/s"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn set_streams_replaces_the_whole_set_and_refuses_bad_entries() {
    let base = serve(ServerAuthConfig::default()).await;
    let c = reqwest::Client::new();
    let set = |body: String, q: &str| c.post(format!("{base}/streams{q}")).body(body).send();
    let gen = json!({"input": {"generate": {"interval": "1h", "mapping": "root = {}"}}, "output": {"drop": {}}});
    let (status, _) = text(set("- not a map\n".into(), "").await.unwrap()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, body) = text(set(json!({"bad id": gen}).to_string(), "").await.unwrap()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("disallowed characters"), "{body}");
    let (status, _) = text(
        set(json!({"a": {"input": 5}}).to_string(), "")
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let missing = json!({"input": {"generate": {"interval": "1h", "mapping": "root = \"${EKIBEN_VAR_NOT_SET_T7583}\""}}, "output": {"drop": {}}});
    let res = set(json!({"a": missing}).to_string(), "").await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let lints: Value = res.json().await.unwrap();
    assert!(
        lints["lint_errors"][0]
            .as_str()
            .unwrap()
            .starts_with("stream 'a': "),
        "{lints}"
    );

    assert_eq!(
        set(json!({"a": gen, "b": gen}).to_string(), "")
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    // A second set keeps `a` (updated), drops `b`, adds `c`.
    assert_eq!(
        set(json!({"a": missing, "c": gen}).to_string(), "?chilled=true")
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let list: Value = c
        .get(format!("{base}/streams"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mut ids: Vec<&str> = list
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    ids.sort();
    assert_eq!(ids, ["a", "c"]);
}

#[tokio::test]
async fn an_org_token_cannot_post_into_another_organization() {
    let base = serve(admin_and_org()).await;
    let c = reqwest::Client::new();
    let body = format!("org: org-b\n{RESIDENT}");
    let (status, msg) = text(
        c.post(format!("{base}/streams/s"))
            .bearer_auth("tok-a")
            .body(body.clone())
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(msg.contains("another organization"), "{msg}");
    let set = json!({"s": serde_norway::from_str::<Value>(&body).unwrap()}).to_string();
    let res = c
        .post(format!("{base}/streams"))
        .bearer_auth("tok-a")
        .body(set)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    // Its own organization named explicitly is fine.
    let own = format!("org: org-a\n{RESIDENT}");
    let res = c
        .post(format!("{base}/streams/s"))
        .bearer_auth("tok-a")
        .body(own)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn core_errors_map_to_their_status_codes() {
    let resources = ResourceManager::new();
    resources.set_org_quotas(
        &OrgId::default(),
        OrgQuotas {
            max_concurrent_streams: 1,
            ..Default::default()
        },
    );
    let base = serve_manager(ServerAuthConfig::default(), manager(resources)).await;
    let c = reqwest::Client::new();
    let post = |id: &str, body: &'static str| {
        c.post(format!("{base}/streams/{id}?chilled=true"))
            .body(body)
            .send()
    };
    assert_eq!(
        post("one", RESIDENT).await.unwrap().status(),
        StatusCode::OK
    );
    let (status, _) = text(post("two", RESIDENT).await.unwrap()).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    // The whole-set POST answers the first stream that does not start.
    let set = json!({"one": serde_norway::from_str::<Value>(RESIDENT).unwrap(), "two": serde_norway::from_str::<Value>(RESIDENT).unwrap()});
    let (status, _) = text(
        c.post(format!("{base}/streams"))
            .body(set.to_string())
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    c.delete(format!("{base}/streams/one"))
        .send()
        .await
        .unwrap();
    let bad_mapping = "input:\n  generate:\n    mapping: 'root = ('\noutput:\n  drop: {}\n";
    let (status, _) = text(post("m", bad_mapping).await.unwrap()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let unknown = "input:\n  no_such_input: {}\noutput:\n  drop: {}\n";
    let (status, _) = text(post("u", unknown).await.unwrap()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // No file root for an organization: access denied.
    let denied = manager(ResourceManager::new()).with_org_file_roots();
    let base = serve_manager(ServerAuthConfig::default(), denied).await;
    let (status, body) = text(
        c.post(format!("{base}/streams/f"))
            .body(RESIDENT)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(body.contains("no file root"), "{body}");

    // A refused save is the host's failure: 502.
    let saving = manager(ResourceManager::new()).with_persistence(Arc::new(Refuses));
    let base = serve_manager(ServerAuthConfig::default(), saving).await;
    let (status, body) = text(
        c.post(format!("{base}/streams/p"))
            .body(RESIDENT)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(body.contains("was not saved: database down"), "{body}");
    let (status, body) = text(
        c.post(format!("{base}/resources/cache/c"))
            .body("memory: {}")
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body, "the resource was not saved: database down");
}

#[tokio::test]
async fn resources_refuse_bad_bodies_kinds_and_components() {
    let base = serve(ServerAuthConfig::default()).await;
    let c = reqwest::Client::new();
    let post = |path: &str, body: &'static str| c.post(format!("{base}{path}")).body(body).send();
    assert_eq!(
        post("/resources/cache/c", "memory: {}")
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let (status, body) = text(post("/resources/widget/w", "memory: {}").await.unwrap()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body.starts_with("Var `type` must be set to one of"),
        "{body}"
    );
    let (status, body) = text(post("/resources/cache/c", "memory: [\n").await.unwrap()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.starts_with("Error: "), "{body}");
    let res = post("/resources/processor/p", "mapping: 'root = ('")
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert!(res.json::<Value>().await.unwrap()["lint_errors"][0].is_string());
    let (status, body) = text(
        post("/resources/processor/p?chilled=true", "mapping: 'root = ('")
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(!body.starts_with('{'), "{body}");
    let missing = "mapping: 'root = \"${EKIBEN_VAR_NOT_SET_T7583}\"'";
    let res = post("/resources/processor/p", missing).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        res.json::<Value>().await.unwrap()["lint_errors"][0],
        "environment variable 'EKIBEN_VAR_NOT_SET_T7583' was not defined"
    );
    assert_eq!(
        post("/resources/processor/p?chilled=true", missing)
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn the_quota_api_takes_only_the_admin_and_validates_the_body() {
    let base = serve(admin_and_org()).await;
    let c = reqwest::Client::new();
    let get = |tok: Option<&str>| {
        let r = c.get(format!("{base}/orgs/org-a/quotas"));
        match tok {
            Some(t) => r.bearer_auth(t.to_string()),
            None => r,
        }
        .send()
    };
    assert_eq!(get(Some(ADMIN)).await.unwrap().status(), StatusCode::OK);
    assert_eq!(
        get(Some("tok-a")).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    let (status, _) = text(
        c.get(format!("{base}/orgs/bad%20org/quotas"))
            .bearer_auth(ADMIN)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let put = |org: &str, body: &'static str, tok: &str| {
        c.put(format!("{base}/orgs/{org}/quotas"))
            .bearer_auth(tok.to_string())
            .body(body)
            .send()
    };
    assert_eq!(
        put("org-a", "{}", "tok-a").await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    let (status, body) = text(put("bad%20org", "{}", ADMIN).await.unwrap()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("organization id"), "{body}");
    let (status, body) = text(put("org-a", "not json", ADMIN).await.unwrap()).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body.starts_with("invalid quotas: "), "{body}");
    let (status, body) = text(
        put("org-a", "{\"max_messages_per_sec\": \"fast\"}", ADMIN)
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body.contains("field `max_messages_per_sec`"), "{body}");
    // An array would set fields by position: refused.
    let (status, body) = text(put("org-a", "[1]", ADMIN).await.unwrap()).await;
    assert_eq!((status, body.as_str()), (StatusCode::UNPROCESSABLE_ENTITY, "invalid quotas: expected a map of quota fields (e.g. {\"max_messages_per_sec\": 50000})"));
    let (status, body) = text(
        put("org-a", "{\"max_messages_per_sec\": 0}", ADMIN)
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        body.ends_with(
            "max_messages_per_sec must be at least 1 (e.g. {\"max_messages_per_sec\": 50000})"
        ),
        "{body}"
    );
    let (status, body) = text(
        put("org-a", "{\"max_message_per_sec\": 5}", ADMIN)
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        body.contains("unknown field `max_message_per_sec`"),
        "{body}"
    );
    let res = put("org-a", "{\"max_messages_per_sec\": 7}", ADMIN)
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.json::<Value>().await.unwrap()["max_messages_per_sec"],
        7
    );
}

#[tokio::test]
async fn a_server_without_an_admin_token_has_no_quota_api() {
    let base = serve(ServerAuthConfig {
        org_tokens: vec![("tok-a".to_string(), OrgId::new("org-a"))],
        ..Default::default()
    })
    .await;
    let c = reqwest::Client::new();
    let res = c
        .get(format!("{base}/orgs/org-a/quotas"))
        .bearer_auth("tok-a")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let base = serve(admin_and_org()).await;
    for req in [
        c.get(format!("{base}/orgs/org-a/quotas")),
        c.get(format!("{base}/orgs/org-a/quotas"))
            .bearer_auth("wrong"),
    ] {
        assert_eq!(req.send().await.unwrap().status(), StatusCode::UNAUTHORIZED);
    }
}

#[tokio::test]
async fn ready_names_unconnected_streams_only_to_their_own_organization() {
    let base = serve(admin_and_org()).await;
    let c = reqwest::Client::new();
    let stalls = "input:\n  stalls: {}\noutput:\n  drop: {}\n";
    let res = c
        .post(format!("{base}/streams/waiting?chilled=true"))
        .bearer_auth(ADMIN)
        .body(stalls)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let ready = |tok: Option<&'static str>| {
        let r = c.get(format!("{base}/ready"));
        match tok {
            Some(t) => r.bearer_auth(t),
            None => r,
        }
        .send()
    };
    let (status, body) = text(ready(Some(ADMIN)).await.unwrap()).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body, "streams waiting are not connected\n");
    let (_, body) = text(ready(Some("tok-a")).await.unwrap()).await;
    assert_eq!(body, "streams are not connected\n");
    let (_, body) = text(ready(None).await.unwrap()).await;
    assert_eq!(body, "streams are not connected\n");
    // An org token's metrics exclude the default organization's series.
    let m = c
        .get(format!("{base}/metrics"))
        .bearer_auth("tok-a")
        .send()
        .await
        .unwrap();
    assert_eq!(m.status(), StatusCode::OK);
    assert!(!m.text().await.unwrap().contains("waiting"));
    tokio::time::sleep(Duration::from_millis(10)).await;
}
