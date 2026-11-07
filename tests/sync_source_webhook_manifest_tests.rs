//! T-2399, MF-44: a `SyncSource`'s `spec.webhook` block through the resource route.
//!
//! The Portal validates a manifest with the jc-core it pins, and `SyncSourceSpec` there is
//! `deny_unknown_fields`. Until the pin carried `WebhookAuth`, a source could not name the secret
//! its webhook is signed with, so no source was reachable through the webhook route at all. Both
//! cases go through `POST /api/v1/projects/{project}/syncsources`, so the pinned copy is what is
//! exercised, not a restatement of it.

mod common;

use common::{checked_send, forge, person, state_on};
use joinedcontext_portal::resource::API_VERSION;
use serde_json::{json, Value};

const PROJECT: &str = "ovzdusie";

fn approver() -> joinedcontext_portal::auth::session::Identity {
    let mut identity = person("demo.steward");
    identity.groups = vec!["portal-approver".into()];
    identity
}

fn source(spec_extra: Value, schedule: Value) -> Value {
    let mut spec = json!({
        "source": { "git": { "url": "https://git.region.sk/udp/models.git", "ref": "main" } },
        "schedule": schedule,
        "mode": "mirror",
        "conflictPolicy": "replace"
    });
    if let (Some(spec), Some(extra)) = (spec.as_object_mut(), spec_extra.as_object()) {
        spec.extend(extra.clone());
    }
    json!({
        "apiVersion": API_VERSION,
        "kind": "SyncSource",
        "metadata": { "name": "regional", "namespace": PROJECT },
        "spec": spec
    })
}

/// MF-44: a webhook-driven source that names its own secret by reference is a write the Portal
/// accepts, and the merge request it opens carries the reference, never a value.
#[tokio::test]
async fn a_webhook_source_naming_its_secret_is_accepted() {
    let gitea = forge().await;
    let state = state_on(&gitea);
    let manifest = source(
        json!({ "webhook": {
            "secretRef": { "name": "regional-hook", "key": "secret" },
            "previousSecretRef": { "name": "regional-hook-old", "key": "secret" }
        } }),
        json!({ "webhook": true }),
    );

    let answer = checked_send(
        &state,
        approver(),
        "POST",
        &format!("/api/v1/projects/{PROJECT}/syncsources"),
        Some(manifest),
    )
    .await;

    assert_eq!(answer.status.as_u16(), 202, "{}", answer.text);
}

/// MF-44: a webhook schedule with no `spec.webhook` would be a door with no lock; it is refused
/// where it is written, and the refusal names the field the author has to add.
#[tokio::test]
async fn a_webhook_schedule_without_its_secret_is_refused_with_the_field_named() {
    let gitea = forge().await;
    let state = state_on(&gitea);

    let answer = checked_send(
        &state,
        approver(),
        "POST",
        &format!("/api/v1/projects/{PROJECT}/syncsources"),
        Some(source(json!({}), json!({ "webhook": true }))),
    )
    .await;

    assert_eq!(answer.status.as_u16(), 400, "{}", answer.text);
    assert!(
        answer.text.contains("spec.webhook"),
        "the refusal names spec.webhook: {}",
        answer.text
    );
}
