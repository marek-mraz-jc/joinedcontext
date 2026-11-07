//! People (T-2683, PF-90…PF-94, ADR-N-031, API/01 §24): the whole lifecycle of a person through
//! the realm's admin API, against a stub Keycloak. Every route is held to its verb on `Person`; no
//! route acts on a person who holds a right the caller lacks, so a reset's temporary password
//! never opens a stronger account; nobody disables or deletes themselves or the last Organization
//! Administrator; the temporary password is answered once and never logged; and deleting a
//! person named in the repository is a Change, the Keycloak user going only once it is merged.
//! The deletion's database half runs where `JC_PORTAL_TEST_DATABASE_URL` names a PostgreSQL
//! (ci-full), as the key store's tests do.

mod common;

use joinedcontext_portal::config::ClientAuth;
use std::io::Write;
use std::sync::{Arc, Mutex};

use axum::http::StatusCode;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use common::{envelope, forge, person, send, REPO};
use joinedcontext_portal::people::People;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;

const PEOPLE: &str = "/api/v1/organization/people";
const ADMIN: &str = "/admin/realms/hel";

/// The tests reading `person_deletions` share one database: one at a time, so a reconciler step
/// of one never finishes the other's removals.
static DATABASE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn user(id: &str, email: &str) -> Value {
    json!({
        "id": id, "username": email, "email": email, "firstName": "F", "lastName": "L",
        "enabled": true, "emailVerified": true, "createdTimestamp": 1_758_700_000_000_i64,
        "requiredActions": [], "attributes": { "locale": ["sk"] },
    })
}

/// A realm holding ada (the organization's administrator), pia (people-admin only) and jana (a
/// pipeline reader through the group `stewards`). `smtp` says whether the realm can send mail.
async fn realm(smtp: bool) -> MockServer {
    let kc = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/realms/hel/protocol/openid-connect/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "access_token": "t" })))
        .mount(&kc)
        .await;
    for (id, email) in [
        ("ada-id", "ada@hel.fi"),
        ("pia-id", "pia@hel.fi"),
        ("jana-id", "jana@hel.fi"),
    ] {
        Mock::given(method("GET"))
            .and(path(format!("{ADMIN}/users/{id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(user(id, email)))
            .mount(&kc)
            .await;
    }
    Mock::given(method("GET"))
        .and(path(format!("{ADMIN}/users/new-id")))
        .respond_with(ResponseTemplate::new(200).set_body_json(user("new-id", "new@example.org")))
        .mount(&kc)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{ADMIN}/users")))
        .respond_with(
            ResponseTemplate::new(201)
                .insert_header("Location", format!("{}{ADMIN}/users/new-id", kc.uri())),
        )
        .mount(&kc)
        .await;
    Mock::given(method("PUT"))
        .and(path_regex(format!(
            "^{ADMIN}/users/[a-z-]+/execute-actions-email$"
        )))
        .respond_with(if smtp {
            ResponseTemplate::new(204)
        } else {
            ResponseTemplate::new(500)
                .set_body_json(json!({ "errorMessage": "Failed to send execute actions email" }))
        })
        .mount(&kc)
        .await;
    for (verb, pattern) in [
        ("PUT", format!("^{ADMIN}/users/[a-z-]+$")),
        ("PUT", format!("^{ADMIN}/users/[a-z-]+/reset-password$")),
        ("POST", format!("^{ADMIN}/users/[a-z-]+/logout$")),
        ("DELETE", format!("^{ADMIN}/users/[a-z-]+$")),
    ] {
        Mock::given(method(verb))
            .and(path_regex(pattern))
            .respond_with(ResponseTemplate::new(204))
            .mount(&kc)
            .await;
    }
    for pattern in [
        "groups",
        "role-mappings/realm/composite",
        "sessions",
        "credentials",
    ] {
        Mock::given(method("GET"))
            .and(path_regex(format!("^{ADMIN}/users/[a-z-]+/{pattern}$")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .mount(&kc)
            .await;
    }
    // No `/{organization}/members` group: a realm before ADR-N-067 (a test mounts its own first).
    Mock::given(method("GET"))
        .and(path(format!("{ADMIN}/groups")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&kc)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{ADMIN}/users")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            user("ada-id", "ada@hel.fi"),
            user("jana-id", "jana@hel.fi"),
            user("pia-id", "pia@hel.fi"),
        ])))
        .mount(&kc)
        .await;
    kc
}

fn state_with(kc: &MockServer, gitea: &MockServer) -> AppState {
    let mut state = common::state_on(gitea);
    state.people = People::new(
        &format!("{}/realms/hel", kc.uri()),
        "portal".into(),
        ClientAuth::Secret("secret".into()),
    )
    .map(Arc::new);
    organization(&state);
    state
}

fn organization(state: &AppState) {
    let people =
        json!({ "kinds": ["Person"], "verbs": ["read", "create", "update", "disable", "delete"] });
    let mirror = &state.mirror;
    mirror.upsert(envelope(
        "Role",
        "people-admin",
        ORG_NAMESPACE,
        json!({ "rules": [people.clone()] }),
    ));
    mirror.upsert(envelope(
        "Role",
        "org-admin",
        ORG_NAMESPACE,
        json!({ "rules": [
            { "kinds": ["Pipeline", "Group", "Role", "RoleBinding"], "verbs": ["propose", "approve", "delete"] },
            people,
        ]}),
    ));
    mirror.upsert(envelope(
        "Role",
        "pipeline-reader",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Pipeline"], "verbs": ["read"] }] }),
    ));
    for (name, subjects, role) in [
        ("admins", json!([{ "user": "ada@hel.fi" }]), "org-admin"),
        (
            "people-admins",
            json!([{ "user": "pia@hel.fi" }]),
            "people-admin",
        ),
        (
            "readers",
            json!([{ "group": "stewards" }]),
            "pipeline-reader",
        ),
        (
            "jana-alone",
            json!([{ "user": "jana@hel.fi" }]),
            "pipeline-reader",
        ),
    ] {
        mirror.upsert(envelope(
            "RoleBinding",
            name,
            ORG_NAMESPACE,
            json!({ "subjects": subjects, "role": role, "scope": { "organization": "hel" } }),
        ));
    }
    mirror.upsert(envelope(
        "Group",
        "stewards",
        ORG_NAMESPACE,
        json!({ "members": [{ "user": "jana@hel.fi" }, { "user": "eva@hel.fi" }] }),
    ));
}

async fn received(kc: &MockServer, verb: &str, suffix: &str) -> Vec<Request> {
    kc.received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.method.as_str() == verb && r.url.path().ends_with(suffix))
        .collect()
}

#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);

impl Write for Log {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| std::io::Error::other("poisoned"))?
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// PF-92: creating a person sends the realm's execute-actions e-mail and answers no password.
#[tokio::test]
async fn creating_a_person_sends_the_realms_actions_email() {
    let (kc, gitea) = (realm(true).await, forge().await);
    let state = state_with(&kc, &gitea);
    let body = json!({ "email": " New@Example.org ", "firstName": "Nora", "lastName": "Nová", "locale": "sk" });

    let created = send(&state, person("pia"), "POST", PEOPLE, Some(body)).await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text);
    let answer: Value = serde_json::from_str(&created.text).expect("json");
    assert_eq!(answer["emailSent"], true);
    assert!(answer.get("temporaryPassword").is_none(), "{answer}");

    let posted = received(&kc, "POST", "/users").await;
    let user: Value = serde_json::from_slice(&posted[0].body).expect("a user");
    assert_eq!(user["email"], "new@example.org");
    assert_eq!(user["username"], "new@example.org");
    assert_eq!(user["attributes"]["locale"], json!(["sk"]));
    let actions = received(&kc, "PUT", "/new-id/execute-actions-email").await;
    let asked: Value = serde_json::from_slice(&actions[0].body).expect("actions");
    assert_eq!(asked, json!(["VERIFY_EMAIL", "UPDATE_PASSWORD"]));
}

