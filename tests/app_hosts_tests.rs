//! The host of every published App, through the API server (T-2838, ADR-N-037, AP-133): a
//! certificate and an edge Ingress are created once when an App is published, never requested
//! again on a reconcile, removed when it is retired, and an object of that name the Portal did
//! not make is never touched.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use joinedcontext_portal::apps::kube::KubeClient;
use joinedcontext_portal::reconciler::app_hosts::{AppHosts, HostState};

const NS: &str = "apisix";
const CERTS: &str = "/apis/cert-manager.io/v1/namespaces/apisix/certificates";
const INGRESSES: &str = "/apis/networking.k8s.io/v1/namespaces/apisix/ingresses";

fn not_found() -> ResponseTemplate {
    ResponseTemplate::new(404).set_body_json(json!({ "kind": "Status", "code": 404 }))
}

fn managed(kind: &str, app: &str, status: Value) -> Value {
    json!({
        "apiVersion": if kind == "Certificate" { "cert-manager.io/v1" } else { "networking.k8s.io/v1" },
        "kind": kind,
        "metadata": {
            "name": format!("app-{app}"),
            "namespace": NS,
            "labels": {
                "app.kubernetes.io/managed-by": "joinedcontext-portal",
                "app.kubernetes.io/component": "app-host",
                "joinedcontext.com/app": app,
            },
        },
        "status": status,
    })
}

/// An API server holding the edge's Ingress and Certificate, and `lists` as what the two list
/// calls answer.
async fn api(listed_certificates: Vec<Value>, listed_ingresses: Vec<Value>) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("{INGRESSES}/apisix")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "metadata": { "name": "apisix", "annotations": { "cert-manager.io/cluster-issuer": "x" } },
            "spec": {
                "ingressClassName": "traefik",
                "rules": [{ "host": "city.example", "http": { "paths": [{
                    "path": "/", "pathType": "Prefix",
                    "backend": { "service": { "name": "apisix-gateway", "port": { "number": 80 } } },
                }] } }],
            },
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{CERTS}/apisix-edge")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "spec": { "issuerRef": { "name": "letsencrypt-prod", "kind": "ClusterIssuer" } },
        })))
        .mount(&server)
        .await;
    for (collection, items) in [(CERTS, listed_certificates), (INGRESSES, listed_ingresses)] {
        Mock::given(method("GET"))
            .and(path(collection))
            .and(query_param(
                "labelSelector",
                "app.kubernetes.io/managed-by=joinedcontext-portal,app.kubernetes.io/component=app-host",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "items": items })))
            .mount(&server)
            .await;
    }
    server
}

fn hosts(server: &MockServer) -> AppHosts {
    AppHosts::new(
        KubeClient::with_token(&server.uri(), "token").expect("a client"),
        NS.to_owned(),
    )
}

fn published(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

async fn posted(server: &MockServer, collection: &str) -> Vec<Value> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path() == collection)
        .map(|r| serde_json::from_slice(&r.body).expect("a JSON body"))
        .collect()
}

async fn deleted(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.method.as_str() == "DELETE")
        .map(|r| r.url.path().to_owned())
        .collect()
}

/// AP-133: a newly published App gets its certificate from the edge's issuer and its Ingress,
/// and reads pending until cert-manager has issued it.
#[tokio::test]
async fn a_published_app_gets_its_certificate_and_ingress_once() {
    let server = api(vec![], vec![]).await;
    for collection in [CERTS, INGRESSES] {
        Mock::given(method("GET"))
            .and(path(format!("{collection}/app-bikes")))
            .respond_with(not_found())
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(collection))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
            .expect(1)
            .mount(&server)
            .await;
    }

    let states = hosts(&server)
        .converge(&published(&["bikes"]), "city.example")
        .await;
    assert!(
        matches!(states.get("bikes"), Some(HostState::Pending(_))),
        "{states:?}"
    );
    let certificate = &posted(&server, CERTS).await[0];
    assert_eq!(
        certificate["spec"]["dnsNames"],
        json!(["bikes.apps.city.example"])
    );
    assert_eq!(certificate["spec"]["issuerRef"]["name"], "letsencrypt-prod");
    let ingress = &posted(&server, INGRESSES).await[0];
    assert_eq!(
        ingress["spec"]["rules"][0]["host"],
        "bikes.apps.city.example"
    );
    assert_eq!(
        ingress["spec"]["rules"][0]["http"]["paths"][0]["backend"]["service"]["name"],
        "apisix-gateway"
    );
    assert!(deleted(&server).await.is_empty());
}

/// A second run requests nothing: the certificate exists, and what cert-manager says of it is
/// the App's state (50 certificates a week per registered domain).
#[tokio::test]
async fn a_second_reconcile_requests_no_certificate() {
    let issued = managed(
        "Certificate",
        "bikes",
        json!({ "conditions": [{ "type": "Ready", "status": "True" }] }),
    );
    let server = api(
        vec![issued.clone()],
        vec![managed("Ingress", "bikes", json!({}))],
    )
    .await;
    Mock::given(method("GET"))
        .and(path(format!("{CERTS}/app-bikes")))
        .respond_with(ResponseTemplate::new(200).set_body_json(issued))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{INGRESSES}/app-bikes")))
        .respond_with(ResponseTemplate::new(200).set_body_json(managed(
            "Ingress",
            "bikes",
            json!({}),
        )))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(201))
        .expect(0)
        .mount(&server)
        .await;

    let states = hosts(&server)
        .converge(&published(&["bikes"]), "city.example")
        .await;
    assert_eq!(states.get("bikes"), Some(&HostState::Ready));
    assert!(deleted(&server).await.is_empty());
}

