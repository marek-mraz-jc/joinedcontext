//! Putting an App's objects on a cluster (T-0411, T-0508, AP-13, AP-13a, AP-18, AP-21, AP-27,
//! ADR-N-019).
//!
//! Every case runs the real converger against a stubbed API server, so what is asserted is the
//! request that actually leaves the Portal: which path, which verb, which body. Three properties
//! cannot be seen from inside the process. A second run of an unchanged app must not mint a new
//! endpoint slug, the Portal must not be able to address anything but the four objects an app
//! owns, and nothing at all is written to the realm: the login front is the edge's one client.

use jcctl::loader::RawManifest;
use joinedcontext_portal::apps::converge::{Converger, Outcome};
use joinedcontext_portal::apps::kube::{KubeClient, FIELD_MANAGER};
use joinedcontext_portal::apps::reconciler::Settings;
use serde_json::{json, Value};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

/// What the build lane proposes and the Portal checked on push: the image's manifest digest.
const DIGEST: &str = "sha256:1111111111111111111111111111111111111111111111111111111111111111";
/// The reference the reconciler composes from its registry setting and that digest (AP-108).
const APP_IMAGE: &str =
    "forge.bb.example.com/joinedcontext/app-air-quality-today@sha256:1111111111111111111111111111111111111111111111111111111111111111";
const NAMESPACE: &str = "joinedcontext";
const SLUG: &str = "abcdefghijklmnopqrstuvwxyz";
const TOKEN: &str = "the-projected-service-account-token-of-the-portal";

fn settings() -> Settings {
    Settings {
        host: "bb.example.com".into(),
        apex: "bb.example.com".into(),
        gateway_url: Some("http://context-gateway.jc.svc.cluster.local:8080".into()),
        service_url: Some("http://portal.joinedcontext.svc.cluster.local:8080".into()),
        namespace: NAMESPACE.into(),
        org_domain: "banskabystrica.sk".into(),
        apisix_namespace: "apisix".into(),
        image_repository: Some("forge.bb.example.com/joinedcontext".into()),
        pull_secret: Some("app-registry".into()),
        basemap_base: None,
        release: Some("dev".into()),
        service_account: Some("portal".into()),
    }
}

/// A published full-stack app whose artifact the build lane has already written back into
/// `status.build` (AP-13a).
fn app(lifecycle: &str, image: Option<&str>) -> RawManifest {
    let metadata = json!({ "name": "air-quality-today", "namespace": "ovzdusie" });
    let status = image.map(|image| {
        json!({ "build": {
            "digest": image,
            "commit": "8c56954a1f0e",
            "sdkVersion": "0.4.1",
            "builtAt": "2026-09-17T06:00:00Z",
        }})
    });
    serde_json::from_value(json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": "App",
        "metadata": metadata,
        "status": status,
        "spec": {
            "kind": "fullstack",
            "source": { "path": "./src" },
            "build": { "rust": "1.90", "node": "22" },
            "visibility": "project",
            "lifecycle": lifecycle,
            "dataNeeds": [{
                "contextSpaceRef": { "kind": "ContextSpace", "name": "ovzdusie" },
                "types": ["AirQualityObserved"],
                "attrs": ["pm10", "location"],
                "operations": ["queryEntity"],
                "representations": ["ngsi-ld"]
            }]
        },
    }))
    .expect("the fixture is a manifest")
}

fn converger(api: &MockServer) -> Converger {
    Converger::new(
        KubeClient::with_token(&api.uri(), TOKEN).expect("a client"),
        settings(),
    )
}

/// The Secret an app that has already been deployed once has, as the API server returns it.
fn stored_secret() -> Value {
    json!({
        "apiVersion": "v1",
        "kind": "Secret",
        "metadata": { "name": "app-air-quality-today-endpoint", "namespace": NAMESPACE },
        "data": { "endpoint-slug": "YWJjZGVmZ2hpamtsbW5vcHFyc3R1dnd4eXo=" },
    })
}

