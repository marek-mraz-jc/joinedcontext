//! `needs_of` (T-2519; CC-84, MF-35, CC-06): what an import cannot copy, named where it is and
//! never with its value, so the operator fills it in afterwards.

use joinedcontext_portal::api::import::{needs_of, Need};
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use serde_json::{json, Value};

const PROJECT: &str = "ovzdusie";

fn manifest(kind: &str, name: &str, spec: Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: kind.into(),
        metadata: ObjectMeta::new(name, PROJECT),
        spec,
        status: None,
    }
}

/// `kind where` of every need, in order.
fn listed(needs: &[Need]) -> Vec<String> {
    needs
        .iter()
        .map(|need| format!("{} {}", need.kind, need.location))
        .collect()
}

/// Every need as its JSON, for the check that no value travels with it.
fn text(needs: &[Need]) -> String {
    serde_json::to_string(needs).expect("needs serialise")
}

/// T-2519, T-2564, CC-84, CC-06: a DataSource's feed credential is one need that names the
/// field where the schema puts it; the secret's name and key stay out of it.
#[test]
fn a_datasource_with_a_present_authorization_reports_one_need_naming_the_field_not_the_value() {
    let source = manifest(
        "DataSource",
        "feed",
        json!({ "http": { "url": "https://x", "authorization": { "headerRef": { "name": "feed-token-7f3a", "key": "token" } } } }),
    );
    let needs = needs_of(&[source], PROJECT);
    assert_eq!(
        listed(&needs),
        ["secret DataSource/feed spec.http.authorization.headerRef"]
    );
    assert!(
        !text(&needs).contains("feed-token-7f3a"),
        "{}",
        text(&needs)
    );
}

/// T-2519, CC-84: a `secretRef` at any depth is found with its full path, and what it names
/// (the secret's name and key) is never in the need.
#[test]
fn a_secret_ref_nested_three_levels_deep_is_found_with_its_full_path() {
    let pipeline = manifest(
        "Pipeline",
        "air",
        json!({ "sink": { "outputs": [{ "auth": { "secretRef": { "name": "sink-pass-91", "key": "p" } } }] } }),
    );
    let needs = needs_of(&[pipeline], PROJECT);
    assert_eq!(
        listed(&needs),
        ["secret Pipeline/air spec.sink.outputs[0].auth.secretRef"]
    );
    assert!(!text(&needs).contains("sink-pass-91"), "{}", text(&needs));
}

/// T-2519, MF-35: a `secrets` list is a need of its own, and a `secretRef` inside one of its
/// entries is one more.
#[test]
fn a_secrets_array_field_is_reported_and_still_walked_for_nested_secret_refs() {
    let source = manifest(
        "DataSource",
        "custom",
        json!({ "secrets": [{ "env": "TOKEN", "secretRef": { "name": "n", "key": "k" } }] }),
    );
    assert_eq!(
        listed(&needs_of(&[source], PROJECT)),
        [
            "secret DataSource/custom spec.secrets",
            "secret DataSource/custom spec.secrets[0].secretRef",
        ]
    );
    // A `secrets` that is not a list is not a list of secrets; one under it is still found.
    let odd = manifest(
        "Pipeline",
        "odd",
        json!({ "secrets": { "secretRef": { "name": "n" } } }),
    );
    assert_eq!(
        listed(&needs_of(&[odd], PROJECT)),
        ["secret Pipeline/odd spec.secrets.secretRef"]
    );
}

/// T-2519: a `secretRef` is one need; whatever it holds, even another `secretRef`, is not read.
#[test]
fn a_secretref_whose_value_is_an_object_is_not_recursed_into_after_being_reported() {
    let pipeline = manifest(
        "Pipeline",
        "p",
        json!({ "secretRef": { "name": "n", "secretRef": { "name": "inner" }, "secrets": ["x"] } }),
    );
    assert_eq!(
        listed(&needs_of(&[pipeline], PROJECT)),
        ["secret Pipeline/p spec.secretRef"]
    );
}