/// ADR-N-067 §2.5 (T-6412): a person invited with no project joins `/{organization}/members` of
/// the installation's Organization, found by its exact path, so they sign in as its member.
#[tokio::test]
async fn an_invited_person_joins_the_organizations_members_group() {
    let (kc, gitea) = (realm(true).await, forge().await);
    Mock::given(method("GET"))
        .and(path(format!("{ADMIN}/groups")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": "espoo-id", "path": "/espoo", "subGroups": [{ "id": "espoo-members", "path": "/espoo/members" }] },
            { "id": "hel-id", "path": "/hel", "subGroups": [{ "id": "hel-members", "path": "/hel/members" }] },
        ])))
        .with_priority(1)
        .mount(&kc)
        .await;
    Mock::given(method("PUT"))
        .and(path(format!("{ADMIN}/users/new-id/groups/hel-members")))
        .respond_with(ResponseTemplate::new(204))
        .mount(&kc)
        .await;
    let state = state_with(&kc, &gitea);
    state.mirror.upsert(envelope(
        "Organization",
        "hel",
        ORG_NAMESPACE,
        json!({ "installation": true }),
    ));
    let body = json!({ "email": "new@example.org", "firstName": "Nora", "lastName": "Nová" });

    let created = send(&state, person("pia"), "POST", PEOPLE, Some(body)).await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text);
    let answer: Value = serde_json::from_str(&created.text).expect("json");
    assert_eq!(answer["memberOf"], "hel");
    assert_eq!(
        received(&kc, "PUT", "/users/new-id/groups/hel-members")
            .await
            .len(),
        1
    );
    assert!(received(&kc, "PUT", "/groups/espoo-members")
        .await
        .is_empty());
}

/// PF-92: without SMTP the creator gets a temporary password once; it never reaches the log.
#[tokio::test]
async fn without_smtp_a_temporary_password_is_answered_once_and_never_logged() {
    let (kc, gitea) = (realm(false).await, forge().await);
    let state = state_with(&kc, &gitea);
    let log = Log::default();
    let writer = log.clone();
    let _guard = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(move || writer.clone())
            .finish(),
    );

    let created = send(
        &state,
        person("pia"),
        "POST",
        PEOPLE,
        Some(json!({ "email": "new@example.org", "firstName": "Nora", "lastName": "Nová" })),
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text);
    let answer: Value = serde_json::from_str(&created.text).expect("json");
    assert_eq!(answer["emailSent"], false);
    let password = answer["temporaryPassword"]
        .as_str()
        .expect("a temporary password")
        .to_owned();
    assert!(
        password.len() >= 20,
        "the strictest realm policy asks for 20"
    );

    let set = received(&kc, "PUT", "/new-id/reset-password").await;
    let credential: Value = serde_json::from_slice(&set[0].body).expect("a credential");
    assert_eq!(
        credential["temporary"], true,
        "the person must set their own at the first login"
    );
    assert_eq!(credential["value"], password.as_str());

    let logged = String::from_utf8_lossy(&log.0.lock().expect("log")).into_owned();
    assert!(
        !logged.contains(&password),
        "the temporary password reached the log"
    );

    // Reading the person again answers no password: it was shown once.
    let read = send(
        &state,
        person("pia"),
        "GET",
        &format!("{PEOPLE}/new-id"),
        None,
    )
    .await;
    assert_eq!(read.status, StatusCode::OK, "{}", read.text);
    assert!(!read.text.contains(&password));
}

/// PF-93: disabling ends every session; the person is written back whole, `enabled: false`.
#[tokio::test]
async fn disabling_a_person_ends_their_sessions() {
    let (kc, gitea) = (realm(true).await, forge().await);
    let state = state_with(&kc, &gitea);

    let answer = send(
        &state,
        person("ada"),
        "POST",
        &format!("{PEOPLE}/jana-id/disable"),
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let put = received(&kc, "PUT", "/users/jana-id").await;
    let written: Value = serde_json::from_slice(&put[0].body).expect("a user");
    assert_eq!(written["enabled"], false);
    assert_eq!(
        written["attributes"]["locale"],
        json!(["sk"]),
        "what the Portal does not edit survives"
    );
    assert_eq!(
        received(&kc, "POST", "/users/jana-id/logout").await.len(),
        1
    );
}

/// PF-93 (integrator's review): a people-admin who is not an administrator cannot reset an
/// administrator's password, or anyone's who holds a right they lack; no password is set or sent.
#[tokio::test]
async fn a_people_admin_cannot_act_on_a_person_holding_a_right_they_lack() {
    let (kc, gitea) = (realm(false).await, forge().await);
    let state = state_with(&kc, &gitea);

    for target in ["ada-id", "jana-id"] {
        let refused = send(
            &state,
            person("pia"),
            "POST",
            &format!("{PEOPLE}/{target}/reset-password"),
            None,
        )
        .await;
        assert_eq!(
            refused.status,
            StatusCode::FORBIDDEN,
            "{target}: {}",
            refused.text
        );
        assert!(
            refused.text.contains("holds a right you do not"),
            "{}",
            refused.text
        );
        assert!(!refused.text.contains("temporaryPassword"));
    }
    assert!(
        received(&kc, "PUT", "/reset-password").await.is_empty(),
        "no password was set"
    );
    assert!(
        received(&kc, "PUT", "/execute-actions-email")
            .await
            .is_empty(),
        "no e-mail was sent"
    );

    // A person holding no more than the caller is theirs to reset.
    let own_level = send(
        &state,
        person("ada"),
        "POST",
        &format!("{PEOPLE}/pia-id/reset-password"),
        None,
    )
    .await;
    assert_eq!(own_level.status, StatusCode::OK, "{}", own_level.text);
}

/// PF-93, PF-03: nobody disables or deletes themselves, or the last Organization Administrator.
#[tokio::test]
async fn the_last_administrator_is_not_disabled_or_deleted() {
    let (kc, gitea) = (realm(true).await, forge().await);
    let state = state_with(&kc, &gitea);

    for (verb, route) in [
        ("POST", format!("{PEOPLE}/ada-id/disable")),
        ("DELETE", format!("{PEOPLE}/ada-id")),
    ] {
        let refused = send(&state, person("ada"), verb, &route, None).await;
        assert_eq!(
            refused.status,
            StatusCode::CONFLICT,
            "{route}: {}",
            refused.text
        );
        assert!(refused.text.contains("yourself"), "{}", refused.text);
    }
    // The bootstrap group may do everything, except leave the organization without its administrator.
    let mut root = person("root");
    root.groups = vec![state.config.bootstrap_admins.clone()];
    let refused = send(
        &state,
        root,
        "POST",
        &format!("{PEOPLE}/ada-id/disable"),
        None,
    )
    .await;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.text);
    assert!(
        refused.text.contains("last Organization Administrator"),
        "{}",
        refused.text
    );
    assert!(received(&kc, "PUT", "/users/ada-id").await.is_empty());
}

