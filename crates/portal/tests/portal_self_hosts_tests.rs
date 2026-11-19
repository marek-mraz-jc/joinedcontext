//! Self-service portal hosts (T-7415): an Organization's administrator adds a portal host with
//! no operator when it is one label under a host the Organization serves or a name under its
//! verified domain (the TXT proof read again at the add); never under another Organization's
//! host or an unverified domain, never a reserved label, at most `SITES_PER_DAY` a day; it
//! removes only its own hosts, never one a Portal answers on; each add and removal is audited,
//! and a Portal of another Organization never takes the host.
//!
//! DNS is a table of canned records; the store is the database `JC_PORTAL_TEST_DATABASE_URL`
//! names (CI's portal-test jobs set it). A missing variable fails, never skips. Each test uses
//! Organizations and hosts of its own run, so the tests share the database.

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::http::StatusCode;
use futures_util::future::BoxFuture;
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::domain_verification::{TxtRecords, Unanswered};
use joinedcontext_portal::host_registry::{seed, seed_organizations, SeedHost, SeedOrganization};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};

use common::{envelope, person, send};

const HOSTS: &str = "/api/v1/organization/portal-hosts";
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

async fn world() -> (AppState, Arc<Dns>) {
    let url = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .expect("set JC_PORTAL_TEST_DATABASE_URL to a PostgreSQL database the Portal migrates");
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("connect + migrate");
    let mut config = Config::for_tests();
    config.installation_hosts = Some((BASE.to_owned(), format!("idm.{BASE}")));
    let dns = Arc::new(Dns::default());
    let state = AppState::new(config, None)
        .with_db(pool)
        .with_txt_records(dns.clone());
    let kinds: Vec<&str> = joinedcontext_portal::resource::kinds()
        .map(|info| info.kind)
        .collect();
    state.mirror.upsert(envelope(
        "Role",
        "org-admin",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": kinds, "verbs": ["read", "propose", "approve", "delete"] }] }),
    ));
    (state, dns)
}

fn run(test: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    format!("{test}{:x}", std::process::id() ^ nanos)
}

/// Organization `org` (domain `{org}.sk`) with its registry row, its served working host
/// `{org}.{BASE}` and one administrator scoped to it, `{org}-admin`.
async fn organization(state: &AppState, org: &str) -> Identity {
    let pool = state.db.as_ref().expect("db");
    state.mirror.upsert(envelope(
        "Organization",
        org,
        ORG_NAMESPACE,
        json!({ "domain": format!("{org}.sk") }),
    ));
    seed_organizations(
        pool,
        &[SeedOrganization {
            name: org.to_owned(),
            organization_id: None,
            host: format!("{org}.{BASE}"),
            catalogue_id: None,
        }],
    )
    .await
    .expect("organization row");
    seed(
        pool,
        &[SeedHost {
            host: format!("{org}.{BASE}"),
            organization: org.to_owned(),
            organization_id: None,
            role: "working".into(),
        }],
    )
    .await
    .expect("working host");
    let name = format!("{org}-admin");
    state.mirror.upsert(envelope(
        "RoleBinding",
        &format!("{org}-admins"),
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": format!("{name}@hel.fi") }], "role": "org-admin",
            "scope": { "organization": org } }),
    ));
    person(&name)
}

async fn add(state: &AppState, who: Identity, org: &str, host: &str) -> (StatusCode, Value) {
    let answer = send(
        state,
        who,
        "POST",
        HOSTS,
        Some(json!({ "organization": org, "host": host })),
    )
    .await;
    (
        answer.status,
        serde_json::from_str(&answer.text).unwrap_or_default(),
    )
}

async fn row(state: &AppState, host: &str) -> Option<(String, String, String)> {
    sqlx::query_as("SELECT organization, role, state FROM organization_hosts WHERE host = $1")
        .bind(host)
        .fetch_optional(state.db.as_ref().expect("db"))
        .await
        .expect("registry")
}

