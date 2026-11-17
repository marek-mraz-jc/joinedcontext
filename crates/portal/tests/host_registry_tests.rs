//! The host registry (evidence/T-7129/design.md §1 and §7, T-7135): a write holds the rules the
//! jc.toml seed holds, a host outside the installation's domain needs the Organization's minted
//! domain (label by label), a host belongs to one Organization even when two writes race, and a
//! removed host stays removed through every later seed (a restart).
//!
//! The store is the database `JC_PORTAL_TEST_DATABASE_URL` names (CI's portal-test jobs set it).
//! A missing variable fails, never skips. Each test uses hosts of its own run, so the tests share
//! the database.

use axum::http::StatusCode;
use axum::response::IntoResponse;
use jc_router::registry::Rules;
use joinedcontext_portal::error::ApiError;
use joinedcontext_portal::host_registry::{
    organizations, propose, remove, rows, seed, seed_organizations, Proposal, SeedHost,
    SeedOrganization, SEEDED_BY,
};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::store::Mirror;
use sqlx::PgPool;

const RULES: Rules<'static> = Rules {
    base_domain: "jcdev.example",
    login_host: "idm.jcdev.example",
    reserved: jc_core::names::RESERVED_LABELS,
};

async fn pool() -> PgPool {
    let url = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .expect("set JC_PORTAL_TEST_DATABASE_URL to a PostgreSQL database the Portal migrates");
    joinedcontext_portal::db::connect(&url)
        .await
        .expect("connect + migrate")
}

/// A label of this run alone, e.g. `r1a2b3c4`.
fn run(test: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    format!("{test}{:x}", std::process::id() ^ nanos)
}

/// Mints `id` for `domain` the way the operator's door does once the TXT record holds.
async fn minted(pool: &PgPool, id: &str, domain: &str) {
    sqlx::query(
        "INSERT INTO organization_ids (id, domain, challenge, requested_by, minted_by, minted_at) \
         VALUES ($1, $2, 'c', 'op', 'op', now())",
    )
    .bind(id)
    .bind(domain)
    .execute(pool)
    .await
    .expect("mint");
}

fn proposal<'a>(host: &'a str, organization: &'a str, id: Option<&'a str>) -> Proposal<'a> {
    Proposal {
        host,
        organization,
        organization_id: id,
        role: "working",
        by: "operator-a",
    }
}

/// Seeds each name as an `active` Organization row without an id, as jc.toml does for an
/// Organization it names without `id`.
async fn register(pool: &PgPool, names: &[&str]) {
    let seeded: Vec<SeedOrganization> = names
        .iter()
        .map(|name| SeedOrganization {
            name: (*name).to_owned(),
            organization_id: None,
            host: format!("smartcity.{name}.jcdev.example"),
        })
        .collect();
    seed_organizations(pool, &seeded).await.expect("seeded");
}

/// A mirror holding each `(name, domain)` as an Organization manifest, each also an Organization
/// row without an id (its manifest's domain proves its hosts).
async fn orgs(pool: &PgPool, all: &[(&str, &str)]) -> Mirror {
    register(pool, &all.iter().map(|(name, _)| *name).collect::<Vec<_>>()).await;
    let mirror = Mirror::new();
    for (name, domain) in all {
        mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: "Organization".to_owned(),
            metadata: ObjectMeta::new(*name, ORG_NAMESPACE),
            spec: serde_json::json!({ "domain": domain }),
            status: None,
        });
    }
    mirror
}

fn status(e: ApiError) -> StatusCode {
    e.into_response().status()
}

#[tokio::test]
async fn no_domain_without_proof() {
    let pool = pool().await;
    let r = run("p");
    let domain = format!("{r}.sk");
    let id = format!("sk_{r}");
    minted(&pool, &id, &domain).await;
    let org = format!("o{r}");
    let unproved = format!("u{r}");
    let never = format!("never{r}.sk");
    let mirror = orgs(&pool, &[(&org, &domain), (&unproved, &never)]).await;
    for host in [
        "smartcity.other.sk".to_owned(),
        format!("x{domain}"),
        format!("{domain}.evil.com"),
    ] {
        let e = propose(&pool, &mirror, &RULES, &proposal(&host, &org, Some(&id)))
            .await
            .expect_err(&host);
        assert_eq!(status(e), StatusCode::UNPROCESSABLE_ENTITY, "{host}");
    }
    let unminted = propose(
        &pool,
        &mirror,
        &RULES,
        &proposal(
            &format!("a.{never}"),
            &unproved,
            Some(&format!("sk_never{r}")),
        ),
    )
    .await
    .expect_err("an id nobody minted");
    assert_eq!(status(unminted), StatusCode::UNPROCESSABLE_ENTITY);
    let host = format!("smartcity.{domain}");
    propose(&pool, &mirror, &RULES, &proposal(&host, &org, Some(&id)))
        .await
        .expect("under the minted domain");
    let row = rows(&pool)
        .await
        .expect("rows")
        .into_iter()
        .find(|row| row.host == host)
        .expect("stored");
    assert_eq!(
        (row.state.as_str(), row.organization_id.as_deref()),
        ("proposed", Some(id.as_str()))
    );
}

