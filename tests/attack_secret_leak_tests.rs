//! Attack vector T-1685 (MF-24, CC-06, MF-17): a secret in a manifest, a log, an error, a Change
//! or an export.
//!
//! The platform names credentials in one place — `api::mutate::SECRET_KEYS` — and three defences
//! read that list: a write carrying one as a literal string is refused (MF-24), a Change's plan
//! diff redacts it (CC-06), and an export drops it (MF-17). The redaction used to hold a shorter
//! copy of the list, so `privateKey`, `accessToken`, `apiToken`, `passphrase`, `botToken` and the
//! rest were refused on the way in and printed in cleartext on the way out, to everybody who may
//! read the Change. These cases are the list's parity and the plan diff that proved it.
//!
//! What the existing suites already play, so this file does not repeat it:
//! `changes_tests::get_change_returns_redacted_plan_diff` (the five names the old copy knew),
//! `export_api_tests::the_yaml_stream_carries_the_project_without_status_or_secrets` (MF-17 for
//! `apiKey`), `edge_verdict_gate_tests::the_refusal_names_the_check_to_run_and_carries_nothing_of_the_manifest`
//! (a refusal carries nothing of the manifest it judged), and `mutate`'s own unit cases for
//! `find_literal_secret`.

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use joinedcontext_portal::api::changes::{is_sensitive_path, redact, ChangeProposal};
use joinedcontext_portal::api::mutate::SECRET_KEYS;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::plan::FieldChange;
use joinedcontext_portal::state::AppState;

/// The value that may never come back out of anything this file reads.
const LEAK: &str = "jc-leaked-credential-7f3a";

/// CC-06, MF-24: the redaction and the refusal read the same list. Every name a write is refused
/// for is a name the plan diff hides — at the root of a spec, nested, and under an array index,
/// which is how `is_sensitive_path` is actually asked. A name added to `SECRET_KEYS` tomorrow is
/// covered by this case without anybody remembering to add it twice.
#[test]
fn every_credential_the_platform_refuses_is_a_credential_it_redacts() {
    for key in SECRET_KEYS {
        for path in [
            (*key).to_owned(),
            format!("spec.auth.{key}"),
            format!("spec.inputs[0].{key}"),
        ] {
            assert!(
                is_sensitive_path(&path),
                "SECRET_KEYS names '{key}', so a write carrying it is refused (MF-24) — but the \
                 plan diff of a Change would print '{path}' in cleartext (CC-06)"
            );
        }
    }
}

/// CC-06: a field whose name is not a credential keeps its values, or a diff would be unreadable.
#[test]
fn a_field_that_is_not_a_credential_keeps_its_value() {
    for path in [
        "spec.url",
        "spec.tokenLifetimeSeconds",
        "spec.secretRef.name",
        "metadata.name",
    ] {
        assert!(!is_sensitive_path(path), "{path} is not a credential");
    }
}

/// CC-06: redaction replaces both sides and keeps the path, so an approver still reads which
/// field moved without reading what it moved to.
#[test]
fn redaction_keeps_the_path_and_takes_both_values() {
    let fields = SECRET_KEYS
        .iter()
        .map(|key| FieldChange {
            path: format!("spec.auth.{key}"),
            from: Some(Value::String(format!("old-{LEAK}"))),
            to: Some(Value::String(format!("new-{LEAK}"))),
        })
        .collect();
    for field in redact(fields) {
        assert!(
            field.path.starts_with("spec.auth."),
            "the path is what an approver needs: {}",
            field.path
        );
        assert_eq!(field.from, Some(Value::String("[REDACTED]".to_owned())));
        assert_eq!(field.to, Some(Value::String("[REDACTED]".to_owned())));
    }
}

