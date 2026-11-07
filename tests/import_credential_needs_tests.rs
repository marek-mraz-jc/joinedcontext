//! An import names every credential its manifests reference, wherever the kind's schema puts it
//! (T-2564, CC-84, MF-35, CC-06): the reference is a need of the target project, named by its
//! path and never by the secret's name, key or value.

use joinedcontext_portal::api::import::{needs_of, Need};
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use serde_json::{json, Value};

const PROJECT: &str = "helsinki";

fn manifest(kind: &str, name: &str, spec: Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: kind.into(),
        metadata: ObjectMeta {
            name: name.into(),
            namespace: Some(PROJECT.into()),
            ..Default::default()
        },
        spec,
        status: None,
    }
}

fn secret_paths(needs: &[Need]) -> Vec<String> {
    needs
        .iter()
        .filter(|n| n.kind == "secret")
        .map(|n| n.location.clone())
        .collect()
}

/// Every `SecretRef` field of the jc-core kinds, each where its schema puts it.
#[test]
fn every_typed_credential_is_a_need_named_by_its_path() {
    let reference = |name: &str| json!({ "name": name, "key": "k" });
    let manifests = [
        manifest(
            "DataSource",
            "feed",
            json!({
                "type": "http",
                "http": { "url": "https://x", "authorization": { "headerRef": reference("feed-token") } },
                "mqtt": { "passwordRef": reference("mqtt-pass") },
                "tls": { "caCertRef": reference("feed-ca") },
            }),
        ),
        manifest(
            "CkanInstance",
            "portal",
            json!({ "apiTokenRef": reference("ckan-token") }),
        ),
        manifest(
            "Pipeline",
            "air",
            json!({ "secretRefs": [reference("air-key")] }),
        ),
        manifest(
            "SyncSource",
            "regional",
            json!({ "webhook": { "secretRef": reference("origin-signing"), "previousSecretRef": reference("origin-signing-old") } }),
        ),
        manifest(
            "Endpoint",
            "peer",
            json!({ "federation": { "cachedTokenSecretRef": reference("peer-token") } }),
        ),
        manifest(
            "DataspaceConnector",
            "edc",
            json!({ "auth": { "tokenSecretRef": reference("edc-token") } }),
        ),
    ];
    let needs = needs_of(&manifests, PROJECT);
    let found = secret_paths(&needs);
    for expected in [
        "DataSource/feed spec.http.authorization.headerRef",
        "DataSource/feed spec.mqtt.passwordRef",
        "DataSource/feed spec.tls.caCertRef",
        "CkanInstance/portal spec.apiTokenRef",
        "Pipeline/air spec.secretRefs",
        "SyncSource/regional spec.webhook.secretRef",
        "SyncSource/regional spec.webhook.previousSecretRef",
        "Endpoint/peer spec.federation.cachedTokenSecretRef",
        "DataspaceConnector/edc spec.auth.tokenSecretRef",
    ] {
        assert!(found.iter().any(|f| f == expected), "{expected}: {found:?}");
    }
    assert_eq!(found.len(), 9, "one need per reference: {found:?}");

    let said = serde_json::to_string(&needs).expect("serializes");
    for name in [
        "feed-token",
        "mqtt-pass",
        "feed-ca",
        "ckan-token",
        "air-key",
        "origin-signing",
        "peer-token",
        "edc-token",
    ] {
        assert!(
            !said.contains(name),
            "the secret name {name} reached the report: {said}"
        );
    }
}

/// A reference to something that is not a secret is not a need: a space, a model, an endpoint.
#[test]
fn a_reference_to_a_resource_is_not_a_secret_need() {
    let needs = needs_of(
        &[manifest(
            "ModelProjection",
            "view",
            json!({
                "contextSpaceRef": "air",
                "dataModelRef": { "kind": "DataModel", "name": "air", "version": "1" },
            }),
        )],
        PROJECT,
    );
    assert!(
        secret_paths(&needs).is_empty(),
        "{:?}",
        secret_paths(&needs)
    );
}

/// MF-35: a `secrets` list is a need of its own and each entry's `secretRef` one more, as before
/// the typed fields were added; a `secrets` that is not a list is walked, not reported.
#[test]
fn a_secrets_list_is_a_need_and_its_entries_are_still_walked() {
    let source = manifest(
        "DataSource",
        "custom",
        json!({ "secrets": [{ "env": "TOKEN", "secretRef": { "name": "n", "key": "k" } }] }),
    );
    assert_eq!(
        secret_paths(&needs_of(&[source], PROJECT)),
        [
            "DataSource/custom spec.secrets",
            "DataSource/custom spec.secrets[0].secretRef",
        ]
    );
    let odd = manifest(
        "Pipeline",
        "odd",
        json!({ "secrets": { "secretRef": { "name": "n" } } }),
    );
    assert_eq!(
        secret_paths(&needs_of(&[odd], PROJECT)),
        ["Pipeline/odd spec.secrets.secretRef"]
    );
}