async fn audit_of(state: &AppState, host: &str) -> Vec<(String, String)> {
    sqlx::query_as("SELECT actor, action FROM operator_audit WHERE after->>'host' = $1 ORDER BY id")
        .bind(host)
        .fetch_all(state.db.as_ref().expect("db"))
        .await
        .expect("audit")
}

/// What the judge says of `org`'s portal `path` on `host` (a dry run): `None` when it passes.
async fn judged(
    state: &AppState,
    who: Identity,
    org: &str,
    path: &str,
    host: &str,
) -> Option<String> {
    let answer = send(
        state,
        who,
        "PUT",
        &format!("/api/v1/projects/{ORG_NAMESPACE}/portals/{org}-{path}?dryRun=All"),
        Some(portal(org, path, host)),
    )
    .await;
    let body: Value = serde_json::from_str(&answer.text).unwrap_or_default();
    match answer.status {
        StatusCode::UNPROCESSABLE_ENTITY => Some(answer.text),
        s if s.is_success() && body["valid"] == json!(false) => Some(answer.text),
        s if s.is_success() => None,
        s => Some(format!("{s}: {}", answer.text)),
    }
}

fn portal(org: &str, path: &str, host: &str) -> Value {
    json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": "Portal",
        "metadata": { "name": format!("{org}-{path}"), "namespace": ORG_NAMESPACE },
        "spec": { "organizationRef": org, "path": path, "host": host, "title": { "sk": "Doprava" } },
    })
}

#[tokio::test]
async fn an_admin_adds_a_label_under_its_own_host_approved_audited_and_listed() {
    let (state, _) = world().await;
    let a = run("a");
    let admin = organization(&state, &a).await;
    let host = format!("opendata2.{a}.{BASE}");
    let (status, made) = add(&state, admin.clone(), &a, &host).await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    assert_eq!(made["bases"], json!([format!("{a}.{BASE}")]), "{made}");
    assert_eq!(made["sites"][0]["host"], json!(host), "{made}");
    assert_eq!(made["sites"][0]["state"], json!("approved"), "{made}");
    assert_eq!(
        row(&state, &host).await,
        Some((a.clone(), "portal-site".into(), "approved".into()))
    );
    assert_eq!(
        audit_of(&state, &host).await,
        vec![(admin.subject.clone(), "portal-host-add".into())]
    );
    // The judge takes it as this Organization's portal host at once.
    assert_eq!(
        judged(&state, admin.clone(), &a, "doprava", &host).await,
        None
    );
    let (again, _) = add(&state, admin, &a, &host).await;
    assert_eq!(
        again,
        StatusCode::CONFLICT,
        "a host enters the registry once"
    );
}

#[tokio::test]
async fn another_organizations_host_an_unverified_domain_and_a_reserved_label_are_refused() {
    let (state, _) = world().await;
    let (a, b) = (run("a"), run("b"));
    let admin_a = organization(&state, &a).await;
    let admin_b = organization(&state, &b).await;
    for host in [
        format!("opendata2.{b}.{BASE}"),
        format!("data.{a}.sk"),
        format!("api.{a}.{BASE}"),
        format!("x.y.{a}.{BASE}"),
    ] {
        let (status, refused) = add(&state, admin_a.clone(), &a, &host).await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{host}: {refused}"
        );
        assert_eq!(
            refused["errors"][0]["path"],
            json!("host"),
            "{host}: {refused}"
        );
        assert_eq!(row(&state, &host).await, None, "{host}");
    }
    // Naming the other Organization in the body is a 403 naming the field.
    let (status, refused) = add(&state, admin_a.clone(), &b, &format!("x.{b}.{BASE}")).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    assert!(refused.to_string().contains("organization"), "{refused}");
    // A non-admin is refused too.
    let (status, _) = add(&state, person("nobody"), &a, &format!("x.{a}.{BASE}")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // B's host is never a Portal host of A's.
    let host_b = format!("katalog.{b}.{BASE}");
    assert_eq!(
        add(&state, admin_b, &b, &host_b).await.0,
        StatusCode::CREATED
    );
    let theirs = judged(&state, admin_a, &a, "doprava", &host_b).await;
    assert!(
        theirs.as_deref().is_some_and(|t| t.contains("spec.host")),
        "{theirs:?}"
    );
}