/// PF-03 (integrator's review): a bulk removal naming every Organization Administrator is
/// refused, though none of them is the last one alone; the realm hears nothing.
#[tokio::test]
async fn a_bulk_removal_never_takes_every_administrator() {
    let (kc, gitea) = (realm(true).await, forge().await);
    let state = state_with(&kc, &gitea);
    state.mirror.upsert(envelope(
        "RoleBinding",
        "admins-two",
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": "pia@hel.fi" }], "role": "org-admin", "scope": { "organization": "hel" } }),
    ));
    let mut root = person("root");
    root.groups = vec![state.config.bootstrap_admins.clone()];
    let refused = send(
        &state,
        root,
        "POST",
        &format!("{PEOPLE}/remove"),
        Some(json!({ "ids": ["ada-id", "pia-id"] })),
    )
    .await;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.text);
    assert!(
        refused.text.contains("every Organization Administrator"),
        "{}",
        refused.text
    );
    assert!(received(&kc, "PUT", "/users/ada-id").await.is_empty());
    assert!(received(&kc, "PUT", "/users/pia-id").await.is_empty());
}

/// PF-91: every route refuses a caller whose role grants nothing on Person, with 403.
#[tokio::test]
async fn a_caller_without_people_admin_is_refused_on_every_route() {
    let (kc, gitea) = (realm(true).await, forge().await);
    let state = state_with(&kc, &gitea);
    let routes = [
        ("GET", PEOPLE.to_owned(), None),
        (
            "POST",
            PEOPLE.to_owned(),
            Some(json!({ "email": "x@example.org", "firstName": "X", "lastName": "Y" })),
        ),
        ("GET", format!("{PEOPLE}/jana-id"), None),
        (
            "PATCH",
            format!("{PEOPLE}/jana-id"),
            Some(json!({ "firstName": "Z" })),
        ),
        ("POST", format!("{PEOPLE}/jana-id/disable"), None),
        ("POST", format!("{PEOPLE}/jana-id/enable"), None),
        ("POST", format!("{PEOPLE}/jana-id/reset-password"), None),
        (
            "POST",
            format!("{PEOPLE}/jana-id/remove-second-factor"),
            None,
        ),
        ("POST", format!("{PEOPLE}/jana-id/sign-out"), None),
        ("DELETE", format!("{PEOPLE}/jana-id"), None),
        (
            "POST",
            format!("{PEOPLE}/remove"),
            Some(json!({ "ids": ["jana-id"] })),
        ),
    ];
    for (verb, route, body) in routes {
        let refused = send(&state, person("jana"), verb, &route, body).await;
        assert_eq!(
            refused.status,
            StatusCode::FORBIDDEN,
            "{verb} {route}: {}",
            refused.text
        );
        assert!(
            refused.text.contains("Person"),
            "{verb} {route}: {}",
            refused.text
        );
    }
    let wrote = kc
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .any(|r| r.url.path().starts_with(ADMIN));
    assert!(!wrote, "nothing reached the realm's admin API");
}

/// T-5000: a page of people asks the realm for each person's sessions a few at a time. The home
/// page lists 50, and 50 session lookups sent together met Keycloak's pool of 7 and answered 503.
/// Each lookup takes `DELAY`, so a page sent at most four at a time takes at least
/// ceil(40 / 4) x `DELAY`; sent all at once it takes about one `DELAY`.
#[tokio::test]
async fn a_page_of_people_asks_for_their_sessions_a_few_at_a_time() {
    const DELAY: std::time::Duration = std::time::Duration::from_millis(100);
    let kc = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/realms/hel/protocol/openid-connect/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "access_token": "t" })))
        .mount(&kc)
        .await;
    let users: Vec<Value> = (0..40)
        .map(|n| user(&format!("p-{n}"), &format!("p{n}@hel.fi")))
        .collect();
    Mock::given(method("GET"))
        .and(path(format!("{ADMIN}/users")))
        .respond_with(ResponseTemplate::new(200).set_body_json(users))
        .mount(&kc)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(format!("^{ADMIN}/users/p-[0-9]+/sessions$")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!([{ "lastAccess": 1_758_700_000_000_i64 }]))
                .set_delay(DELAY),
        )
        .mount(&kc)
        .await;
    let gitea = forge().await;
    let state = state_with(&kc, &gitea);

    let started = std::time::Instant::now();
    let listed = send(&state, person("ada"), "GET", PEOPLE, None).await;
    let took = started.elapsed();

    assert_eq!(listed.status, StatusCode::OK, "{}", listed.text);
    let page: Value = serde_json::from_str(&listed.text).unwrap_or_default();
    let items = page["items"].as_array().cloned().unwrap_or_default();
    assert_eq!(items.len(), 40);
    assert!(
        items.iter().all(|item| item["lastSeen"].is_string()),
        "every person keeps their last access"
    );
    assert_eq!(received(&kc, "GET", "/sessions").await.len(), 40);
    assert!(
        took >= DELAY * 10,
        "40 session lookups took {took:?}: they were sent more than four at a time"
    );
}

/// PF-92: an e-mail another person has is a 409 naming it; bad input is a 400 in RFC 7807.
#[tokio::test]
async fn a_taken_email_is_a_conflict_and_bad_input_a_bad_request() {
    let (kc, gitea) = (realm(true).await, forge().await);
    Mock::given(method("POST"))
        .and(path(format!("{ADMIN}/users")))
        .respond_with(
            ResponseTemplate::new(409)
                .set_body_json(json!({ "errorMessage": "User exists with same email" })),
        )
        .with_priority(1)
        .mount(&kc)
        .await;
    let state = state_with(&kc, &gitea);

    let taken = send(
        &state,
        person("pia"),
        "POST",
        PEOPLE,
        Some(json!({ "email": "jana@hel.fi", "firstName": "J", "lastName": "K" })),
    )
    .await;
    assert_eq!(taken.status, StatusCode::CONFLICT, "{}", taken.text);
    assert!(taken.text.contains("jana@hel.fi"), "{}", taken.text);

    for body in [
        json!({ "email": "not-an-address", "firstName": "J", "lastName": "K" }),
        json!({ "email": "a@example.org", "firstName": " ", "lastName": "K" }),
        json!({ "email": "a@example.org", "firstName": "J", "lastName": "K", "locale": "fr" }),
        json!({ "email": "a@example.org", "firstName": "J", "lastName": "K", "password": "x" }),
    ] {
        let refused = send(&state, person("pia"), "POST", PEOPLE, Some(body.clone())).await;
        assert_eq!(
            refused.status,
            StatusCode::BAD_REQUEST,
            "{body}: {}",
            refused.text
        );
        assert!(
            refused.text.contains("\"status\":400"),
            "RFC 7807: {}",
            refused.text
        );
    }
    let paged = send(
        &state,
        person("pia"),
        "GET",
        &format!("{PEOPLE}?max=500"),
        None,
    )
    .await;
    assert_eq!(paged.status, StatusCode::BAD_REQUEST, "{}", paged.text);
}

