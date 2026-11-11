//! T-6769 (ADR-N-068): every entity URN names the Organization owning the space it is written
//! to, `urn:ngsi-ld:{Type}:{orgId}:...`. A create or upsert naming another owner, or none, is
//! refused `422` naming `id` before the broker is asked; the URN grants nothing, so a write the
//! Policy refuses stays a Policy refusal whatever owner it names (TEN-02).

use axum::body::Body;
use axum::extract::Request;
use axum::http::{Method, StatusCode};
use axum::routing::any;
use axum::Router;
use context_gateway::app::{router, Gateway};
use context_gateway::pdp::PolicyPdp;
use context_gateway::proxy::Broker;
use context_gateway::resolver::Endpoint;
use context_gateway::store::Provenance;
use jc_core::kinds::{Audience, PolicySpec, Representation};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

const SLUG: &str = "bbsk-kpi";
const SPACE: &str = "otvorene-data";
const PROJECT: &str = "bbsk-otvorene-data";

type Hops = Arc<Mutex<Vec<String>>>;

async fn broker() -> (String, Hops) {
    let hops: Hops = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&hops);
    let app = Router::new().fallback(any(move |request: Request| {
        let recorder = Arc::clone(&recorder);
        async move {
            recorder.lock().expect("the hop log").push(format!(
                "{} {}",
                request.method(),
                request.uri().path()
            ));
            (
                StatusCode::CREATED,
                [("content-type", "application/json")],
                String::new(),
            )
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a free port");
    let address = listener.local_addr().expect("the bound address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{address}"), hops)
}

/// `role` may create and upsert devices of this space.
fn policy(role: &str) -> PolicySpec {
    serde_norway::from_str(&format!(
        "contextSpaceRef: {SPACE}\n\
         assigner: did:web:bbsk.sk\n\
         assignee: {{ kind: role, id: {role} }}\n\
         operations: [createEntity, upsertBatch, createBatch]\n\
         information:\n\
         \x20 - entities:\n\
         \x20     - type: Device\n"
    ))
    .expect("the policy spec parses")
}

fn endpoint(project: &str, role: &str) -> Endpoint {
    Endpoint {
        declared_types: None,
        roles: Default::default(),
        slug: SLUG.to_owned(),
        title: BTreeMap::new(),
        description: BTreeMap::new(),
        space: SPACE.to_owned(),
        project: project.to_owned(),
        audience: Audience::Public,
        allowed_projects: Vec::new(),
        representations: vec![Representation::NgsiLd],
        rate_limit: None,
        creates: None,
        file_limits: None,
        hidden_attributes: Default::default(),
        projection: None,
        view_mapping: None,
        catalog: None,
        deprecation: None,
        feeds: Vec::new(),
        calendars: Vec::new(),
        map_defaults: Default::default(),
        page: None,
        base_path: format!("/api/endpoint/{SLUG}"),
        models: Vec::new(),
        policy_names: Vec::new(),
        manifest_name: String::new(),
        policies: vec![policy(role)],
    }
}

/// The deployment: the Organizations by name and domain, which owns which project, and the
/// operator's `JC_GATEWAY_URN_OWNER_PREFIX`.
struct Deployment {
    organizations: &'static [(&'static str, &'static str)],
    projects: &'static [(&'static str, &'static str)],
    check: bool,
}

const FOUR: Deployment = Deployment {
    organizations: &[
        ("bbsk", "bbsk.sk"),
        ("bratislava", "bratislava.sk"),
        ("praha", "praha.eu"),
        ("hel", "hel.fi"),
    ],
    projects: &[(PROJECT, "bbsk"), ("ba-data", "bratislava")],
    check: true,
};

async fn send(
    deployment: &Deployment,
    endpoint: Endpoint,
    uri: &str,
    body: Value,
) -> (StatusCode, Value, Vec<String>) {
    let (upstream, hops) = broker().await;
    let mut gateway = Gateway::new(Broker::new(upstream), Box::new(PolicyPdp), "bbsk.sk");
    gateway.urn_owner_prefix = deployment.check;
    let gateway = gateway.serve([endpoint]);
    gateway.replace_provenance(Provenance {
        organizations: deployment
            .projects
            .iter()
            .map(|(project, organization)| ((*project).to_owned(), (*organization).to_owned()))
            .collect(),
        domains: deployment
            .organizations
            .iter()
            .map(|(name, domain)| ((*name).to_owned(), (*domain).to_owned()))
            .collect(),
        ..Default::default()
    });
    let response = router(Arc::new(gateway))
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/endpoint/{SLUG}/ngsi-ld/v1{uri}"))
                .header("content-type", "application/ld+json")
                .body(Body::from(body.to_string()))
                .expect("a request"),
        )
        .await
        .expect("the gateway answers");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("a body");
    let problem = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let asked = hops.lock().expect("the hop log").clone();
    (status, problem, asked)
}

fn device(id: &str) -> Value {
    json!({
        "id": id,
        "type": "Device",
        "status": { "type": "Property", "value": "on" },
        "@context": "https://uri.etsi.org/ngsi-ld/v1/ngsi-ld-core-context-v1.8.jsonld"
    })
}

fn assert_refused_on_id(status: StatusCode, problem: &Value, asked: &[String], what: &str) {
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{what}: {problem}"
    );
    // The NGSI-LD surface answers in CIM 009's types and keeps the platform's slug in `code`.
    assert_eq!(problem["code"], "urn-owner", "{what}: {problem}");
    assert_eq!(problem["field"], "id", "{what}: {problem}");
    assert!(
        problem["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("sk_bbsk"),
        "{what}: the refusal says which owner to write: {problem}"
    );
    assert!(asked.is_empty(), "{what}: the broker was asked: {asked:?}");
}

/// A URN naming Bratislava, or no Organization, written into a space of BBSK is refused 422 on
/// `id`, a single create and a batch alike, and the broker is never asked.
#[tokio::test]
async fn a_foreign_or_missing_owner_is_refused_on_the_id() {
    for (uri, body, what) in [
        (
            "/entities",
            device("urn:ngsi-ld:Device:sk_bratislava:1"),
            "foreign owner",
        ),
        ("/entities", device("urn:ngsi-ld:Device:lamp-1"), "no owner"),
        (
            "/entities",
            device("urn:ngsi-ld:Device:bratislava.sk:x:1"),
            "foreign domain",
        ),
        (
            "/entityOperations/upsert",
            json!([
                device("urn:ngsi-ld:Device:sk_bbsk:1"),
                device("urn:ngsi-ld:Device:sk_bratislava:2")
            ]),
            "a batch with one foreign member",
        ),
    ] {
        let (status, problem, asked) = send(&FOUR, endpoint(PROJECT, "public"), uri, body).await;
        assert_refused_on_id(status, &problem, &asked, what);
    }
}

/// The owner's own id passes, and so does its domain, the form minted before the id existed.
#[tokio::test]
async fn the_owners_id_and_its_domain_pass() {
    for id in [
        "urn:ngsi-ld:Device:sk_bbsk:bbsk-otvorene-data:lamp-1",
        "urn:ngsi-ld:Device:bbsk.sk:otvorene-data:lamp-1",
    ] {
        let (status, problem, asked) =
            send(&FOUR, endpoint(PROJECT, "public"), "/entities", device(id)).await;
        assert_eq!(status, StatusCode::CREATED, "{id}: {problem}");
        assert_eq!(asked.len(), 1, "{id}: {asked:?}");
    }
}

/// The URN grants nothing (TEN-02): a write into Bratislava's space that the Policy does not
/// grant the caller is a Policy refusal, whether its URN names BBSK, Bratislava or nobody, and
/// never reaches the broker because a URN names someone.
#[tokio::test]
async fn a_urn_never_authorizes_a_write_into_another_organizations_space() {
    for id in [
        "urn:ngsi-ld:Device:sk_bbsk:1",
        "urn:ngsi-ld:Device:sk_bratislava:1",
        "urn:ngsi-ld:Device:1",
    ] {
        let (status, problem, asked) = send(
            &FOUR,
            endpoint("ba-data", "editor"),
            "/entities",
            device(id),
        )
        .await;
        assert!(
            matches!(
                status,
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
            ),
            "{id}: {status} {problem}"
        );
        assert!(asked.is_empty(), "{id}: the broker was asked: {asked:?}");
    }
}

/// On a deployment of one Organization the operator may switch the check off: a foreign or
/// missing owner is then accepted. Switched on, the single Organization's id is required, and
/// with several Organizations `off` does not hold.
#[tokio::test]
async fn a_single_deployment_may_switch_the_check_off_and_several_may_not() {
    let single_off = Deployment {
        organizations: &[("bbsk", "bbsk.sk")],
        projects: &[(PROJECT, "bbsk")],
        check: false,
    };
    for id in [
        "urn:ngsi-ld:Device:sk_bratislava:1",
        "urn:ngsi-ld:Device:lamp-1",
    ] {
        let (status, problem, asked) = send(
            &single_off,
            endpoint(PROJECT, "public"),
            "/entities",
            device(id),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{id}: {problem}");
        assert_eq!(asked.len(), 1);
    }
    let single_on = Deployment {
        check: true,
        ..single_off
    };
    let (status, problem, asked) = send(
        &single_on,
        endpoint(PROJECT, "public"),
        "/entities",
        device("urn:ngsi-ld:Device:lamp-1"),
    )
    .await;
    assert_refused_on_id(status, &problem, &asked, "one Organization, check on");
    let several_off = Deployment {
        check: false,
        ..FOUR
    };
    let (status, problem, asked) = send(
        &several_off,
        endpoint(PROJECT, "public"),
        "/entities",
        device("urn:ngsi-ld:Device:sk_bratislava:1"),
    )
    .await;
    assert_refused_on_id(status, &problem, &asked, "several Organizations, check off");
}
