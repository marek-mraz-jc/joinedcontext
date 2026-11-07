//! The hub registration wave against a broker (T-2505, PF-48, MF-36): what is written, what is
//! deleted, and what a refusal becomes. The broker is mocked; the pure helpers are covered in
//! `reconciler::registrations::tests`.

use joinedcontext_portal::reconciler::registrations::{RegistrationOutcome, RegistrationSync};
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

const REGISTRATIONS: &str = "/ngsi-ld/v1/csourceRegistrations";

fn envelope(project: &str, kind: &str, name: &str, spec: Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: kind.into(),
        metadata: ObjectMeta {
            name: name.into(),
            namespace: Some(project.into()),
            ..Default::default()
        },
        spec,
        status: None,
    }
}

/// A registration of `project` in hub space `hub`, over the project's `{name}-read` Endpoint,
/// with that Endpoint beside it.
fn declared(project: &str, hub: &str, name: &str) -> Vec<ResourceEnvelope> {
    vec![
        envelope(
            project,
            "ContextSourceRegistration",
            name,
            json!({
                "contextSpaceRef": hub,
                "endpointRef": { "kind": "Endpoint", "name": format!("{name}-read") },
                "information": [{ "entities": [{ "type": "Vehicle" }] }],
                "federation": {
                    "identity": "serviceAccount",
                    "serviceAccountRef": { "kind": "ServiceAccount", "name": "hub-reader" }
                }
            }),
        ),
        envelope(
            project,
            "Endpoint",
            &format!("{name}-read"),
            json!({ "contextSpaceRef": name }),
        ),
    ]
}

fn mirror(items: Vec<Vec<ResourceEnvelope>>) -> Mirror {
    let mirror = Mirror::new();
    for item in items.into_iter().flatten() {
        mirror.upsert(item);
    }
    mirror
}

/// A broker answering a create with `create`, a patch with 204 and a delete with `delete`.
async fn broker(create: u16, delete: u16) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path_regex(format!("^{REGISTRATIONS}$")))
        .respond_with(ResponseTemplate::new(create).set_body_string(match create {
            400 => r#"{"type":"https://uri.etsi.org/ngsi-ld/errors/BadRequestData","title":"no such attribute"}"#,
            _ => "",
        }))
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .respond_with(ResponseTemplate::new(delete))
        .mount(&server)
        .await;
    server
}

/// Every call the broker got: `(method, path, tenant, body)`.
async fn calls(broker: &MockServer) -> Vec<(String, String, String, Value)> {
    broker
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| {
            (
                r.method.to_string(),
                r.url.path().to_owned(),
                r.headers
                    .get("NGSILD-Tenant")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default()
                    .to_owned(),
                serde_json::from_slice(&r.body).unwrap_or(Value::Null),
            )
        })
        .collect()
}

fn deletes(calls: &[(String, String, String, Value)]) -> Vec<(String, String)> {
    calls
        .iter()
        .filter(|(m, ..)| m == "DELETE")
        .map(|(_, path, tenant, _)| (path.clone(), tenant.clone()))
        .collect()
}

fn id_path(name: &str) -> String {
    format!("{REGISTRATIONS}/urn:ngsi-ld:ContextSourceRegistration:{name}")
}

/// Cases 5, 6 and 10: a new manifest is written and said to be, one still declared is written
/// again and never deleted, and a first pass (nothing before it) deletes nothing.
#[tokio::test]
async fn a_new_or_kept_manifest_is_written_and_nothing_is_deleted() {
    let broker = broker(201, 204).await;
    let sync = RegistrationSync::new(broker.uri());
    let now = mirror(vec![declared("helsinki", "hub", "transport")]);

    for previous in [
        Mirror::new(),
        mirror(vec![declared("helsinki", "hub", "transport")]),
    ] {
        let outcomes = sync.converge(&now, &previous).await;
        assert_eq!(
            outcomes,
            [(
                "helsinki".to_owned(),
                "transport".to_owned(),
                RegistrationOutcome::Written
            )]
        );
    }
    let seen = calls(&broker).await;
    assert!(deletes(&seen).is_empty(), "{seen:?}");
    let (_, path, tenant, body) = &seen[0];
    assert_eq!(path, REGISTRATIONS);
    assert_eq!(tenant, "helsinki-hub");
    assert_eq!(
        body["id"],
        "urn:ngsi-ld:ContextSourceRegistration:transport"
    );
}