/// PF-94: a person's page lists their groups, platform roles with scope and way, and app roles.
#[tokio::test]
async fn a_persons_page_names_their_groups_and_roles() {
    let (kc, gitea) = (realm(true).await, forge().await);
    let state = state_with(&kc, &gitea);
    state.mirror.upsert(envelope(
        "App",
        "alerts",
        "helsinki",
        json!({ "kind": "static", "roles": [{ "name": "viewer" }], "access": [{ "role": "viewer", "subjects": [{ "group": "stewards" }] }] }),
    ));

    let read = send(
        &state,
        person("pia"),
        "GET",
        &format!("{PEOPLE}/jana-id"),
        None,
    )
    .await;
    assert_eq!(read.status, StatusCode::OK, "{}", read.text);
    let detail: Value = serde_json::from_str(&read.text).expect("json");
    assert_eq!(detail["person"]["email"], "jana@hel.fi");
    assert_eq!(detail["groups"], json!([{ "name": "stewards" }]));
    let ways: Vec<Value> = detail["platformRoles"]
        .as_array()
        .expect("roles")
        .iter()
        .map(|r| r["via"].clone())
        .collect();
    assert!(
        ways.contains(&json!({ "group": "stewards" }))
            && ways.contains(&json!({ "user": "jana@hel.fi" })),
        "{ways:?}"
    );
    assert_eq!(
        detail["appRoles"],
        json!([{ "project": "helsinki", "app": "alerts", "role": "viewer", "via": { "group": "stewards" } }])
    );
}

/// PF-93: deleting a person named in the repository proposes one Change taking them out of every
/// Group and RoleBinding, disables them at once, and deletes the Keycloak user only once that
/// Change is merged. Needs a PostgreSQL (ci-full); without one it says so and checks nothing.
#[tokio::test]
async fn deleting_a_person_proposes_the_change_and_deletes_them_only_after_the_merge() {
    let Some(url) = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())
    else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let _alone = DATABASE.lock().await;
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("connect + migrate");
    joinedcontext_portal::db::remove_person_deletion(&pool, "jana-id")
        .await
        .expect("a clean row");
    let (kc, gitea) = (realm(true).await, forge().await);
    Mock::given(method("GET"))
        .and(path_regex(format!("^{REPO}/contents/.+")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "sha": "blob-1", "content": "" })),
        )
        .with_priority(5)
        .mount(&gitea)
        .await;
    let mut state = state_with(&kc, &gitea).with_db(pool.clone());
    state.people = People::new(
        &format!("{}/realms/hel", kc.uri()),
        "portal".into(),
        ClientAuth::Secret("secret".into()),
    )
    .map(Arc::new);

    let answer = send(
        &state,
        person("ada"),
        "DELETE",
        &format!("{PEOPLE}/jana-id"),
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.text);

    let commits: Vec<Value> = gitea
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path() == format!("{REPO}/contents"))
        .filter_map(|r| serde_json::from_slice(&r.body).ok())
        .collect();
    let files = commits[0]["files"].as_array().expect("files").clone();
    let find = |suffix: &str| {
        files
            .iter()
            .find(|f| f["path"].as_str().is_some_and(|p| p.ends_with(suffix)))
            .cloned()
    };
    assert_eq!(
        find("groups/stewards.yaml").expect("the group is edited")["operation"],
        "upload"
    );
    assert_eq!(
        find("jana-alone.yaml").expect("her own binding goes")["operation"],
        "delete"
    );
    assert!(
        find("readers.yaml").is_none(),
        "a binding naming her group only is not touched"
    );

    let disabled = received(&kc, "PUT", "/users/jana-id").await;
    assert_eq!(
        serde_json::from_slice::<Value>(&disabled[0].body).expect("user")["enabled"],
        false
    );
    assert!(
        received(&kc, "DELETE", "/users/jana-id").await.is_empty(),
        "not before the merge"
    );

    // A second removal while this one is open is refused.
    let again = send(
        &state,
        person("ada"),
        "DELETE",
        &format!("{PEOPLE}/jana-id"),
        None,
    )
    .await;
    assert_eq!(again.status, StatusCode::CONFLICT, "{}", again.text);

    let pull = |merged: bool, state: &str| json!({ "number": 9, "html_url": "https://gitea.example/pulls/9", "state": state, "merged": merged });
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/pulls/9")))
        .respond_with(ResponseTemplate::new(200).set_body_json(pull(false, "open")))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&gitea)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/pulls/9")))
        .respond_with(ResponseTemplate::new(200).set_body_json(pull(true, "closed")))
        .with_priority(2)
        .mount(&gitea)
        .await;
    let people = state.people.clone().expect("people");
    let forge_client = state.forge_for(ORG_NAMESPACE).expect("forge");
    joinedcontext_portal::api::people::finish_deletions(&people, &pool, &forge_client).await;
    assert!(
        received(&kc, "DELETE", "/users/jana-id").await.is_empty(),
        "the Change is still open"
    );
    joinedcontext_portal::api::people::finish_deletions(&people, &pool, &forge_client).await;
    assert_eq!(
        received(&kc, "DELETE", "/users/jana-id").await.len(),
        1,
        "merged: the user goes"
    );
    assert!(joinedcontext_portal::db::person_deletion(&pool, "jana-id")
        .await
        .expect("read")
        .is_none());
}

/// A person nothing in the repository names is deleted at once.
#[tokio::test]
async fn a_person_nothing_names_is_deleted_at_once() {
    let (kc, gitea) = (realm(true).await, forge().await);
    let state = state_with(&kc, &gitea);

    let answer = send(
        &state,
        person("ada"),
        "DELETE",
        &format!("{PEOPLE}/pia-id"),
        None,
    )
    .await;
    // pia is named by `people-admins`: that is a Change, so give her a colleague who is named nowhere.
    assert_eq!(
        answer.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "no database holds the pending removal: {}",
        answer.text
    );
    Mock::given(method("GET"))
        .and(path(format!("{ADMIN}/users/eva-id")))
        .respond_with(ResponseTemplate::new(200).set_body_json(user("eva-id", "eve@example.org")))
        .mount(&kc)
        .await;
    let answer = send(
        &state,
        person("ada"),
        "DELETE",
        &format!("{PEOPLE}/eva-id"),
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT, "{}", answer.text);
    assert_eq!(received(&kc, "DELETE", "/users/eva-id").await.len(), 1);
}

