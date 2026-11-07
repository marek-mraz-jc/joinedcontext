//! The reconciler owns the Keycloak groups the repository declares (T-0866, PF-62, PF-63).
//!
//! Against a mocked Keycloak admin API: a manifest with no group in the realm is created and
//! filled, a member nobody put in the manifest is pruned, what somebody changed in the console
//! is overwritten and reported as drift, a group without the managed attribute is never
//! touched, a member the realm has no user for is a warning, not a failure, and a manifest
//! still naming members by e-mail leaves the group as the realm holds it (ADR-N-061).

use joinedcontext_portal::config::ClientAuth;
use std::sync::Arc;

use joinedcontext_portal::reconciler::groups::{
    derived_groups, DerivedGroup, GroupSync, MANAGED_BY, MANAGED_VALUE,
};
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, path_regex, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const REALM: &str = "/admin/realms/bb";

// Keycloak user ids, as a manifest names a person (ADR-N-061).
const LEAD: &str = "00000000-0000-4000-8000-00000000000a";
const SNEAKY: &str = "00000000-0000-4000-8000-00000000000b";
const NEWCOMER: &str = "00000000-0000-4000-8000-00000000000c";
const LEFT: &str = "00000000-0000-4000-8000-00000000000d";
const READER: &str = "00000000-0000-4000-8000-00000000000e";
const WRITER: &str = "00000000-0000-4000-8000-00000000000f";

fn group(name: &str, members: &[&str]) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: "Group".to_owned(),
        metadata: ObjectMeta::new(name, joinedcontext_portal::permissions::ORG_NAMESPACE),
        spec: json!({ "members": members.iter().map(|m| json!({ "user": m })).collect::<Vec<_>>() }),
        status: Some(joinedcontext_portal::resource::Status {
            phase: joinedcontext_portal::resource::Phase::Live,
            observed_revision: None,
            source_url: None,
            conditions: Vec::new(),
            build: None,
            shard: None,
            domain_verification: None,
            jobs: Vec::new(),
            notifications: None,
        }),
    }
}

fn mirror_with(groups: Vec<ResourceEnvelope>) -> Arc<Mirror> {
    let mirror = Arc::new(Mirror::new());
    for envelope in groups {
        mirror.upsert(envelope);
    }
    mirror
}

/// A realm that hands out a token and answers the reads a run makes; each test mounts the
/// groups and users it wants on top.
async fn realm() -> MockServer {
    let keycloak = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/realms/bb/protocol/openid-connect/token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "access_token": "admin-token", "expires_in": 60 })),
        )
        .mount(&keycloak)
        .await;
    keycloak
}

fn sync(keycloak: &MockServer) -> GroupSync {
    GroupSync::new(
        &format!("{}/realms/bb", keycloak.uri()),
        "portal-reconciler".to_owned(),
        ClientAuth::Secret("s3cret".into()),
    )
    .expect("a realm issuer")
}

/// What the run wrote, as `METHOD path`.
async fn wrote(keycloak: &MockServer) -> Vec<String> {
    keycloak
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.method.as_str() != "GET")
        .map(|r| format!("{} {}", r.method.as_str(), r.url.path()))
        .collect()
}

#[tokio::test]
async fn a_manifest_the_realm_does_not_have_is_created_and_filled() {
    let keycloak = realm().await;
    // Nothing in the realm yet, then the group the create made.
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/groups")))
        .and(query_param("briefRepresentation", "false"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&keycloak)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{REALM}/groups")))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&keycloak)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/groups")))
        .and(query_param("exact", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": "g1", "name": "city-leads", "attributes": { MANAGED_BY: [MANAGED_VALUE] } }
        ])))
        .mount(&keycloak)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/groups/g1/members")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&keycloak)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/users/{LEAD}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": LEAD })))
        .mount(&keycloak)
        .await;
    Mock::given(method("PUT"))
        .and(path_regex(format!("^{REALM}/users/.*/groups/.*$")))
        .respond_with(ResponseTemplate::new(204))
        .mount(&keycloak)
        .await;

    let mirror = mirror_with(vec![group("city-leads", &[LEAD])]);
    let outcomes = sync(&keycloak).converge(&mirror).await;

    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    assert_eq!(outcomes[0].error, None, "{outcomes:?}");
    assert!(
        outcomes[0].drift.iter().any(|d| d.contains("was created")),
        "{outcomes:?}"
    );
    assert!(
        outcomes[0].drift.iter().any(|d| d.contains("was added")),
        "{outcomes:?}"
    );
    let wrote = wrote(&keycloak).await;
    assert!(wrote.contains(&format!("POST {REALM}/groups")), "{wrote:?}");
    assert!(
        wrote.contains(&format!("PUT {REALM}/users/{LEAD}/groups/g1")),
        "{wrote:?}"
    );

    // The drift is on the manifest, where the Access page reads it (PF-62).
    joinedcontext_portal::reconciler::groups::record(&mirror, &outcomes);
    let held = mirror
        .get(
            joinedcontext_portal::permissions::ORG_NAMESPACE,
            "Group",
            "city-leads",
        )
        .expect("the group");
    let condition = &held.status.expect("status").conditions[0];
    assert_eq!(condition.r#type, "GroupSynced");
    assert_eq!(condition.reason.as_deref(), Some("DriftCorrected"));
}