/// Cases 1 and 4: a manifest that is gone takes its registration with it, from the tenant it
/// was written to; with nothing declared now, every earlier registration goes.
#[tokio::test]
async fn a_manifest_removed_since_the_last_pass_has_its_registration_deleted() {
    let broker = broker(201, 204).await;
    let sync = RegistrationSync::new(broker.uri());
    let previous = mirror(vec![
        declared("helsinki", "hub", "transport"),
        declared("helsinki", "hub", "parking"),
    ]);

    let kept = mirror(vec![declared("helsinki", "hub", "transport")]);
    sync.converge(&kept, &previous).await;
    assert_eq!(
        deletes(&calls(&broker).await),
        [(id_path("parking"), "helsinki-hub".to_owned())]
    );

    broker.reset().await;
    let broker_again = broker;
    Mock::given(method("DELETE"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&broker_again)
        .await;
    let outcomes = sync.converge(&Mirror::new(), &previous).await;
    assert!(outcomes.is_empty());
    let mut gone = deletes(&calls(&broker_again).await);
    gone.sort();
    assert_eq!(
        gone,
        [
            (id_path("parking"), "helsinki-hub".to_owned()),
            (id_path("transport"), "helsinki-hub".to_owned()),
        ]
    );
}

/// Case 2: two registrations of one hub space keep each other.
#[tokio::test]
async fn two_manifests_of_the_same_space_do_not_delete_each_others_registration() {
    let broker = broker(201, 204).await;
    let sync = RegistrationSync::new(broker.uri());
    let both = mirror(vec![
        declared("helsinki", "hub", "transport"),
        declared("helsinki", "hub", "parking"),
    ]);
    let outcomes = sync.converge(&both, &both).await;
    assert_eq!(outcomes.len(), 2);
    assert!(outcomes
        .iter()
        .all(|(_, _, o)| *o == RegistrationOutcome::Written));
    assert!(deletes(&calls(&broker).await).is_empty());
}

/// Cases 3 and 11: one manifest name in two projects is two registrations, each in its own
/// project's tenant, and each outcome is keyed by its project. Removing one never touches the
/// other.
#[tokio::test]
async fn one_name_in_two_projects_is_two_registrations_in_two_tenants() {
    let broker = broker(201, 204).await;
    let sync = RegistrationSync::new(broker.uri());
    let both = mirror(vec![
        declared("helsinki", "hub", "transport"),
        declared("espoo", "hub", "transport"),
    ]);
    let mut outcomes = sync.converge(&both, &Mirror::new()).await;
    outcomes.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        outcomes,
        [
            (
                "espoo".to_owned(),
                "transport".to_owned(),
                RegistrationOutcome::Written
            ),
            (
                "helsinki".to_owned(),
                "transport".to_owned(),
                RegistrationOutcome::Written
            ),
        ]
    );
    let mut tenants: Vec<String> = calls(&broker)
        .await
        .into_iter()
        .map(|(_, _, tenant, _)| tenant)
        .collect();
    tenants.sort();
    assert_eq!(tenants, ["espoo-hub", "helsinki-hub"]);

    broker.reset().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(201))
        .mount(&broker)
        .await;
    Mock::given(method("DELETE"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&broker)
        .await;
    let only_helsinki = mirror(vec![declared("helsinki", "hub", "transport")]);
    sync.converge(&only_helsinki, &both).await;
    assert_eq!(
        deletes(&calls(&broker).await),
        [(id_path("transport"), "espoo-hub".to_owned())]
    );
}

/// An existing registration is patched in place, without the id or the type in the body.
#[tokio::test]
async fn a_registration_that_is_already_there_is_patched_without_its_id() {
    let broker = broker(409, 204).await;
    let sync = RegistrationSync::new(broker.uri());
    let now = mirror(vec![declared("helsinki", "hub", "transport")]);
    let outcomes = sync.converge(&now, &now).await;
    assert_eq!(outcomes[0].2, RegistrationOutcome::Written);
    let seen = calls(&broker).await;
    let (_, path, tenant, body) = seen
        .iter()
        .find(|(m, ..)| m == "PATCH")
        .expect("the registration was patched");
    assert_eq!(path, &id_path("transport"));
    assert_eq!(tenant, "helsinki-hub");
    assert!(
        body.get("id").is_none() && body.get("type").is_none(),
        "{body}"
    );
}