#[tokio::test]
async fn an_organization_never_proves_a_host_by_another_organizations_id() {
    let pool = pool().await;
    let r = run("b");
    let (a_domain, b_domain) = (format!("a{r}.sk"), format!("b{r}.sk"));
    let b_id = format!("sk_b{r}");
    minted(&pool, &format!("sk_a{r}"), &a_domain).await;
    minted(&pool, &b_id, &b_domain).await;
    let (a, b) = (format!("oa{r}"), format!("ob{r}"));
    let mirror = orgs(&pool, &[(&a, &a_domain), (&b, &b_domain)]).await;
    let host = format!("smartcity.{b_domain}");
    // A names B's minted id to pass the minted-domain rule for a host under B's domain.
    let e = propose(&pool, &mirror, &RULES, &proposal(&host, &a, Some(&b_id)))
        .await
        .expect_err("another Organization's id");
    assert_eq!(status(e), StatusCode::UNPROCESSABLE_ENTITY);
    // An Organization the registry does not hold has no id to prove a host by.
    let e = propose(
        &pool,
        &mirror,
        &RULES,
        &proposal(&host, "nobody", Some(&b_id)),
    )
    .await
    .expect_err("no manifest");
    assert_eq!(status(e), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(rows(&pool)
        .await
        .expect("rows")
        .iter()
        .all(|row| row.host != host));
    propose(&pool, &mirror, &RULES, &proposal(&host, &b, Some(&b_id)))
        .await
        .expect("B under its own domain");
}

#[tokio::test]
async fn reserved_labels_hold_at_the_portal_as_on_the_seed() {
    let pool = pool().await;
    let org = format!("o{}", run("r"));
    register(&pool, &[&org]).await;
    for host in [
        "api.jcdev.example",
        "login.jcdev.example",
        "idm.jcdev.example",
    ] {
        let e = propose(&pool, &Mirror::new(), &RULES, &proposal(host, &org, None))
            .await
            .expect_err(host);
        assert_eq!(status(e), StatusCode::UNPROCESSABLE_ENTITY, "{host}");
    }
}

/// T-7139 (integrator, T-7135 review): the installation's own domain is the shared host every
/// `/{org}/` path answers on, so no Organization takes it, whatever it proves.
#[tokio::test]
async fn the_installations_own_domain_is_no_organizations_host() {
    let pool = pool().await;
    let org = format!("o{}", run("d"));
    register(&pool, &[&org]).await;
    for host in ["jcdev.example", "JCDEV.example"] {
        let e = propose(&pool, &Mirror::new(), &RULES, &proposal(host, &org, None))
            .await
            .expect_err(host);
        assert_eq!(status(e), StatusCode::UNPROCESSABLE_ENTITY, "{host}");
    }
}

/// The channel of the next notification, within five seconds.
async fn woke(listener: &mut sqlx::postgres::PgListener) -> String {
    tokio::time::timeout(std::time::Duration::from_secs(5), listener.recv())
        .await
        .expect("a notification in time")
        .expect("a notification")
        .channel()
        .to_owned()
}

/// T-7136 (T-7129 §1): each write to the registry wakes jc's host table with
/// `NOTIFY organization_hosts` (migration 0061), a removal as much as an addition.
#[tokio::test]
async fn a_registry_write_wakes_the_router() {
    let pool = pool().await;
    let mut listener = sqlx::postgres::PgListener::connect_with(&pool)
        .await
        .expect("a listener");
    listener
        .listen("organization_hosts")
        .await
        .expect("listening");
    let host = format!("{}.notify.example", run("n"));
    let added = seed(
        &pool,
        &[SeedHost {
            host: host.clone(),
            organization: "zilina".into(),
            organization_id: None,
            role: "working".into(),
        }],
    )
    .await
    .expect("seeded");
    assert_eq!(added, 1);
    assert_eq!(woke(&mut listener).await, "organization_hosts");
    assert!(remove(&pool, &host).await.expect("removed"));
    assert_eq!(woke(&mut listener).await, "organization_hosts");
}

/// T-7181: a jc.toml Organization reads back from the registry as jc.toml names it, and a later
/// start changes nothing but the id it names once minted; a removed one stays removed.
#[tokio::test]
async fn a_seeded_organization_reads_back_unchanged_and_a_removed_one_stays_removed() {
    let pool = pool().await;
    let r = run("s");
    let id = format!("sk_s{r}");
    minted(&pool, &id, &format!("s{r}.sk")).await;
    let (kept, gone) = (format!("k{r}"), format!("g{r}"));
    let mut seeded = vec![
        SeedOrganization {
            name: kept.clone(),
            organization_id: None,
            host: format!("smartcity.s{r}.sk"),
        },
        SeedOrganization {
            name: gone.clone(),
            organization_id: None,
            host: format!("{gone}.jcdev.example"),
        },
    ];
    assert_eq!(
        seed_organizations(&pool, &seeded)
            .await
            .expect("first start"),
        2
    );
    let read = |all: &[joinedcontext_portal::host_registry::OrganizationRow], name: &str| {
        all.iter()
            .find(|o| o.name == name)
            .map(|o| (o.organization_id.clone(), o.host.clone(), o.state.clone()))
    };
    let all = organizations(&pool).await.expect("rows");
    assert_eq!(
        read(&all, &kept),
        Some((None, format!("smartcity.s{r}.sk"), "active".to_owned()))
    );
    sqlx::query("UPDATE organizations SET state = 'removed' WHERE name = $1")
        .bind(&gone)
        .execute(&pool)
        .await
        .expect("removed");
    // The next start names the minted id and another working host.
    seeded[0].organization_id = Some(id.clone());
    seeded[0].host = format!("other.s{r}.sk");
    assert_eq!(
        seed_organizations(&pool, &seeded).await.expect("restart"),
        0
    );
    let all = organizations(&pool).await.expect("rows");
    assert_eq!(
        read(&all, &kept),
        Some((
            Some(id.clone()),
            format!("smartcity.s{r}.sk"),
            "active".to_owned()
        )),
        "the id is set once, the working host stays the row's"
    );
    assert_eq!(read(&all, &gone).map(|o| o.2).as_deref(), Some("removed"));
    for (sql, bind) in [
        (
            "UPDATE organizations SET state = 'active' WHERE name = $1",
            &gone,
        ),
        ("DELETE FROM organizations WHERE name = $1", &gone),
        (
            "UPDATE organizations SET organization_id = NULL WHERE name = $1",
            &kept,
        ),
        (
            "UPDATE organizations SET name = name || 'x' WHERE name = $1",
            &kept,
        ),
    ] {
        let err = sqlx::query(sql)
            .bind(bind)
            .execute(&pool)
            .await
            .expect_err(sql);
        assert!(err.to_string().contains("Organization"), "{sql}: {err}");
    }
    // A second Organization naming the same id never takes it.
    let twin = format!("t{r}");
    seed_organizations(
        &pool,
        &[SeedOrganization {
            name: twin.clone(),
            organization_id: Some(id.clone()),
            host: format!("{twin}.jcdev.example"),
        }],
    )
    .await
    .expect("seeded");
    let all = organizations(&pool).await.expect("rows");
    assert_eq!(read(&all, &twin).and_then(|o| o.0), None);
}

/// T-7181: an Organization created at run time (a row, as an operator Change writes it) takes
/// hosts at once, proved by the row's id with no manifest anywhere; a removed one takes none.
#[tokio::test]
async fn a_run_time_organization_takes_hosts_and_a_removed_one_none() {
    let pool = pool().await;
    let r = run("o");
    let domain = format!("o{r}.sk");
    let id = format!("sk_o{r}");
    minted(&pool, &id, &domain).await;
    let org = format!("rt{r}");
    let host = format!("smartcity.{domain}");
    let none = Mirror::new();
    let e = propose(&pool, &none, &RULES, &proposal(&host, &org, Some(&id)))
        .await
        .expect_err("no Organization yet");
    assert_eq!(status(e), StatusCode::UNPROCESSABLE_ENTITY);
    sqlx::query(
        "INSERT INTO organizations (name, organization_id, host, state, created_by) \
         VALUES ($1, $2, $3, 'active', 'operator-a')",
    )
    .bind(&org)
    .bind(&id)
    .bind(&host)
    .execute(&pool)
    .await
    .expect("created");
    let other = format!("sk_x{r}");
    minted(&pool, &other, &format!("x{r}.sk")).await;
    let e = propose(
        &pool,
        &none,
        &RULES,
        &proposal(&format!("a.x{r}.sk"), &org, Some(&other)),
    )
    .await
    .expect_err("an id the row does not hold");
    assert_eq!(status(e), StatusCode::UNPROCESSABLE_ENTITY);
    propose(&pool, &none, &RULES, &proposal(&host, &org, Some(&id)))
        .await
        .expect("under the row's minted domain, no restart, no manifest");
    sqlx::query("UPDATE organizations SET state = 'removed' WHERE name = $1")
        .bind(&org)
        .execute(&pool)
        .await
        .expect("removed");
    let e = propose(
        &pool,
        &none,
        &RULES,
        &proposal(&format!("api.{domain}"), &org, Some(&id)),
    )
    .await
    .expect_err("a removed Organization");
    assert_eq!(status(e), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn one_host_one_organization_also_when_two_writes_race() {
    let pool = pool().await;
    let host = format!("smartcity.{}.jcdev.example", run("c"));
    let (alpha, beta) = (
        proposal(&host, "alpha", None),
        proposal(&host, "beta", None),
    );
    register(&pool, &["alpha", "beta"]).await;
    let mirror = Mirror::new();
    let (a, b) = tokio::join!(
        propose(&pool, &mirror, &RULES, &alpha),
        propose(&pool, &mirror, &RULES, &beta),
    );
    let lost = match (a, b) {
        (Ok(()), Err(e)) | (Err(e), Ok(())) => e,
        other => panic!("exactly one write wins: {other:?}"),
    };
    assert_eq!(status(lost), StatusCode::CONFLICT);
}

#[tokio::test]
async fn a_removed_host_stays_removed_through_a_restart() {
    let pool = pool().await;
    let r = run("t");
    let kept = format!("smartcity.{r}.jcdev.example");
    let gone = format!("api.{r}.jcdev.example");
    let seeded: Vec<SeedHost> = [(&kept, "working"), (&gone, "api")]
        .into_iter()
        .map(|(host, role)| SeedHost {
            host: host.clone(),
            organization: format!("o{r}"),
            organization_id: Some("sk_not_minted_here".into()),
            role: role.into(),
        })
        .collect();
    assert_eq!(seed(&pool, &seeded).await.expect("first start"), 2);
    assert!(remove(&pool, &gone).await.expect("remove"));
    // The next start seeds the same jc.toml again.
    assert_eq!(seed(&pool, &seeded).await.expect("restart"), 0);
    let all = rows(&pool).await.expect("rows");
    let state = |h: &str| {
        all.iter()
            .find(|row| row.host == h)
            .map(|row| row.state.clone())
    };
    assert_eq!(state(&kept).as_deref(), Some("served"));
    assert_eq!(state(&gone).as_deref(), Some("removed"));
    assert!(all
        .iter()
        .all(|row| row.host != kept || row.organization_id.is_none()));
    assert!(!remove(&pool, &gone).await.expect("twice"));
    // The tombstone is never revived, rewritten or deleted, by any path.
    for sql in [
        "UPDATE organization_hosts SET state = 'served' WHERE host = $1",
        "DELETE FROM organization_hosts WHERE host = $1",
    ] {
        let err = sqlx::query(sql)
            .bind(&gone)
            .execute(&pool)
            .await
            .expect_err(sql);
        assert!(err.to_string().contains("stays"), "{sql}: {err}");
    }
    let err = sqlx::query("DELETE FROM organization_hosts WHERE host = $1")
        .bind(&kept)
        .execute(&pool)
        .await
        .expect_err("a served row is not deleted either");
    assert!(err.to_string().contains("never delete"), "{err}");
    let seeded_by: String =
        sqlx::query_scalar("SELECT created_by FROM organization_hosts WHERE host = $1")
            .bind(&kept)
            .fetch_one(&pool)
            .await
            .expect("row");
    assert_eq!(seeded_by, SEEDED_BY);
}