#[tokio::test]
async fn a_console_edit_is_overwritten_and_reported_and_an_unknown_address_is_only_warned() {
    let keycloak = realm().await;
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/groups")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": "g1", "name": "city-leads", "attributes": { MANAGED_BY: [MANAGED_VALUE] } }
        ])))
        .mount(&keycloak)
        .await;
    // Somebody added themselves in the console, with no e-mail on the account; the manifest
    // names two other people.
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/groups/g1/members")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{ "id": SNEAKY }])))
        .mount(&keycloak)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/users/{LEAD}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": LEAD })))
        .mount(&keycloak)
        .await;
    // The realm has no user with this id.
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/users/{NEWCOMER}")))
        .respond_with(ResponseTemplate::new(404))
        .mount(&keycloak)
        .await;
    Mock::given(method("PUT"))
        .and(path_regex(format!("^{REALM}/users/.*/groups/.*$")))
        .respond_with(ResponseTemplate::new(204))
        .mount(&keycloak)
        .await;
    Mock::given(method("DELETE"))
        .and(path_regex(format!("^{REALM}/users/.*/groups/.*$")))
        .respond_with(ResponseTemplate::new(204))
        .mount(&keycloak)
        .await;

    let mirror = mirror_with(vec![group("city-leads", &[LEAD, NEWCOMER])]);
    let outcomes = sync(&keycloak).converge(&mirror).await;

    assert_eq!(outcomes[0].error, None, "{outcomes:?}");
    assert!(
        outcomes[0]
            .drift
            .iter()
            .any(|d| d.contains(SNEAKY) && d.contains("removed")),
        "the console edit is reported: {outcomes:?}"
    );
    assert!(
        outcomes[0].warnings.iter().any(|w| w.contains(NEWCOMER)),
        "an id with no user is a warning: {outcomes:?}"
    );
    let wrote = wrote(&keycloak).await;
    assert!(
        wrote.contains(&format!("DELETE {REALM}/users/{SNEAKY}/groups/g1")),
        "{wrote:?}"
    );
    assert!(
        wrote.contains(&format!("PUT {REALM}/users/{LEAD}/groups/g1")),
        "{wrote:?}"
    );
    assert!(!wrote.iter().any(|w| w.contains(NEWCOMER)), "{wrote:?}");
}

#[tokio::test]
async fn a_manifest_naming_members_by_e_mail_leaves_the_group_as_the_realm_holds_it() {
    let keycloak = realm().await;
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/groups")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": "g1", "name": "city-leads", "attributes": { MANAGED_BY: [MANAGED_VALUE] } }
        ])))
        .mount(&keycloak)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/groups/g1/members")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{ "id": LEAD }])))
        .mount(&keycloak)
        .await;

    let mirror = mirror_with(vec![group("city-leads", &["lead@hel.fi", NEWCOMER])]);
    let outcomes = sync(&keycloak).converge(&mirror).await;

    let error = outcomes[0].error.as_deref().unwrap_or_default();
    assert!(
        error.contains("1 member(s)") && error.contains("ADR-N-061"),
        "{outcomes:?}"
    );
    assert!(
        !error.contains("lead@hel.fi"),
        "no address in the outcome: {error}"
    );
    assert_eq!(
        wrote(&keycloak).await,
        vec!["POST /realms/bb/protocol/openid-connect/token".to_owned()],
        "nobody is removed or added"
    );
}