/// A forge holding merge request 11: a ContextSpace of `ovzdusie` whose credentials change, with
/// both sides of the file mounted.
async fn forge_with_change() -> MockServer {
    let gitea = common::forge().await;
    Mock::given(method("GET"))
        .and(path(format!("{}/pulls/11", common::REPO)))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "number": 11,
            "html_url": "https://gitea.example/pulls/11",
            "state": "open",
            "title": "update ContextSpace mobility",
            "head": { "ref": "portal/update-contextspace-mobility-1685aaaa" },
            "base": { "ref": "main" },
            "created_at": "2026-09-18T09:14:22Z",
            "user": { "login": "dev.user", "full_name": "Dev User", "email": "dev@hel.fi" },
            "mergeable": true,
            "merged": false
        })))
        .mount(&gitea)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{}/pulls/11/files", common::REPO)))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "filename": "projects/ovzdusie/spaces/mobility/space.yaml", "status": "modified" }
        ])))
        .mount(&gitea)
        .await;
    for (git_ref, age) in [
        ("portal/update-contextspace-mobility-1685aaaa", "new"),
        ("main", "old"),
    ] {
        // One manifest carrying every name the platform calls a credential, so no name can be
        // redacted by accident of the fixture.
        let mut auth = serde_json::Map::new();
        for key in SECRET_KEYS {
            auth.insert((*key).to_owned(), Value::String(format!("{age}-{LEAK}")));
        }
        let manifest = json!({
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": "ContextSpace",
            "metadata": { "name": "mobility", "namespace": "ovzdusie" },
            "spec": { "isSandbox": age == "new", "auth": Value::Object(auth) },
        });
        let yaml = serde_norway::to_string(&manifest).expect("yaml");
        Mock::given(method("GET"))
            .and(path(format!(
                "{}/contents/projects/ovzdusie/spaces/mobility/space.yaml",
                common::REPO
            )))
            .and(query_param("ref", git_ref))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": format!("blob-{age}"),
                "content": common::encode(&yaml),
            })))
            .mount(&gitea)
            .await;
    }
    gitea
}

/// `reader@hel.fi` reads ContextSpace across the organization: enough to open the Change, which
/// is exactly the caller this vector is about.
fn state_with(gitea: &MockServer) -> AppState {
    let state = common::state_on(gitea);
    state.mirror.upsert(common::envelope(
        "Role",
        "reads-spaces",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["ContextSpace"], "verbs": ["read", "approve"] }] }),
    ));
    state.mirror.upsert(common::envelope(
        "RoleBinding",
        "reader-everywhere",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "user": "reader@hel.fi" }],
            "role": "reads-spaces",
            "scope": { "organization": "hel" },
        }),
    ));
    state
}

/// CC-06: the Change page is where a credential that reached the repository is read by everyone
/// who may review it. Every name the platform refuses on a write is `[REDACTED]` on both sides
/// here, and the value itself is nowhere in the answer.
#[tokio::test]
async fn no_credential_of_a_manifest_reaches_the_page_that_reviews_it() {
    let gitea = forge_with_change().await;
    let state = state_with(&gitea);
    let answer = common::send(
        &state,
        common::person("reader"),
        "GET",
        "/api/v1/projects/ovzdusie/changes/chg-0000000b",
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    assert!(
        !answer.text.contains(LEAK),
        "a credential of the manifest reached the review page: {}",
        answer.text
    );
    let proposal: ChangeProposal = serde_json::from_str(&answer.text).expect("a proposal");
    let fields = proposal.plan_fields.as_deref().unwrap_or_default();
    let redacted: Vec<&FieldChange> = fields
        .iter()
        .filter(|field| is_sensitive_path(&field.path))
        .collect();
    assert_eq!(
        redacted.len(),
        SECRET_KEYS.len(),
        "every credential of the manifest is a field of the diff: {fields:?}"
    );
    for field in redacted {
        assert_eq!(
            (field.from.as_ref(), field.to.as_ref()),
            (
                Some(&Value::String("[REDACTED]".to_owned())),
                Some(&Value::String("[REDACTED]".to_owned()))
            ),
            "{} came back in cleartext",
            field.path
        );
    }
}

/// MF-24: the check a person runs in the form refuses a credential typed into a manifest, names
/// the field so they can go and fix it, and never echoes the value they typed — the answer is
/// stored as a draft and read back by the page.
#[tokio::test]
async fn the_check_that_refuses_a_typed_credential_names_the_field_and_not_the_value() {
    let gitea = common::forge().await;
    let state = common::state_on(&gitea);
    for key in SECRET_KEYS {
        let manifest = json!({
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": "ContextSpace",
            "metadata": { "name": "mobility", "namespace": "ovzdusie" },
            "spec": { "auth": { *key: LEAK } },
        });
        let answer = common::send(
            &state,
            // The bootstrap group, so the refusal is the credential and never a missing role.
            bootstrap(),
            "POST",
            "/api/v1/projects/ovzdusie/spaces?dryRun=All",
            Some(manifest),
        )
        .await;
        assert!(
            !answer.text.contains(LEAK),
            "the check echoed the credential typed into '{key}': {}",
            answer.text
        );
        assert!(
            answer.text.contains(*key) && answer.text.contains("secretRef"),
            "the refusal of '{key}' has to name the field and the way to do it right: {}",
            answer.text
        );
    }
}

/// A member of the bootstrap group: everything, everywhere, so a refusal in this file is the
/// credential and not a missing binding.
fn bootstrap() -> joinedcontext_portal::auth::session::Identity {
    let mut identity = common::person("admin");
    identity.groups = vec!["portal-approver".to_owned()];
    identity
}