#[tokio::test]
async fn a_name_under_the_verified_domain_needs_the_txt_proof_now() {
    let (state, dns) = world().await;
    let a = run("v");
    let admin = organization(&state, &a).await;
    let domain = format!("{a}.sk");
    sqlx::query(
        "INSERT INTO organization_ids (id, domain, challenge, requested_by, minted_by, minted_at) \
         VALUES ($1, $2, 'c0ffee', 'op', 'op', now())",
    )
    .bind(format!("sk_{a}"))
    .bind(&domain)
    .execute(state.db.as_ref().expect("db"))
    .await
    .expect("mint");
    let host = format!("data.{domain}");
    let record = format!("_joinedcontext.{domain}");
    dns.set(&record, &["v=spf1 -all"]);
    let (status, refused) = add(&state, admin.clone(), &a, &host).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert!(refused.to_string().contains("TXT"), "{refused}");
    assert_eq!(row(&state, &host).await, None);
    dns.set(&record, &["jc-verify=c0ffee"]);
    let (status, made) = add(&state, admin, &a, &host).await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    assert_eq!(made["verifiedDomain"], json!(domain), "{made}");
}

#[tokio::test]
async fn the_daily_share_names_the_limit() {
    let (state, _) = world().await;
    let a = run("p");
    let admin = organization(&state, &a).await;
    for n in 0..joinedcontext_portal::api::portal_hosts::SITES_PER_DAY {
        let (status, made) = add(&state, admin.clone(), &a, &format!("s{n}.{a}.{BASE}")).await;
        assert_eq!(status, StatusCode::CREATED, "{made}");
    }
    let (status, refused) = add(&state, admin, &a, &format!("more.{a}.{BASE}")).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{refused}");
    assert!(
        refused.to_string().contains("in the last 24 hours"),
        "{refused}"
    );
}

#[tokio::test]
async fn removal_is_the_owners_alone_refused_while_a_portal_answers_and_audited() {
    let (state, _) = world().await;
    let (a, b) = (run("r"), run("s"));
    let admin_a = organization(&state, &a).await;
    let admin_b = organization(&state, &b).await;
    let host = format!("old.{a}.{BASE}");
    assert_eq!(
        add(&state, admin_a.clone(), &a, &host).await.0,
        StatusCode::CREATED
    );
    let path = format!("{HOSTS}/{host}");
    let theirs = send(&state, admin_b, "DELETE", &path, None).await;
    assert_eq!(theirs.status, StatusCode::NOT_FOUND, "{}", theirs.text);
    // A seeded host is the operator's.
    let seeded = send(
        &state,
        admin_a.clone(),
        "DELETE",
        &format!("{HOSTS}/{a}.{BASE}"),
        None,
    )
    .await;
    assert_eq!(
        seeded.status,
        StatusCode::NOT_FOUND,
        "a working host is no portal host"
    );
    state
        .mirror
        .upsert(serde_json::from_value(portal(&a, "doprava", &host)).expect("envelope"));
    let used = send(&state, admin_a.clone(), "DELETE", &path, None).await;
    assert_eq!(used.status, StatusCode::CONFLICT, "{}", used.text);
    assert!(used.text.contains("doprava"), "{}", used.text);
    state
        .mirror
        .remove(&joinedcontext_portal::resource::ResourceKey {
            namespace: ORG_NAMESPACE.into(),
            kind: "Portal".into(),
            name: format!("{a}-doprava"),
        });
    let gone = send(&state, admin_a.clone(), "DELETE", &path, None).await;
    assert_eq!(gone.status, StatusCode::OK, "{}", gone.text);
    assert_eq!(
        row(&state, &host).await.map(|r| r.2),
        Some("removed".into())
    );
    assert_eq!(
        audit_of(&state, &host).await,
        vec![
            (admin_a.subject.clone(), "portal-host-add".into()),
            (admin_a.subject.clone(), "portal-host-remove".into()),
        ]
    );
}