/// Mounts realm users the shared realm lacks: `eva` a member of `stewards`, two named nowhere.
async fn colleagues(kc: &MockServer) {
    for (id, email) in [
        ("eva-id", "eva@hel.fi"),
        ("ola-id", "ola@example.org"),
        ("ivo-id", "ivo@example.org"),
    ] {
        Mock::given(method("GET"))
            .and(path(format!("{ADMIN}/users/{id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(user(id, email)))
            .mount(kc)
            .await;
    }
}

/// The `person.changed` entries a subscriber to the organization's feed has seen so far.
fn person_changes(
    feed: &mut tokio::sync::broadcast::Receiver<joinedcontext_portal::activity::ActivityEvent>,
) -> Vec<Value> {
    let mut seen = Vec::new();
    while let Ok(event) = feed.try_recv() {
        if event.kind == "person.changed" {
            seen.push(event.details);
        }
    }
    seen
}

/// T-5606: a bulk removal holds each person to the single route's rules before anything is
/// written: the bound, the caller in the list, a right the caller lacks, an unknown id.
#[tokio::test]
async fn a_bulk_removal_holds_each_person_to_the_single_routes_rules() {
    let (kc, gitea) = (realm(true).await, forge().await);
    colleagues(&kc).await;
    let state = state_with(&kc, &gitea);
    let remove = |who: &'static str, ids: Value| {
        let state = state.clone();
        async move {
            send(
                &state,
                person(who),
                "POST",
                &format!("{PEOPLE}/remove"),
                Some(json!({ "ids": ids })),
            )
            .await
        }
    };

    let too_many: Vec<String> = (0..201).map(|n| format!("p{n}-id")).collect();
    for ids in [json!([]), json!(too_many)] {
        let refused = remove("ada", ids).await;
        assert_eq!(
            refused.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{}",
            refused.text
        );
        assert!(refused.text.contains("1 to 200"), "{}", refused.text);
    }
    let refused = remove("ada", json!(["ola-id", "ada-id"])).await;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.text);
    assert!(refused.text.contains("yourself"), "{}", refused.text);

    let refused = remove("pia", json!(["ola-id", "jana-id"])).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.text);
    assert!(
        refused
            .text
            .contains("jana@hel.fi holds a right you do not"),
        "names the person: {}",
        refused.text
    );

    let refused = remove("ada", json!(["ola-id", "nobody-id"])).await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND, "{}", refused.text);
    assert!(refused.text.contains("nobody-id"), "{}", refused.text);

    let wrote = kc
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .any(|r| r.method.as_str() != "GET" && r.url.path().starts_with(ADMIN));
    assert!(!wrote, "a refused removal changes nobody");
}

/// T-5606: people nothing in the repository names are deleted at once, one `person.changed` entry
/// each naming the actor, and an id sent twice is one person.
#[tokio::test]
async fn people_nothing_names_are_removed_at_once_each_on_the_feed() {
    let (kc, gitea) = (realm(true).await, forge().await);
    colleagues(&kc).await;
    let state = state_with(&kc, &gitea);
    let mut feed = state.activity_events.subscribe(ORG_NAMESPACE).await;

    let answer = send(
        &state,
        person("ada"),
        "POST",
        &format!("{PEOPLE}/remove"),
        Some(json!({ "ids": ["ola-id", "ivo-id", "ola-id"] })),
    )
    .await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT, "{}", answer.text);
    assert_eq!(received(&kc, "DELETE", "/users/ola-id").await.len(), 1);
    assert_eq!(received(&kc, "DELETE", "/users/ivo-id").await.len(), 1);
    let seen = person_changes(&mut feed);
    assert_eq!(seen.len(), 2, "{seen:?}");
    for entry in &seen {
        assert_eq!(entry["actor"], "ada", "{entry}");
        assert_eq!(entry["action"], "deleted", "{entry}");
    }
}

/// T-5606: a department of people named in the repository leaves in ONE Change listing every
/// edit, each person disabled at once and on the feed, and the merge deletes all of them. Needs a
/// PostgreSQL (ci-full); without one it says so and checks nothing.
#[tokio::test]
async fn a_department_leaves_in_one_change_and_one_merge() {
    let Some(url) = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())
    else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let _alone = DATABASE.lock().await;
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("connect + migrate");
    for id in ["jana-id", "eva-id", "ola-id"] {
        joinedcontext_portal::db::remove_person_deletion(&pool, id)
            .await
            .expect("a clean row");
    }
    let (kc, gitea) = (realm(true).await, forge().await);
    colleagues(&kc).await;
    Mock::given(method("GET"))
        .and(path_regex(format!("^{REPO}/contents/.+")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "sha": "blob-1", "content": "" })),
        )
        .with_priority(5)
        .mount(&gitea)
        .await;
    let state = state_with(&kc, &gitea).with_db(pool.clone());
    let mut feed = state.activity_events.subscribe(ORG_NAMESPACE).await;

    let answer = send(
        &state,
        person("ada"),
        "POST",
        &format!("{PEOPLE}/remove"),
        Some(json!({ "ids": ["jana-id", "eva-id", "ola-id"] })),
    )
    .await;
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.text);

    let posted = |suffix: &'static str| {
        let gitea = &gitea;
        async move {
            gitea
                .received_requests()
                .await
                .unwrap_or_default()
                .into_iter()
                .filter(|r| {
                    r.method.as_str() == "POST" && r.url.path() == format!("{REPO}{suffix}")
                })
                .filter_map(|r| serde_json::from_slice::<Value>(&r.body).ok())
                .collect::<Vec<_>>()
        }
    };
    let commits = posted("/contents").await;
    assert_eq!(commits.len(), 1, "one commit");
    assert_eq!(posted("/pulls").await.len(), 1, "one Change");
    let files = commits[0]["files"].as_array().expect("files").clone();
    let operation = |suffix: &str| {
        files
            .iter()
            .find(|f| f["path"].as_str().is_some_and(|p| p.ends_with(suffix)))
            .map(|f| f["operation"].clone())
    };
    assert_eq!(operation("groups/stewards.yaml"), Some(json!("upload")));
    assert_eq!(operation("jana-alone.yaml"), Some(json!("delete")));
    assert_eq!(files.len(), 2, "{files:?}");
    let pulls = posted("/pulls").await;
    let text = pulls[0].to_string();
    assert!(text.contains("3 people"), "{text}");
    assert!(
        !text.contains("hel.fi"),
        "ids only, never the e-mails: {text}"
    );

    for id in ["jana-id", "eva-id", "ola-id"] {
        let disabled = received(&kc, "PUT", &format!("/users/{id}")).await;
        assert_eq!(
            serde_json::from_slice::<Value>(&disabled[0].body).expect("user")["enabled"],
            false,
            "{id}"
        );
        assert_eq!(
            received(&kc, "POST", &format!("/users/{id}/logout"))
                .await
                .len(),
            1
        );
        let pending = joinedcontext_portal::db::person_deletion(&pool, id)
            .await
            .expect("read")
            .expect("pending");
        assert_eq!(pending.pull_request, 9, "{id}");
    }
    let seen = person_changes(&mut feed);
    assert_eq!(seen.len(), 3, "{seen:?}");
    assert!(seen.iter().all(|e| e["actor"] == "ada"), "{seen:?}");

    // A second removal naming one of them is refused while the Change is open.
    let again = send(
        &state,
        person("ada"),
        "POST",
        &format!("{PEOPLE}/remove"),
        Some(json!({ "ids": ["eva-id"] })),
    )
    .await;
    assert_eq!(again.status, StatusCode::CONFLICT, "{}", again.text);
    assert!(again.text.contains("eva-id"), "{}", again.text);

    Mock::given(method("GET"))
        .and(path(format!("{REPO}/pulls/9")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "number": 9, "html_url": "https://gitea.example/pulls/9", "state": "closed", "merged": true })))
        .mount(&gitea)
        .await;
    let people = state.people.clone().expect("people");
    let forge_client = state.forge_for(ORG_NAMESPACE).expect("forge");
    joinedcontext_portal::api::people::finish_deletions(&people, &pool, &forge_client).await;
    for id in ["jana-id", "eva-id", "ola-id"] {
        assert_eq!(
            received(&kc, "DELETE", &format!("/users/{id}")).await.len(),
            1,
            "{id}"
        );
    }
    let reads = gitea
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.method.as_str() == "GET" && r.url.path() == format!("{REPO}/pulls/9"))
        .count();
    assert_eq!(reads, 1, "the one Change is read once for all three");
}

