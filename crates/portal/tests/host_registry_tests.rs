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
    propose, remove, rows, seed, Proposal, SeedHost, SEEDED_BY,
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

/// A mirror holding each `(name, domain)` as an Organization manifest.
fn orgs(all: &[(&str, &str)]) -> Mirror {
    let mirror = Mirror::new();
    for (name, domain) in all {
        mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: "Organization".to_owned(),
            metadata: ObjectMeta::new(name, ORG_NAMESPACE),
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
    let mirror = orgs(&[(&org, &domain), (&unproved, &never)]);
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
    let mirror = orgs(&[(&a, &a_domain), (&b, &b_domain)]);
    let host = format!("smartcity.{b_domain}");
    // A names B's minted id to pass the minted-domain rule for a host under B's domain.
    let e = propose(&pool, &mirror, &RULES, &proposal(&host, &a, Some(&b_id)))
        .await
        .expect_err("another Organization's id");
    assert_eq!(status(e), StatusCode::UNPROCESSABLE_ENTITY);
    // An Organization the mirror does not hold has no id to prove a host by.
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

#[tokio::test]
async fn one_host_one_organization_also_when_two_writes_race() {
    let pool = pool().await;
    let host = format!("smartcity.{}.jcdev.example", run("c"));
    let (alpha, beta) = (
        proposal(&host, "alpha", None),
        proposal(&host, "beta", None),
    );
    let (a, b) = tokio::join!(
        propose(&pool, &Mirror::new(), &RULES, &alpha),
        propose(&pool, &Mirror::new(), &RULES, &beta),
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