/// T-2519: `authorization: null` declares no credential.
#[test]
fn a_datasource_with_a_null_authorization_reports_no_need() {
    for spec in [json!({ "authorization": null }), json!({})] {
        let source = manifest("DataSource", "open", spec.clone());
        assert!(needs_of(&[source], PROJECT).is_empty(), "{spec}");
    }
}

/// T-2519: needs are per manifest, in manifest order, each naming its own resource.
#[test]
fn two_manifests_of_the_same_kind_each_get_their_own_needs_list() {
    let needs = needs_of(
        &[
            manifest(
                "Pipeline",
                "b",
                json!({ "x": { "secretRef": { "name": "one" } } }),
            ),
            manifest(
                "Pipeline",
                "a",
                json!({ "y": [{ "secretRef": { "name": "two" } }] }),
            ),
        ],
        PROJECT,
    );
    assert_eq!(
        listed(&needs),
        [
            "secret Pipeline/b spec.x.secretRef",
            "secret Pipeline/a spec.y[0].secretRef",
        ]
    );
}

/// T-2519: nothing the origin keeps, nothing needed: a key that only contains the word, a
/// `secretRef` as a value rather than a key, and plain data.
#[test]
fn a_manifest_with_no_secretref_anywhere_reports_zero_needs() {
    let space = manifest(
        "ContextSpace",
        "air",
        json!({ "secretRefName": "x", "note": "secretRef", "list": [1, "secrets"], "mySecrets": [] }),
    );
    assert!(needs_of(&[space], PROJECT).is_empty());
}

/// T-2519, CC-84: the link is the page of the resource's own kind in the project it lands in; a
/// kind the catalogue does not know links the resources page.
#[test]
fn the_link_field_points_at_the_resources_own_plural_route() {
    let secret = json!({ "secretRef": { "name": "n" } });
    for (kind, link) in [
        ("Pipeline", "/projects/ovzdusie/pipelines"),
        ("DataSource", "/projects/ovzdusie/datasources"),
        ("Nonsense", "/projects/ovzdusie/resources"),
    ] {
        let needs = needs_of(&[manifest(kind, "x", secret.clone())], PROJECT);
        assert_eq!(needs.len(), 1, "{kind}");
        assert_eq!(needs[0].link, link, "{kind}");
    }
}

#[test]
fn an_empty_manifests_list_returns_an_empty_needs_list() {
    assert!(needs_of(&[], PROJECT).is_empty());
}

/// T-2519, CC-84: a person named by a binding, a group or a Policy is a `person` need, one per
/// person, and never a `secret`; a group subject is nobody's person.
#[test]
fn a_role_binding_naming_a_person_is_reported_as_a_person_need_not_a_secret() {
    let needs = needs_of(
        &[
            manifest(
                "RoleBinding",
                "stewards",
                json!({ "role": "steward", "subjects": [{ "user": "jana@origin.sk" }, { "group": "city" }, { "user": "eva@origin.sk" }] }),
            ),
            manifest(
                "Group",
                "team",
                json!({ "members": [{ "user": "peter@origin.sk" }] }),
            ),
            manifest(
                "Policy",
                "one",
                json!({ "assignee": { "kind": "user", "id": "ida@origin.sk" } }),
            ),
            manifest(
                "Policy",
                "role",
                json!({ "assignee": { "kind": "role", "id": "public" } }),
            ),
        ],
        PROJECT,
    );
    assert_eq!(
        listed(&needs),
        [
            "person RoleBinding/stewards spec.subjects[0].user",
            "person RoleBinding/stewards spec.subjects[2].user",
            "person Group/team spec.members[0].user",
            "person Policy/one spec.assignee.id",
        ]
    );
    assert!(
        needs[0].why.starts_with("jana@origin.sk is a person"),
        "{}",
        needs[0].why
    );
}