/// One operation of the registry, as the assistant or an MCP client calls it (T-2732).
async fn op(state: &AppState, who: &str, name: &str, input: Value) -> common::Answer {
    send(
        state,
        person(who),
        "POST",
        &format!("/api/v1/projects/{ORG_NAMESPACE}/ops/{name}"),
        Some(input),
    )
    .await
}

/// T-2732, AG-77: every person operation calls its route, so it answers as the page's call does.
#[tokio::test]
async fn the_person_operations_do_what_the_routes_do() {
    let (kc, gitea) = (realm(true).await, forge().await);
    let state = state_with(&kc, &gitea);

    let listed = op(
        &state,
        "ada",
        "jc_person_list",
        json!({ "search": "hel.fi" }),
    )
    .await;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.text);
    assert!(listed.text.contains("jana@hel.fi"), "{}", listed.text);

    let read = op(&state, "ada", "jc_person_get", json!({ "id": "jana-id" })).await;
    assert_eq!(read.status, StatusCode::OK, "{}", read.text);
    assert!(read.text.contains("jana@hel.fi"), "{}", read.text);

    let edited = op(
        &state,
        "ada",
        "jc_person_edit",
        json!({ "id": "jana-id", "firstName": "Jana" }),
    )
    .await;
    assert_eq!(edited.status, StatusCode::OK, "{}", edited.text);
    let put = received(&kc, "PUT", "/users/jana-id").await;
    let written: Value = serde_json::from_slice(&put[0].body).expect("a user");
    assert_eq!(written["firstName"], "Jana");

    for (name, verb, suffix) in [
        ("jc_person_disable", "PUT", "/users/jana-id"),
        ("jc_person_enable", "PUT", "/users/jana-id"),
        ("jc_person_sign_out", "POST", "/users/jana-id/logout"),
    ] {
        let before = received(&kc, verb, suffix).await.len();
        let answer = op(&state, "ada", name, json!({ "id": "jana-id" })).await;
        assert_eq!(answer.status, StatusCode::OK, "{name}: {}", answer.text);
        assert!(
            received(&kc, verb, suffix).await.len() > before,
            "{name} reached no realm call"
        );
    }

    let unknown = op(&state, "ada", "jc_person_get", json!({ "id": "nobody-id" })).await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND, "{}", unknown.text);
    let stray = op(
        &state,
        "ada",
        "jc_person_edit",
        json!({ "id": "jana-id", "password": "x" }),
    )
    .await;
    // A field the route does not take is refused by the registry before the realm is asked.
    assert_eq!(
        stray.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        stray.text
    );
    assert!(stray.text.contains("password"), "{}", stray.text);
}

/// T-2732, PF-92: the route answers a temporary password once, to the person at the Portal; the
/// operation's answer reaches a model, so it never carries one and says who hands one over.
#[tokio::test]
async fn creating_a_person_through_an_operation_never_answers_the_password() {
    let (kc, gitea) = (realm(false).await, forge().await);
    let state = state_with(&kc, &gitea);

    let created = op(
        &state,
        "pia",
        "jc_person_create",
        json!({ "email": "new@example.org", "firstName": "Nora", "lastName": "Nová" }),
    )
    .await;
    assert_eq!(created.status, StatusCode::OK, "{}", created.text);
    let set = received(&kc, "PUT", "/new-id/reset-password").await;
    let credential: Value = serde_json::from_slice(&set[0].body).expect("a credential");
    let password = credential["value"].as_str().expect("a password was set");
    assert!(
        !created.text.contains(password),
        "the password reached the answer"
    );
    let answer: Value = serde_json::from_str(&created.text).expect("json");
    assert!(answer.get("temporaryPassword").is_none(), "{answer}");
    assert_eq!(answer["emailSent"], false);
    assert!(
        answer["handOver"]
            .as_str()
            .is_some_and(|text| text.contains("Reset password")),
        "{answer}"
    );
}

/// T-2732, PF-91, PF-93: the operations refuse whom the routes refuse — a person with no verb on
/// Person, and a people-admin acting on an administrator — and the realm hears nothing.
#[tokio::test]
async fn the_person_operations_refuse_whom_the_routes_refuse() {
    let (kc, gitea) = (realm(true).await, forge().await);
    let state = state_with(&kc, &gitea);

    for (name, input) in [
        ("jc_person_list", json!({})),
        ("jc_person_get", json!({ "id": "ada-id" })),
        (
            "jc_person_create",
            json!({ "email": "x@example.org", "firstName": "X", "lastName": "Y" }),
        ),
        (
            "jc_person_edit",
            json!({ "id": "ada-id", "firstName": "A" }),
        ),
        ("jc_person_disable", json!({ "id": "ada-id" })),
        ("jc_person_enable", json!({ "id": "ada-id" })),
        ("jc_person_sign_out", json!({ "id": "ada-id" })),
    ] {
        let answer = op(&state, "jana", name, input).await;
        assert_eq!(
            answer.status,
            StatusCode::FORBIDDEN,
            "{name}: {}",
            answer.text
        );
    }

    for name in ["jc_person_disable", "jc_person_sign_out"] {
        let answer = op(&state, "pia", name, json!({ "id": "ada-id" })).await;
        assert_eq!(
            answer.status,
            StatusCode::FORBIDDEN,
            "{name}: {}",
            answer.text
        );
    }
    assert!(received(&kc, "PUT", "/users/ada-id").await.is_empty());
    assert!(received(&kc, "POST", "/users/ada-id/logout")
        .await
        .is_empty());
}

/// The `lifespan` the Portal asked the realm's execute-actions e-mail for, in seconds.
async fn invitation_lifespan(state: &AppState, kc: &MockServer) -> Option<String> {
    let body = json!({ "email": "new@example.org", "firstName": "Nora", "lastName": "Nová" });
    let created = send(state, person("pia"), "POST", PEOPLE, Some(body)).await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text);
    let asked = received(kc, "PUT", "/new-id/execute-actions-email").await;
    asked[0]
        .url
        .query_pairs()
        .find(|(name, _)| name == "lifespan")
        .map(|(_, value)| value.into_owned())
}

fn organization_with_invitation_hours(state: &AppState, hours: u32) {
    state.mirror.upsert(envelope(
        "Organization",
        "hel",
        ORG_NAMESPACE,
        json!({
            "domain": "hel.fi", "locales": ["en"], "defaultLocale": "en",
            "limits": { "people": { "invitationHours": hours } },
        }),
    ));
}

/// ADR-N-035, T-2892: an organization that sets nothing gets the catalog's 12 hours, which is
/// the realm's own admin action-token lifespan, so nothing changes for it.
#[tokio::test]
async fn an_invitation_link_lives_twelve_hours_when_the_organization_sets_nothing() {
    let (kc, gitea) = (realm(true).await, forge().await);
    let state = state_with(&kc, &gitea);
    assert_eq!(
        invitation_lifespan(&state, &kc).await.as_deref(),
        Some("43200")
    );
}

/// ADR-N-035, T-2892: `spec.limits.people.invitationHours` reaches the realm's e-mail.
#[tokio::test]
async fn an_invitation_link_lives_the_organizations_invitation_hours() {
    let (kc, gitea) = (realm(true).await, forge().await);
    let state = state_with(&kc, &gitea);
    organization_with_invitation_hours(&state, 48);
    assert_eq!(
        invitation_lifespan(&state, &kc).await.as_deref(),
        Some("172800")
    );
}