#[tokio::test]
async fn a_group_without_the_attribute_is_never_written_and_never_removed() {
    let keycloak = realm().await;
    // The bootstrap administrators' group, made by hand, plus a managed one the repository
    // no longer declares.
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/groups")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": "g0", "name": "platform-admins", "attributes": {} },
            { "id": "g7", "name": "old-team", "attributes": { MANAGED_BY: [MANAGED_VALUE] } }
        ])))
        .mount(&keycloak)
        .await;
    Mock::given(method("DELETE"))
        .and(path(format!("{REALM}/groups/g7")))
        .respond_with(ResponseTemplate::new(204))
        .mount(&keycloak)
        .await;

    // The repository declares a group that shares its name with the unmanaged one.
    let mirror = mirror_with(vec![group("platform-admins", &[])]);
    let outcomes = sync(&keycloak).converge(&mirror).await;

    let clash = outcomes
        .iter()
        .find(|o| o.name == "platform-admins")
        .expect("the manifest");
    assert!(
        clash
            .error
            .as_deref()
            .is_some_and(|err| err.contains("never writes a group it does not manage")),
        "{outcomes:?}"
    );
    let removed = outcomes
        .iter()
        .find(|o| o.name == "old-team")
        .expect("the managed group nothing declares");
    assert!(removed.error.is_none(), "{outcomes:?}");

    let wrote = wrote(&keycloak).await;
    assert_eq!(
        wrote,
        vec![
            "POST /realms/bb/protocol/openid-connect/token".to_owned(),
            format!("DELETE {REALM}/groups/g7"),
        ],
        "only the managed group nothing declares was written"
    );
}

#[tokio::test]
async fn a_realm_that_refuses_the_client_fails_the_wave_and_writes_nothing() {
    let keycloak = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/realms/bb/protocol/openid-connect/token"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({ "error": "unauthorized" })))
        .mount(&keycloak)
        .await;

    let mirror = mirror_with(vec![group("city-leads", &[LEAD])]);
    let outcomes = sync(&keycloak).converge(&mirror).await;

    assert_eq!(outcomes.len(), 1);
    assert!(
        outcomes[0]
            .error
            .as_deref()
            .is_some_and(|err| err.contains("refused the reconciler's client")),
        "{outcomes:?}"
    );
    assert!(wrote(&keycloak).await.len() == 1, "only the token request");
}

/// The secret never leaves the configuration: nothing this wave logs or reports carries it.
#[tokio::test]
async fn nothing_the_wave_answers_carries_the_client_secret() {
    let keycloak = realm().await;
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/groups")))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&keycloak)
        .await;
    let outcomes = sync(&keycloak).converge(&mirror_with(vec![])).await;
    let said: Value = json!(outcomes
        .iter()
        .map(|o| format!("{o:?}"))
        .collect::<Vec<_>>());
    assert!(!said.to_string().contains("s3cret"), "{said}");
}

/// AP-115: a run records the groups it may not write, where the write doors read them; a
/// managed group is this platform's own and is not among them.
#[tokio::test]
async fn a_run_records_the_realms_unmanaged_groups_for_the_write_doors() {
    let keycloak = realm().await;
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/groups")))
        .and(query_param("briefRepresentation", "false"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": "g1", "name": "admins", "attributes": {} },
            { "id": "g2", "name": "retired", "attributes": { MANAGED_BY: [MANAGED_VALUE] } }
        ])))
        .mount(&keycloak)
        .await;
    Mock::given(method("DELETE"))
        .and(path(format!("{REALM}/groups/g2")))
        .respond_with(ResponseTemplate::new(204))
        .mount(&keycloak)
        .await;
    let foreign = Arc::new(joinedcontext_portal::reconciler::foreign::ForeignNames::default());

    sync(&keycloak)
        .with_foreign(Arc::clone(&foreign))
        .converge(&mirror_with(vec![]))
        .await;

    assert!(foreign.has_group("admins"));
    assert!(!foreign.has_group("retired"));
}

/// PF-87, T-2655: a project's derived groups are kept like a manifest's: the one the realm holds
/// gets the members the bindings name and loses the one they do not, and a Group manifest that
/// takes a derived group's name keeps it, with the clash as the derived group's error.
fn derived(name: &str, member: &str, organization: Option<&str>) -> DerivedGroup {
    DerivedGroup {
        name: name.to_owned(),
        members: [member.to_owned()].into_iter().collect(),
        jc_org: None,
        jc_groups: organization.map(|organization| format!("/{organization}/{name}")),
    }
}