/// Cases 7 and 8: a refused write is an outcome that says what the broker said, and a refused
/// delete is logged and costs the pass nothing.
#[tokio::test]
async fn a_refusal_from_the_broker_is_an_outcome_and_never_stops_the_pass() {
    for (create, why) in [(400u16, "no such attribute"), (500, "500")] {
        let broker = broker(create, 500).await;
        let sync = RegistrationSync::new(broker.uri());
        let now = mirror(vec![declared("helsinki", "hub", "transport")]);
        let previous = mirror(vec![declared("helsinki", "hub", "parking")]);
        let outcomes = sync.converge(&now, &previous).await;
        match &outcomes[..] {
            [(_, name, RegistrationOutcome::Error(reason))] => {
                assert_eq!(name, "transport");
                assert!(reason.contains("refused the registration"), "{reason}");
                assert!(reason.contains(why), "{reason}");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            deletes(&calls(&broker).await).len(),
            1,
            "the delete was tried"
        );
    }
}

/// Case 9: a manifest that does not parse, or reaches outside its project (PF-48), is an Error
/// outcome, so its status says it is not written; the rest of the pass goes on, and the
/// registration it had before is removed rather than left exposing its member.
#[tokio::test]
async fn a_manifest_that_does_not_parse_is_reported_and_the_rest_of_the_pass_continues() {
    let broker = broker(201, 204).await;
    let sync = RegistrationSync::new(broker.uri());
    let broken = envelope(
        "helsinki",
        "ContextSourceRegistration",
        "parking",
        json!({ "contextSpaceRef": "hub" }),
    );
    let mut outside = declared("helsinki", "hub", "bikes");
    outside[0].spec["endpointRef"]["namespace"] = json!("espoo");
    let now = mirror(vec![
        declared("helsinki", "hub", "transport"),
        vec![broken],
        outside,
    ]);
    let previous = mirror(vec![
        declared("helsinki", "hub", "transport"),
        declared("helsinki", "hub", "parking"),
    ]);

    let mut outcomes = sync.converge(&now, &previous).await;
    outcomes.sort_by(|a, b| a.1.cmp(&b.1));
    let names: Vec<&str> = outcomes.iter().map(|(_, n, _)| n.as_str()).collect();
    assert_eq!(names, ["bikes", "parking", "transport"], "{outcomes:?}");
    assert!(
        matches!(outcomes[0].2, RegistrationOutcome::Error(ref r) if r.contains("project")),
        "{outcomes:?}"
    );
    assert!(
        matches!(outcomes[1].2, RegistrationOutcome::Error(_)),
        "{outcomes:?}"
    );
    assert_eq!(outcomes[2].2, RegistrationOutcome::Written);
    assert_eq!(
        deletes(&calls(&broker).await),
        [(id_path("parking"), "helsinki-hub".to_owned())]
    );
}

/// T-5032: the hop carries no subject, so every create, patch and delete sends the broker's
/// admin token as its bearer, read from the file on each call.
#[tokio::test]
async fn every_registration_call_sends_the_broker_admin_token() {
    let file = std::env::temp_dir().join(format!("t5032-broker-admin-{}", std::process::id()));
    std::fs::write(&file, "broker-admin-secret\n").expect("token written");
    let broker = broker(409, 204).await;
    let sync = RegistrationSync::new(broker.uri()).with_admin_token(Some(file.clone()));
    let now = mirror(vec![declared("helsinki", "hub", "transport")]);
    let previous = mirror(vec![declared("helsinki", "hub", "parking")]);
    let outcomes = sync.converge(&now, &previous).await;
    let _ = std::fs::remove_file(&file);
    assert_eq!(outcomes[0].2, RegistrationOutcome::Written);
    let seen = broker.received_requests().await.unwrap_or_default();
    let methods: Vec<String> = seen.iter().map(|r| r.method.to_string()).collect();
    assert_eq!(methods, ["POST", "PATCH", "DELETE"]);
    for request in &seen {
        assert_eq!(
            request
                .headers
                .get("authorization")
                .and_then(|v| v.to_str().ok()),
            Some("Bearer broker-admin-secret"),
            "{} {}",
            request.method,
            request.url
        );
    }
}

/// T-5032: a token file that is configured and unreadable fails the write rather than sending
/// it without the credential, and the broker hears nothing.
#[tokio::test]
async fn an_unreadable_admin_token_writes_nothing() {
    let broker = broker(201, 204).await;
    let sync = RegistrationSync::new(broker.uri())
        .with_admin_token(Some("/nonexistent/broker-admin-token".into()));
    let now = mirror(vec![declared("helsinki", "hub", "transport")]);
    let outcomes = sync.converge(&now, &Mirror::new()).await;
    match &outcomes[..] {
        [(_, _, RegistrationOutcome::Error(reason))] => {
            assert!(reason.contains("admin token is not readable"), "{reason}");
        }
        other => panic!("{other:?}"),
    }
    assert!(calls(&broker).await.is_empty());
}
