//! Every ServiceAccount bound to a workload gets a federated Keycloak client (PF-47, T-1513).
//!
//! Against a mocked Keycloak admin API: an account with `spec.workload.kubernetes` gets its
//! derived client with `federated-jwt` authentication for its one Kubernetes subject,
//! `client_credentials` only, and audience mappers for the Endpoints of the spaces its roles
//! scope, plus the Portal's audience when a role names a `Role`; no secret is ever read. An
//! account without a workload gets nothing; a console change is written back; a client of that id
//! this platform did not create is never touched; a managed client whose account lost its
//! workload is deleted; and one Kubernetes subject opens one account at most, the first to hold
//! it. A `wasm` App with jobs gets its job principal's client, its one audience the App's own
//! Endpoint, and a Kubernetes ServiceAccount in the identities namespace (AP-159, T-3539). An
//! account without a workload whose `oauth-client` credential is used from outside gets the same
//! client with `client-secret`, or `client-jwt` against its device keys (T-4015, T-5626); the
//! platform's own client of such an account is left alone without a failure.

use jc_core::kinds::service_account::KubernetesBinding;
use joinedcontext_portal::config::ClientAuth;
use joinedcontext_portal::reconciler::app_clients::{audience_mapper, groups_mapper};
use joinedcontext_portal::reconciler::groups::{MANAGED_BY, MANAGED_VALUE};
use joinedcontext_portal::reconciler::streams::Bentos;
use joinedcontext_portal::reconciler::workload_clients::{
    desired, desired_external, ClientCredential, WorkloadClientSync,
};
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, path_regex, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const REALM: &str = "/admin/realms/bb";
const SLUG: &str = "k7m2qz4tv6xh3n5jb2ryd3wcfa";

fn envelope(kind: &str, project: &str, name: &str, spec: Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: kind.to_owned(),
        metadata: ObjectMeta::new(name, project),
        spec,
        status: None,
    }
}

/// A ServiceAccount of `project` writing into space `air`, bound to `namespace/sa` when given.
fn account(
    project: &str,
    name: &str,
    workload: Option<(&str, &str)>,
    role: &str,
) -> ResourceEnvelope {
    let mut spec = json!({
        "owner": { "user": "jana.kovacova@hel.fi" },
        "purpose": "the air-quality adapter of the environment department",
        "roles": [{ "role": role, "scope": { "contextSpace": "air" } }],
        "credentials": [],
    });
    if let Some((namespace, sa)) = workload {
        spec["workload"] =
            json!({ "kubernetes": { "namespace": namespace, "serviceAccount": sa } });
    }
    envelope("ServiceAccount", project, name, spec)
}

fn endpoint(project: &str, name: &str, space: &str, slug: &str) -> ResourceEnvelope {
    envelope(
        "Endpoint",
        project,
        name,
        json!({ "contextSpaceRef": space, "slug": slug, "audience": "organization" }),
    )
}

fn binding(namespace: &str, sa: &str) -> KubernetesBinding {
    serde_json::from_value(json!({ "namespace": namespace, "serviceAccount": sa }))
        .expect("binding")
}

fn mirror_with(items: Vec<ResourceEnvelope>) -> Mirror {
    let mirror = Mirror::new();
    for item in items {
        mirror.upsert(item);
    }
    mirror
}

async fn realm(listed: Value) -> MockServer {
    let keycloak = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/realms/bb/protocol/openid-connect/token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "access_token": "admin-token", "expires_in": 60 })),
        )
        .mount(&keycloak)
        .await;
    // The full list the wave reads first: who holds a subject, and what is left to delete.
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/clients")))
        .and(query_param("briefRepresentation", "false"))
        .respond_with(ResponseTemplate::new(200).set_body_json(listed))
        .mount(&keycloak)
        .await;
    for verb in ["POST", "PUT", "DELETE"] {
        Mock::given(method(verb))
            .and(path_regex(format!(
                "^{REALM}/clients/[^/]+(/protocol-mappers/models.*|/(default|optional)-client-scopes/[^/]+)?$"
            )))
            .respond_with(ResponseTemplate::new(204))
            .with_priority(10)
            .mount(&keycloak)
            .await;
    }
    // A client created with `basic` alone holds it and nothing else (T-6051).
    client_scopes(
        &keycloak,
        "[^/]+",
        json!([{ "id": "s-basic", "name": "basic" }]),
        json!([]),
    )
    .await;
    keycloak
}

/// The default and optional scopes the realm answers for the client `uuid` (a regex), below any
/// test's own answer.
async fn client_scopes(keycloak: &MockServer, uuid: &str, default: Value, optional: Value) {
    for (kind, held) in [("default", default), ("optional", optional)] {
        Mock::given(method("GET"))
            .and(path_regex(format!(
                "^{REALM}/clients/{uuid}/{kind}-client-scopes$"
            )))
            .respond_with(ResponseTemplate::new(200).set_body_json(held))
            .with_priority(if uuid == "[^/]+" { 10 } else { 1 })
            .mount(keycloak)
            .await;
    }
}

fn sync(keycloak: &MockServer) -> WorkloadClientSync {
    WorkloadClientSync::new(
        &format!("{}/realms/bb", keycloak.uri()),
        "portal-api".to_owned(),
        ClientAuth::Secret("portal-api-credential".into()),
    )
    .expect("a realm issuer")
}

async fn client_lookup(keycloak: &MockServer, id: &str, found: Value) {
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/clients")))
        .and(query_param("clientId", id))
        .respond_with(ResponseTemplate::new(200).set_body_json(found))
        .mount(keycloak)
        .await;
}

async fn mappers(keycloak: &MockServer, uuid: &str, held: Value) {
    Mock::given(method("GET"))
        .and(path(format!(
            "{REALM}/clients/{uuid}/protocol-mappers/models"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(held))
        .mount(keycloak)
        .await;
}

/// What the run wrote, as `METHOD path`.
async fn wrote(keycloak: &MockServer) -> Vec<String> {
    keycloak
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.method.as_str() != "GET" && !r.url.path().ends_with("/token"))
        .map(|r| format!("{} {}", r.method.as_str(), r.url.path()))
        .collect()
}

/// The bodies the run sent to one `METHOD path`.
async fn bodies(keycloak: &MockServer, verb: &str, at: &str) -> Vec<Value> {
    keycloak
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.method.as_str() == verb && r.url.path() == at)
        .map(|r| serde_json::from_slice(&r.body).expect("json"))
        .collect()
}

fn managed_client(uuid: &str, project: &str, name: &str, namespace: &str, sa: &str) -> Value {
    let mut client = desired(project, name, &binding(namespace, sa));
    client["id"] = json!(uuid);
    client
}

