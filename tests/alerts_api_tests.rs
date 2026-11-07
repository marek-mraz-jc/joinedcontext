//! Alerts a person chooses (API/01 §37, PL-71, T-3261): a reader of a pipeline subscribes, mutes
//! and stops; a caller who may not read it is told it is not there; e-mail is refused with its
//! reason; a notice the evaluator left is read only by its subscriber, and not once they lost read
//! on the pipeline.

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::{json, Value};

use common::{envelope, person, send};
use joinedcontext_portal::alerts::Evaluator;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::ResourceKey;
use joinedcontext_portal::state::AppState;

const ALERTS: &str = "/api/v1/alerts";
const NOTICES: &str = "/api/v1/alerts/notices";

async fn world() -> AppState {
    let state = common::state_on_forge().await;
    state.mirror.upsert(envelope(
        "Role",
        "pipeline-reader",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Pipeline", "ContextSpace"], "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "jana-reader",
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": "jana@hel.fi" }], "role": "pipeline-reader", "scope": { "project": "helsinki" } }),
    ));
    state.mirror.upsert(envelope(
        "Pipeline",
        "stations",
        "helsinki",
        json!({ "class": "resident", "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:helsinki:bikes" }),
    ));
    state
}

fn body(answer: &common::Answer) -> Value {
    serde_json::from_str(&answer.text).unwrap_or(Value::Null)
}

