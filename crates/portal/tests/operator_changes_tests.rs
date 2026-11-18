//! Operator Changes (evidence/T-7129/design.md §4 and §7, T-7139): only a person of the
//! operators' group with a second factor proposes or approves, the second factor is asked with
//! the installation switch off too, the proposer never approves whatever the kind, membership is
//! read again at approval, a lapsed TXT record refuses a host's approval, a host taken since the
//! proposal is `409`, the operators' group changes only through an approved `OperatorAdd`, and
//! the audit of it all is append-only.
//!
//! DNS is a table of canned records, the realm a mock; the store is the database
//! `JC_PORTAL_TEST_DATABASE_URL` names (CI's portal-test jobs set it). A missing variable fails,
//! never skips. Each test uses hosts and domains of its own run, so the tests share the database.

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::http::StatusCode;
use futures_util::future::BoxFuture;
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::config::{ClientAuth, Config};
use joinedcontext_portal::domain_verification::{TxtRecords, Unanswered};
use joinedcontext_portal::host_registry::{
    rows, seed, seed_organizations, SeedHost, SeedOrganization,
};
use joinedcontext_portal::people::People;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{envelope, person, send};

const CHANGES: &str = "/api/v1/operator/changes";
const BASE: &str = "jcdev.example";

#[derive(Default)]
struct Dns {
    records: Mutex<HashMap<String, Vec<String>>>,
}

impl Dns {
    fn set(&self, name: &str, texts: &[&str]) {
        self.records.lock().expect("dns").insert(
            name.to_owned(),
            texts.iter().map(|t| (*t).to_owned()).collect(),
        );
    }
}

impl TxtRecords for Dns {
    fn txt<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Vec<String>, Unanswered>> {
        let found = self.records.lock().expect("dns").get(name).cloned();
        Box::pin(async move { Ok(found.unwrap_or_default()) })
    }
}

/// A Portal with the database, the installation's hosts, and the second-factor switch off: the
/// operators' door asks for the factor all the same.
async fn world() -> (AppState, Arc<Dns>) {
    let url = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .expect("set JC_PORTAL_TEST_DATABASE_URL to a PostgreSQL database the Portal migrates");
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("connect + migrate");
    let mut config = Config::for_tests();
    config.decisions_need_second_factor = false;
    config.installation_hosts = Some((BASE.to_owned(), format!("idm.{BASE}")));
    let dns = Arc::new(Dns::default());
    let state = AppState::new(config, None)
        .with_db(pool)
        .with_txt_records(dns.clone());
    (state, dns)
}

fn run(test: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    format!("{test}{:x}", std::process::id() ^ nanos)
}

fn operator(state: &AppState, name: &str) -> Identity {
    let mut who = person(name);
    who.groups = vec![state.config.bootstrap_admins.clone()];
    who.second_factor = true;
    who
}

/// An Organization manifest and its registry row (T-7181), so a host can name it.
async fn organization(state: &AppState, name: &str, domain: &str) {
    state.mirror.upsert(envelope(
        "Organization",
        name,
        ORG_NAMESPACE,
        json!({ "domain": domain }),
    ));
    seed_organizations(
        state.db.as_ref().expect("db"),
        &[SeedOrganization {
            name: name.to_owned(),
            organization_id: None,
            host: format!("smartcity.{domain}"),
            catalogue_id: None,
        }],
    )
    .await
    .expect("seeded");
}

fn body(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_default()
}

async fn propose(state: &AppState, who: Identity, change: Value) -> (StatusCode, Value) {
    let answer = send(state, who, "POST", CHANGES, Some(change)).await;
    (answer.status, body(&answer.text))
}

async fn decide(state: &AppState, who: Identity, id: &str, verb: &str) -> (StatusCode, Value) {
    let answer = send(state, who, "POST", &format!("{CHANGES}/{id}/{verb}"), None).await;
    (answer.status, body(&answer.text))
}

async fn audit_of(state: &AppState, id: &str) -> Vec<(String, String)> {
    sqlx::query_as(
        "SELECT actor, action FROM operator_audit WHERE change_id = $1::uuid ORDER BY id",
    )
    .bind(id)
    .fetch_all(state.db.as_ref().expect("db"))
    .await
    .expect("audit")
}

async fn state_of(state: &AppState, host: &str) -> Option<(String, Option<String>)> {
    sqlx::query_as("SELECT state, approved_by FROM organization_hosts WHERE host = $1")
        .bind(host)
        .fetch_optional(state.db.as_ref().expect("db"))
        .await
        .expect("registry")
}