/// T-4424: the realm behind the edge's rate limit answers 429 to the token and to the lookup
/// once each; the wave waits as asked and converges the client in the same run.
#[tokio::test]
async fn a_realm_that_answers_429_once_still_converges_the_client_in_one_run() {
    let keycloak = realm(json!([])).await;
    for (verb, at) in [
        (
            "POST",
            "/realms/bb/protocol/openid-connect/token".to_owned(),
        ),
        ("GET", format!("{REALM}/clients")),
    ] {
        Mock::given(method(verb))
            .and(path(at))
            .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "0"))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&keycloak)
            .await;
    }
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/clients")))
        .and(query_param("clientId", "helsinki-air-adapter"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .up_to_n_times(1)
        .mount(&keycloak)
        .await;
    client_lookup(
        &keycloak,
        "helsinki-air-adapter",
        json!([managed_client(
            "uuid-air",
            "helsinki",
            "air-adapter",
            "env-dept",
            "adapter"
        )]),
    )
    .await;
    Mock::given(method("POST"))
        .and(path(format!("{REALM}/clients")))
        .respond_with(ResponseTemplate::new(201))
        .mount(&keycloak)
        .await;
    mappers(&keycloak, "uuid-air", json!([])).await;

    let outcomes = sync(&keycloak)
        .converge(
            &mirror_with(vec![
                account(
                    "helsinki",
                    "air-adapter",
                    Some(("env-dept", "adapter")),
                    "space-writer",
                ),
                endpoint("helsinki", "air-all", "air", SLUG),
            ]),
            &Bentos::new(),
        )
        .await;

    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    assert_eq!(outcomes[0].error, None, "{outcomes:?}");
    assert_eq!(
        wrote(&keycloak).await,
        vec![
            format!("POST {REALM}/clients"),
            format!("POST {REALM}/clients/uuid-air/protocol-mappers/models"),
        ]
    );
}

/// PF-47: a workload-bound account gets a federated client for its one subject, audienced to the
/// Endpoints of the space its role scopes, and no secret is read or written.
#[tokio::test]
async fn a_bound_account_gets_a_federated_client_and_no_secret() {
    let keycloak = realm(json!([])).await;
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/clients")))
        .and(query_param("clientId", "helsinki-air-adapter"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .up_to_n_times(1)
        .mount(&keycloak)
        .await;
    client_lookup(
        &keycloak,
        "helsinki-air-adapter",
        json!([managed_client(
            "uuid-air",
            "helsinki",
            "air-adapter",
            "env-dept",
            "adapter"
        )]),
    )
    .await;
    Mock::given(method("POST"))
        .and(path(format!("{REALM}/clients")))
        .respond_with(ResponseTemplate::new(201))
        .mount(&keycloak)
        .await;
    mappers(&keycloak, "uuid-air", json!([])).await;

    let outcomes = sync(&keycloak)
        .converge(
            &mirror_with(vec![
                account(
                    "helsinki",
                    "air-adapter",
                    Some(("env-dept", "adapter")),
                    "space-writer",
                ),
                endpoint("helsinki", "air-all", "air", SLUG),
                // Another space's endpoint is no audience of this account.
                endpoint(
                    "helsinki",
                    "bikes-all",
                    "bikes",
                    "b2ryd3wcfak7m2qz4tv6xh3n5j",
                ),
            ]),
            &Bentos::new(),
        )
        .await;

    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    assert_eq!(outcomes[0].app, "helsinki/air-adapter");
    assert_eq!(outcomes[0].error, None, "{outcomes:?}");
    assert_eq!(
        wrote(&keycloak).await,
        vec![
            format!("POST {REALM}/clients"),
            format!("POST {REALM}/clients/uuid-air/protocol-mappers/models"),
        ]
    );
    let created = &bodies(&keycloak, "POST", &format!("{REALM}/clients")).await[0];
    assert_eq!(created["clientId"], "helsinki-air-adapter");
    assert_eq!(created["clientAuthenticatorType"], "federated-jwt");
    assert_eq!(created["attributes"]["jwt.credential.issuer"], "kubernetes");
    assert_eq!(
        created["attributes"]["jwt.credential.sub"],
        "system:serviceaccount:env-dept:adapter"
    );
    assert_eq!(created["attributes"][MANAGED_BY], MANAGED_VALUE);
    assert_eq!(created["serviceAccountsEnabled"], true);
    for flow in [
        "standardFlowEnabled",
        "implicitFlowEnabled",
        "directAccessGrantsEnabled",
        "publicClient",
    ] {
        assert_eq!(created[flow], false, "{flow}");
    }
    let mapper = &bodies(
        &keycloak,
        "POST",
        &format!("{REALM}/clients/uuid-air/protocol-mappers/models"),
    )
    .await[0];
    assert_eq!(mapper["config"]["included.custom.audience"], SLUG);
    let asked: Vec<String> = keycloak
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .map(|r| r.url.path().to_owned())
        .collect();
    assert!(
        !asked.iter().any(|p| p.ends_with("/client-secret")),
        "no secret is read: {asked:?}"
    );
}

/// PF-47: an account that names a `Role` acts on the Portal, so its token carries the Portal's
/// audience besides its endpoints'.
#[tokio::test]
async fn an_account_holding_a_portal_role_gets_the_portal_audience() {
    let keycloak = realm(json!([])).await;
    client_lookup(
        &keycloak,
        "helsinki-air-adapter",
        json!([managed_client(
            "uuid-air",
            "helsinki",
            "air-adapter",
            "env-dept",
            "adapter"
        )]),
    )
    .await;
    mappers(&keycloak, "uuid-air", json!([])).await;

    let outcomes = sync(&keycloak)
        .converge(
            &mirror_with(vec![
                account(
                    "helsinki",
                    "air-adapter",
                    Some(("env-dept", "adapter")),
                    "proposer",
                ),
                envelope(
                    "Role",
                    "org",
                    "proposer",
                    json!({ "rules": [{ "kinds": ["Pipeline"], "verbs": ["propose"] }] }),
                ),
                endpoint("helsinki", "air-all", "air", SLUG),
            ]),
            &Bentos::new(),
        )
        .await;

    assert_eq!(outcomes[0].error, None, "{outcomes:?}");
    let audiences: Vec<Value> = bodies(
        &keycloak,
        "POST",
        &format!("{REALM}/clients/uuid-air/protocol-mappers/models"),
    )
    .await
    .into_iter()
    .map(|m| m["config"]["included.custom.audience"].clone())
    .collect();
    assert_eq!(audiences, vec![json!(SLUG), json!("portal-api")]);
}

/// T-4038: the project's `pipelines` account is the identity its streams report outcomes,
/// rejections and samples with, so its token is for the Portal's internal listener as well; the
/// listener still admits it only for its own project's pipelines (T-3193).
#[tokio::test]
async fn a_projects_pipelines_account_gets_the_internal_listeners_audience() {
    let keycloak = realm(json!([])).await;
    client_lookup(
        &keycloak,
        "zilina-pipelines",
        json!([managed_client(
            "uuid-pl",
            "zilina",
            "pipelines",
            "pipeline-runner",
            "pipelines"
        )]),
    )
    .await;
    mappers(&keycloak, "uuid-pl", json!([])).await;

    let outcomes = sync(&keycloak)
        .converge(
            &mirror_with(vec![
                account(
                    "zilina",
                    "pipelines",
                    Some(("pipeline-runner", "pipelines")),
                    "space-writer",
                ),
                endpoint("zilina", "air-all", "air", SLUG),
            ]),
            &Bentos::new(),
        )
        .await;

    assert_eq!(outcomes[0].error, None, "{outcomes:?}");
    let audiences: Vec<Value> = bodies(
        &keycloak,
        "POST",
        &format!("{REALM}/clients/uuid-pl/protocol-mappers/models"),
    )
    .await
    .into_iter()
    .map(|m| m["config"]["included.custom.audience"].clone())
    .collect();
    assert_eq!(audiences, vec![json!(SLUG), json!("portal-internal")]);
}

/// PF-47: an account with no workload is not this wave's; nothing is written for it.
#[tokio::test]
async fn an_account_without_a_workload_gets_no_client() {
    let keycloak = realm(json!([])).await;

    let outcomes = sync(&keycloak)
        .converge(
            &mirror_with(vec![account(
                "helsinki",
                "vendor-push",
                None,
                "space-writer",
            )]),
            &Bentos::new(),
        )
        .await;

    assert!(outcomes.is_empty(), "{outcomes:?}");
    assert!(wrote(&keycloak).await.is_empty());
}

/// PF-47: a client switched to a secret in the console is written back to federated
/// authentication, and the drift names what changed.
#[tokio::test]
async fn a_console_change_is_written_back_and_reported() {
    let mut edited = managed_client("uuid-air", "helsinki", "air-adapter", "env-dept", "adapter");
    edited["clientAuthenticatorType"] = json!("client-secret");
    let keycloak = realm(json!([edited.clone()])).await;
    client_lookup(&keycloak, "helsinki-air-adapter", json!([edited])).await;
    let mut held = audience_mapper(SLUG);
    held["id"] = json!("m-1");
    mappers(&keycloak, "uuid-air", json!([held])).await;

    let outcomes = sync(&keycloak)
        .converge(
            &mirror_with(vec![
                account(
                    "helsinki",
                    "air-adapter",
                    Some(("env-dept", "adapter")),
                    "space-writer",
                ),
                endpoint("helsinki", "air-all", "air", SLUG),
            ]),
            &Bentos::new(),
        )
        .await;

    assert_eq!(outcomes[0].error, None, "{outcomes:?}");
    assert!(
        outcomes[0]
            .drift
            .iter()
            .any(|d| d.contains("clientAuthenticatorType")),
        "{outcomes:?}"
    );
    assert_eq!(
        wrote(&keycloak).await,
        vec![format!("PUT {REALM}/clients/uuid-air")]
    );
    let written = &bodies(&keycloak, "PUT", &format!("{REALM}/clients/uuid-air")).await[0];
    assert_eq!(written["clientAuthenticatorType"], "federated-jwt");
}

/// T-3033: only an App's client carries the platform's groups mapper. A workload's grants are its
/// manifest's (PF-35), so the same mapper on an account's client is removed like any other.
#[tokio::test]
async fn an_accounts_client_carries_no_groups_mapper() {
    let client = managed_client("uuid-air", "helsinki", "air-adapter", "env-dept", "adapter");
    let keycloak = realm(json!([client.clone()])).await;
    client_lookup(&keycloak, "helsinki-air-adapter", json!([client])).await;
    let mut audience = audience_mapper(SLUG);
    audience["id"] = json!("m-1");
    let mut groups = groups_mapper();
    groups["id"] = json!("m-groups");
    mappers(&keycloak, "uuid-air", json!([audience, groups])).await;

    let outcomes = sync(&keycloak)
        .converge(
            &mirror_with(vec![
                account(
                    "helsinki",
                    "air-adapter",
                    Some(("env-dept", "adapter")),
                    "space-writer",
                ),
                endpoint("helsinki", "air-all", "air", SLUG),
            ]),
            &Bentos::new(),
        )
        .await;

    assert_eq!(outcomes[0].error, None, "{outcomes:?}");
    assert_eq!(
        wrote(&keycloak).await,
        vec![format!(
            "DELETE {REALM}/clients/uuid-air/protocol-mappers/models/m-groups"
        )]
    );
    assert_eq!(
        outcomes[0].removed_mappers,
        vec!["oidc-group-membership-mapper mapper groups".to_owned()]
    );
}

/// PF-47: a client of the derived id this platform did not create is never touched, and the
/// account is reported.
#[tokio::test]
async fn a_client_the_platform_did_not_create_is_left_alone() {
    let foreign =
        json!({ "id": "uuid-hand", "clientId": "helsinki-air-adapter", "attributes": {} });
    let keycloak = realm(json!([foreign.clone()])).await;
    client_lookup(&keycloak, "helsinki-air-adapter", json!([foreign])).await;

    let outcomes = sync(&keycloak)
        .converge(
            &mirror_with(vec![account(
                "helsinki",
                "air-adapter",
                Some(("env-dept", "adapter")),
                "space-writer",
            )]),
            &Bentos::new(),
        )
        .await;

    let error = outcomes[0].error.as_deref().unwrap_or_default();
    assert!(error.contains("did not create"), "{outcomes:?}");
    assert!(wrote(&keycloak).await.is_empty());
}

/// PF-47: a managed client whose account no longer names a workload is deleted; a hand-made one
/// in the same list is not.
#[tokio::test]
async fn a_client_whose_account_lost_its_workload_is_deleted() {
    let keycloak = realm(json!([
        managed_client("uuid-air", "helsinki", "air-adapter", "env-dept", "adapter"),
        { "id": "uuid-hand", "clientId": "helsinki-pipelines", "attributes": {} },
    ]))
    .await;

    let outcomes = sync(&keycloak)
        .converge(
            &mirror_with(vec![account(
                "helsinki",
                "air-adapter",
                None,
                "space-writer",
            )]),
            &Bentos::new(),
        )
        .await;

    assert_eq!(
        wrote(&keycloak).await,
        vec![format!("DELETE {REALM}/clients/uuid-air")]
    );
    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    assert_eq!(outcomes[0].app, "helsinki/air-adapter");
    assert_eq!(outcomes[0].error, None);
}

/// T-5568, AR-6: the removed client's service-account user is revoked before the client goes, so
/// its unexpired tokens stop opening the gateway at its next poll; a failed lookup is a warning
/// and the client still goes.
#[tokio::test]
async fn a_removed_accounts_tokens_are_revoked_with_its_client() {
    let listed = json!([managed_client(
        "uuid-air",
        "helsinki",
        "air-adapter",
        "env-dept",
        "adapter"
    )]);
    let gone = mirror_with(vec![account(
        "helsinki",
        "air-adapter",
        None,
        "space-writer",
    )]);
    let revoked = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let into = std::sync::Arc::clone(&revoked);
    let revoke: joinedcontext_portal::reconciler::workload_clients::Revoke =
        std::sync::Arc::new(move |subject: String| {
            let into = std::sync::Arc::clone(&into);
            Box::pin(async move { into.lock().expect("the list").push(subject) })
        });

    let keycloak = realm(listed.clone()).await;
    Mock::given(method("GET"))
        .and(path(format!(
            "{REALM}/clients/uuid-air/service-account-user"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "sa-user-air" })))
        .mount(&keycloak)
        .await;
    let outcomes = sync(&keycloak)
        .with_revoke(std::sync::Arc::clone(&revoke))
        .converge(&gone, &Bentos::new())
        .await;
    assert_eq!(*revoked.lock().expect("the list"), ["sa-user-air"]);
    assert_eq!(
        wrote(&keycloak).await,
        vec![format!("DELETE {REALM}/clients/uuid-air")]
    );
    assert!(outcomes[0].warnings.is_empty(), "{outcomes:?}");

    // No service-account user to read: the client is removed all the same, and the outcome says
    // its tokens outlive it.
    let keycloak = realm(listed).await;
    let outcomes = sync(&keycloak)
        .with_revoke(revoke)
        .converge(&gone, &Bentos::new())
        .await;
    assert_eq!(
        wrote(&keycloak).await,
        vec![format!("DELETE {REALM}/clients/uuid-air")]
    );
    assert_eq!(outcomes[0].error, None);
    assert_eq!(outcomes[0].warnings.len(), 1, "{outcomes:?}");
    assert_eq!(revoked.lock().expect("the list").len(), 1);
}

/// PF-47: a second account binding a subject another account's client already holds gets no
/// client, so a later proposal can never take a pod's identity from the account that had it.
#[tokio::test]
async fn a_second_binding_of_a_held_subject_is_refused() {
    let holder = managed_client("uuid-air", "helsinki", "air-adapter", "env-dept", "adapter");
    let keycloak = realm(json!([holder.clone()])).await;
    client_lookup(&keycloak, "helsinki-air-adapter", json!([holder])).await;
    let mut held = audience_mapper(SLUG);
    held["id"] = json!("m-1");
    mappers(&keycloak, "uuid-air", json!([held])).await;

    let outcomes = sync(&keycloak)
        .converge(
            &mirror_with(vec![
                account(
                    "helsinki",
                    "air-adapter",
                    Some(("env-dept", "adapter")),
                    "space-writer",
                ),
                account(
                    "kosice",
                    "squatter",
                    Some(("env-dept", "adapter")),
                    "space-writer",
                ),
                endpoint("helsinki", "air-all", "air", SLUG),
            ]),
            &Bentos::new(),
        )
        .await;

    let of = |label: &str| outcomes.iter().find(|o| o.app == label).expect(label);
    assert_eq!(of("helsinki/air-adapter").error, None, "{outcomes:?}");
    let refused = of("kosice/squatter").error.as_deref().unwrap_or_default();
    assert!(
        refused.contains("already opens the client of helsinki/air-adapter"),
        "{refused}"
    );
    assert!(
        wrote(&keycloak).await.is_empty(),
        "nothing is created for the second binding"
    );
}

/// PF-47: two accounts binding one subject that no client holds yet both wait, and nothing is
/// created, because Keycloak would pick one of them by the subject alone.
#[tokio::test]
async fn two_new_bindings_of_one_subject_both_wait() {
    let keycloak = realm(json!([])).await;

    let outcomes = sync(&keycloak)
        .converge(
            &mirror_with(vec![
                account(
                    "helsinki",
                    "air-adapter",
                    Some(("env-dept", "adapter")),
                    "space-writer",
                ),
                account(
                    "kosice",
                    "squatter",
                    Some(("env-dept", "adapter")),
                    "space-writer",
                ),
            ]),
            &Bentos::new(),
        )
        .await;

    assert_eq!(outcomes.len(), 2, "{outcomes:?}");
    for outcome in &outcomes {
        let error = outcome.error.as_deref().unwrap_or_default();
        assert!(error.contains("is bound by"), "{outcome:?}");
    }
    assert!(wrote(&keycloak).await.is_empty());
}

const PIPELINES: &str = "jc-pipeline-identities";

fn drepo() -> ResourceEnvelope {
    envelope(
        "Pipeline",
        "zilina",
        "drepo",
        json!({
            "class": "scheduled",
            "schedule": "23 3 * * 2",
            "source": { "dataSourceRef": { "kind": "DataSource", "name": "uniza-drepo" } },
            "targetEndpoint": "urn:ngsi-ld:Endpoint:zilina.sk:zilina-uniza:zilina-uniza",
            "output": { "type": "CreativeWork", "mode": "upsert" },
        }),
    )
}

/// PL-19 (T-1508): with the pipelines' namespace set, a Pipeline's derived account gets its
/// federated client `{project}-pl-{pipeline}` for its own Kubernetes subject in that namespace,
/// audienced to the Endpoint it writes through; without it the Pipeline gets none.
#[tokio::test]
async fn a_pipeline_gets_its_own_federated_client_only_when_the_namespace_is_set() {
    let mirror = mirror_with(vec![
        drepo(),
        endpoint("zilina", "zilina-uniza", "zilina-uniza", SLUG),
    ]);
    let keycloak = realm(json!([])).await;
    assert!(sync(&keycloak)
        .converge(&mirror, &Bentos::new())
        .await
        .is_empty());
    assert!(wrote(&keycloak).await.is_empty());

    Mock::given(method("GET"))
        .and(path(format!("{REALM}/clients")))
        .and(query_param("clientId", "zilina-pl-drepo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .up_to_n_times(1)
        .mount(&keycloak)
        .await;
    client_lookup(
        &keycloak,
        "zilina-pl-drepo",
        json!([managed_client(
            "uuid-pl",
            "zilina",
            "pl-drepo",
            PIPELINES,
            "pl-zilina-drepo"
        )]),
    )
    .await;
    Mock::given(method("POST"))
        .and(path(format!("{REALM}/clients")))
        .respond_with(ResponseTemplate::new(201))
        .mount(&keycloak)
        .await;
    mappers(&keycloak, "uuid-pl", json!([])).await;

    let outcomes = sync(&keycloak)
        .with_pipelines(PIPELINES.to_owned())
        .converge(&mirror, &Bentos::new())
        .await;
    // The pipeline's, and the project's harness principal beside it (T-4050).
    let apps: Vec<&str> = outcomes.iter().map(|o| o.app.as_str()).collect();
    assert_eq!(
        apps,
        ["zilina/pl-drepo", "zilina/pl-pipeline-test"],
        "{outcomes:?}"
    );
    assert_eq!(outcomes[0].error, None, "{outcomes:?}");
    let created = bodies(&keycloak, "POST", &format!("{REALM}/clients")).await;
    let created = created
        .iter()
        .find(|c| c["clientId"] == "zilina-pl-drepo")
        .expect("the pipeline's client");
    assert_eq!(
        created["attributes"]["jwt.credential.sub"],
        format!("system:serviceaccount:{PIPELINES}:pl-zilina-drepo")
    );
    // Its Endpoint, and the Portal's internal listener its stream reports outcomes to (T-4038).
    let mut audiences: Vec<Value> = bodies(
        &keycloak,
        "POST",
        &format!("{REALM}/clients/uuid-pl/protocol-mappers/models"),
    )
    .await
    .into_iter()
    .map(|m| m["config"]["included.custom.audience"].clone())
    .collect();
    audiences.sort_by_key(Value::to_string);
    let mut want = vec![json!(SLUG), json!("portal-internal")];
    want.sort_by_key(Value::to_string);
    assert_eq!(audiences, want);
}

/// Creates `client` (absent on the first lookup, then held as `uuid`) bound to `namespace/sa`.
async fn creatable(keycloak: &MockServer, client: (&str, &str), uuid: &str, subject: (&str, &str)) {
    let (project, name) = client;
    let id = format!("{project}-{name}");
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/clients")))
        .and(query_param("clientId", id.as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .up_to_n_times(1)
        .mount(keycloak)
        .await;
    client_lookup(
        keycloak,
        &id,
        json!([managed_client(uuid, project, name, subject.0, subject.1)]),
    )
    .await;
    mappers(keycloak, uuid, json!([])).await;
}

/// The audiences the run wrote on client `uuid`, sorted.
async fn audiences_of(keycloak: &MockServer, uuid: &str) -> Vec<String> {
    let mut found: Vec<String> = bodies(
        keycloak,
        "POST",
        &format!("{REALM}/clients/{uuid}/protocol-mappers/models"),
    )
    .await
    .into_iter()
    .filter_map(|m| {
        m["config"]["included.custom.audience"]
            .as_str()
            .map(str::to_owned)
    })
    .collect();
    found.sort();
    found
}

/// T-5757: a Pipeline's token names the Endpoints the Pipeline names (its output's, and each
/// one of its project its `bento.yaml` calls), not every Endpoint of its space: a Bratislava
/// pipeline beside 453 Endpoints carried 455 audiences and a 22 KB token the edge refused.
/// A ServiceAccount with a role on that space still gets every Endpoint of it.
#[tokio::test]
async fn a_pipelines_token_names_only_the_endpoints_the_pipeline_names() {
    const READ: &str = "r5wq3n7kcm2xt4zb6hdy2vfja3";
    const SIBLING: &str = "s2bq6xm4ty7nc3wk5rdh2jzfa4";
    const ELSEWHERE: &str = "e3kd7pw2vn5xq4tb6mzh2cyra5";
    let mirror = mirror_with(vec![
        drepo(),
        endpoint("zilina", "zilina-uniza", "zilina-uniza", SLUG),
        endpoint("zilina", "zilina-read", "zilina-uniza", READ),
        endpoint("zilina", "zilina-sibling", "zilina-uniza", SIBLING),
        endpoint("trnava", "trnava-all", "trnava", ELSEWHERE),
        envelope(
            "ServiceAccount",
            "zilina",
            "adapter",
            json!({
                "owner": { "user": "jana.kovacova@zilina.sk" },
                "purpose": "the university's own adapter",
                "roles": [{ "role": "space-writer", "scope": { "contextSpace": "zilina-uniza" } }],
                "credentials": [],
                "workload": { "kubernetes": { "namespace": "uniza", "serviceAccount": "adapter" } },
            }),
        ),
    ]);
    // Its mapping reads one Endpoint of its project, one of another project, and one nobody has.
    let bento = format!(
        "pipeline:\n  processors:\n    - http:\n        url: ${{JC_GATEWAY_URL}}/api/endpoint/{READ}/ngsi-ld/v1/entities\n\
         # api/endpoint/{ELSEWHERE}/ngsi-ld/v1/entities and api/endpoint/zz2zz2zz2zz2zz2zz2zz2zz2zz\n"
    );
    let bentos = Bentos::from([(("zilina".to_owned(), "drepo".to_owned()), bento)]);
    let keycloak = realm(json!([])).await;
    creatable(
        &keycloak,
        ("zilina", "pl-drepo"),
        "uuid-pl",
        (PIPELINES, "pl-zilina-drepo"),
    )
    .await;
    creatable(
        &keycloak,
        ("zilina", "adapter"),
        "uuid-sa",
        ("uniza", "adapter"),
    )
    .await;
    Mock::given(method("POST"))
        .and(path(format!("{REALM}/clients")))
        .respond_with(ResponseTemplate::new(201))
        .mount(&keycloak)
        .await;

    let outcomes = sync(&keycloak)
        .with_pipelines(PIPELINES.to_owned())
        .converge(&mirror, &bentos)
        .await;
    // The harness principals' clients are not this test's: only the two it mocks must converge.
    for app in ["zilina/adapter", "zilina/pl-drepo"] {
        let outcome = outcomes.iter().find(|o| o.app == app);
        assert_eq!(outcome.map(|o| o.error.clone()), Some(None), "{outcomes:?}");
    }

    let mut want = vec![SLUG, READ, "portal-internal"];
    want.sort_unstable();
    assert_eq!(audiences_of(&keycloak, "uuid-pl").await, want);
    let mut want = vec![SLUG, READ, SIBLING];
    want.sort_unstable();
    assert_eq!(audiences_of(&keycloak, "uuid-sa").await, want);
}

/// T-4050: each project gets its pipeline-test harness principal `{project}-pl-pipeline-test`,
/// federated to `pl-{project}-pipeline-test` in the pipelines' namespace, with the Portal's
/// internal listener as its one audience: no Endpoint, even beside a Pipeline that writes one.
/// A Pipeline that takes the reserved name anyway gets no client of its own.
#[tokio::test]
async fn each_project_gets_a_harness_principal_for_the_internal_listener_alone() {
    let mut taken = drepo();
    taken.metadata.name = "pipeline-test".to_owned();
    let mirror = mirror_with(vec![
        taken,
        endpoint("zilina", "zilina-uniza", "zilina-uniza", SLUG),
    ]);
    let keycloak = realm(json!([])).await;
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/clients")))
        .and(query_param("clientId", "zilina-pl-pipeline-test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .up_to_n_times(1)
        .mount(&keycloak)
        .await;
    client_lookup(
        &keycloak,
        "zilina-pl-pipeline-test",
        json!([managed_client(
            "uuid-harness",
            "zilina",
            "pl-pipeline-test",
            PIPELINES,
            "pl-zilina-pipeline-test"
        )]),
    )
    .await;
    Mock::given(method("POST"))
        .and(path(format!("{REALM}/clients")))
        .respond_with(ResponseTemplate::new(201))
        .mount(&keycloak)
        .await;
    mappers(&keycloak, "uuid-harness", json!([])).await;

    let outcomes = sync(&keycloak)
        .with_pipelines(PIPELINES.to_owned())
        .converge(&mirror, &Bentos::new())
        .await;
    let apps: Vec<&str> = outcomes.iter().map(|o| o.app.as_str()).collect();
    assert_eq!(apps, ["zilina/pl-pipeline-test"], "{outcomes:?}");
    assert_eq!(outcomes[0].error, None, "{outcomes:?}");
    let created = bodies(&keycloak, "POST", &format!("{REALM}/clients")).await;
    let [created] = created.as_slice() else {
        panic!("one client created");
    };
    assert_eq!(created["clientId"], "zilina-pl-pipeline-test");
    assert_eq!(
        created["attributes"]["jwt.credential.sub"],
        format!("system:serviceaccount:{PIPELINES}:pl-zilina-pipeline-test")
    );
    assert!(created.get("secret").is_none(), "{created}");
    let audiences: Vec<Value> = bodies(
        &keycloak,
        "POST",
        &format!("{REALM}/clients/uuid-harness/protocol-mappers/models"),
    )
    .await
    .into_iter()
    .map(|m| m["config"]["included.custom.audience"].clone())
    .collect();
    assert_eq!(audiences, vec![json!("portal-internal")]);
}

/// PL-19 (T-1508): each Pipeline's Kubernetes ServiceAccount is applied in the pipelines'
/// namespace, labelled and never automounted; a labelled `pl-` account no Pipeline names is
/// deleted, and one without the `pl-` prefix is left alone.
#[tokio::test]
async fn the_pipelines_kubernetes_accounts_follow_the_pipelines() {
    use joinedcontext_portal::apps::kube::KubeClient;
    use joinedcontext_portal::reconciler::workload_clients::converge_pipeline_service_accounts;

    let api = MockServer::start().await;
    let base = format!("/api/v1/namespaces/{PIPELINES}/serviceaccounts");
    Mock::given(method("PATCH"))
        .and(path_regex(format!("^{base}/[^/]+$")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&api)
        .await;
    Mock::given(method("GET"))
        .and(path(base.clone()))
        .and(query_param(
            "labelSelector",
            "joinedcontext.com/pipeline-identity=true",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "items": [
            { "metadata": { "name": "pl-zilina-drepo" } },
            { "metadata": { "name": "pl-zilina-gone" } },
            { "metadata": { "name": "pipeline-tokens" } },
        ] })))
        .mount(&api)
        .await;
    Mock::given(method("DELETE"))
        .and(path_regex(format!("^{base}/[^/]+$")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&api)
        .await;

    let kube = KubeClient::with_token(&api.uri(), "token").expect("a client");
    let failed =
        converge_pipeline_service_accounts(&kube, &mirror_with(vec![drepo()]), PIPELINES).await;
    assert!(failed.is_empty(), "{failed:?}");

    let requests = api.received_requests().await.unwrap_or_default();
    let applied: Vec<Value> = requests
        .iter()
        .filter(|r| r.method.as_str() == "PATCH")
        .map(|r| serde_json::from_slice(&r.body).expect("json"))
        .collect();
    // The pipeline's, and the project's harness principal's (T-4050).
    let [account, harness] = applied.as_slice() else {
        panic!("two applied: {applied:?}");
    };
    assert_eq!(account["metadata"]["name"], "pl-zilina-drepo");
    assert_eq!(harness["metadata"]["name"], "pl-zilina-pipeline-test");
    assert_eq!(harness["automountServiceAccountToken"], false);
    assert_eq!(account["metadata"]["namespace"], PIPELINES);
    assert_eq!(
        account["metadata"]["labels"]["joinedcontext.com/pipeline-identity"],
        "true"
    );
    assert_eq!(account["automountServiceAccountToken"], false);
    let deleted: Vec<String> = requests
        .iter()
        .filter(|r| r.method.as_str() == "DELETE")
        .map(|r| r.url.path().to_owned())
        .collect();
    assert_eq!(deleted, vec![format!("{base}/pl-zilina-gone")]);
}

const IDENTITIES: &str = "jc-app-identities";

/// A published `wasm` App of `zilina` placed on shard 0, with `jobs` as its `spec.server.jobs`.
fn wasm_app(name: &str, jobs: Value) -> ResourceEnvelope {
    serde_json::from_value(json!({
        "apiVersion": API_VERSION, "kind": "App",
        "metadata": { "name": name, "namespace": "zilina" },
        "spec": { "kind": "wasm", "lifecycle": "published", "server": { "jobs": jobs } },
        "status": { "shard": 0, "build": {
            "digest": format!("sha256:{}", "b".repeat(64)), "commit": "c0ffee",
            "sdkVersion": "1.0.0", "builtAt": "2026-10-10T00:00:00Z",
            "component": format!("sha256:{}", "a".repeat(64)) } },
    }))
    .expect("an App")
}

fn hourly() -> Value {
    json!([{ "name": "hourly", "schedule": "0 * * * *", "export": "compute-kpis" }])
}

/// The Endpoint the App reconciler writes for App `app`.
fn app_endpoint(app: &str, slug: &str) -> ResourceEnvelope {
    let mut endpoint = endpoint("zilina", &format!("app-{app}"), "zilina-uniza", slug);
    endpoint.metadata.annotations.insert(
        jc_core::annotations::GENERATED_BY.to_owned(),
        joinedcontext_portal::apps::reconciler::GENERATOR.to_owned(),
    );
    endpoint
}

/// AP-159 (T-3539): with the identities namespace set, an App with jobs gets the federated client
/// `{project}-appjob-{app}` for `appjob-{project}-{app}` there, whose one audience is the App's
/// own Endpoint, though another Endpoint serves the same space; an App without jobs gets none,
/// and without the namespace no App does.
#[tokio::test]
async fn an_apps_job_principal_gets_a_client_for_its_own_endpoint_only() {
    let mirror = mirror_with(vec![
        wasm_app("kpi", hourly()),
        app_endpoint("kpi", SLUG),
        wasm_app("notes", json!([])),
        app_endpoint("notes", "notesslug"),
        endpoint("zilina", "zilina-uniza", "zilina-uniza", "spaceslug"),
    ]);
    let keycloak = realm(json!([])).await;
    assert!(sync(&keycloak)
        .converge(&mirror, &Bentos::new())
        .await
        .is_empty());
    assert!(wrote(&keycloak).await.is_empty());

    Mock::given(method("GET"))
        .and(path(format!("{REALM}/clients")))
        .and(query_param("clientId", "zilina-appjob-kpi"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .up_to_n_times(1)
        .mount(&keycloak)
        .await;
    client_lookup(
        &keycloak,
        "zilina-appjob-kpi",
        json!([managed_client(
            "uuid-aj",
            "zilina",
            "appjob-kpi",
            IDENTITIES,
            "appjob-zilina-kpi"
        )]),
    )
    .await;
    Mock::given(method("POST"))
        .and(path(format!("{REALM}/clients")))
        .respond_with(ResponseTemplate::new(201))
        .mount(&keycloak)
        .await;
    mappers(&keycloak, "uuid-aj", json!([])).await;

    let outcomes = sync(&keycloak)
        .with_app_jobs(IDENTITIES.to_owned())
        .converge(&mirror, &Bentos::new())
        .await;
    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    assert_eq!(outcomes[0].app, "zilina/appjob-kpi");
    assert_eq!(outcomes[0].error, None, "{outcomes:?}");
    let created = bodies(&keycloak, "POST", &format!("{REALM}/clients")).await;
    let [created] = created.as_slice() else {
        panic!("one client: {created:?}");
    };
    assert_eq!(created["clientId"], "zilina-appjob-kpi");
    assert_eq!(created["clientAuthenticatorType"], "federated-jwt");
    assert_eq!(
        created["attributes"]["jwt.credential.sub"],
        format!("system:serviceaccount:{IDENTITIES}:appjob-zilina-kpi")
    );
    let mapped = bodies(
        &keycloak,
        "POST",
        &format!("{REALM}/clients/uuid-aj/protocol-mappers/models"),
    )
    .await;
    let [mapper] = mapped.as_slice() else {
        panic!("one audience: {mapped:?}");
    };
    assert_eq!(mapper["config"]["included.custom.audience"], SLUG);
}

/// AP-159 (T-3539): an App that drops its jobs, or is gone, loses its job principal's client.
#[tokio::test]
async fn an_app_that_loses_its_jobs_loses_its_principals_client() {
    let held = json!([managed_client(
        "uuid-aj",
        "zilina",
        "appjob-kpi",
        IDENTITIES,
        "appjob-zilina-kpi"
    )]);
    for mirror in [
        mirror_with(vec![wasm_app("kpi", json!([])), app_endpoint("kpi", SLUG)]),
        mirror_with(vec![]),
    ] {
        let keycloak = realm(held.clone()).await;
        let outcomes = sync(&keycloak)
            .with_app_jobs(IDENTITIES.to_owned())
            .converge(&mirror, &Bentos::new())
            .await;
        assert_eq!(outcomes.len(), 1, "{outcomes:?}");
        assert_eq!(outcomes[0].app, "zilina/appjob-kpi");
        assert_eq!(outcomes[0].error, None, "{outcomes:?}");
        assert_eq!(
            wrote(&keycloak).await,
            vec![format!("DELETE {REALM}/clients/uuid-aj")]
        );
    }
}

/// One run of the Apps' ServiceAccount convergence against a cluster that lists `held` with the
/// label: what it applied and what it deleted.
async fn app_job_accounts_run(mirror: &Mirror, held: &[&str]) -> (Vec<Value>, Vec<String>) {
    use joinedcontext_portal::apps::kube::KubeClient;
    use joinedcontext_portal::reconciler::workload_clients::converge_app_job_service_accounts;

    let api = MockServer::start().await;
    let base = format!("/api/v1/namespaces/{IDENTITIES}/serviceaccounts");
    Mock::given(method("PATCH"))
        .and(path_regex(format!("^{base}/[^/]+$")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&api)
        .await;
    let items: Vec<Value> = held
        .iter()
        .map(|name| json!({ "metadata": { "name": name } }))
        .collect();
    Mock::given(method("GET"))
        .and(path(base.clone()))
        .and(query_param(
            "labelSelector",
            "joinedcontext.com/app-job-identity=true",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "items": items })))
        .mount(&api)
        .await;
    Mock::given(method("DELETE"))
        .and(path_regex(format!("^{base}/[^/]+$")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&api)
        .await;

    let kube = KubeClient::with_token(&api.uri(), "token").expect("a client");
    let failed = converge_app_job_service_accounts(&kube, mirror, IDENTITIES).await;
    assert!(failed.is_empty(), "{failed:?}");
    let requests = api.received_requests().await.unwrap_or_default();
    let applied = requests
        .iter()
        .filter(|r| r.method.as_str() == "PATCH")
        .map(|r| serde_json::from_slice(&r.body).expect("json"))
        .collect();
    let deleted = requests
        .iter()
        .filter(|r| r.method.as_str() == "DELETE")
        .map(|r| {
            r.url
                .path()
                .trim_start_matches(&format!("{base}/"))
                .to_owned()
        })
        .collect();
    (applied, deleted)
}

/// AP-159 (T-3539): an App with jobs has its Kubernetes ServiceAccount in the identities
/// namespace, labelled and never automounted; it goes when the App drops its jobs; an App without
/// jobs gets none; and a ServiceAccount there without the `appjob-` prefix is never deleted,
/// labelled or not.
#[tokio::test]
async fn the_apps_job_accounts_follow_the_apps_jobs() {
    let with_jobs = mirror_with(vec![
        wasm_app("kpi", hourly()),
        app_endpoint("kpi", SLUG),
        wasm_app("notes", json!([])),
        app_endpoint("notes", "notesslug"),
    ]);
    let (applied, deleted) =
        app_job_accounts_run(&with_jobs, &["appjob-zilina-kpi", "token-service"]).await;
    let [account] = applied.as_slice() else {
        panic!("one applied: {applied:?}");
    };
    assert_eq!(account["metadata"]["name"], "appjob-zilina-kpi");
    assert_eq!(account["metadata"]["namespace"], IDENTITIES);
    assert_eq!(
        account["metadata"]["labels"]["joinedcontext.com/app-job-identity"],
        "true"
    );
    assert_eq!(
        account["metadata"]["annotations"]["joinedcontext.serviceaccount"],
        "zilina/appjob-kpi"
    );
    assert_eq!(account["automountServiceAccountToken"], false);
    assert!(deleted.is_empty(), "{deleted:?}");

    let without_jobs = mirror_with(vec![wasm_app("kpi", json!([])), app_endpoint("kpi", SLUG)]);
    let (applied, deleted) =
        app_job_accounts_run(&without_jobs, &["appjob-zilina-kpi", "token-service"]).await;
    assert!(applied.is_empty(), "{applied:?}");
    assert_eq!(deleted, vec!["appjob-zilina-kpi".to_owned()]);
}

/// An account with no workload whose credentials are `credentials`.
fn external(project: &str, name: &str, credentials: Value) -> ResourceEnvelope {
    let mut env = account(project, name, None, "space-writer");
    env.spec["credentials"] = credentials;
    env
}

/// One P-256 public key, as a device's credential lists it.
fn device_key(kid: &str) -> Value {
    json!({ "kty": "EC", "crv": "P-256", "kid": kid, "alg": "ES256", "use": "sig",
            "x": "f83OJ3D2xF1Bg8vub9tLe1gHMzV76e8Tus9uPHvRVEU",
            "y": "x_FEzRu9m36HLN_tue659LNpXW6pCyStikYjKIWI5a0" })
}

/// Creates `id` on the first lookup and answers `created` from then on, as the realm would.
async fn creates(keycloak: &MockServer, id: &str, created: Value) {
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/clients")))
        .and(query_param("clientId", id))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .up_to_n_times(1)
        .mount(keycloak)
        .await;
    client_lookup(keycloak, id, json!([created])).await;
    Mock::given(method("POST"))
        .and(path(format!("{REALM}/clients")))
        .respond_with(ResponseTemplate::new(201))
        .mount(keycloak)
        .await;
}

/// T-5626: an account an external system signs in as gets a confidential `client-secret` client,
/// `client_credentials` only, with its roles' audiences; the wave never reads its secret.
#[tokio::test]
async fn an_external_account_gets_a_secret_client_and_no_secret_is_read() {
    let keycloak = realm(json!([])).await;
    let mut held = desired_external("helsinki", "vendor", &ClientCredential::Secret);
    held["id"] = json!("uuid-vendor");
    creates(&keycloak, "helsinki-vendor", held).await;
    mappers(&keycloak, "uuid-vendor", json!([])).await;

    let outcomes = sync(&keycloak)
        .converge(
            &mirror_with(vec![
                external(
                    "helsinki",
                    "vendor",
                    json!([{ "kind": "oauth-client", "name": "main" }]),
                ),
                endpoint("helsinki", "air-all", "air", SLUG),
            ]),
            &Bentos::new(),
        )
        .await;

    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    assert_eq!(outcomes[0].app, "helsinki/vendor");
    assert_eq!(outcomes[0].error, None, "{outcomes:?}");
    let created = &bodies(&keycloak, "POST", &format!("{REALM}/clients")).await[0];
    assert_eq!(created["clientId"], "helsinki-vendor");
    assert_eq!(created["clientAuthenticatorType"], "client-secret");
    // T-6051: `basic` alone, so the first token carries none of the realm's default scopes.
    assert_eq!(created["defaultClientScopes"], json!(["basic"]));
    assert_eq!(created["optionalClientScopes"], json!([]));
    assert_eq!(created["serviceAccountsEnabled"], true);
    assert_eq!(created["directAccessGrantsEnabled"], false);
    assert_eq!(created["standardFlowEnabled"], false);
    assert_eq!(created["attributes"][MANAGED_BY], MANAGED_VALUE);
    assert_eq!(
        created["attributes"]["joinedcontext.serviceaccount"],
        "helsinki/vendor"
    );
    assert_eq!(created.get("secret"), None, "the wave sets no secret");
    let mapper = &bodies(
        &keycloak,
        "POST",
        &format!("{REALM}/clients/uuid-vendor/protocol-mappers/models"),
    )
    .await[0];
    assert_eq!(mapper["config"]["included.custom.audience"], SLUG);
    let asked: Vec<String> = keycloak
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .map(|r| r.url.path().to_owned())
        .collect();
    assert!(
        !asked.iter().any(|p| p.ends_with("/client-secret")),
        "no secret is read: {asked:?}"
    );
}

/// T-4015, T-5626: a device's account gets `client-jwt` against the public keys its credentials
/// list, each kid once, and no secret.
#[tokio::test]
async fn a_device_account_gets_a_key_pair_client() {
    let keycloak = realm(json!([])).await;
    let credential = ClientCredential::Keys(json!({ "keys": [device_key("k1")] }).to_string());
    let mut held = desired_external("helsinki", "sensor", &credential);
    held["id"] = json!("uuid-sensor");
    creates(&keycloak, "helsinki-sensor", held).await;
    mappers(&keycloak, "uuid-sensor", json!([])).await;

    let outcomes = sync(&keycloak)
        .converge(&mirror_with(vec![external(
            "helsinki",
            "sensor",
            json!([
                { "kind": "oauth-client", "name": "device-key", "jwks": { "keys": [device_key("k1")] } },
                { "kind": "oauth-client", "name": "spare", "jwks": { "keys": [device_key("k1")] } },
            ]),
        )]), &Bentos::new())
        .await;

    assert_eq!(outcomes[0].error, None, "{outcomes:?}");
    let created = &bodies(&keycloak, "POST", &format!("{REALM}/clients")).await[0];
    assert_eq!(created["clientAuthenticatorType"], "client-jwt");
    assert_eq!(created["attributes"]["use.jwks.string"], "true");
    assert_eq!(created["attributes"]["use.jwks.url"], "false");
    let jwks: Value = serde_json::from_str(
        created["attributes"]["jwks.string"]
            .as_str()
            .expect("a JWKS string"),
    )
    .expect("json");
    assert_eq!(jwks, json!({ "keys": [device_key("k1")] }));
}

/// T-5626: the platform's own client of an external account (`keycloak-clients.yaml`) serves it
/// as it is: nothing is written and nothing fails; one without a credential gets no client.
#[tokio::test]
async fn an_external_accounts_platform_client_is_left_alone_quietly() {
    let foreign =
        json!({ "id": "uuid-hand", "clientId": "banskabystrica-conformance", "attributes": {} });
    let keycloak = realm(json!([foreign.clone()])).await;
    client_lookup(&keycloak, "banskabystrica-conformance", json!([foreign])).await;

    let outcomes = sync(&keycloak)
        .converge(
            &mirror_with(vec![
                external(
                    "banskabystrica",
                    "conformance",
                    json!([{ "kind": "oauth-client", "name": "main" }]),
                ),
                external("banskabystrica", "nothing", json!([])),
            ]),
            &Bentos::new(),
        )
        .await;

    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    assert_eq!(outcomes[0].error, None, "{outcomes:?}");
    assert!(outcomes[0].drift.is_empty(), "{outcomes:?}");
    assert!(wrote(&keycloak).await.is_empty());
}

/// T-5626: an external account's managed client goes with the account, its tokens revoked first.
#[tokio::test]
async fn an_external_accounts_client_is_deleted_with_the_account() {
    let mut held = desired_external("helsinki", "vendor", &ClientCredential::Secret);
    held["id"] = json!("uuid-vendor");
    let keycloak = realm(json!([held])).await;

    let outcomes = sync(&keycloak)
        .converge(&mirror_with(vec![]), &Bentos::new())
        .await;

    assert_eq!(
        wrote(&keycloak).await,
        vec![format!("DELETE {REALM}/clients/uuid-vendor")]
    );
    assert_eq!(outcomes[0].app, "helsinki/vendor");
    assert_eq!(outcomes[0].error, None);
}

/// T-5766: a Pipeline's token fits the smallest header limit on its hops beside a project of 455
/// Endpoints on its one space, as Bratislava's is. Before T-5757 its `aud` named all 455 and the
/// 22 KB token got 431 from the gateway and the Portal's internal listener, so no Bratislava
/// write ever landed. The rest of a token, `aud` aside, is taken from the 2,526-byte
/// helsinki/air-quality token T-5757 measured; `aud` travels base64url-encoded (4/3).
#[tokio::test]
async fn a_pipelines_token_beside_455_endpoints_fits_the_header_limit() {
    const HEADER_LIMIT: usize = 8 * 1024;
    const TOKEN_WITHOUT_AUD: usize = 2_600;
    const BEARER: usize = "Authorization: Bearer ".len();
    let mut items = vec![
        drepo(),
        endpoint("zilina", "zilina-uniza", "zilina-uniza", SLUG),
    ];
    let base32 = b"abcdefghijklmnopqrstuvwxyz234567";
    for i in 0..454_usize {
        let slug: String = (0..26)
            .map(|d| char::from(base32[(i >> (5 * (d % 2))) % 32]))
            .collect();
        items.push(endpoint(
            "zilina",
            &format!("zilina-kpi-{i}"),
            "zilina-uniza",
            &slug,
        ));
    }
    let mirror = mirror_with(items);
    let keycloak = realm(json!([])).await;
    creatable(
        &keycloak,
        ("zilina", "pl-drepo"),
        "uuid-pl",
        (PIPELINES, "pl-zilina-drepo"),
    )
    .await;
    Mock::given(method("POST"))
        .and(path(format!("{REALM}/clients")))
        .respond_with(ResponseTemplate::new(201))
        .mount(&keycloak)
        .await;

    let outcomes = sync(&keycloak)
        .with_pipelines(PIPELINES.to_owned())
        .converge(&mirror, &Bentos::new())
        .await;
    let outcome = outcomes.iter().find(|o| o.app == "zilina/pl-drepo");
    assert_eq!(outcome.map(|o| o.error.clone()), Some(None), "{outcomes:?}");

    let aud = audiences_of(&keycloak, "uuid-pl").await;
    let encoded = json!(aud).to_string().len().div_ceil(3) * 4;
    let header = BEARER + TOKEN_WITHOUT_AUD + encoded;
    assert!(
        header <= HEADER_LIMIT,
        "{header} bytes over {HEADER_LIMIT} for {} audiences",
        aud.len()
    );
}

/// T-6051: a client made before the trim carries the realm's default and optional scopes; one run
/// removes every one of them but `basic`, reports it, and a later run finds nothing to do.
#[tokio::test]
async fn an_accounts_client_loses_the_realms_default_scopes() {
    let held = desired_external("helsinki", "hsl-bridge", &ClientCredential::Secret);
    let mut held = held;
    held["id"] = json!("uuid-hsl");
    let keycloak = realm(json!([held.clone()])).await;
    client_lookup(&keycloak, "helsinki-hsl-bridge", json!([held])).await;
    mappers(&keycloak, "uuid-hsl", json!([])).await;
    let named = |names: &[&str]| -> Value {
        names
            .iter()
            .map(|n| json!({ "id": format!("s-{n}"), "name": n }))
            .collect()
    };
    client_scopes(
        &keycloak,
        "uuid-hsl",
        named(&[
            "web-origins",
            "service_account",
            "acr",
            "roles",
            "profile",
            "basic",
            "email",
        ]),
        named(&["offline_access"]),
    )
    .await;
    let mirror = mirror_with(vec![external(
        "helsinki",
        "hsl-bridge",
        json!([{ "kind": "oauth-client", "name": "main" }]),
    )]);

    let outcomes = sync(&keycloak).converge(&mirror, &Bentos::new()).await;

    assert_eq!(outcomes[0].error, None, "{outcomes:?}");
    let scopes: Vec<String> = wrote(&keycloak)
        .await
        .into_iter()
        .filter(|w| w.contains("-client-scopes/"))
        .collect();
    let at = format!("DELETE {REALM}/clients/uuid-hsl");
    assert_eq!(
        scopes,
        vec![
            format!("{at}/default-client-scopes/s-web-origins"),
            format!("{at}/default-client-scopes/s-service_account"),
            format!("{at}/default-client-scopes/s-acr"),
            format!("{at}/default-client-scopes/s-roles"),
            format!("{at}/default-client-scopes/s-profile"),
            format!("{at}/default-client-scopes/s-email"),
            format!("{at}/optional-client-scopes/s-offline_access"),
        ]
    );
    assert!(
        outcomes[0]
            .drift
            .iter()
            .any(|d| d.contains("roles") && d.contains("removed")),
        "{outcomes:?}"
    );
}

/// T-6051: a client without `basic` gets it back from the realm's scopes, or its tokens carry no
/// `sub`; a realm without it is an error naming the scope, never a silent client.
#[tokio::test]
async fn an_accounts_client_without_basic_gets_it() {
    let mut held = desired_external("helsinki", "vendor", &ClientCredential::Secret);
    held["id"] = json!("uuid-v");
    let keycloak = realm(json!([held.clone()])).await;
    client_lookup(&keycloak, "helsinki-vendor", json!([held])).await;
    mappers(&keycloak, "uuid-v", json!([])).await;
    client_scopes(&keycloak, "uuid-v", json!([]), json!([])).await;
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/client-scopes")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": "s-email", "name": "email" },
            { "id": "s-basic", "name": "basic" },
        ])))
        .mount(&keycloak)
        .await;
    let mirror = mirror_with(vec![external(
        "helsinki",
        "vendor",
        json!([{ "kind": "oauth-client", "name": "main" }]),
    )]);

    let outcomes = sync(&keycloak).converge(&mirror, &Bentos::new()).await;

    assert_eq!(outcomes[0].error, None, "{outcomes:?}");
    assert!(wrote(&keycloak).await.contains(&format!(
        "PUT {REALM}/clients/uuid-v/default-client-scopes/s-basic"
    )));
}
