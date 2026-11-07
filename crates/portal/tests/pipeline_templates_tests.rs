//! Pipeline templates of a project (T-3976, API/01 §42): a saved template holds no secretRef and
//! no reference into the project, a credential written out saves nothing, one project's
//! templates never list in another, and saving or deleting needs `propose` on Pipeline.

mod common;

use serde_json::{json, Value};

use common::{envelope, forge, person, send, state_on};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;

async fn world() -> (wiremock::MockServer, AppState) {
    let gitea = forge().await;
    let state = state_on(&gitea);
    for (role, verbs, project) in [
        ("editor", json!(["read", "propose"]), "helsinki"),
        ("reader", json!(["read"]), "helsinki"),
        ("espoo-editor", json!(["read", "propose"]), "espoo"),
    ] {
        state.mirror.upsert(envelope(
            "Role",
            role,
            ORG_NAMESPACE,
            json!({ "rules": [{ "kinds": ["Pipeline"], "verbs": verbs }] }),
        ));
        state.mirror.upsert(envelope(
            "RoleBinding",
            &format!("{role}s"),
            ORG_NAMESPACE,
            json!({ "subjects": [{ "user": format!("{role}@hel.fi") }], "role": role, "scope": { "project": project } }),
        ));
    }
    (gitea, state)
}

async fn call(
    state: &AppState,
    who: &str,
    verb: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let answer = send(
        state,
        person(who),
        verb,
        &format!("/api/v1/projects/{path}"),
        body,
    )
    .await;
    (
        answer.status.as_u16(),
        serde_json::from_str(&answer.text).unwrap_or(Value::Null),
    )
}

fn pipeline() -> Value {
    json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": "Pipeline",
        "metadata": { "name": "aq-mqtt-ingest", "namespace": "helsinki" },
        "spec": {
            "class": "resident",
            "contextSpaceRef": "air",
            "secretRefs": [{ "name": "mqtt-credentials", "key": "password", "envVar": "MQTT_PASSWORD" }],
            "sources": [{ "dataSourceRef": { "kind": "DataSource", "name": "aq-mqtt" } }],
            "steps": [{ "kind": "bloblang", "bloblang": "root = this" }],
            "outputs": [{ "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:air:all", "type": "AirQualityObserved", "mode": "upsert" }]
        }
    })
}

#[tokio::test]
async fn a_saved_template_holds_the_shape_and_nothing_of_the_project() {
    let (_gitea, state) = world().await;
    let (status, saved) = call(
        &state,
        "editor",
        "POST",
        "helsinki/pipelinetemplates",
        Some(json!({ "title": "MQTT sensor feed", "manifest": pipeline() })),
    )
    .await;
    assert_eq!(status, 201, "{saved}");
    let text = saved.to_string();
    for gone in [
        "secretRefs",
        "mqtt-credentials",
        "aq-mqtt",
        "urn:ngsi-ld:Endpoint",
        "contextSpaceRef",
        "aq-mqtt-ingest",
    ] {
        assert!(!text.contains(gone), "{gone} in {text}");
    }
    assert_eq!(
        saved["spec"]["outputs"],
        json!([{ "type": "AirQualityObserved", "mode": "upsert" }])
    );
    assert_eq!(saved["spec"]["steps"][0]["bloblang"], json!("root = this"));

    let (status, list) = call(&state, "reader", "GET", "helsinki/pipelinetemplates", None).await;
    assert_eq!(status, 200, "{list}");
    assert_eq!(list["items"][0]["title"], json!("MQTT sensor feed"));
    // Another project's people see none of it, and cannot reach it by its id either.
    let (status, theirs) = call(
        &state,
        "espoo-editor",
        "GET",
        "espoo/pipelinetemplates",
        None,
    )
    .await;
    assert_eq!((status, theirs["items"].clone()), (200, json!([])));
    let id = saved["id"].as_str().expect("an id");
    let (status, _) = call(
        &state,
        "espoo-editor",
        "DELETE",
        &format!("espoo/pipelinetemplates/{id}"),
        None,
    )
    .await;
    assert_eq!(status, 404);
    let (status, _) = call(
        &state,
        "espoo-editor",
        "GET",
        "helsinki/pipelinetemplates",
        None,
    )
    .await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn a_credential_written_out_saves_nothing() {
    let (_gitea, state) = world().await;
    let mut leak = pipeline();
    leak["spec"]["sources"][0]["query"] = json!({ "token": "s3cr3t" });
    let (status, body) = call(
        &state,
        "editor",
        "POST",
        "helsinki/pipelinetemplates",
        Some(json!({ "title": "leak", "manifest": leak })),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    assert!(
        body["detail"].as_str().unwrap_or("").contains("token"),
        "{body}"
    );
    assert!(!body.to_string().contains("s3cr3t"));
    let (_, list) = call(&state, "editor", "GET", "helsinki/pipelinetemplates", None).await;
    assert_eq!(list["items"], json!([]));
    let (status, _) = call(
        &state,
        "editor",
        "POST",
        "helsinki/pipelinetemplates",
        Some(json!({ "title": "x", "manifest": pipeline(), "extra": 1 })),
    )
    .await;
    assert_eq!(status, 400, "an unknown field is refused");
}

#[tokio::test]
async fn a_reader_lists_and_saves_or_deletes_nothing() {
    let (_gitea, state) = world().await;
    let (_, saved) = call(
        &state,
        "editor",
        "POST",
        "helsinki/pipelinetemplates",
        Some(json!({ "title": "feed", "manifest": pipeline() })),
    )
    .await;
    let id = saved["id"].as_str().expect("an id").to_owned();
    let (status, _) = call(
        &state,
        "reader",
        "POST",
        "helsinki/pipelinetemplates",
        Some(json!({ "title": "mine", "manifest": pipeline() })),
    )
    .await;
    assert_eq!(status, 403);
    let (status, _) = call(
        &state,
        "reader",
        "DELETE",
        &format!("helsinki/pipelinetemplates/{id}"),
        None,
    )
    .await;
    assert_eq!(status, 403);
    let (status, _) = call(
        &state,
        "stranger",
        "GET",
        "helsinki/pipelinetemplates",
        None,
    )
    .await;
    assert_eq!(status, 404);

    let (status, _) = call(
        &state,
        "editor",
        "DELETE",
        &format!("helsinki/pipelinetemplates/{id}"),
        None,
    )
    .await;
    assert_eq!(status, 204);
    let (status, _) = call(
        &state,
        "editor",
        "DELETE",
        &format!("helsinki/pipelinetemplates/{id}"),
        None,
    )
    .await;
    assert_eq!(status, 404);
    let (status, _) = call(
        &state,
        "editor",
        "DELETE",
        "helsinki/pipelinetemplates/not-an-id",
        None,
    )
    .await;
    assert_eq!(status, 404);
}