#[tokio::test]
async fn only_an_operator_with_a_second_factor_proposes_or_approves() {
    let (state, _) = world().await;
    let r = run("w");
    organization(&state, &r, &format!("{r}.sk")).await;
    let change = json!({ "kind": "HostAdd", "host": format!("smartcity.{r}.{BASE}"), "organization": r, "role": "working" });
    let nobody = person("nobody");
    assert_eq!(
        propose(&state, nobody.clone(), change.clone()).await.0,
        StatusCode::FORBIDDEN
    );
    let mut password_only = operator(&state, "ops-a");
    password_only.second_factor = false;
    let (status, refused) = propose(&state, password_only.clone(), change.clone()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    let administrator = common::organization_admin(&state.mirror, "org-admin");
    let (status, refused) = propose(&state, administrator.clone(), change.clone()).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "an Organization administrator: {refused}"
    );
    let (status, made) = propose(&state, operator(&state, "ops-a"), change).await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let id = made["id"].as_str().expect("an id");
    for who in [nobody, password_only, administrator] {
        assert_eq!(
            decide(&state, who, id, "approve").await.0,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        send(&state, person("nobody"), "GET", CHANGES, None)
            .await
            .status,
        StatusCode::FORBIDDEN
    );
}

/// The operators' pages read the registry, the audit and the Organization ids (T-7140): an
/// operator with a second factor reads each, with what a host waits for and the certificate
/// slots; anybody else is `403` on every one of them, a password-only operator too.
#[tokio::test]
async fn only_an_operator_reads_the_hosts_the_audit_and_the_organizations() {
    let (state, _) = world().await;
    let r = run("read");
    let domain = format!("{r}.sk");
    organization(&state, &r, &domain).await;
    let host = format!("smartcity.{r}.{BASE}");
    let pool = state.db.as_ref().expect("db");
    sqlx::query(
        "INSERT INTO organization_hosts (host, organization, role, state, created_by, detail, detail_at) \
         VALUES ($1, $2, 'working', 'dns-ok', 'ops-a', 'waiting for a certificate slot until 2026-11-18 09:00 UTC', now())",
    )
    .bind(&host)
    .bind(&r)
    .execute(pool)
    .await
    .expect("a host");
    sqlx::query(
        "INSERT INTO organization_ids (id, domain, challenge, requested_by) VALUES ($1, $2, 'c0ffee', 'ops-a')",
    )
    .bind(format!("sk_{r}"))
    .bind(&domain)
    .execute(pool)
    .await
    .expect("an id");
    let reads = [
        "/api/v1/operator/hosts",
        "/api/v1/operator/audit",
        "/api/v1/operator/organizations",
    ];
    let mut password_only = operator(&state, "ops-a");
    password_only.second_factor = false;
    // An Organization's own administrator is no installation operator (T-7138 bounce row 5).
    let administrator = common::organization_admin(&state.mirror, "org-admin");
    for path in reads {
        for who in [
            person("nobody"),
            password_only.clone(),
            administrator.clone(),
        ] {
            let answer = send(&state, who, "GET", path, None).await;
            assert_eq!(
                answer.status,
                StatusCode::FORBIDDEN,
                "{path}: {}",
                answer.text
            );
        }
    }
    let ops = operator(&state, "ops-a");
    let answer = send(&state, ops.clone(), "GET", reads[0], None).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let listed = body(&answer.text);
    let row = listed["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|item| item["host"] == host.as_str())
        .expect("the host");
    assert_eq!(row["state"], "dns-ok");
    assert_eq!(
        row["detail"],
        "waiting for a certificate slot until 2026-11-18 09:00 UTC"
    );
    assert_eq!(listed["certificates"]["perDay"], 10);
    assert_eq!(listed["certificates"]["perDomainWeek"], 20);

    let (status, made) = propose(
        &state,
        ops.clone(),
        json!({ "kind": "HostRemove", "host": host }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let answer = send(&state, ops.clone(), "GET", reads[1], None).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let audit = body(&answer.text);
    let newest = audit["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|item| item["changeId"] == made["id"])
        .expect("the proposal's row");
    assert_eq!(newest["action"], "proposed HostRemove");
    assert_eq!(newest["actor"], ops.subject.as_str());

    let answer = send(&state, ops, "GET", reads[2], None).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let orgs = body(&answer.text);
    let org = orgs["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|item| item["name"] == r.as_str())
        .expect("the Organization");
    assert_eq!(org["domain"], domain.as_str());
    assert_eq!(org["hosts"]["dns-ok"], 1);
    let id = orgs["ids"]
        .as_array()
        .expect("ids")
        .iter()
        .find(|item| item["domain"] == domain.as_str())
        .expect("the id");
    assert_eq!(id["recordName"], format!("_joinedcontext.{domain}"));
    assert_eq!(id["recordValue"], "jc-verify=c0ffee");
    assert_eq!(id["mintedAt"], Value::Null);
    assert_eq!(id["organizations"], json!([r]));
    // Out of every other test's certificate wave: the shared database keeps no dns-ok row behind.
    joinedcontext_portal::host_registry::remove(pool, &host)
        .await
        .expect("removed");
}

#[tokio::test]
async fn the_proposer_never_approves_and_a_second_operator_applies_a_host() {
    let (state, _) = world().await;
    let r = run("s");
    organization(&state, &r, &format!("{r}.sk")).await;
    let host = format!("smartcity.{r}.{BASE}");
    let (status, made) = propose(
        &state,
        operator(&state, "ops-a"),
        json!({ "kind": "HostAdd", "host": host, "organization": r, "role": "working" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    assert_eq!(made["state"], "proposed");
    let id = made["id"].as_str().expect("an id").to_owned();
    assert_eq!(state_of(&state, &host).await, None, "nothing written yet");
    let (status, refused) = decide(&state, operator(&state, "ops-a"), &id, "approve").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    assert!(refused.to_string().contains("self-approval"), "{refused}");
    let (status, applied) = decide(&state, operator(&state, "ops-b"), &id, "approve").await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    assert_eq!(applied["state"], "applied");
    assert_eq!(applied["decidedBy"], "sub-ops-b");
    assert_eq!(
        state_of(&state, &host).await,
        Some(("approved".to_owned(), Some("sub-ops-b".to_owned())))
    );
    assert_eq!(
        decide(&state, operator(&state, "ops-c"), &id, "approve")
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        audit_of(&state, &id).await,
        [
            ("sub-ops-a".to_owned(), "proposed HostAdd".to_owned()),
            ("sub-ops-b".to_owned(), "applied HostAdd".to_owned()),
        ]
    );
    let listed = send(&state, operator(&state, "ops-c"), "GET", CHANGES, None).await;
    assert_eq!(listed.status, StatusCode::OK);
    assert!(listed.text.contains(&id), "{}", listed.text);
}

#[tokio::test]
async fn an_operator_who_left_the_group_cannot_approve() {
    let (state, _) = world().await;
    let r = run("l");
    organization(&state, &r, &format!("{r}.sk")).await;
    let (_, made) = propose(
        &state,
        operator(&state, "ops-a"),
        json!({ "kind": "HostAdd", "host": format!("data.{r}.{BASE}"), "organization": r, "role": "opendata" }),
    )
    .await;
    let id = made["id"].as_str().expect("an id");
    let mut left = operator(&state, "ops-b");
    left.groups.clear();
    assert_eq!(
        decide(&state, left, id, "approve").await.0,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn a_lapsed_txt_record_refuses_the_approval() {
    let (state, dns) = world().await;
    let r = run("t");
    let domain = format!("{r}.sk");
    let id = format!("sk_{r}");
    sqlx::query(
        "INSERT INTO organization_ids (id, domain, challenge, requested_by, minted_by, minted_at) \
         VALUES ($1, $2, 'c0ffee', 'op', 'op', now())",
    )
    .bind(&id)
    .bind(&domain)
    .execute(state.db.as_ref().expect("db"))
    .await
    .expect("mint");
    organization(&state, &r, &domain).await;
    let record = format!("_joinedcontext.{domain}");
    dns.set(&record, &["jc-verify=c0ffee"]);
    let host = format!("smartcity.{domain}");
    let (status, made) = propose(
        &state,
        operator(&state, "ops-a"),
        json!({ "kind": "HostAdd", "host": host, "organization": r, "organizationId": id, "role": "working" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let change = made["id"].as_str().expect("an id").to_owned();
    dns.set(&record, &["v=spf1 -all"]);
    let (status, refused) = decide(&state, operator(&state, "ops-b"), &change, "approve").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert!(refused.to_string().contains("organizationId"), "{refused}");
    assert_eq!(state_of(&state, &host).await, None);
    dns.set(&record, &["jc-verify=c0ffee"]);
    let (status, applied) = decide(&state, operator(&state, "ops-b"), &change, "approve").await;
    assert_eq!(status, StatusCode::OK, "{applied}");
}

#[tokio::test]
async fn a_host_taken_since_the_proposal_is_409_at_approval() {
    let (state, _) = world().await;
    let r = run("c");
    organization(&state, &r, &format!("{r}.sk")).await;
    let host = format!("portal.{r}.{BASE}");
    let (_, made) = propose(
        &state,
        operator(&state, "ops-a"),
        json!({ "kind": "HostAdd", "host": host, "organization": r, "role": "working" }),
    )
    .await;
    let id = made["id"].as_str().expect("an id");
    let again = propose(
        &state,
        operator(&state, "ops-c"),
        json!({ "kind": "HostAdd", "host": host, "organization": r, "role": "working" }),
    )
    .await;
    assert_eq!(again.0, StatusCode::CONFLICT, "one open Change per host");
    seed(
        &state.db.clone().expect("db"),
        &[SeedHost {
            host: host.clone(),
            organization: format!("other{r}"),
            organization_id: None,
            role: "working".into(),
        }],
    )
    .await
    .expect("seeded");
    let (status, refused) = decide(&state, operator(&state, "ops-b"), id, "approve").await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
}

#[tokio::test]
async fn each_field_is_checked_and_named() {
    let (state, _) = world().await;
    let r = run("f");
    organization(&state, &r, &format!("{r}.sk")).await;
    for (change, field) in [
        (
            json!({ "kind": "HostAdd", "host": BASE, "organization": r, "role": "working" }),
            "host",
        ),
        (
            json!({ "kind": "HostAdd", "host": format!("api.{BASE}"), "organization": r, "role": "working" }),
            "host",
        ),
        (
            json!({ "kind": "HostAdd", "host": format!("a.{r}.{BASE}"), "organization": r, "role": "admin" }),
            "role",
        ),
        (
            json!({ "kind": "HostAdd", "host": format!("a.{r}.{BASE}"), "organization": format!("none{r}"), "role": "working" }),
            "organization",
        ),
        (
            json!({ "kind": "HostAdd", "host": format!("a.{r}.{BASE}"), "organization": r, "role": "working", "subject": "x" }),
            "subject",
        ),
        (
            json!({ "kind": "HostAdd", "organization": r, "role": "working" }),
            "host",
        ),
        (
            json!({ "kind": "HostRemove", "host": format!("never.{r}.{BASE}") }),
            "host",
        ),
        (
            json!({ "kind": "OperatorAdd", "subject": "ops-b" }),
            "subject",
        ),
    ] {
        let (status, refused) = propose(&state, operator(&state, "ops-a"), change.clone()).await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{change}: {refused}"
        );
        assert!(
            refused["errors"]
                .as_array()
                .is_some_and(|errors| errors.iter().any(|e| e["path"] == field)),
            "{change}: {refused}"
        );
    }
    let unknown = propose(
        &state,
        operator(&state, "ops-a"),
        json!({ "kind": "SignInLook", "host": "x" }),
    )
    .await;
    assert_eq!(unknown.0, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_removal_waits_for_a_second_operator_and_any_operator_rejects() {
    let (state, _) = world().await;
    let r = run("r");
    let host = format!("old.{r}.{BASE}");
    seed(
        &state.db.clone().expect("db"),
        &[SeedHost {
            host: host.clone(),
            organization: r.clone(),
            organization_id: None,
            role: "working".into(),
        }],
    )
    .await
    .expect("seeded");
    let (_, made) = propose(
        &state,
        operator(&state, "ops-a"),
        json!({ "kind": "HostRemove", "host": host }),
    )
    .await;
    let id = made["id"].as_str().expect("an id").to_owned();
    let (status, rejected) = decide(&state, operator(&state, "ops-a"), &id, "reject").await;
    assert_eq!(status, StatusCode::OK, "{rejected}");
    assert_eq!(rejected["state"], "rejected");
    assert_eq!(
        state_of(&state, &host).await.map(|s| s.0).as_deref(),
        Some("served")
    );
    let (_, made) = propose(
        &state,
        operator(&state, "ops-a"),
        json!({ "kind": "HostRemove", "host": host }),
    )
    .await;
    let id = made["id"].as_str().expect("an id").to_owned();
    assert_eq!(
        decide(&state, operator(&state, "ops-b"), &id, "approve")
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        state_of(&state, &host).await.map(|s| s.0).as_deref(),
        Some("removed")
    );
    let served = rows(&state.db.clone().expect("db"))
        .await
        .expect("rows")
        .into_iter()
        .any(|row| row.host == host && row.state == "served");
    assert!(!served);
}

/// A person id of the installation realm no other run of these tests uses.
fn fresh_subject() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!(
        "{:08x}-0000-4000-8000-{:012x}",
        std::process::id(),
        nanos & 0xffff_ffff_ffff
    )
}

/// The installation realm as a mock: `subject` is the person a Change names (as `username`),
/// the operators' group has `members`, and the proposer `ops-a` is in it when `proposer_in`.
/// Each test mounts the membership write it expects on the returned server.
async fn realm(
    state: &mut AppState,
    subject: &str,
    username: &str,
    proposer_in: bool,
    members: &[&str],
) -> MockServer {
    let kc = MockServer::start().await;
    let admin = "/admin/realms/ops";
    let group = state.config.bootstrap_admins.clone();
    let user = |id: &str, username: &str| json!({ "id": id, "username": username, "enabled": true, "emailVerified": true });
    Mock::given(method("POST"))
        .and(path("/realms/ops/protocol/openid-connect/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "access_token": "t" })))
        .mount(&kc)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{admin}/users/{subject}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(user(subject, username)))
        .mount(&kc)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{admin}/groups")))
        .and(query_param("search", group.as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": "g-ops", "name": group, "path": format!("/{group}"), "subGroups": [] }
        ])))
        .mount(&kc)
        .await;
    let proposer_groups = if proposer_in {
        json!([{ "path": format!("/{group}") }])
    } else {
        json!([])
    };
    Mock::given(method("GET"))
        .and(path(format!("{admin}/users/sub-ops-a/groups")))
        .respond_with(ResponseTemplate::new(200).set_body_json(proposer_groups))
        .mount(&kc)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "{admin}/users/sub-ops-a/role-mappings/realm/composite"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&kc)
        .await;
    let listed: Vec<Value> = members.iter().map(|id| user(id, id)).collect();
    Mock::given(method("GET"))
        .and(path(format!("{admin}/groups/g-ops/members")))
        .respond_with(ResponseTemplate::new(200).set_body_json(listed))
        .mount(&kc)
        .await;
    state.people = People::new(
        &format!("{}/realms/ops", kc.uri()),
        "portal".into(),
        ClientAuth::Secret("secret".into()),
    )
    .map(Arc::new);
    kc
}

/// The reads name the operator behind a user id when the realm answers it, and keep the id
/// alone when it does not (the reconciler, an id the realm lacks).
#[tokio::test]
async fn the_changes_and_the_audit_name_the_operator_who_asked() {
    let (mut state, _) = world().await;
    let subject = fresh_subject();
    let _kc = realm(&mut state, &subject, "jana@ops.example", true, &[]).await;
    let mut jana = operator(&state, "jana");
    jana.subject = subject.clone();
    let r = run("n");
    organization(&state, &r, &format!("{r}.sk")).await;
    let host = format!("named.{r}.{BASE}");
    let (status, made) = propose(
        &state,
        jana,
        json!({ "kind": "HostAdd", "host": host, "organization": r, "role": "working" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let ops = operator(&state, "ops-b");
    let changes = body(&send(&state, ops.clone(), "GET", CHANGES, None).await.text);
    let mine = changes["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|c| c["id"] == made["id"])
        .expect("the Change");
    assert_eq!(mine["proposedBy"], subject.as_str());
    assert_eq!(mine["proposedByName"], "jana@ops.example");
    let audit = body(
        &send(&state, ops, "GET", "/api/v1/operator/audit", None)
            .await
            .text,
    );
    let rows = audit["items"].as_array().expect("items");
    let row = rows
        .iter()
        .find(|row| row["actor"] == subject.as_str())
        .expect("the proposal's audit row");
    assert_eq!(row["actorName"], "jana@ops.example");
    assert!(
        rows.iter()
            .filter(|row| row["actor"] == "reconciler")
            .all(|row| row.get("actorName").is_none()),
        "the reconciler keeps no name"
    );
}

/// The membership write of an approval, expected `times` times.
async fn expect_write(kc: &MockServer, verb: &str, subject: &str, status: u16, times: u64) {
    Mock::given(method(verb))
        .and(path(format!(
            "/admin/realms/ops/users/{subject}/groups/g-ops"
        )))
        .respond_with(ResponseTemplate::new(status))
        .expect(times)
        .mount(kc)
        .await;
}

async fn change_state(state: &AppState, id: &str) -> String {
    sqlx::query_scalar("SELECT state FROM operator_changes WHERE id = $1::uuid")
        .bind(id)
        .fetch_one(state.db.as_ref().expect("db"))
        .await
        .expect("change")
}

async fn propose_operator(state: &AppState, kind: &str, subject: &str) -> String {
    let (status, made) = propose(
        state,
        operator(state, "ops-a"),
        json!({ "kind": kind, "subject": subject }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    made["id"].as_str().expect("an id").to_owned()
}

/// The operators' group changes only through an approved `OperatorAdd`: the realm's group of
/// that name, found by an exact search, gets the person once a second operator approves.
#[tokio::test]
async fn an_approved_operator_add_puts_the_person_into_the_operators_group() {
    let (mut state, _) = world().await;
    let subject = fresh_subject();
    let kc = realm(&mut state, &subject, "eva@ops.example", true, &[]).await;
    expect_write(&kc, "PUT", &subject, 204, 1).await;
    let id = propose_operator(&state, "OperatorAdd", &subject).await;
    assert_eq!(
        decide(&state, operator(&state, "ops-a"), &id, "approve")
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let (status, applied) = decide(&state, operator(&state, "ops-b"), &id, "approve").await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    assert_eq!(change_state(&state, &id).await, "applied");
    kc.verify().await;
}

/// A client's service account is never proposed as an operator (T-7139 bounce row 4).
#[tokio::test]
async fn a_service_account_is_never_an_operator() {
    let (mut state, _) = world().await;
    let subject = fresh_subject();
    let _kc = realm(&mut state, &subject, "service-account-ci", true, &[]).await;
    let (status, refused) = propose(
        &state,
        operator(&state, "ops-a"),
        json!({ "kind": "OperatorAdd", "subject": subject }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert!(refused.to_string().contains("service account"), "{refused}");
}

/// A removal that would leave one operator is refused at approval, so nobody needs the cluster
/// to approve a Change again; with a third operator it applies (bounce row 2).
#[tokio::test]
async fn the_last_two_operators_are_never_removed() {
    let (mut state, _) = world().await;
    let subject = fresh_subject();
    let kc = realm(
        &mut state,
        &subject,
        "b@ops.example",
        true,
        &[&subject, "sub-ops-a"],
    )
    .await;
    expect_write(&kc, "DELETE", &subject, 204, 0).await;
    let id = propose_operator(&state, "OperatorRemove", &subject).await;
    let (status, refused) = decide(&state, operator(&state, "ops-b"), &id, "approve").await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert!(refused.to_string().contains("at least 2"), "{refused}");
    assert_eq!(change_state(&state, &id).await, "proposed");
    kc.verify().await;

    let kc = realm(
        &mut state,
        &subject,
        "b@ops.example",
        true,
        &[&subject, "sub-ops-a", "sub-ops-c"],
    )
    .await;
    expect_write(&kc, "DELETE", &subject, 204, 1).await;
    let (status, applied) = decide(&state, operator(&state, "ops-c"), &id, "approve").await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    kc.verify().await;
}

/// The proposer is read again at approval: two operators who each propose the other's removal
/// cannot approve both, since the first approval takes the second proposer out (bounce row 2).
#[tokio::test]
async fn a_proposer_who_left_the_group_has_no_change_approved() {
    let (mut state, _) = world().await;
    let subject = fresh_subject();
    let kc = realm(&mut state, &subject, "c@ops.example", true, &[]).await;
    let id = propose_operator(&state, "OperatorAdd", &subject).await;
    drop(kc);
    let kc = realm(&mut state, &subject, "c@ops.example", false, &[]).await;
    expect_write(&kc, "PUT", &subject, 204, 0).await;
    let (status, refused) = decide(&state, operator(&state, "ops-b"), &id, "approve").await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert!(
        refused.to_string().contains("no operator any more"),
        "{refused}"
    );
    assert_eq!(change_state(&state, &id).await, "proposed");
    kc.verify().await;
}

/// The realm changes only after the decision and its audit row are written: an audit that
/// cannot be written leaves the realm untouched, and a realm that refuses leaves the Change open
/// with no `applied` audit row (bounce row 3).
#[tokio::test]
async fn the_realm_changes_only_after_the_audit_and_a_refusal_keeps_the_change_open() {
    let (mut state, _) = world().await;
    let pool = state.db.clone().expect("db");
    sqlx::raw_sql(
        "CREATE OR REPLACE FUNCTION test_audit_fails() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN IF NEW.actor = 'sub-audit-fails' THEN RAISE EXCEPTION 'audit store down'; END IF; \
         RETURN NEW; END $$; \
         DROP TRIGGER IF EXISTS test_audit_fails ON operator_audit; \
         CREATE TRIGGER test_audit_fails BEFORE INSERT ON operator_audit \
         FOR EACH ROW EXECUTE FUNCTION test_audit_fails();",
    )
    .execute(&pool)
    .await
    .expect("a failing audit for one actor");
    let subject = fresh_subject();
    let kc = realm(&mut state, &subject, "d@ops.example", true, &[]).await;
    expect_write(&kc, "PUT", &subject, 204, 0).await;
    let id = propose_operator(&state, "OperatorAdd", &subject).await;
    let (status, _) = decide(&state, operator(&state, "audit-fails"), &id, "approve").await;
    assert!(status.is_server_error(), "{status}");
    assert_eq!(change_state(&state, &id).await, "proposed");
    kc.verify().await;

    let kc = realm(&mut state, &subject, "d@ops.example", true, &[]).await;
    expect_write(&kc, "PUT", &subject, 500, 1).await;
    let (status, _) = decide(&state, operator(&state, "ops-b"), &id, "approve").await;
    assert_ne!(status, StatusCode::OK);
    assert_eq!(change_state(&state, &id).await, "proposed");
    assert!(
        !audit_of(&state, &id)
            .await
            .iter()
            .any(|(_, action)| action.starts_with("applied")),
        "no applied row"
    );
    kc.verify().await;
}

/// The Portal's own age key, which seals a Change's SMTP password until its approval.
fn with_age_key(state: &mut AppState) {
    use age::secrecy::ExposeSecret;
    let identity = age::x25519::Identity::generate();
    let dir = std::env::temp_dir().join(format!("operator-smtp-{}", run("k")));
    std::fs::create_dir_all(&dir).expect("dir");
    let key_file = dir.join("keys.txt");
    std::fs::write(
        &key_file,
        format!("{}\n", identity.to_string().expose_secret()),
    )
    .expect("key");
    let mut config = (*state.config).clone();
    config.pipeline_secrets = Some(joinedcontext_portal::pipeline_secrets::Backend::Sops {
        age_key_file: key_file,
    });
    state.config = Arc::new(config);
}

/// T-7152: the installation's e-mail server is an operator Change like any other. An
/// Organization's administrator reads and asks for nothing; the proposer never approves; the
/// password is sealed until a second operator approves, written to the sign-in service then,
/// and never in a read, the payload, the audit or a decided row.
#[tokio::test]
async fn the_e_mail_server_waits_for_a_second_operator_and_its_password_is_never_read_back() {
    let (mut state, _) = world().await;
    with_age_key(&mut state);
    let kc = realm(&mut state, "sub-ops-b", "ops-b", true, &[]).await;
    let smtp_door = "/admin/realms/ops/smtp-server";
    Mock::given(method("GET"))
        .and(path(smtp_door))
        .respond_with(ResponseTemplate::new(404))
        .mount(&kc)
        .await;
    Mock::given(method("PUT"))
        .and(path(smtp_door))
        .respond_with(ResponseTemplate::new(204))
        .mount(&kc)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{smtp_door}/test")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "sent": true })))
        .mount(&kc)
        .await;
    let secret = format!("smtp-pw-{}", run("p"));
    let change = |host: &str, tls: &str| {
        json!({ "kind": "SmtpSet", "smtp": {
            "host": host, "port": 587, "tls": tls, "from": "noreply@example.org",
            "user": "mailer", "password": secret,
        }})
    };

    // An Organization's administrator and a password-only operator get 403 on every door (a
    // bearer token in the group: `a_bearer_in_the_operators_group_is_no_operator`).
    let org_admin = common::organization_admin(&state.mirror, "org-admin");
    let mut password_only = operator(&state, "ops-c");
    password_only.second_factor = false;
    for who in [org_admin, password_only] {
        for (verb, uri, body) in [
            ("GET", "/api/v1/operator/installation", None),
            ("POST", "/api/v1/operator/installation/test-email", None),
            (
                "POST",
                CHANGES,
                Some(change("smtp.example.org", "starttls")),
            ),
        ] {
            let answer = send(&state, who.clone(), verb, uri, body).await;
            assert_eq!(
                answer.status,
                StatusCode::FORBIDDEN,
                "{verb} {uri}: {}",
                answer.text
            );
        }
    }

    // Each field is checked and named.
    for (host, tls, field) in [
        ("10.43.0.10", "starttls", "smtp.host"),
        ("mail.default.svc.cluster.local", "starttls", "smtp.host"),
        ("smtp.example.org", "none", "smtp.tls"),
    ] {
        let (status, problem) = propose(&state, operator(&state, "ops-a"), change(host, tls)).await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{host}: {problem}"
        );
        assert!(problem.to_string().contains(field), "{field}: {problem}");
    }

    let (status, made) = propose(
        &state,
        operator(&state, "ops-a"),
        change("smtp.example.org", "starttls"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    assert_eq!(made["payload"]["smtp"]["newPassword"], json!(true));
    let id = made["id"].as_str().expect("id").to_owned();
    let pool = state.db.clone().expect("db");
    let (payload, sealed): (Value, Option<String>) =
        sqlx::query_as("SELECT payload, sealed_secret FROM operator_changes WHERE id = $1::uuid")
            .bind(&id)
            .fetch_one(&pool)
            .await
            .expect("row");
    let sealed = sealed.expect("the password is sealed while the Change is open");
    assert!(
        !payload.to_string().contains(&secret) && !sealed.contains(&secret),
        "{payload}"
    );

    let (status, _) = decide(&state, operator(&state, "ops-a"), &id, "approve").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "the proposer never approves");
    let (status, applied) = decide(&state, operator(&state, "ops-b"), &id, "approve").await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    assert_eq!(applied["state"], json!("applied"));

    // The sign-in service got the password once; nothing the Portal keeps or answers holds it.
    let puts: Vec<Value> = kc
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.method.as_str() == "PUT" && r.url.path() == smtp_door)
        .map(|r| serde_json::from_slice(&r.body).unwrap_or_default())
        .collect();
    assert_eq!(puts.len(), 1, "{puts:?}");
    assert_eq!(puts[0]["password"], json!(secret));
    assert_eq!(puts[0]["host"], json!("smtp.example.org"));
    let sealed: Option<String> =
        sqlx::query_scalar("SELECT sealed_secret FROM operator_changes WHERE id = $1::uuid")
            .bind(&id)
            .fetch_one(&pool)
            .await
            .expect("row");
    assert_eq!(sealed, None, "a decided Change holds no secret");
    let audit: Vec<(Option<Value>, Option<Value>)> =
        sqlx::query_as("SELECT before, after FROM operator_audit WHERE change_id = $1::uuid")
            .bind(&id)
            .fetch_all(&pool)
            .await
            .expect("audit");
    assert_eq!(audit.len(), 2, "proposed and applied: {audit:?}");
    let list = send(&state, operator(&state, "ops-a"), "GET", CHANGES, None).await;
    for text in [format!("{audit:?}"), list.text.clone()] {
        assert!(!text.contains(&secret), "{text}");
    }

    // The Installation page reads the settings back without the password, and the test e-mail
    // goes to the operator's own address.
    Mock::given(method("GET"))
        .and(path(smtp_door))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "host": "smtp.example.org", "port": 587, "tls": "starttls",
            "from": "noreply@example.org", "user": "mailer", "passwordSet": true
        })))
        .with_priority(1)
        .mount(&kc)
        .await;
    let read = send(
        &state,
        operator(&state, "ops-a"),
        "GET",
        "/api/v1/operator/installation",
        None,
    )
    .await;
    assert_eq!(read.status, StatusCode::OK, "{}", read.text);
    let read = body(&read.text);
    assert_eq!(read["smtp"]["passwordSet"], json!(true));
    assert_eq!(read["smtpChange"], Value::Null);
    let tested = send(
        &state,
        operator(&state, "ops-a"),
        "POST",
        "/api/v1/operator/installation/test-email",
        None,
    )
    .await;
    assert_eq!(tested.status, StatusCode::OK, "{}", tested.text);
    assert_eq!(body(&tested.text)["sent"], json!(true));
    let to: Vec<Value> = kc
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.url.path().ends_with("/smtp-server/test"))
        .map(|r| serde_json::from_slice(&r.body).unwrap_or_default())
        .collect();
    assert_eq!(to, vec![json!({ "to": "ops-a@hel.fi" })]);
}

/// T-7188: the installation's look is an operator Change. An Organization's administrator asks
/// for none; a logo with script, a CSS colour or an Organization's dashboards are refused by
/// field; the proposer never approves; once a second operator approves, the Portal's branding and
/// logo show it from the next request on, a restarted Portal reads it back, and the audit keeps
/// no SVG.
#[tokio::test]
async fn the_installation_look_waits_for_a_second_operator_and_then_shows_on_every_page() {
    let (mut state, _) = world().await;
    let kc = realm(&mut state, "sub-ops-b", "ops-b", true, &[]).await;
    Mock::given(method("GET"))
        .and(path("/admin/realms/ops/smtp-server"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&kc)
        .await;
    let name = format!("Smart City {}", run("look"));
    let logo = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 8 8"><rect width="8" height="8" fill="#0b5394"/></svg>"##;
    let change = |branding: Value| json!({ "kind": "BrandingSet", "branding": branding });
    let good = change(json!({
        "instanceName": name, "colours": { "primary": "#0b5394" }, "logoSvg": logo,
    }));

    let org_admin = common::organization_admin(&state.mirror, "org-admin");
    let (status, _) = propose(&state, org_admin, good.clone()).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "only an operator asks for it"
    );

    for (branding, field) in [
        (
            json!({ "logoSvg": "<svg xmlns=\"http://www.w3.org/2000/svg\"><script>alert(1)</script></svg>" }),
            "branding.logoSvg",
        ),
        (
            json!({ "colours": { "primary": "red;}" } }),
            "branding.colours.primary",
        ),
        (
            json!({ "homeWidgets": ["doprava/prehlad"] }),
            "branding.homeWidgets",
        ),
        (
            json!({ "instanceName": "<b>x</b>" }),
            "branding.instanceName",
        ),
    ] {
        let (status, problem) = propose(&state, operator(&state, "ops-a"), change(branding)).await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{field}: {problem}"
        );
        assert!(problem.to_string().contains(field), "{field}: {problem}");
    }
    let (status, problem) = propose(
        &state,
        operator(&state, "ops-a"),
        json!({ "kind": "SmtpSet", "branding": {} }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");

    let (status, made) = propose(&state, operator(&state, "ops-a"), good).await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let id = made["id"].as_str().expect("id").to_owned();
    let read = send(
        &state,
        operator(&state, "ops-a"),
        "GET",
        "/api/v1/operator/installation",
        None,
    )
    .await;
    assert_eq!(read.status, StatusCode::OK, "{}", read.text);
    assert_eq!(body(&read.text)["brandingChange"], json!(id));
    let shown = send(
        &state,
        operator(&state, "ops-a"),
        "GET",
        "/api/v1/branding",
        None,
    )
    .await;
    assert_ne!(
        body(&shown.text)["instanceName"],
        json!(name),
        "nothing shows before the approval"
    );

    let (status, _) = decide(&state, operator(&state, "ops-a"), &id, "approve").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "the proposer never approves");
    let (status, applied) = decide(&state, operator(&state, "ops-b"), &id, "approve").await;
    assert_eq!(status, StatusCode::OK, "{applied}");

    let check = |state: AppState| {
        let name = name.clone();
        async move {
            let shown = send(
                &state,
                operator(&state, "ops-a"),
                "GET",
                "/api/v1/branding",
                None,
            )
            .await;
            let shown = body(&shown.text);
            assert_eq!(shown["instanceName"], json!(name), "{shown}");
            assert_eq!(shown["colours"]["primary"], json!("#0b5394"));
            assert_eq!(shown["logo"], json!("organization.svg"));
            let served = send(
                &state,
                operator(&state, "ops-a"),
                "GET",
                "/api/v1/branding/logo",
                None,
            )
            .await;
            assert_eq!(served.status, StatusCode::OK);
            assert_eq!(served.text, logo);
        }
    };
    check(state.clone()).await;
    let read = send(
        &state,
        operator(&state, "ops-a"),
        "GET",
        "/api/v1/operator/installation",
        None,
    )
    .await;
    let read = body(&read.text);
    assert_eq!(read["branding"]["instanceName"], json!(name), "{read}");
    assert_eq!(read["brandingChange"], Value::Null);

    // A restarted Portal reads the look back from the last applied Change.
    state.set_installation_look(Default::default());
    assert!(state.installation_look().is_none());
    state.load_installation_look().await;
    check(state.clone()).await;

    let audit: Vec<Option<Value>> =
        sqlx::query_scalar("SELECT after FROM operator_audit WHERE change_id = $1::uuid AND action = 'applied BrandingSet'")
            .bind(&id)
            .fetch_all(state.db.as_ref().expect("db"))
            .await
            .expect("audit");
    assert_eq!(audit.len(), 1, "{audit:?}");
    let after = audit[0].clone().expect("after");
    assert_eq!(after["logo"], json!(true));
    assert!(!after.to_string().contains("<svg"), "{after}");

    // An empty look shows the branding file again.
    let (_, reset) = propose(&state, operator(&state, "ops-a"), change(json!({}))).await;
    let reset = reset["id"].as_str().expect("id").to_owned();
    let (status, _) = decide(&state, operator(&state, "ops-b"), &reset, "approve").await;
    assert_eq!(status, StatusCode::OK);
    assert!(state.installation_look().is_none());
    let shown = send(
        &state,
        operator(&state, "ops-a"),
        "GET",
        "/api/v1/branding",
        None,
    )
    .await;
    assert_ne!(body(&shown.text)["instanceName"], json!(name));
}

/// Row-level security does not apply to a SUPERUSER or BYPASSRLS role, so the Portal refuses to
/// serve as one: its forced tenancy walls would hold nothing.
#[tokio::test]
async fn a_serving_role_that_bypasses_row_level_security_is_refused() {
    let owner_url = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .expect("set JC_PORTAL_TEST_DATABASE_URL to a PostgreSQL database the Portal migrates");
    let owner = joinedcontext_portal::db::connect(&owner_url)
        .await
        .expect("connect + migrate");
    let role = run("bypass");
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "CREATE ROLE \"{role}\" LOGIN BYPASSRLS PASSWORD 'bypass-pw'"
    )))
    .execute(&owner)
    .await
    .expect("a role with BYPASSRLS");
    let mut serving_url = url::Url::parse(&owner_url).expect("the test URL");
    serving_url.set_username(&role).expect("a user");
    serving_url
        .set_password(Some("bypass-pw"))
        .expect("a password");
    let refused = joinedcontext_portal::db::connect_as(serving_url.as_str(), Some(&owner_url))
        .await
        .expect_err("a role that bypasses row-level security");
    assert!(refused.to_string().contains("BYPASSRLS"), "{refused}");
}

/// The running Portal serves as a role apart from the one that migrates (T-7179): that role
/// appends to the audit and reads it, and changes, deletes, truncates or drops the triggers of
/// none of it. The Portal boots and serves with both URLs; one role named in both is refused.
#[tokio::test]
async fn the_serving_role_cannot_alter_the_audit_or_drop_its_triggers() {
    let owner_url = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .expect("set JC_PORTAL_TEST_DATABASE_URL to a PostgreSQL database the Portal migrates");
    let owner = joinedcontext_portal::db::connect(&owner_url)
        .await
        .expect("connect + migrate");
    let role = run("serving");
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "CREATE ROLE \"{role}\" LOGIN PASSWORD 'serving-pw'"
    )))
    .execute(&owner)
    .await
    .expect("a serving role");
    let mut serving_url = url::Url::parse(&owner_url).expect("the test URL");
    serving_url.set_username(&role).expect("a user");
    serving_url
        .set_password(Some("serving-pw"))
        .expect("a password");

    let same = joinedcontext_portal::db::connect_as(&owner_url, Some(&owner_url))
        .await
        .expect_err("one role named twice");
    assert!(
        same.to_string()
            .contains("JC_PORTAL_MIGRATION_DATABASE_URL"),
        "{same}"
    );

    let config = Config {
        database_url: Some(serving_url.to_string()),
        migration_database_url: Some(owner_url.clone()),
        ..Config::for_tests()
    };
    let state = AppState::from_config(config)
        .await
        .expect("the Portal boots");
    let pool = state.db.clone().expect("db");
    let me: String = sqlx::query_scalar("SELECT current_user::text")
        .fetch_one(&pool)
        .await
        .expect("who");
    assert_eq!(me, role, "the Portal serves as the serving role");
    joinedcontext_portal::db::save_preferences(&pool, &role, &json!({"theme": "dark"}))
        .await
        .expect("the serving role writes its rows");
    sqlx::query("INSERT INTO operator_audit (actor, action) VALUES ($1, 'probe')")
        .bind(&role)
        .execute(&pool)
        .await
        .expect("an append");
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM operator_audit WHERE actor = $1")
        .bind(&role)
        .fetch_one(&pool)
        .await
        .expect("a read");
    assert_eq!(rows, 1);
    for statement in [
        "UPDATE operator_audit SET actor = 'x' WHERE action = 'probe'",
        "DELETE FROM operator_audit WHERE action = 'probe'",
        "TRUNCATE operator_audit",
        "DROP TRIGGER operator_audit_append_only ON operator_audit",
        "DROP TRIGGER operator_audit_no_truncate ON operator_audit",
        "ALTER TABLE operator_audit DISABLE TRIGGER ALL",
        "CREATE OR REPLACE FUNCTION operator_audit_append_only() RETURNS trigger \
         LANGUAGE plpgsql AS $$ BEGIN RETURN NEW; END $$",
        "DELETE FROM _sqlx_migrations",
        "SELECT count(*) FROM project_rows_set_aside",
    ] {
        let err = sqlx::query(statement)
            .execute(&pool)
            .await
            .expect_err(statement);
        let text = err.to_string();
        assert!(
            text.contains("permission denied") || text.contains("must be owner"),
            "{statement}: {text}"
        );
    }
    drop(state);
    pool.close().await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "DROP OWNED BY \"{role}\"; DROP ROLE \"{role}\""
    )))
    .execute(&owner)
    .await
    .expect("the role removed");
}

/// The audit is append-only: the Portal's own role changes, deletes and truncates none of it.
#[tokio::test]
async fn the_audit_refuses_update_delete_and_truncate() {
    let (state, _) = world().await;
    let pool = state.db.clone().expect("db");
    sqlx::query("INSERT INTO operator_audit (actor, action) VALUES ('t', 'probe')")
        .execute(&pool)
        .await
        .expect("an append");
    for statement in [
        "UPDATE operator_audit SET actor = 'x' WHERE action = 'probe'",
        "DELETE FROM operator_audit WHERE action = 'probe'",
        "TRUNCATE operator_audit",
    ] {
        let err = sqlx::query(statement)
            .execute(&pool)
            .await
            .expect_err(statement);
        assert!(
            err.to_string().contains("append-only"),
            "{statement}: {err}"
        );
    }
}