fn bound_group(organization: &str, member: &str) -> DerivedGroup {
    DerivedGroup {
        name: format!("{organization}_bound"),
        members: [member.to_owned()].into_iter().collect(),
        jc_org: Some(organization.to_owned()),
        jc_groups: None,
    }
}

/// T-6461, T-6470: a project's derived group names itself in `jc_groups` when it is created and
/// on a group already in the realm, and a project that moved Organization takes the path with it.
/// It names no Organization in `jc_org`, so one stamped by T-6461 loses that; only the
/// Organization's own group carries it. A group that already says so is not written.
#[tokio::test]
async fn a_derived_group_carries_the_organization_of_its_project() {
    let keycloak = realm().await;
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/groups")))
        .and(query_param("briefRepresentation", "false"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": "old", "name": "old-readers", "attributes": { MANAGED_BY: [MANAGED_VALUE] } },
            { "id": "moved", "name": "moved-readers", "attributes": {
                MANAGED_BY: [MANAGED_VALUE], "jc_groups": ["/hel/moved-readers"]
            } },
            { "id": "kept", "name": "kept-readers", "attributes": {
                MANAGED_BY: [MANAGED_VALUE], "jc_groups": ["/hel/kept-readers"]
            } },
            { "id": "stamped", "name": "stamped-readers", "attributes": {
                MANAGED_BY: [MANAGED_VALUE], "jc_org": ["hel"], "jc_groups": ["/hel/stamped-readers"]
            } },
            { "id": "org", "name": "hel_bound", "attributes": {
                MANAGED_BY: [MANAGED_VALUE], "jc_org": ["hel"]
            } }
        ])))
        .mount(&keycloak)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/groups")))
        .and(query_param("search", "new-readers"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!([{ "id": "new", "name": "new-readers" }])),
        )
        .mount(&keycloak)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{REALM}/groups")))
        .respond_with(ResponseTemplate::new(201))
        .mount(&keycloak)
        .await;
    Mock::given(method("PUT"))
        .and(path_regex(format!("^{REALM}/groups/(old|moved|stamped)$")))
        .respond_with(ResponseTemplate::new(204))
        .mount(&keycloak)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(format!("^{REALM}/groups/[a-z]+/members$")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{ "id": READER }])))
        .mount(&keycloak)
        .await;

    let generated = vec![
        derived("new-readers", READER, Some("hel")),
        derived("old-readers", READER, Some("hel")),
        derived("moved-readers", READER, Some("espoo")),
        derived("kept-readers", READER, Some("hel")),
        derived("stamped-readers", READER, Some("hel")),
        bound_group("hel", READER),
    ];
    let outcomes = sync(&keycloak)
        .converge_with(&mirror_with(Vec::new()), &generated)
        .await;
    assert!(outcomes.iter().all(|o| o.error.is_none()), "{outcomes:?}");

    let requests = keycloak.received_requests().await.expect("recorded");
    let wrote = |verb: &str, address: String| {
        requests
            .iter()
            .find(|r| r.method.as_str() == verb && r.url.path() == address)
            .map(|r| serde_json::from_slice::<Value>(&r.body).expect("a JSON body"))
    };
    let attributes = |organization: &str, group: &str| {
        json!({
            MANAGED_BY: [MANAGED_VALUE],
            "jc_groups": [format!("/{organization}/{group}")]
        })
    };
    let made = wrote("POST", format!("{REALM}/groups")).expect("the new group");
    assert_eq!(made["attributes"], attributes("hel", "new-readers"));
    let old = wrote("PUT", format!("{REALM}/groups/old")).expect("the old group");
    assert_eq!(old["name"], "old-readers");
    assert_eq!(old["attributes"], attributes("hel", "old-readers"));
    let moved = wrote("PUT", format!("{REALM}/groups/moved")).expect("the moved group");
    assert_eq!(moved["attributes"], attributes("espoo", "moved-readers"));
    let stamped = wrote("PUT", format!("{REALM}/groups/stamped")).expect("the stamped group");
    assert_eq!(stamped["attributes"], attributes("hel", "stamped-readers"));
    assert!(wrote("PUT", format!("{REALM}/groups/kept")).is_none());
    assert!(wrote("PUT", format!("{REALM}/groups/org")).is_none());
}