#[tokio::test]
async fn a_reader_subscribes_mutes_and_stops_and_nobody_else_touches_it() {
    let state = world().await;
    let new = json!({ "project": "helsinki", "scope": "pipeline", "target": "stations", "events": ["failure", "zero"], "delivery": "portal" });
    let made = send(&state, person("jana"), "PUT", ALERTS, Some(new.clone())).await;
    assert_eq!(made.status, StatusCode::OK, "{}", made.text);
    let id = body(&made)["id"].as_i64().expect("an id");
    assert_eq!(body(&made)["events"], json!(["failure", "zero"]));

    // A second PUT of the same target changes it.
    let changed = send(
        &state,
        person("jana"),
        "PUT",
        ALERTS,
        Some({
            let mut stale = new.clone();
            stale["events"] = json!(["stale"]);
            stale
        }),
    )
    .await;
    assert_eq!(body(&changed)["id"], id);
    let mine = send(&state, person("jana"), "GET", ALERTS, None).await;
    assert_eq!(body(&mine)["items"].as_array().map(Vec::len), Some(1));
    assert_eq!(body(&mine)["items"][0]["events"], json!(["stale"]));

    let muted = send(
        &state,
        person("jana"),
        "POST",
        &format!("{ALERTS}/{id}/mute"),
        Some(json!({ "for": "1d" })),
    )
    .await;
    assert_eq!(muted.status, StatusCode::OK, "{}", muted.text);
    assert!(body(&muted)["mutedUntil"].is_string());
    let unmuted = send(
        &state,
        person("jana"),
        "POST",
        &format!("{ALERTS}/{id}/mute"),
        Some(json!({ "for": null })),
    )
    .await;
    assert!(body(&unmuted)["mutedUntil"].is_null());
    let bad = send(
        &state,
        person("jana"),
        "POST",
        &format!("{ALERTS}/{id}/mute"),
        Some(json!({ "for": "a while" })),
    )
    .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);

    // Another person meets nothing of it.
    assert!(
        body(&send(&state, person("mikko"), "GET", ALERTS, None).await)["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        send(
            &state,
            person("mikko"),
            "POST",
            &format!("{ALERTS}/{id}/mute"),
            Some(json!({ "for": "1h" }))
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            &state,
            person("mikko"),
            "DELETE",
            &format!("{ALERTS}/{id}"),
            None
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            &state,
            person("jana"),
            "DELETE",
            &format!("{ALERTS}/{id}"),
            None
        )
        .await
        .status,
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn a_target_the_caller_may_not_read_is_not_there_and_a_bad_subscription_says_why() {
    let state = world().await;
    let new = |over: Value| {
        let mut base = json!({ "project": "helsinki", "scope": "pipeline", "target": "stations", "events": ["failure"], "delivery": "portal" });
        base.as_object_mut()
            .unwrap()
            .extend(over.as_object().unwrap().clone());
        Some(base)
    };
    assert_eq!(
        send(&state, person("mikko"), "PUT", ALERTS, new(json!({})))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            &state,
            person("jana"),
            "PUT",
            ALERTS,
            new(json!({ "target": "nobody" }))
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            &state,
            person("jana"),
            "PUT",
            ALERTS,
            new(json!({ "events": ["boom"] }))
        )
        .await
        .status,
        StatusCode::BAD_REQUEST
    );
    let mail = send(
        &state,
        person("jana"),
        "PUT",
        ALERTS,
        new(json!({ "delivery": "email" })),
    )
    .await;
    assert_eq!(mail.status, StatusCode::BAD_REQUEST);
    assert!(mail.text.contains("no mail relay"), "{}", mail.text);
    assert_eq!(
        send(
            &state,
            person("jana"),
            "PUT",
            ALERTS,
            new(json!({ "colour": "red" }))
        )
        .await
        .status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[tokio::test]
async fn a_notice_is_its_subscriber_s_and_goes_quiet_when_they_lose_read() {
    let state = world().await;
    let new = json!({ "project": "helsinki", "scope": "pipeline", "target": "stations", "events": ["failure"], "delivery": "portal" });
    assert_eq!(
        send(&state, person("jana"), "PUT", ALERTS, Some(new))
            .await
            .status,
        StatusCode::OK
    );

    let mut pipeline = state
        .mirror
        .get("helsinki", "Pipeline", "stations")
        .expect("the pipeline");
    pipeline.status = Some(serde_json::from_value(json!({ "phase": "Error", "conditions": [{ "type": "Ready", "status": "False", "message": "runner refused" }] })).unwrap());
    state.mirror.upsert(pipeline);
    Evaluator::new(
        Arc::clone(&state.mirror),
        Arc::clone(&state.quality),
        Arc::clone(&state.alerts),
    )
    .run(chrono::Utc::now())
    .await
    .expect("a run");

    let notices = send(&state, person("jana"), "GET", NOTICES, None).await;
    assert_eq!(notices.status, StatusCode::OK, "{}", notices.text);
    let notices = body(&notices);
    assert_eq!(notices["unread"], 1);
    assert_eq!(notices["items"][0]["change"], "opened");
    assert_eq!(notices["items"][0]["event"], "failure");
    assert_eq!(notices["items"][0]["detail"], "runner refused");
    let id = notices["items"][0]["id"].as_i64().unwrap();
    assert_eq!(
        send(
            &state,
            person("mikko"),
            "POST",
            &format!("{NOTICES}/{id}/read"),
            None
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            &state,
            person("jana"),
            "POST",
            &format!("{NOTICES}/{id}/read"),
            None
        )
        .await
        .status,
        StatusCode::NO_CONTENT
    );

    state.mirror.remove(&ResourceKey {
        namespace: ORG_NAMESPACE.into(),
        kind: "RoleBinding".into(),
        name: "jana-reader".into(),
    });
    let after = body(&send(&state, person("jana"), "GET", NOTICES, None).await);
    assert!(after["items"].as_array().unwrap().is_empty(), "{after}");
}

/// Agent alerts as notices (T-5301): an approver of the project subscribes to an agent profile;
/// a reader who is not one is refused naming the target, an unknown profile is not there; the
/// refusal spike reaches the approver's inbox and goes quiet when they lose the approver role.
#[tokio::test]
async fn an_approver_hears_of_an_agent_s_refusal_spike_and_a_reader_may_not_subscribe() {
    let state = world().await;
    state.mirror.upsert(envelope(
        "AgentProfile",
        "analyst",
        ORG_NAMESPACE,
        json!({ "model": { "maxTokensPerRun": 1000 } }),
    ));
    state.mirror.upsert(envelope(
        "Role",
        "approver",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Pipeline"], "verbs": ["read", "approve"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "eeva-approver",
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": "eeva@hel.fi" }], "role": "approver", "scope": { "project": "helsinki" } }),
    ));
    let new = |target: &str| json!({ "project": "helsinki", "scope": "agent", "target": target, "events": ["refusalSpike"], "delivery": "portal" });

    let reader = send(&state, person("jana"), "PUT", ALERTS, Some(new("analyst"))).await;
    assert_eq!(reader.status, StatusCode::FORBIDDEN, "{}", reader.text);
    assert!(reader.text.contains("project:"), "{}", reader.text);
    // Whether a profile exists is nothing a non-approver learns.
    let probe = send(&state, person("jana"), "PUT", ALERTS, Some(new("nobody"))).await;
    assert_eq!(probe.status, StatusCode::FORBIDDEN, "{}", probe.text);
    let unknown = send(&state, person("eeva"), "PUT", ALERTS, Some(new("nobody"))).await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND, "{}", unknown.text);
    let wrong = send(
        &state,
        person("eeva"),
        "PUT",
        ALERTS,
        Some(json!({ "project": "helsinki", "scope": "agent", "target": "analyst", "events": ["failure"], "delivery": "portal" })),
    )
    .await;
    assert_eq!(wrong.status, StatusCode::BAD_REQUEST, "{}", wrong.text);
    let made = send(&state, person("eeva"), "PUT", ALERTS, Some(new("analyst"))).await;
    assert_eq!(made.status, StatusCode::OK, "{}", made.text);
    assert_eq!(body(&made)["scope"], "agent");

    let run: joinedcontext_portal::agents::run::AgentRun = serde_json::from_value(json!({
        "id": "r1", "project": "helsinki", "appName": "air", "endpointName": "air",
        "endpointSlug": "air", "endpoints": [], "profile": "analyst", "kind": "data",
        "unattended": false, "appClass": "ui", "visibility": "project", "prompt": "p",
        "promptDigest": "sha256:p", "dataNeeds": [], "allowsWrite": false, "branch": "b",
        "pathPrefix": "p/", "status": "building", "ticketHash": "x", "files": {},
        "steps": 0, "tokensUsed": 0, "createdBy": "jana@hel.fi", "origin": "person",
        "starter": null, "createdAt": chrono::Utc::now().to_rfc3339(),
        "expiresAt": "2099-01-01T00:00:00Z",
    }))
    .expect("a run");
    state.agents.create_run(&run).await.expect("stored");
    for _ in 0..5 {
        state
            .agents
            .append_event(
                "r1",
                "tool",
                json!({ "tool": "jc_resource_list", "status": "failed", "error": "no" }),
            )
            .await
            .expect("an event");
    }
    Evaluator::new(
        Arc::clone(&state.mirror),
        Arc::clone(&state.quality),
        Arc::clone(&state.alerts),
    )
    .with_agent_runs(Arc::clone(&state.agents))
    .run(chrono::Utc::now())
    .await
    .expect("a run");

    let notices = body(&send(&state, person("eeva"), "GET", NOTICES, None).await);
    assert_eq!(notices["unread"], 1, "{notices}");
    assert_eq!(notices["items"][0]["event"], "refusalSpike");
    assert_eq!(notices["items"][0]["pipeline"], "analyst");
    assert!(
        body(&send(&state, person("jana"), "GET", NOTICES, None).await)["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    state.mirror.remove(&ResourceKey {
        namespace: ORG_NAMESPACE.into(),
        kind: "RoleBinding".into(),
        name: "eeva-approver".into(),
    });
    let after = body(&send(&state, person("eeva"), "GET", NOTICES, None).await);
    assert!(after["items"].as_array().unwrap().is_empty(), "{after}");
}