/// Retirement removes both objects of an App no longer published, and nothing of an App that
/// still is.
#[tokio::test]
async fn a_retired_app_loses_its_certificate_and_ingress() {
    let server = api(
        vec![managed("Certificate", "old", json!({}))],
        vec![managed("Ingress", "old", json!({}))],
    )
    .await;
    for collection in [CERTS, INGRESSES] {
        Mock::given(method("DELETE"))
            .and(path(format!("{collection}/app-old")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(1)
            .mount(&server)
            .await;
    }
    let hosts = hosts(&server);
    let states = hosts.converge(&BTreeSet::new(), "city.example").await;
    assert!(states.is_empty());
    assert!(
        deleted(&server).await.is_empty(),
        "converging creates; only retire removes"
    );
    hosts.retire(&BTreeSet::new()).await;
    let mut gone = deleted(&server).await;
    gone.sort();
    assert_eq!(
        gone,
        [format!("{CERTS}/app-old"), format!("{INGRESSES}/app-old")]
    );
}

/// An object of the App's name that the Portal did not create is reported and left alone.
#[tokio::test]
async fn a_certificate_somebody_else_made_is_never_touched() {
    let server = api(vec![], vec![]).await;
    Mock::given(method("GET"))
        .and(path(format!("{CERTS}/app-bikes")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "metadata": { "name": "app-bikes" },
            "status": { "conditions": [{ "type": "Ready", "status": "True" }] },
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(201))
        .expect(0)
        .mount(&server)
        .await;

    let states = hosts(&server)
        .converge(&published(&["bikes"]), "city.example")
        .await;
    let Some(HostState::Failed(reason)) = states.get("bikes") else {
        panic!("{states:?}");
    };
    assert!(reason.contains("did not create"), "{reason}");
    assert!(deleted(&server).await.is_empty());
}

/// Without the edge's own Ingress there is nothing to copy: every App says why, and nothing is
/// requested.
#[tokio::test]
async fn no_edge_ingress_requests_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(not_found())
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(201))
        .expect(0)
        .mount(&server)
        .await;
    let states = hosts(&server)
        .converge(&published(&["bikes"]), "city.example")
        .await;
    let Some(HostState::Failed(reason)) = states.get("bikes") else {
        panic!("{states:?}");
    };
    assert!(reason.contains("edge Ingress apisix"), "{reason}");
}

/// ADR-N-037 §6 (T-3013): where the wildcard certificate is issued and the edge Ingress carries
/// it, a published App is ready at once and nothing of its own is requested.
#[tokio::test]
async fn an_issued_wildcard_serves_every_app_host_with_nothing_requested() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("{INGRESSES}/apisix")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "metadata": { "name": "apisix" },
            "spec": {
                "rules": [{ "host": "city.example", "http": { "paths": [{
                    "path": "/", "pathType": "Prefix",
                    "backend": { "service": { "name": "apisix-gateway", "port": { "number": 80 } } },
                }] } }],
                "tls": [
                    { "hosts": ["city.example"], "secretName": "apisix-edge-tls" },
                    { "hosts": ["*.apps.city.example"], "secretName": "apisix-apps-wildcard-tls" },
                ],
            },
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{CERTS}/apisix-apps-wildcard")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "spec": { "secretName": "apisix-apps-wildcard-tls", "dnsNames": ["*.apps.city.example"] },
            "status": { "conditions": [{ "type": "Ready", "status": "True" }] },
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(201))
        .expect(0)
        .mount(&server)
        .await;

    let states = hosts(&server)
        .converge(&published(&["bikes", "never-seen"]), "city.example")
        .await;
    assert_eq!(states.get("bikes"), Some(&HostState::Ready), "{states:?}");
    assert_eq!(
        states.get("never-seen"),
        Some(&HostState::Ready),
        "{states:?}"
    );
    let asked: Vec<String> = server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| r.url.path().to_owned())
        .collect();
    assert!(
        !asked
            .iter()
            .any(|p| p.ends_with("/app-bikes") || p.ends_with("/app-never-seen")),
        "no App object is read or made: {asked:?}"
    );
}

/// A wildcard that is not issued yet changes nothing: the App gets its own certificate and
/// Ingress as before, so a DNS-01 order stuck on the delegation never leaves a host unserved.
#[tokio::test]
async fn a_wildcard_not_issued_yet_leaves_the_app_its_own_certificate() {
    let server = api(vec![], vec![]).await;
    Mock::given(method("GET"))
        .and(path(format!("{CERTS}/apisix-apps-wildcard")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "spec": { "secretName": "apisix-apps-wildcard-tls", "dnsNames": ["*.apps.city.example"] },
            "status": { "conditions": [{ "type": "Ready", "status": "False", "message": "Waiting for DNS-01" }] },
        })))
        .mount(&server)
        .await;
    for collection in [CERTS, INGRESSES] {
        Mock::given(method("GET"))
            .and(path(format!("{collection}/app-bikes")))
            .respond_with(not_found())
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(collection))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
            .expect(1)
            .mount(&server)
            .await;
    }
    let states = hosts(&server)
        .converge(&published(&["bikes"]), "city.example")
        .await;
    assert!(
        matches!(states.get("bikes"), Some(HostState::Pending(_))),
        "{states:?}"
    );
}