fn organizational(kind: &str, name: &str, spec: Value) -> ResourceEnvelope {
    ResourceEnvelope {
        kind: kind.to_owned(),
        spec,
        ..group(name, &[])
    }
}

fn bound_in(name: &str, person: &str, scope: Value) -> ResourceEnvelope {
    organizational(
        "RoleBinding",
        name,
        json!({ "subjects": [{ "user": person }], "role": "steward", "scope": scope }),
    )
}

/// EP-172, T-6461: membership follows bindings and nothing else. The binding of the project and
/// of its Organization count, with the project's Organization; one of another Organization, or
/// of another project, does not; taking a binding away takes the person out.
#[test]
fn the_derived_groups_follow_the_bindings_of_their_organization_only() {
    let world = || {
        vec![
            organizational("Organization", "hel", json!({})),
            organizational("Organization", "espoo", json!({})),
            organizational("Project", "doprava", json!({ "organizationRef": "hel" })),
            organizational("Project", "tapiola", json!({ "organizationRef": "espoo" })),
            organizational(
                "Role",
                "steward",
                json!({ "rules": [{ "kinds": ["*"], "verbs": ["read", "propose"] }] }),
            ),
            bound_in("hers", READER, json!({ "project": "doprava" })),
            bound_in("wide", LEAD, json!({ "organization": "hel" })),
            bound_in("theirs", SNEAKY, json!({ "organization": "espoo" })),
            bound_in("next-door", NEWCOMER, json!({ "project": "tapiola" })),
        ]
    };
    let projects = [("doprava".to_owned(), "doprava".to_owned())]
        .into_iter()
        .collect();
    let readers_of = |mirror: &Mirror| {
        derived_groups(mirror, &projects, chrono::Utc::now())
            .into_iter()
            .find(|group| group.name == "doprava-readers")
            .expect("the readers")
    };

    let readers = readers_of(&mirror_with(world()));
    assert_eq!(readers.jc_org, None);
    assert_eq!(readers.jc_groups.as_deref(), Some("/hel/doprava-readers"));
    assert_eq!(
        readers.members,
        [READER, LEAD].map(str::to_owned).into_iter().collect()
    );

    let unbound = world()
        .into_iter()
        .filter(|envelope| envelope.metadata.name != "hers")
        .collect();
    assert_eq!(
        readers_of(&mirror_with(unbound)).members,
        [LEAD.to_owned()].into_iter().collect()
    );
}

/// EP-173, T-6470: `jc_org` follows an Organization-scoped binding alone. An outside person bound
/// on one project sits in its groups and holds no Organization, whatever their binding's role; a
/// binding of another Organization, of another project or run out counts for nothing here; a
/// person of the Organization by `/hel/members` is no member of this group and needs none, the
/// realm's group gives them `jc_org`; taking the binding away takes the Organization away.
#[test]
fn only_an_organization_scoped_binding_gives_the_organization() {
    let vendor = NEWCOMER;
    let world = || {
        vec![
            organizational("Organization", "hel", json!({})),
            organizational("Organization", "espoo", json!({})),
            organizational("Project", "doprava", json!({ "organizationRef": "hel" })),
            organizational("Project", "tapiola", json!({ "organizationRef": "espoo" })),
            organizational(
                "Role",
                "steward",
                json!({ "rules": [{ "kinds": ["*"], "verbs": ["read", "propose"] }] }),
            ),
            bound_in("vendor", vendor, json!({ "project": "doprava" })),
            bound_in("wide", LEAD, json!({ "organization": "hel" })),
            bound_in("theirs", SNEAKY, json!({ "organization": "espoo" })),
            organizational(
                "RoleBinding",
                "over",
                json!({ "subjects": [{ "user": LEFT }], "role": "steward",
                        "scope": { "organization": "hel" },
                        "validity": { "notAfter": "2020-01-01T00:00:00Z" } }),
            ),
        ]
    };
    let projects = [
        ("doprava".to_owned(), "doprava".to_owned()),
        ("tapiola".to_owned(), "tapiola".to_owned()),
    ]
    .into_iter()
    .collect();
    let groups = |mirror: &Mirror| derived_groups(mirror, &projects, chrono::Utc::now());
    let find = |groups: &[DerivedGroup], name: &str| {
        groups
            .iter()
            .find(|group| group.name == name)
            .cloned()
            .expect(name)
    };

    let all = groups(&mirror_with(world()));
    let hel = find(&all, "hel_bound");
    assert_eq!(hel.jc_org.as_deref(), Some("hel"));
    assert_eq!(hel.members, [LEAD.to_owned()].into_iter().collect());
    let espoo = find(&all, "espoo_bound");
    assert_eq!(espoo.jc_org.as_deref(), Some("espoo"));
    assert_eq!(espoo.members, [SNEAKY.to_owned()].into_iter().collect());
    // The vendor reaches the project by name, and no group that gives them `jc_org` holds them.
    assert!(find(&all, "doprava-readers").members.contains(vendor));
    assert!(find(&all, "doprava-writers").members.contains(vendor));
    assert!(all
        .iter()
        .filter(|group| group.jc_org.is_some())
        .all(|group| !group.members.contains(vendor)));

    let unbound = world()
        .into_iter()
        .filter(|envelope| envelope.metadata.name != "wide")
        .collect();
    assert!(find(&groups(&mirror_with(unbound)), "hel_bound")
        .members
        .is_empty());
}