/// ADR-N-035 §2: a bound the operator lowered after the change was approved still holds.
#[tokio::test]
async fn an_invitation_link_never_outlives_the_operators_bound() {
    let (kc, gitea) = (realm(true).await, forge().await);
    let mut state = state_with(&kc, &gitea);
    let mut config = (*state.config).clone();
    config.organization_bounds = serde_json::from_value(json!({
        "spec.limits.people.invitationHours": { "max": 24 },
    }))
    .expect("bounds");
    state.config = Arc::new(config);
    organization_with_invitation_hours(&state, 48);
    assert_eq!(
        invitation_lifespan(&state, &kc).await.as_deref(),
        Some("86400")
    );
}

fn with_invitation_client(state: &mut AppState) {
    let mut config = (*state.config).clone();
    config.invitation_client_id = Some("portal-ui".into());
    config.public_base_url = "https://portal.example.org".parse().expect("url");
    state.config = Arc::new(config);
    state.mirror.upsert(envelope(
        "ContextSpace",
        "air",
        "helsinki",
        json!({ "description": "air" }),
    ));
}

/// The link the Portal asked the realm's e-mail to return through, as (client, page).
async fn returns_to(kc: &MockServer, id: &str) -> Option<(String, String)> {
    let asked = received(kc, "PUT", &format!("/{id}/execute-actions-email")).await;
    let query: std::collections::HashMap<String, String> =
        asked.last()?.url.query_pairs().into_owned().collect();
    Some((
        query.get("client_id")?.clone(),
        query.get("redirect_uri")?.clone(),
    ))
}

/// PF-108: an invitation into a project returns, once the password is set, to that project's
/// home page through the client the operator named; a project the caller cannot read is `404`
/// and nobody is created. With a database the person's expiry is the organization's lifespan.
#[tokio::test]
async fn an_invitation_into_a_project_links_to_its_home_and_says_when_it_expires() {
    let (kc, gitea) = (realm(true).await, forge().await);
    let mut state = state_with(&kc, &gitea);
    with_invitation_client(&mut state);

    let unknown = json!({ "email": "new@example.org", "firstName": "N", "lastName": "N", "project": "nowhere" });
    let refused = send(&state, person("pia"), "POST", PEOPLE, Some(unknown)).await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND, "{}", refused.text);
    assert!(received(&kc, "POST", "/users").await.is_empty());

    let body = json!({ "email": "new@example.org", "firstName": "N", "lastName": "N", "project": "helsinki" });
    let created = send(&state, person("pia"), "POST", PEOPLE, Some(body)).await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text);
    assert_eq!(
        returns_to(&kc, "new-id").await,
        Some((
            "portal-ui".to_owned(),
            "https://portal.example.org/projects/helsinki/home?welcome=1".to_owned()
        ))
    );

    let Some(url) = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())
    else {
        eprintln!("the expiry half needs JC_PORTAL_TEST_DATABASE_URL");
        return;
    };
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("connect + migrate");
    joinedcontext_portal::db::remove_person_invitation(&pool, "wait-id")
        .await
        .expect("a clean row");
    let waiting = pending_person(&kc).await;
    state.db = Some(pool.clone());
    let before = chrono::Utc::now();
    let resent = send(
        &state,
        person("pia"),
        "POST",
        &format!("{PEOPLE}/{waiting}/resend-invitation"),
        None,
    )
    .await;
    assert_eq!(resent.status, StatusCode::ACCEPTED, "{}", resent.text);
    let page = send(&state, person("pia"), "GET", PEOPLE, None).await;
    let listed: Value = serde_json::from_str(&page.text).expect("json");
    let expires = listed["items"]
        .as_array()
        .and_then(|items| items.iter().find(|p| p["id"] == waiting))
        .and_then(|p| p["invitationExpires"].as_str())
        .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
        .expect("the pending invitation's expiry");
    let twelve_hours = chrono::Duration::hours(12);
    assert!(
        expires >= before + twelve_hours - chrono::Duration::seconds(5)
            && expires <= chrono::Utc::now() + twelve_hours,
        "{expires}"
    );
    // A person who took every step shows no expiry.
    let ada = listed["items"]
        .as_array()
        .and_then(|items| items.iter().find(|p| p["id"] == "ada-id"))
        .expect("ada");
    assert_eq!(ada["invitationExpires"], Value::Null);
}

/// A person who has not accepted their invitation yet: both steps still required.
async fn pending_person(kc: &MockServer) -> &'static str {
    let mut waiting = user("wait-id", "wait@example.org");
    waiting["requiredActions"] = json!(["VERIFY_EMAIL", "UPDATE_PASSWORD"]);
    Mock::given(method("GET"))
        .and(path(format!("{ADMIN}/users/wait-id")))
        .respond_with(ResponseTemplate::new(200).set_body_json(waiting.clone()))
        .with_priority(1)
        .mount(kc)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{ADMIN}/users")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!([user("ada-id", "ada@hel.fi"), waiting,])),
        )
        .with_priority(1)
        .mount(kc)
        .await;
    "wait-id"
}

/// PF-108: a pending invitation is sent again for the steps still left; one already accepted is
/// `409` and sends nothing; a caller without `create` on Person is refused.
#[tokio::test]
async fn a_pending_invitation_is_sent_again_and_an_accepted_one_is_not() {
    let (kc, gitea) = (realm(true).await, forge().await);
    let state = state_with(&kc, &gitea);
    let waiting = pending_person(&kc).await;

    let resent = send(
        &state,
        person("pia"),
        "POST",
        &format!("{PEOPLE}/{waiting}/resend-invitation"),
        None,
    )
    .await;
    assert_eq!(resent.status, StatusCode::ACCEPTED, "{}", resent.text);
    let answer: Value = serde_json::from_str(&resent.text).expect("json");
    assert_eq!(answer["emailSent"], true);
    let actions = received(&kc, "PUT", "/wait-id/execute-actions-email").await;
    let asked: Value = serde_json::from_slice(&actions[0].body).expect("actions");
    assert_eq!(asked, json!(["VERIFY_EMAIL", "UPDATE_PASSWORD"]));
    // No client was named for the way back: the link ends on the realm's own page.
    assert_eq!(returns_to(&kc, "wait-id").await, None);

    let accepted = send(
        &state,
        person("ada"),
        "POST",
        &format!("{PEOPLE}/jana-id/resend-invitation"),
        None,
    )
    .await;
    assert_eq!(accepted.status, StatusCode::CONFLICT, "{}", accepted.text);
    assert!(received(&kc, "PUT", "/jana-id/execute-actions-email")
        .await
        .is_empty());

    let refused = send(
        &state,
        person("jana"),
        "POST",
        &format!("{PEOPLE}/{waiting}/resend-invitation"),
        None,
    )
    .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.text);
}

/// What `/api/v1/auth/me` answers the session cookie `cookie`.
async fn me_with(state: &AppState, cookie: &str) -> StatusCode {
    use tower::ServiceExt as _;
    joinedcontext_portal::server::app(state.clone())
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/v1/auth/me")
                .header(axum::http::header::COOKIE, cookie)
                .body(axum::body::Body::empty())
                .expect("a request"),
        )
        .await
        .expect("a response")
        .status()
}

