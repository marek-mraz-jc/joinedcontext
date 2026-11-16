//! The host registry (evidence/T-7129/design.md §1, T-7135): table `organization_hosts`
//! (migration 0059), every host an Organization answers on, its role and state. jc seeds it
//! from jc.toml at start ([`seed`]); an operator's host Change writes through [`propose`], which
//! holds the rules jc holds on the seed (`jc_router::registry`), and [`remove`] leaves a
//! tombstone the seed never revives.

use crate::error::ApiError;
use crate::store::Mirror;
use jc_router::registry::{self, Entry, Refusal, Rules};
use sqlx::PgPool;

/// Who wrote a seeded row.
pub const SEEDED_BY: &str = "jc.toml";

/// One host jc.toml names for an Organization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedHost {
    /// Lowercase, without a port.
    pub host: String,
    pub organization: String,
    /// The Organization id jc.toml sets (`sk_bbsk`); linked only when it is in `organization_ids`.
    pub organization_id: Option<String>,
    /// What answers on the host: `working`, `opendata`, `api`, `signin`, `apps`, `portal-site`, `host`.
    pub role: String,
}

/// One registry row.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct HostRow {
    pub host: String,
    pub organization: String,
    pub organization_id: Option<String>,
    pub role: String,
    pub state: String,
}

/// The role of a sign-in host, as jc's router names it.
pub const SIGN_IN_ROLE: &str = "signin";

/// Inserts each seeded host that has no row yet, as `served` (jc.toml's hosts answer today).
/// A row already there, a `removed` one above all, is never changed. Returns the rows inserted.
pub async fn seed(pool: &PgPool, hosts: &[SeedHost]) -> Result<u64, sqlx::Error> {
    let mut inserted = 0;
    for h in hosts {
        inserted += sqlx::query(
            "INSERT INTO organization_hosts (host, organization, organization_id, role, state, created_by) \
             VALUES ($1, $2, (SELECT id FROM organization_ids WHERE id = $3), $4, 'served', $5) \
             ON CONFLICT (host) DO NOTHING",
        )
        .bind(&h.host)
        .bind(&h.organization)
        .bind(&h.organization_id)
        .bind(&h.role)
        .bind(SEEDED_BY)
        .execute(pool)
        .await?
        .rows_affected();
    }
    Ok(inserted)
}

/// Every row, removed ones included, by host.
pub async fn rows(pool: &PgPool) -> Result<Vec<HostRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT host, organization, organization_id, role, state FROM organization_hosts ORDER BY host",
    )
    .fetch_all(pool)
    .await
}

/// A host an operator asks for.
#[derive(Debug, Clone, Copy)]
pub struct Proposal<'a> {
    pub host: &'a str,
    pub organization: &'a str,
    /// The minted Organization id the host is proved by: the one `organization`'s manifest
    /// derives from its `spec.domain`, never another Organization's.
    pub organization_id: Option<&'a str>,
    pub role: &'a str,
    pub by: &'a str,
}

/// Adds `p` as `proposed` once the registry's rules hold against every row, in one
/// transaction that locks out a concurrent write (two Changes for one host: the second is `409`).
/// A host outside the installation's domain must lie under the domain of the minted id `p` names,
/// and that id must be the one `p.organization`'s manifest in `mirror` derives.
pub async fn propose(
    pool: &PgPool,
    mirror: &Mirror,
    rules: &Rules<'_>,
    p: &Proposal<'_>,
) -> Result<(), ApiError> {
    if let Some(id) = p.organization_id {
        let own = crate::reconciler::groups::organization_id(mirror, p.organization);
        if own.as_deref() != Some(id) {
            return Err(refused(Refusal::Invalid(format!(
                "organizationId '{id}' is not the id of Organization '{}'",
                p.organization
            ))));
        }
    }
    let host = p.host.trim().to_ascii_lowercase();
    let mut tx = pool.begin().await.map_err(db)?;
    sqlx::query("LOCK TABLE organization_hosts IN SHARE ROW EXCLUSIVE MODE")
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    let existing: Vec<HostRow> = sqlx::query_as(
        "SELECT host, organization, organization_id, role, state FROM organization_hosts",
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(db)?;
    let minted: Option<String> = match p.organization_id {
        Some(id) => sqlx::query_scalar(
            "SELECT domain FROM organization_ids WHERE id = $1 AND minted_at IS NOT NULL",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?,
        None => None,
    };
    if p.organization_id.is_some() && minted.is_none() {
        return Err(refused(Refusal::Invalid(format!(
            "organizationId '{}' is not a minted Organization id",
            p.organization_id.unwrap_or_default()
        ))));
    }
    let entries: Vec<Entry<'_>> = existing.iter().map(entry).collect();
    let new = Entry {
        host: &host,
        organization: p.organization,
        sign_in: p.role == SIGN_IN_ROLE,
    };
    registry::check(rules, &entries, &new, minted.as_deref()).map_err(refused)?;
    sqlx::query(
        "INSERT INTO organization_hosts (host, organization, organization_id, role, state, created_by) \
         VALUES ($1, $2, $3, $4, 'proposed', $5)",
    )
    .bind(&host)
    .bind(p.organization)
    .bind(p.organization_id)
    .bind(p.role)
    .bind(p.by)
    .execute(&mut *tx)
    .await
    .map_err(db)?;
    tx.commit().await.map_err(db)
}

/// Marks `host` removed; `false` when it has no row or is removed already. The row stays as the
/// tombstone that keeps the seed from adding the host again.
pub async fn remove(pool: &PgPool, host: &str) -> Result<bool, sqlx::Error> {
    Ok(sqlx::query(
        "UPDATE organization_hosts SET state = 'removed', updated_at = now() \
         WHERE host = $1 AND state <> 'removed'",
    )
    .bind(host)
    .execute(pool)
    .await?
    .rows_affected()
        == 1)
}

fn entry(row: &HostRow) -> Entry<'_> {
    Entry {
        host: &row.host,
        organization: &row.organization,
        sign_in: row.role == SIGN_IN_ROLE,
    }
}

fn refused(r: Refusal) -> ApiError {
    match r {
        Refusal::Invalid(m) => ApiError::Unprocessable {
            detail: m.clone(),
            errors: vec![format!("host: {m}")],
        },
        Refusal::Conflict(m) => ApiError::Conflict(format!("host: {m}")),
    }
}

fn db(e: sqlx::Error) -> ApiError {
    tracing::error!(error = %e, "host registry");
    ApiError::Unavailable("the host registry could not be read or written; try again".into())
}