#[test]
fn a_service_account_in_an_organization_binding_joins_no_derived_group() {
    let mirror = mirror_with(vec![
        organizational("Organization", "hel", json!({})),
        organizational("Project", "doprava", json!({ "organizationRef": "hel" })),
        organizational(
            "Role",
            "steward",
            json!({ "rules": [{ "kinds": ["*"], "verbs": ["read", "propose"] }] }),
        ),
        bound_in("lead", LEAD, json!({ "organization": "hel" })),
        bound_in(
            "lane",
            "service-account-jc-build-lane",
            json!({ "organization": "hel" }),
        ),
    ]);
    let projects = [("doprava".to_owned(), "doprava".to_owned())]
        .into_iter()
        .collect();
    for group in derived_groups(&mirror, &projects, chrono::Utc::now()) {
        assert_eq!(
            group.members.iter().collect::<Vec<_>>(),
            vec![LEAD],
            "{}",
            group.name
        );
    }
}

#[tokio::test]
async fn a_projects_derived_groups_follow_its_bindings_and_never_take_a_manifests_name() {
    let keycloak = realm().await;
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/groups")))
        .and(query_param("briefRepresentation", "false"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": "r1", "name": "doprava-readers", "attributes": { MANAGED_BY: [MANAGED_VALUE] } },
            { "id": "w1", "name": "doprava-writers", "attributes": { MANAGED_BY: [MANAGED_VALUE] } }
        ])))
        .mount(&keycloak)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/groups/r1/members")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{ "id": LEFT }])))
        .mount(&keycloak)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/users/{READER}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": READER })))
        .mount(&keycloak)
        .await;
    Mock::given(method("PUT"))
        .and(path_regex(format!("^{REALM}/users/.*/groups/.*$")))
        .respond_with(ResponseTemplate::new(204))
        .mount(&keycloak)
        .await;
    Mock::given(method("DELETE"))
        .and(path_regex(format!("^{REALM}/users/.*/groups/.*$")))
        .respond_with(ResponseTemplate::new(204))
        .mount(&keycloak)
        .await;

    // The manifest holds `doprava-writers`, so the derived group of that name is refused.
    let mirror = mirror_with(vec![group("doprava-writers", &[])]);
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/groups/w1/members")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&keycloak)
        .await;
    let generated = vec![
        derived("doprava-readers", READER, None),
        derived("doprava-writers", WRITER, None),
    ];
    let outcomes = sync(&keycloak).converge_with(&mirror, &generated).await;

    let readers = outcomes
        .iter()
        .find(|o| o.name == "doprava-readers")
        .expect("the readers' outcome");
    assert_eq!(readers.error, None, "{outcomes:?}");
    let clash = outcomes
        .iter()
        .find(|o| o.name == "doprava-writers" && o.error.is_some())
        .expect("the clash is reported");
    assert!(
        clash
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("Rename the manifest"),
        "{clash:?}"
    );
    let wrote = wrote(&keycloak).await;
    assert!(
        wrote.contains(&format!("PUT {REALM}/users/{READER}/groups/r1")),
        "{wrote:?}"
    );
    assert!(
        wrote.contains(&format!("DELETE {REALM}/users/{LEFT}/groups/r1")),
        "{wrote:?}"
    );
    // The writer the bindings name is not written into the manifest's group.
    assert!(!wrote.iter().any(|w| w.contains("/groups/w1")), "{wrote:?}");
}