/// Answers 404 for the app's Secret: an app nobody has deployed yet.
async fn no_secret(api: &MockServer) {
    Mock::given(method("GET"))
        .and(path(SECRET))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "kind": "Status", "status": "Failure", "reason": "NotFound",
            "message": "secrets \"app-air-quality-today-endpoint\" not found",
        })))
        .mount(api)
        .await;
}

/// Accepts a server-side apply on one path and records it.
async fn accepts_apply(api: &MockServer, api_path: &str) {
    Mock::given(method("PATCH"))
        .and(path(api_path.to_owned()))
        .and(query_param("fieldManager", FIELD_MANAGER))
        .and(header("content-type", "application/apply-patch+yaml"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "kind": "Status" })))
        .mount(api)
        .await;
}

fn applied<'a>(requests: &'a [Request], api_path: &str) -> &'a Request {
    requests
        .iter()
        .find(|request| request.url.path() == api_path && request.method.as_str() == "PATCH")
        .unwrap_or_else(|| panic!("nothing was applied to {api_path}"))
}

fn body_of(request: &Request) -> Value {
    serde_json::from_slice(&request.body).expect("the body is the object")
}

/// AP-27, ADR-N-019: every request the converger makes is a Kubernetes one. A path outside
/// `/api/` and `/apis/` would be the realm's Admin API, which no app needs any more.
fn only_the_cluster_was_called(requests: &[Request]) {
    for request in requests {
        let path = request.url.path();
        assert!(
            path.starts_with("/api/") || path.starts_with("/apis/"),
            "{} {path} is not a Kubernetes call",
            request.method
        );
    }
}

const DEPLOYMENT: &str =
    "/apis/apps/v1/namespaces/dev-ovzdusie-apps/deployments/app-air-quality-today";
const SERVICE: &str = "/api/v1/namespaces/dev-ovzdusie-apps/services/app-air-quality-today";
const SECRET: &str = "/api/v1/namespaces/dev-ovzdusie-apps/secrets/app-air-quality-today-endpoint";
const POLICY: &str =
    "/apis/networking.k8s.io/v1/namespaces/dev-ovzdusie-apps/networkpolicies/app-air-quality-today";

#[tokio::test]
async fn a_published_app_becomes_its_four_objects_on_the_cluster_and_nothing_on_the_realm() {
    let api = MockServer::start().await;
    no_secret(&api).await;
    for api_path in [DEPLOYMENT, SERVICE, SECRET, POLICY] {
        accepts_apply(&api, api_path).await;
    }

    let outcome = converger(&api)
        .converge_one(&app("published", Some(DIGEST)))
        .await
        .expect("the app converges");
    assert_eq!(outcome, Outcome::Applied);

    let requests = api
        .received_requests()
        .await
        .expect("the stub recorded them");
    only_the_cluster_was_called(&requests);
    assert_eq!(
        body_of(applied(&requests, SERVICE))["kind"],
        json!("Service")
    );
    assert_eq!(
        body_of(applied(&requests, POLICY))["kind"],
        json!("NetworkPolicy")
    );

    // AP-13, AP-26, AP-108: the digest status.build carries, under this installation's registry,
    // is the image of the pod, and the pod is that one container on port 8080, pulled with the
    // configured Secret, with no sidecar beside it.
    let deployment = body_of(applied(&requests, DEPLOYMENT));
    assert_eq!(deployment["kind"], json!("Deployment"));
    let containers = deployment["spec"]["template"]["spec"]["containers"]
        .as_array()
        .expect("containers");
    assert_eq!(containers.len(), 1, "{containers:?}");
    assert_eq!(containers[0]["name"], json!("app"));
    assert_eq!(containers[0]["image"], json!(APP_IMAGE));
    assert_eq!(
        deployment["spec"]["template"]["spec"]["imagePullSecrets"],
        json!([{ "name": "app-registry" }])
    );
    assert_eq!(containers[0]["ports"][0]["containerPort"], json!(8080));
    assert_eq!(
        body_of(applied(&requests, SERVICE))["spec"]["ports"][0]["port"],
        json!(8080)
    );

    // The Secret carries the slug and nothing else, and it is written before the Deployment,
    // so a run that fails halfway cannot mint a second slug for a pod already running.
    let secret = body_of(applied(&requests, SECRET));
    let slug = secret["stringData"]["endpoint-slug"]
        .as_str()
        .expect("a generated slug");
    assert!(slug.len() >= 26, "{slug}");
    assert_eq!(
        secret["stringData"].as_object().map(|keys| keys.len()),
        Some(1),
        "no client secret, no cookie secret (AP-27): {secret}"
    );
    let order: Vec<&str> = requests
        .iter()
        .filter(|request| request.method.as_str() == "PATCH")
        .map(|request| request.url.path())
        .collect();
    let secret_at = order.iter().position(|p| *p == SECRET).expect("the secret");
    let deployment_at = order
        .iter()
        .position(|p| *p == DEPLOYMENT)
        .expect("the deployment");
    assert!(secret_at < deployment_at, "{order:?}");
}

