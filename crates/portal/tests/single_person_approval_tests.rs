//! Single-person approvals (T-7388): an installation operator turns `approvals.singlePerson` on
//! for an Organization where one person holds a role (a second operator approves the Change);
//! then that person approves their own access Change alone, as a person and only after a fresh
//! second factor. Every other case keeps PF-58's second person.
//!
//! The store is the database `JC_PORTAL_TEST_DATABASE_URL` names (CI's portal-test jobs set it).

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use joinedcontext_portal::api::changes::{approve_change_as, ApprovedBy};
use joinedcontext_portal::auth::session::{now_unix, Identity};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::error::ApiError;
use joinedcontext_portal::forge::ShakoClient;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::{self, API_VERSION};
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{envelope, person, send};

const CHANGES: &str = "/api/v1/operator/changes";
const BINDING: &str = "rolebinding-x";
/// The Keycloak user ids a binding names its people by (PF-126).
const OWNER: &str = "3f2a9c1e-7b4d-4e8a-9c61-0d5e2b7f8a14";
const SECOND: &str = "7c1d2e3f-4a5b-4c6d-8e9f-0a1b2c3d4e5f";

/// The Organization's one person, as their session names them.
fn owner() -> Identity {
    Identity {
        subject: OWNER.to_owned(),
        ..person("owner")
    }
}

async fn pool() -> sqlx::PgPool {
    let url = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .expect("set JC_PORTAL_TEST_DATABASE_URL to a PostgreSQL database the Portal migrates");
    // Two tests of this binary migrating at once race on the migration table.
    static BOOT: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _booting = BOOT.lock().await;
    joinedcontext_portal::db::connect(&url)
        .await
        .expect("connect + migrate")
}

fn run(test: &str) -> String {
    format!("{test}-{}", common::unique::unique())
}

fn operator(state: &AppState, name: &str) -> Identity {
    let mut who = person(name);
    who.groups = vec![state.config.bootstrap_admins.clone()];
    who.second_factor = true;
    who
}

/// The Organization `organization` in the registry and the mirror, `people` bound to an
/// administrator's role across it.
async fn organization(state: &AppState, organization: &str, people: &[&str]) {
    forge_named(state, organization, organization, people).await;
}

/// [`organization`] with the forge keeping its manifest and binding under `forge` (jc-prod:
/// registry `smartcity`, forge `hel`).
async fn forge_named(state: &AppState, organization: &str, forge: &str, people: &[&str]) {
    sqlx::query(
        "INSERT INTO organizations (name, host, state, created_by) \
         VALUES ($1, $1 || '.example', 'active', 'test') ON CONFLICT DO NOTHING",
    )
    .bind(organization)
    .execute(state.db.as_ref().expect("db"))
    .await
    .expect("an Organization");
    state.mirror.upsert(envelope(
        "Organization",
        forge,
        ORG_NAMESPACE,
        json!({ "installation": true }),
    ));
    state.mirror.upsert(envelope(
        "Role",
        "admin-role",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Role", "RoleBinding", "Group", "ContextSpace"], "verbs": ["propose", "approve", "delete"] }] }),
    ));
    let subjects: Vec<Value> = people.iter().map(|p| json!({ "user": p })).collect();
    state.mirror.upsert(envelope(
        "RoleBinding",
        "admin-binding",
        ORG_NAMESPACE,
        json!({ "subjects": subjects, "role": "admin-role", "scope": { "organization": forge } }),
    ));
}

async fn propose(
    state: &AppState,
    who: Identity,
    organization: &str,
    on: bool,
) -> (StatusCode, Value) {
    let change =
        json!({ "kind": "SinglePersonSet", "organization": organization, "singlePerson": on });
    let answer = send(state, who, "POST", CHANGES, Some(change)).await;
    (
        answer.status,
        serde_json::from_str(&answer.text).unwrap_or_default(),
    )
}

async fn mode(state: &AppState, who: Identity, organization: &str) -> Value {
    let uri = format!("/api/v1/organization/approval-mode?organization={organization}");
    let answer = send(state, who, "GET", &uri, None).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    serde_json::from_str(&answer.text).expect("json")
}

