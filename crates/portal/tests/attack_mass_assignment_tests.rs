//! Attack vector T-1686 (MF-05, MF-04, AP-11): mass assignment and unknown fields.
//!
//! Every field a caller sends is a field somebody chose to accept. `status` is the platform's own
//! computation, `metadata` has six members and no seventh, a Change's lane is classified from the
//! manifest and never read from the body, and an approver is the session. These cases send each of
//! them anyway.
//!
//! What the existing suites already play, so this file does not repeat it:
//! `mutate::tests` (the envelope's own `deny_unknown_fields` and the `status` refusal of MF-04),
//! `edge_change_lane_tests` (a lane is classified from the kind and the spec, with no body in
//! sight), `changes_tests::approve_self_approval_returns_403_and_makes_no_git_mutations` (who the
//! approver is, is the session). What was left is the sweep: the same five smuggled members
//! against the doors a person actually posts to.

mod common;

use serde_json::{json, Value};

use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::state::AppState;

const PROJECT: &str = "ovzdusie";

/// A member of the bootstrap group: everything, everywhere, so every refusal below is the field
/// and never a missing binding.
fn admin() -> Identity {
    let mut identity = common::person("admin");
    identity.groups = vec!["portal-approver".to_owned()];
    identity
}

fn space(extra: Value) -> Value {
    let mut manifest = json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": "ContextSpace",
        "metadata": { "name": "mobility", "namespace": PROJECT },
        "spec": { "isSandbox": true },
    });
    if let (Some(target), Some(extra)) = (manifest.as_object_mut(), extra.as_object()) {
        for (key, value) in extra {
            // A dotted key writes into the object it names, so a case can smuggle a member into
            // `metadata` or `spec` as easily as into the envelope.
            match key.split_once('.') {
                Some((parent, child)) => {
                    if let Some(parent) = target.get_mut(parent).and_then(Value::as_object_mut) {
                        parent.insert(child.to_owned(), value.clone());
                    }
                }
                None => {
                    target.insert(key.clone(), value.clone());
                }
            }
        }
    }
    manifest
}

/// Whether the check refused the manifest. A dry run answers `200` with `valid: false` and the
/// reason as a finding (T-2234: one shape for one outcome), so a refusal is read from the body
/// and not from the status alone — a `4xx` is a refusal too, for the faults that never reach a
/// verdict.
fn refused(answer: &common::Answer) -> bool {
    if answer.status.is_client_error() {
        return true;
    }
    serde_json::from_str::<Value>(&answer.text)
        .map(|body| body["valid"] == json!(false))
        .unwrap_or(false)
}

async fn check(state: &AppState, manifest: Value) -> common::Answer {
    common::send(
        state,
        admin(),
        "POST",
        &format!("/api/v1/projects/{PROJECT}/spaces?dryRun=All"),
        Some(manifest),
    )
    .await
}

/// MF-05, MF-04: every member nobody declared is refused by name, and the answer says which one.
/// `status` is the platform's own and `metadata.uid` is not a member of `metadata` at all.
#[tokio::test]
async fn a_member_nobody_declared_is_refused_and_named() {
    let state = common::state_on_forge().await;
    for (member, extra) in [
        (
            "uid",
            json!({ "metadata.uid": "00000000-0000-0000-0000-000000000000" }),
        ),
        ("owner", json!({ "owner": "admin@hel.fi" })),
        ("approvedBy", json!({ "approvedBy": "admin@hel.fi" })),
        ("lane", json!({ "lane": "green" })),
        (
            "createdAt",
            json!({ "metadata.createdAt": "2020-01-01T00:00:00Z" }),
        ),
        (
            "resourceVersion",
            json!({ "metadata.resourceVersion": "9" }),
        ),
        ("smuggled", json!({ "spec.smuggled": true })),
    ] {
        let answer = check(&state, space(extra)).await;
        assert!(
            answer.text.contains(member),
            "'{member}' was swallowed instead of refused by name: {}",
            answer.text
        );
        assert!(refused(&answer), "'{member}' was accepted: {}", answer.text);
    }
}

/// MF-04: `status` is written by the platform, never proposed. The refusal says so, and it says
/// it for a status that is merely empty as well — the rule is the field, not its content.
#[tokio::test]
async fn nobody_proposes_their_own_status() {
    let state = common::state_on_forge().await;
    for status in [json!({}), json!({ "phase": "Live" })] {
        let answer = check(&state, space(json!({ "status": status }))).await;
        assert!(
            refused(&answer),
            "a proposal wrote its own status: {}",
            answer.text
        );
        assert!(
            answer.text.contains("status"),
            "the refusal has to name the field: {}",
            answer.text
        );
    }
}

/// AP-11, AP-13a: the annotations the build lane writes back when it publishes an image are not
/// something a proposal carries — an App deploys the image this platform built and no other.
#[tokio::test]
async fn nobody_annotates_their_own_build() {
    let state = common::state_on_forge().await;
    for key in joinedcontext_portal::apps::converge::BUILT_ANNOTATIONS {
        let manifest = space(json!({ "metadata.annotations": { key: "sha256:dead" } }));
        let answer = check(&state, manifest).await;
        assert!(
            refused(&answer),
            "'{key}' was accepted from a proposal: {}",
            answer.text
        );
        assert!(
            answer.text.contains(key),
            "the refusal has to name the annotation: {}",
            answer.text
        );
    }
}

/// MF-05: the manifest a caller sends decides the kind, and the kind decides the plural — a body
/// claiming another kind than the route is refused, so nobody writes a `RoleBinding` through the
/// `spaces` door by putting the word in the body.
#[tokio::test]
async fn the_body_cannot_name_a_kind_the_route_does_not() {
    let state = common::state_on_forge().await;
    let mut manifest = space(json!({}));
    manifest["kind"] = json!("RoleBinding");
    let answer = check(&state, manifest).await;
    assert!(refused(&answer), "{}", answer.text);
    assert!(
        answer.text.contains("RoleBinding") && answer.text.contains("spaces"),
        "the refusal names both the kind and the plural: {}",
        answer.text
    );
}