/// EP-02: the run that follows a run must not move the endpoint.
#[tokio::test]
async fn a_second_run_reuses_the_slug_the_app_already_has() {
    let api = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(SECRET))
        .respond_with(ResponseTemplate::new(200).set_body_json(stored_secret()))
        .mount(&api)
        .await;
    for api_path in [DEPLOYMENT, SERVICE, SECRET, POLICY] {
        accepts_apply(&api, api_path).await;
    }

    converger(&api)
        .converge_one(&app("published", Some(DIGEST)))
        .await
        .expect("the app converges");

    let requests = api
        .received_requests()
        .await
        .expect("the stub recorded them");
    only_the_cluster_was_called(&requests);
    let secret = body_of(applied(&requests, SECRET));
    assert_eq!(secret["stringData"]["endpoint-slug"], json!(SLUG));
    let deployment = body_of(applied(&requests, DEPLOYMENT)).to_string();
    assert!(
        deployment.contains(&format!(
            "http://context-gateway.jc.svc.cluster.local:8080/api/endpoint/{SLUG}/"
        )),
        "the pod is pointed at the endpoint it already had: {deployment}"
    );
}

/// A Secret of that name that somebody else wrote is not silently taken over.
#[tokio::test]
async fn a_secret_without_the_slug_is_not_this_reconcilers_and_stops_the_app() {
    let api = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(SECRET))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "apiVersion": "v1",
            "kind": "Secret",
            "metadata": { "name": "app-air-quality-today-endpoint", "namespace": NAMESPACE },
            "data": { "password": "aHVudGVyMg==" },
        })))
        .mount(&api)
        .await;

    let refused = converger(&api)
        .converge_one(&app("published", Some(DIGEST)))
        .await
        .expect_err("a foreign secret was taken over");
    assert!(refused.to_string().contains("endpoint-slug"), "{refused}");
    let applied: Vec<String> = api
        .received_requests()
        .await
        .expect("the stub recorded them")
        .iter()
        .filter(|request| request.method.as_str() == "PATCH")
        .map(|request| request.url.path().to_owned())
        .collect();
    assert!(applied.is_empty(), "{applied:?} reached the cluster anyway");
}

