//! T-5921: the load generator's verified token reads (its class reads only), counted in its
//! own share; the write refusal and the shares are the `in_flight_tests` binary's.

use crate::common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use context_gateway::app::{router, Gateway};
use context_gateway::auth::accounts::{accounts_of, ServiceAccounts};
use context_gateway::pdp::PolicyPdp;
use context_gateway::proxy::Broker;
use context_gateway::resolver::Endpoint;
use jc_core::kinds::{Audience, PolicySpec, Representation};
use serde_json::json;
use std::sync::Arc;
use tower::ServiceExt;

const SLUG: &str = "k7r2m4xq9vbn3tdw6hcy5pajfe";
const SPACE: &str = "perf";
const DOMAIN: &str = "banskabystrica.sk";
/// The client of the load generator's ServiceAccount, `perf/loadtest`.
const LOADTEST: &str = "perf-loadtest";

fn endpoint() -> Endpoint {
    let policy: PolicySpec = serde_norway::from_str(&format!(
        "contextSpaceRef: {SPACE}\n\
         assigner: did:web:{DOMAIN}\n\
         assignee: {{ kind: role, id: public }}\n\
         operations: [queryEntity]\n\
         information:\n\
         \x20 - entities:\n\
         \x20     - type: AirQualityObserved\n"
    ))
    .expect("the policy spec parses");
    Endpoint {
        declared_types: None,
        catalog: None,
        deprecation: None,
        feeds: Vec::new(),
        calendars: Vec::new(),
        map_defaults: Default::default(),
        page: None,
        open_data: false,
        roles: Default::default(),
        slug: SLUG.to_owned(),
        title: Default::default(),
        description: Default::default(),
        space: SPACE.to_owned(),
        tenant: context_gateway::middleware::tenancy::Tenant::segment(SPACE).ok(),
        project: SPACE.to_owned(),
        audience: Audience::Public,
        allowed_projects: Vec::new(),
        representations: vec![Representation::NgsiLd],
        rate_limit: None,
        creates: None,
        file_limits: None,
        hidden_attributes: Default::default(),
        projection: None,
        view_mapping: None,
        base_path: format!("/api/endpoint/{SLUG}"),
        models: Vec::new(),
        policy_names: Vec::new(),
        manifest_name: String::new(),
        policies: vec![policy],
    }
}

/// The identity table of a repository holding the ServiceAccount `loadtest` of project `perf`.
fn loadtest_account() -> ServiceAccounts {
    let dir = std::env::temp_dir().join(format!(
        "gw-it-loadtest-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let space = dir.join("projects/perf/spaces/perf");
    std::fs::create_dir_all(&space).expect("a space directory");
    std::fs::write(
        space.join("space.yaml"),
        "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: perf\n  namespace: perf\nspec:\n  urnSegment: perf\n",
    )
    .expect("the space");
    let accounts = dir.join("projects/perf/access/serviceaccounts");
    std::fs::create_dir_all(&accounts).expect("an account directory");
    std::fs::write(
        accounts.join("loadtest.yaml"),
        "apiVersion: joinedcontext.com/v1alpha1\nkind: ServiceAccount\nmetadata:\n  name: loadtest\n  namespace: perf\nspec:\n  owner:\n    user: demo.steward\n  purpose: \"load runs\"\n  roles:\n    - role: public\n      scope: { contextSpace: perf }\n  credentials:\n    - kind: oauth-client\n      name: main\n",
    )
    .expect("the account");
    let table =
        accounts_of(&jc_manifest::loader::Repository::load(&dir).expect("the repository loads"));
    let _ = std::fs::remove_dir_all(&dir);
    table
}

/// T-5921: a read with the load generator's verified token is answered.
#[tokio::test]
async fn a_loadtest_read_is_answered() {
    let broker = common::BrokerStub::start(vec![json!([])]).await;
    let realm = common::Realm::new();
    let app = router(Arc::new(
        Gateway::new(Broker::new(&broker.url), Box::new(PolicyPdp), DOMAIN)
            .serve_loadtest(Some(LOADTEST.to_owned()), 8)
            .serve([endpoint()])
            .authenticate(Arc::new(realm.verifier()), loadtest_account(), None),
    ));
    let token = realm.workload_token(LOADTEST, json!(SLUG));
    let read = Request::builder()
        .uri(format!(
            "/api/endpoint/{SLUG}/ngsi-ld/v1/entities?type=AirQualityObserved"
        ))
        .header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .expect("a request");
    let answer = app.oneshot(read).await.expect("an answer");
    assert_eq!(answer.status(), StatusCode::OK);
}