#[tokio::test]
async fn only_an_operator_turns_the_mode_on_and_a_second_one_approves_it() {
    let state = AppState::new(Config::for_tests(), None).with_db(pool().await);
    let org = run("solo");
    organization(&state, &org, &[OWNER]).await;
    let owner = owner();

    let (status, refused) = propose(&state, owner.clone(), &org, true).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");

    let (status, proposed) = propose(&state, operator(&state, "op-a"), &org, true).await;
    assert_eq!(status, StatusCode::CREATED, "{proposed}");
    assert_eq!(
        proposed["state"], "proposed",
        "turning on waits for a second operator"
    );
    assert_eq!(
        mode(&state, owner.clone(), &org).await["singlePerson"],
        false
    );
    let id = proposed["id"].as_str().expect("an id");

    let own = send(
        &state,
        operator(&state, "op-a"),
        "POST",
        &format!("{CHANGES}/{id}/approve"),
        None,
    )
    .await;
    assert_eq!(own.status, StatusCode::FORBIDDEN, "{}", own.text);
    let applied = send(
        &state,
        operator(&state, "op-b"),
        "POST",
        &format!("{CHANGES}/{id}/approve"),
        None,
    )
    .await;
    assert_eq!(applied.status, StatusCode::OK, "{}", applied.text);
    let on = mode(&state, owner.clone(), &org).await;
    assert_eq!(
        (on["singlePerson"].clone(), on["peopleWithARole"].clone()),
        (json!(true), json!(1))
    );

    // Off restores PF-58: one operator, applied at once.
    let (status, off) = propose(&state, operator(&state, "op-a"), &org, false).await;
    assert_eq!(
        (status, off["state"].clone()),
        (StatusCode::CREATED, json!("applied")),
        "{off}"
    );
    assert_eq!(mode(&state, owner, &org).await["singlePerson"], false);
}