/// AP-21: retiring an app removes what it was running, and nothing is rendered to do it.
#[tokio::test]
async fn a_retired_app_has_its_objects_deleted() {
    let api = MockServer::start().await;
    for api_path in [DEPLOYMENT, SERVICE, SECRET, POLICY] {
        Mock::given(method("DELETE"))
            .and(path(api_path.to_owned()))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "kind": "Status" })))
            .mount(&api)
            .await;
    }

    let outcome = converger(&api)
        .converge_one(&app("retired", Some(DIGEST)))
        .await
        .expect("the app converges");
    assert_eq!(outcome, Outcome::Deleted);

    let requests = api
        .received_requests()
        .await
        .expect("the stub recorded them");
    only_the_cluster_was_called(&requests);
    let deleted: Vec<&str> = requests
        .iter()
        .filter(|request| request.method.as_str() == "DELETE")
        .map(|request| request.url.path())
        .collect();
    // In the project's namespace only: the Portal's own holds no App object (AP-116, T-4798).
    assert_eq!(deleted.len(), 4, "{deleted:?}");
    for api_path in [DEPLOYMENT, SERVICE, SECRET, POLICY] {
        assert!(deleted.contains(&api_path), "{api_path} was not deleted");
    }
}

/// A retirement that failed halfway is re-run without a special case (CC-18).
#[tokio::test]
async fn deleting_an_object_that_is_already_gone_succeeds() {
    let api = MockServer::start().await;
    Mock::given(method("DELETE"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "kind": "Status", "reason": "NotFound", "message": "not found",
        })))
        .mount(&api)
        .await;

    let outcome = converger(&api)
        .converge_one(&app("retired", None))
        .await
        .expect("removing what is not there is not a failure");
    assert_eq!(outcome, Outcome::Deleted);
}

/// AP-18: a draft renders no pod, and the cluster is not called at all.
#[tokio::test]
async fn a_draft_app_deploys_nothing_and_asks_the_cluster_nothing() {
    let api = MockServer::start().await;

    let outcome = converger(&api)
        .converge_one(&app("draft", Some(DIGEST)))
        .await
        .expect("a draft is not an error");
    assert!(matches!(outcome, Outcome::Skipped(_)), "{outcome:?}");
    assert!(api
        .received_requests()
        .await
        .is_some_and(|requests| requests.is_empty()));
}

/// AP-13a: an app whose build lane has published no artifact yet waits, rather than deploying
/// whatever ran last time.
#[tokio::test]
async fn an_app_whose_build_published_nothing_yet_waits_for_it() {
    let api = MockServer::start().await;

    let outcome = converger(&api)
        .converge_one(&app("published", None))
        .await
        .expect("a missing image is not an error");
    match outcome {
        Outcome::Skipped(reason) => assert!(reason.contains("status.build"), "{reason}"),
        other => panic!("{other:?}"),
    }
    assert!(api
        .received_requests()
        .await
        .is_some_and(|requests| requests.is_empty()));
}

/// AP-108: without a registry setting no image reference can be composed, so the app waits and
/// the cluster is asked nothing; a manifest never supplies the registry.
#[tokio::test]
async fn an_installation_without_a_registry_deploys_no_pod() {
    let api = MockServer::start().await;
    let mut settings = settings();
    settings.image_repository = None;
    let converger = Converger::new(
        KubeClient::with_token(&api.uri(), TOKEN).expect("a client"),
        settings,
    );

    match converger
        .converge_one(&app("published", Some(DIGEST)))
        .await
    {
        Ok(Outcome::Skipped(reason)) => {
            assert!(reason.contains("JC_PORTAL_APPS_REGISTRY"), "{reason}")
        }
        other => panic!("{other:?}"),
    }
    assert!(api
        .received_requests()
        .await
        .is_some_and(|requests| requests.is_empty()));
}

/// AP-13: the Portal addresses four kinds in one namespace. A request for anything else is
/// refused here, before RBAC is ever asked, so the two controls are independent.
#[tokio::test]
async fn the_portal_cannot_address_a_kind_an_app_does_not_own() {
    let api = MockServer::start().await;
    let client = KubeClient::with_token(&api.uri(), TOKEN).expect("a client");

    let refused = client
        .apply(&json!({
            "apiVersion": "rbac.authorization.k8s.io/v1",
            "kind": "ClusterRoleBinding",
            "metadata": { "name": "portal-admin", "namespace": NAMESPACE },
        }))
        .await;
    assert!(refused.is_err(), "a ClusterRoleBinding was addressable");
    assert!(api
        .received_requests()
        .await
        .is_some_and(|requests| requests.is_empty()));
}