/// T-5568, TR-03187 AR-6: disabling a person refuses the session they already hold at once, and
/// lists them for the gateway, which refuses their unexpired tokens on its own doors; signing
/// out does the same; a sign-in after the mark is admitted again.
#[tokio::test]
async fn a_disabled_person_is_refused_before_their_token_expires() {
    let (kc, gitea) = (realm(true).await, forge().await);
    let state = state_with(&kc, &gitea);
    let jana = common::cookie(
        &state.config,
        joinedcontext_portal::auth::session::Identity {
            subject: "jana-id".into(),
            ..person("jana")
        },
    );
    assert_eq!(me_with(&state, &jana).await, StatusCode::OK);
    assert!(state.revocations(0).is_empty());

    let answer = send(
        &state,
        person("ada"),
        "POST",
        &format!("{PEOPLE}/jana-id/disable"),
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    assert_eq!(me_with(&state, &jana).await, StatusCode::UNAUTHORIZED);
    let listed = state.revocations(joinedcontext_portal::auth::session::now_unix());
    assert_eq!(
        listed.iter().map(|(s, _)| s.as_str()).collect::<Vec<_>>(),
        ["jana-id"]
    );
    // Enabled and signed in again, a second after the mark: admitted.
    let mut later = joinedcontext_portal::auth::session::Session {
        identity: joinedcontext_portal::auth::session::Identity {
            subject: "jana-id".into(),
            ..person("jana")
        },
        expires_at: listed[0].1 + 3600,
        issued_at: listed[0].1 + 1,
        signed_in_at: None,
        id_token: String::new(),
        access_expires_at: listed[0].1 + 3600,
        refresh_token: None,
    };
    assert!(!state.is_revoked(&later));
    later.issued_at = listed[0].1;
    assert!(state.is_revoked(&later));

    // Signing a person out ends their tokens too, not only their Keycloak sessions.
    let answer = send(
        &state,
        person("ada"),
        "POST",
        &format!("{PEOPLE}/pia-id/sign-out"),
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT, "{}", answer.text);
    assert!(state
        .revocations(joinedcontext_portal::auth::session::now_unix())
        .iter()
        .any(|(s, _)| s == "pia-id"));
}

/// T-5975: disabling a person revokes every live connector of theirs, not another person's, and
/// the feed names how many; enabling them again revives none.
#[tokio::test]
async fn disabling_a_person_revokes_their_connectors_on_the_feed() {
    let (kc, gitea) = (realm(true).await, forge().await);
    let state = state_with(&kc, &gitea);
    let mut feed = state.activity_events.subscribe(ORG_NAMESPACE).await;
    let new = |name: &str| -> joinedcontext_portal::connectors::NewConnector {
        serde_json::from_value(json!({
            "name": name, "segments": ["data-read"],
            "scope": [{ "project": "doprava", "access": "read" }],
            "expiresAt": (chrono::Utc::now() + chrono::Duration::days(7)).to_rfc3339(),
        }))
        .expect("a connector")
    };
    for (owner, name) in [
        ("jana-id", "claude-code"),
        ("jana-id", "chatgpt"),
        ("pia-id", "pia"),
    ] {
        state
            .connectors
            .create(owner, new(name), Vec::new(), "")
            .await
            .expect("created");
    }

    for verb in ["disable", "enable"] {
        let answer = send(
            &state,
            person("ada"),
            "POST",
            &format!("{PEOPLE}/jana-id/{verb}"),
            None,
        )
        .await;
        assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    }
    let states = |owner: &'static str| {
        let state = state.clone();
        async move {
            state
                .connectors
                .of_owner(owner, "")
                .await
                .expect("list")
                .into_iter()
                .map(|c| c.state)
                .collect::<Vec<_>>()
        }
    };
    assert_eq!(states("jana-id").await, ["revoked", "revoked"]);
    assert_eq!(states("pia-id").await, ["active"]);
    let actions: Vec<Value> = person_changes(&mut feed)
        .into_iter()
        .map(|d| d["action"].clone())
        .collect();
    assert_eq!(
        actions,
        [
            json!("revoked 2 connectors of"),
            json!("disabled"),
            json!("enabled")
        ]
    );
}

const JANA_ID: &str = "00000000-0000-4000-8000-00000000000a";

/// ADR-N-061 (T-5711): the names behind ids go to whoever updates people or approves groups, and
/// to nobody else; an id that is not one is refused naming the field.
#[tokio::test]
async fn names_behind_ids_are_read_by_a_people_admin_or_a_group_approver_alone() {
    let (kc, gitea) = (realm(true).await, forge().await);
    Mock::given(method("GET"))
        .and(path(format!("{ADMIN}/users/{JANA_ID}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(user(JANA_ID, "jana@hel.fi")))
        .mount(&kc)
        .await;
    let state = state_with(&kc, &gitea);
    let names = format!("{PEOPLE}/names?ids={JANA_ID}");

    for who in ["pia", "ada"] {
        let read = send(&state, person(who), "GET", &names, None).await;
        assert_eq!(read.status, StatusCode::OK, "{who}: {}", read.text);
        let body: Value = serde_json::from_str(&read.text).expect("json");
        assert_eq!(body["items"], json!([{ "id": JANA_ID, "name": "F L" }]));
    }
    let refused = send(&state, person("jana"), "GET", &names, None).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.text);
    assert!(!refused.text.contains("F L"), "{}", refused.text);
    let bad = send(
        &state,
        person("pia"),
        "GET",
        &format!("{PEOPLE}/names?ids=jana@hel.fi"),
        None,
    )
    .await;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", bad.text);
    let bad_body: Value = serde_json::from_str(&bad.text).expect("json");
    assert_eq!(bad_body["errors"][0]["path"], "ids", "{}", bad.text);
}

/// ADR-N-061 (T-5711): the dry run over the organization's repository rewrites every address a
/// realm user carries into that user's id and lists the rest; it proposes nothing, and only a
/// people administrator runs it.
#[tokio::test]
async fn the_migration_dry_run_plans_the_rewrite_and_proposes_nothing() {
    let (kc, gitea) = (realm(true).await, forge().await);
    let state = state_with(&kc, &gitea);
    let mut pia = person("pia");
    pia.second_factor = true;
    let answer = send(
        &state,
        pia,
        "POST",
        &format!("{PEOPLE}/name-by-id?dryRun=true"),
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let plan: Value = serde_json::from_str(&answer.text).expect("json");
    let organization = &plan["repositories"][0];
    assert_eq!(organization["repository"], "organization");
    assert_eq!(organization["change"], Value::Null);
    let manifests: Vec<&str> = organization["manifests"]
        .as_array()
        .expect("manifests")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    for named in [
        "RoleBinding org/admins",
        "RoleBinding org/people-admins",
        "RoleBinding org/jana-alone",
        "Group org/stewards",
    ] {
        assert!(manifests.contains(&named), "{named}: {manifests:?}");
    }
    // eva is in the group and nobody in the realm carries her address.
    assert_eq!(plan["unresolved"], json!(["eva@hel.fi"]));
    let proposed = gitea
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.method.as_str() != "GET")
        .count();
    assert_eq!(proposed, 0, "a dry run writes nothing to the forge");

    let jana = send(
        &state,
        person("jana"),
        "POST",
        &format!("{PEOPLE}/name-by-id?dryRun=true"),
        None,
    )
    .await;
    assert_eq!(jana.status, StatusCode::FORBIDDEN, "{}", jana.text);
}