#[tokio::test]
async fn the_mode_turns_on_only_where_one_person_holds_a_role() {
    let state = AppState::new(Config::for_tests(), None).with_db(pool().await);
    let org = run("pair");
    organization(&state, &org, &[OWNER, SECOND]).await;
    let (status, refused) = propose(&state, operator(&state, "op-a"), &org, true).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(refused["errors"][0]["path"], "singlePerson", "{refused}");
    assert!(
        refused["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("2 people"),
        "{refused}"
    );
    let (status, unknown) = propose(&state, operator(&state, "op-a"), "no-such-org", true).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{unknown}");
    assert_eq!(unknown["errors"][0]["path"], "organization", "{unknown}");
}

#[tokio::test]
async fn a_second_person_turns_the_mode_off_and_says_so() {
    let state = AppState::new(Config::for_tests(), None).with_db(pool().await);
    let org = run("grown");
    organization(&state, &org, &[OWNER]).await;
    on(&state, &org).await;
    assert_eq!(mode(&state, owner(), &org).await["singlePerson"], true);
    organization(&state, &org, &[OWNER, SECOND]).await;
    let off = mode(&state, owner(), &org).await;
    assert_eq!(off["singlePerson"], false, "{off}");
    assert!(
        off["offReason"]
            .as_str()
            .unwrap_or_default()
            .contains("2 people"),
        "{off}"
    );
    // The row says so too, not only this answer.
    let stored: (bool, Option<String>) = sqlx::query_as(
        "SELECT single_person, off_reason FROM organization_approval_modes WHERE organization = $1",
    )
    .bind(&org)
    .fetch_one(state.db.as_ref().expect("db"))
    .await
    .expect("the row");
    assert!(!stored.0 && stored.1.is_some());
}

/// The automation login of a test: a username its RoleBinding names, unique per run, since a
/// mark refuses its approvals in every Organization.
fn bot(test: &str) -> Identity {
    person(&run(test))
}

async fn mark(
    state: &AppState,
    who: Identity,
    organization: &str,
    login: &str,
) -> (StatusCode, Value) {
    let change = json!({ "kind": "AutomationLoginSet", "organization": organization, "subject": login, "automation": true, "connector": "org-mcp" });
    let answer = send(state, who, "POST", CHANGES, Some(change)).await;
    (
        answer.status,
        serde_json::from_str(&answer.text).unwrap_or_default(),
    )
}

/// An operator marks `login` and a second one approves it.
async fn marked(state: &AppState, organization: &str, login: &str) {
    let (status, proposed) = mark(state, operator(state, "op-a"), organization, login).await;
    assert_eq!(status, StatusCode::CREATED, "{proposed}");
    let id = proposed["id"].as_str().expect("an id");
    let applied = send(
        state,
        operator(state, "op-b"),
        "POST",
        &format!("{CHANGES}/{id}/approve"),
        None,
    )
    .await;
    assert_eq!(applied.status, StatusCode::OK, "{}", applied.text);
}

#[tokio::test]
async fn only_an_operator_marks_an_automation_login_and_people_count_without_it() {
    let state = AppState::new(Config::for_tests(), None).with_db(pool().await);
    let org = run("bot-org");
    let bot = bot("mcp-admin");
    organization(&state, &org, &[OWNER, &bot.username]).await;

    // Today's defect: the automation login is a second person.
    let (status, refused) = propose(&state, operator(&state, "op-a"), &org, true).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");

    // Neither the Organization's administrator nor the login itself sets the mark.
    for who in [owner(), bot.clone()] {
        let (status, body) = mark(&state, who, &org, &bot.username).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    }
    // A login without a role there is no login of it.
    let (status, unknown) = mark(&state, operator(&state, "op-a"), &org, "nobody-here").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{unknown}");
    assert_eq!(unknown["errors"][0]["path"], "subject", "{unknown}");

    marked(&state, &org, &bot.username).await;
    let (status, on) = propose(&state, operator(&state, "op-a"), &org, true).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "one person and an automation login: {on}"
    );
    assert_eq!(mode(&state, owner(), &org).await["peopleWithARole"], 1);

    // Two people and the login: PF-58's second person exists.
    let pair = run("bot-pair");
    organization(&state, &pair, &[OWNER, SECOND, &bot.username]).await;
    marked(&state, &pair, &bot.username).await;
    let (status, refused) = propose(&state, operator(&state, "op-a"), &pair, true).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert!(
        refused["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("2 people"),
        "{refused}"
    );
}

/// jc-prod (T-7436): the forge keeps the installation Organization under another name than the
/// registry. The operator door takes either name, keeps the mark under the registry's, and
/// counts the people its forge-named bindings hold.
#[tokio::test]
async fn the_installation_is_marked_and_counted_under_either_name() {
    let state = AppState::new(Config::for_tests(), None).with_db(pool().await);
    let registry = run("smartcity");
    let forge = run("hel");
    let bot = bot("mcp-alias");
    state.mirror.set_configured_installation(&registry);
    assert_eq!(state.mirror.configured_installation(), Some(registry.as_str()));
    forge_named(&state, &registry, &forge, &[OWNER, SECOND, &bot.username]).await;
    let stored = |state: &AppState| {
        let state = state.clone();
        async move {
            joinedcontext_portal::automation::marked(&state, None)
                .await
                .expect("marks")
                .into_iter()
                .filter(|row| row.login == bot.username)
                .map(|row| row.organization)
                .collect::<Vec<_>>()
        }
    };

    marked(&state, &registry, &bot.username).await;
    assert_eq!(stored(&state).await, [registry.clone()]);

    // The forge's name clears it, and marks it again under the registry's.
    let clear = json!({ "kind": "AutomationLoginSet", "organization": forge, "subject": bot.username, "automation": false });
    let proposed = send(&state, operator(&state, "op-a"), "POST", CHANGES, Some(clear)).await;
    assert_eq!(proposed.status, StatusCode::CREATED, "{}", proposed.text);
    let proposed: Value = serde_json::from_str(&proposed.text).expect("json");
    assert_eq!(proposed["payload"]["organization"], registry.as_str(), "{proposed}");
    let id = proposed["id"].as_str().expect("an id");
    let applied = send(&state, operator(&state, "op-b"), "POST", &format!("{CHANGES}/{id}/approve"), None).await;
    assert_eq!(applied.status, StatusCode::OK, "{}", applied.text);
    assert!(stored(&state).await.is_empty());
    marked(&state, &forge, &bot.username).await;
    assert_eq!(stored(&state).await, [registry.clone()]);

    // Two people hold a role through the forge-named binding: the mode stays off.
    let (status, refused) = propose(&state, operator(&state, "op-a"), &registry, true).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert!(refused["detail"].as_str().unwrap_or_default().contains("2 people"), "{refused}");

    // An Organization the registry lacks is still refused.
    let (status, unknown) = mark(&state, operator(&state, "op-a"), &run("nowhere"), &bot.username).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{unknown}");
    assert_eq!(unknown["errors"][0]["path"], "organization", "{unknown}");
}

#[tokio::test]
async fn an_automation_login_never_approves_a_change() {
    // Unmarked, the login is a valid second approver: the defect the mark closes.
    let org = run("bot-open");
    let free = bot("mcp-free");
    let (server, state) = forge(&org, &[OWNER, &free.username]).await;
    approve_change_as(
        &state,
        &free,
        ORG_NAMESPACE,
        "chg-org-00000001",
        Some(BINDING),
        ApprovedBy::Person,
        None,
    )
    .await
    .expect("an unmarked login approves");
    assert!(merged(&server).await);

    let org = run("bot-shut");
    let bot = bot("mcp-shut");
    let (server, state) = forge(&org, &[OWNER, &bot.username]).await;
    marked(&state, &org, &bot.username).await;
    for by in [
        ApprovedBy::Person,
        ApprovedBy::Operation,
        ApprovedBy::Connector("org-mcp".into()),
    ] {
        let err = approve_change_as(
            &state,
            &bot,
            ORG_NAMESPACE,
            "chg-org-00000001",
            Some(BINDING),
            by.clone(),
            Some(now_unix()),
        )
        .await
        .expect_err("an automation login approves nothing");
        assert!(
            matches!(&err, ApiError::Denied(why) if why.contains("automation login") && why.contains("T-7432")),
            "{by:?}: {err:?}"
        );
    }
    assert!(!merged(&server).await);
}

#[tokio::test]
async fn a_change_the_automation_login_proposed_is_approved_by_the_one_person() {
    let org = run("bot-solo");
    let bot = bot("mcp-solo");
    let (server, state) = forge_by(&org, &[OWNER, &bot.username], &bot.username).await;
    marked(&state, &org, &bot.username).await;
    on(&state, &org).await;
    approve(&state, ApprovedBy::Person, Some(now_unix() - 10))
        .await
        .expect("the one person approves the login's change");
    assert!(merged(&server).await);
    assert_eq!(
        mode(&state, owner(), &org).await["singlePerson"],
        true,
        "the login keeps the mode on"
    );
}

async fn on(state: &AppState, organization: &str) {
    let mut conn = state
        .db
        .as_ref()
        .expect("db")
        .acquire()
        .await
        .expect("conn");
    joinedcontext_portal::single_person::set(&mut conn, organization, true, "op-b", None)
        .await
        .expect("mode on");
}

/// A forge holding a RoleBinding proposed by `owner@hel.fi` that grants its own author access,
/// in a Portal of Organization `org` where `people` hold a role.
async fn forge(org: &str, people: &[&str]) -> (MockServer, AppState) {
    forge_by(org, people, "owner").await
}

/// [`forge`] with the RoleBinding proposed by `author` (`{author}@hel.fi`).
async fn forge_by(org: &str, people: &[&str], author: &str) -> (MockServer, AppState) {
    let server = MockServer::start().await;
    let repo = "/odata/Repositories(Owner='test-owner',Name='test-repo')";
    let ok = |body: Value| ResponseTemplate::new(200).set_body_json(body);
    let yaml = format!(
        "apiVersion: {API_VERSION}\nkind: RoleBinding\nmetadata:\n  name: {BINDING}\n  namespace: {ORG_NAMESPACE}\nspec: {}\n",
        json!({ "subjects": [{ "user": OWNER }], "role": "admin-role", "scope": { "organization": org } })
    );
    let info = resource::by_kind("RoleBinding").expect("a served kind");
    let file = resource::repository_path(info, ORG_NAMESPACE, None, BINDING).expect("a path");
    Mock::given(method("GET"))
        .and(path(format!("{repo}/Shako.File(Path=@p,Ref=@r)")))
        .respond_with(ok(
            json!({ "Sha": "blob-1", "Content": STANDARD.encode(&yaml) }),
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{repo}/PullRequests(1)")))
        .respond_with(ok(json!({
            "Number": 1,
            "HtmlUrl": "https://shako.example.sk/pulls/1",
            "State": "open",
            "Title": format!("create RoleBinding {BINDING}"),
            "Head": format!("portal/create-rolebinding-{BINDING}-11111111"),
            "HeadSha": "head-1",
            "Base": "main",
            "CreatedAt": "2026-11-19T09:14:22Z",
            "Author": author,
            "AuthorName": author,
            "AuthorEmail": format!("{author}@hel.fi"),
            "Mergeable": true,
            "Merged": false
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{repo}/PullRequests(1)/Files")))
        .respond_with(ok(
            json!({ "value": [{ "Filename": file, "Status": "added" }] }),
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{repo}/PullRequests(1)/Shako.Merge")))
        .respond_with(ok(json!({})))
        .mount(&server)
        .await;
    let client = ShakoClient::new(
        server.uri().parse().expect("url"),
        "test-owner",
        "test-repo",
        "token-xyz",
    )
    .expect("client");
    let state = AppState::new(Config::for_tests(), None)
        .with_shako(Arc::new(client))
        .with_db(pool().await);
    organization(&state, org, people).await;
    (server, state)
}

async fn merged(server: &MockServer) -> bool {
    server
        .received_requests()
        .await
        .expect("requests")
        .iter()
        .any(|r| r.url.path().ends_with("/PullRequests(1)/Shako.Merge"))
}

async fn approve(
    state: &AppState,
    by: ApprovedBy,
    stepped_up_at: Option<i64>,
) -> Result<(), ApiError> {
    approve_change_as(
        state,
        &owner(),
        ORG_NAMESPACE,
        "chg-org-00000001",
        Some(BINDING),
        by,
        stepped_up_at,
    )
    .await
    .map(drop)
}

#[tokio::test]
async fn the_one_person_approves_their_access_change_after_a_fresh_second_factor() {
    let org = run("alone");
    let (server, state) = forge(&org, &[OWNER]).await;
    on(&state, &org).await;
    approve(&state, ApprovedBy::Person, Some(now_unix() - 10))
        .await
        .expect("approved alone in single-person mode");
    assert!(merged(&server).await);
}

#[tokio::test]
async fn each_refusal_keeps_the_change_unmerged() {
    // Mode off: PF-58 as everywhere.
    let org = run("off");
    let (server, state) = forge(&org, &[OWNER]).await;
    let err = approve(&state, ApprovedBy::Person, Some(now_unix()))
        .await
        .expect_err("mode off");
    assert!(matches!(err, ApiError::SelfApproval(_)), "{err:?}");
    assert!(!merged(&server).await);

    // Mode on: no step-up, a stale one, an agent.
    let org = run("on");
    let (server, state) = forge(&org, &[OWNER]).await;
    on(&state, &org).await;
    for stepped_up_at in [None, Some(now_unix() - 301)] {
        let err = approve(&state, ApprovedBy::Person, stepped_up_at)
            .await
            .expect_err("not fresh");
        assert!(
            matches!(err, ApiError::SecondFactorRequired),
            "{stepped_up_at:?}: {err:?}"
        );
    }
    let err = approve(&state, ApprovedBy::Operation, Some(now_unix()))
        .await
        .expect_err("an agent");
    assert!(matches!(err, ApiError::SelfApproval(_)), "{err:?}");
    assert!(!merged(&server).await);

    // Mode on, a second person with a role: refused, and the mode is off from now on.
    let org = run("two");
    let (server, state) = forge(&org, &[OWNER, SECOND]).await;
    on(&state, &org).await;
    let err = approve(&state, ApprovedBy::Person, Some(now_unix()))
        .await
        .expect_err("two people");
    assert!(matches!(err, ApiError::SelfApproval(_)), "{err:?}");
    assert!(!merged(&server).await);
    assert_eq!(mode(&state, owner(), &org).await["singlePerson"], false);
}