/// A name is a path segment, so a name that is not a DNS label never becomes one.
#[tokio::test]
async fn a_name_that_is_not_a_dns_label_never_becomes_a_path() {
    let api = MockServer::start().await;
    let client = KubeClient::with_token(&api.uri(), TOKEN).expect("a client");

    let refused = client
        .apply(&json!({
            "apiVersion": "v1",
            "kind": "Secret",
            "metadata": { "name": "../../secrets/kubernetes-admin", "namespace": NAMESPACE },
        }))
        .await;
    assert!(refused.is_err(), "a traversal reached the API server");
    assert!(api
        .received_requests()
        .await
        .is_some_and(|requests| requests.is_empty()));
}

/// T-2632: the slug the door committed in the App's Endpoint is the one the gateway routes, so
/// the pod is deployed on it even where an older Secret holds another.
#[tokio::test]
async fn the_pod_calls_the_slug_its_committed_endpoint_carries() {
    let api = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(SECRET))
        .respond_with(ResponseTemplate::new(200).set_body_json(stored_secret()))
        .mount(&api)
        .await;
    for api_path in [DEPLOYMENT, SERVICE, SECRET, POLICY] {
        accepts_apply(&api, api_path).await;
    }

    let root =
        std::env::temp_dir().join(format!("jc-app-converge-committed-{}", std::process::id()));
    let space = root.join("projects/ovzdusie/spaces/ovzdusie");
    std::fs::create_dir_all(space.join("endpoints")).expect("a scratch repository");
    let endpoint = |name: &str, annotations: &str| {
        format!(
            "apiVersion: joinedcontext.com/v1alpha1\nkind: Endpoint\nmetadata:\n  name: {name}\n  namespace: ovzdusie\n{annotations}spec:\n  slug: committedslugofthegeneratedendpoint\n"
        )
    };
    std::fs::write(
        space.join("endpoints/app-air-quality-today.yaml"),
        endpoint(
            "app-air-quality-today",
            "  annotations:\n    joinedcontext.com/generated-by: portal/app-reconciler\n",
        ),
    )
    .expect("the committed endpoint");
    // An Endpoint somebody wrote by hand under another App's name is not that App's.
    std::fs::write(
        space.join("endpoints/app-other.yaml"),
        endpoint("app-other", ""),
    )
    .expect("a hand-written endpoint");
    let repository = jcctl::loader::Repository::load(&root).expect("the repository loads");
    let mut other = app("published", Some(DIGEST));
    other.metadata.name = "other".to_owned();
    assert_eq!(
        joinedcontext_portal::apps::converge::committed_slug(&repository, &other),
        None
    );
    let committed = joinedcontext_portal::apps::converge::committed_slug(
        &repository,
        &app("published", Some(DIGEST)),
    )
    .expect("the committed endpoint's slug");
    let _ = std::fs::remove_dir_all(&root);

    converger(&api)
        .converge_with(&app("published", Some(DIGEST)), Some(&committed))
        .await
        .expect("the app converges");
    let requests = api.received_requests().await.expect("recorded");
    let secret = body_of(applied(&requests, SECRET));
    assert_eq!(
        secret["stringData"]["endpoint-slug"],
        json!("committedslugofthegeneratedendpoint")
    );
    let deployment = body_of(applied(&requests, DEPLOYMENT)).to_string();
    assert!(
        deployment.contains("committedslugofthegeneratedendpoint"),
        "{deployment}"
    );
    assert!(!deployment.contains(SLUG), "not the Secret's older slug");
}
