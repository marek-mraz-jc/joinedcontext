//! The host registry (evidence/T-7129/design.md §1, T-7135): table `organization_hosts`
//! (migration 0059), every host an Organization answers on, its role and state. jc seeds it
//! from jc.toml at start ([`seed`]); an operator's host Change writes through [`propose`], which
//! holds the rules jc holds on the seed (`jc_router::registry`), and [`remove`] leaves a
//! tombstone the seed never revives.
//!
//! Beside it, table `organizations` (migration 0063, T-7181): the Organizations jc runs, each
//! with its name, the minted id it is proved by and its working host. jc seeds it from jc.toml
//! the same way ([`seed_organizations`]); a host names one of its `active` rows.

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

/// Who wrote an Organization row migration 0063 made from the host registry.
const MIGRATED_BY: &str = "migration 0063";

/// One `[[organizations]]` entry of jc.toml.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedOrganization {
    pub name: String,
    /// The Organization id jc.toml sets (`sk_bbsk`); linked only when it is in `organization_ids`
    /// and no other Organization holds it.
    pub organization_id: Option<String>,
    /// The working host, lowercase, with the port jc.toml gives it (none on https).
    pub host: String,
}

/// One Organization row.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct OrganizationRow {
    pub name: String,
    pub organization_id: Option<String>,
    pub host: String,
    /// `active` or `removed`.
    pub state: String,
}

/// Inserts each seeded Organization that has no row yet, as `active`; returns the rows
/// inserted. A row already there keeps its working host and its state (a `removed` one stays
/// removed); the only changes are the id an `active` row without one gets once jc.toml names a
/// minted one, because the id is never set from a request, and the working host of a row
/// migration 0063 made.
pub async fn seed_organizations(
    pool: &PgPool,
    orgs: &[SeedOrganization],
) -> Result<u64, sqlx::Error> {
    let mut inserted = 0;
    for o in orgs {
        let mut tx = pool.begin().await?;
        let id: Option<String> = sqlx::query_scalar(
            "SELECT id FROM organization_ids WHERE id = $2 \
             AND NOT EXISTS (SELECT 1 FROM organizations WHERE organization_id = $2 AND name <> $1)",
        )
        .bind(&o.name)
        .bind(&o.organization_id)
        .fetch_optional(&mut *tx)
        .await?;
        let added = sqlx::query(
            "INSERT INTO organizations (name, organization_id, host, state, created_by) \
             VALUES ($1, $2, $3, 'active', $4) ON CONFLICT (name) DO NOTHING",
        )
        .bind(&o.name)
        .bind(&id)
        .bind(&o.host)
        .bind(SEEDED_BY)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if added == 0 {
            // A row migration 0063 made from the host registry takes jc.toml's working host
            // once (its port included), and is jc.toml's from then on.
            sqlx::query(
                "UPDATE organizations SET host = $2, created_by = $3, updated_at = now() \
                 WHERE name = $1 AND created_by = $4 AND state = 'active'",
            )
            .bind(&o.name)
            .bind(&o.host)
            .bind(SEEDED_BY)
            .bind(MIGRATED_BY)
            .execute(&mut *tx)
            .await?;
        }
        if added == 0 && id.is_some() {
            sqlx::query(
                "UPDATE organizations SET organization_id = $2, updated_at = now() \
                 WHERE name = $1 AND organization_id IS NULL AND state = 'active'",
            )
            .bind(&o.name)
            .bind(&id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        inserted += added;
    }
    Ok(inserted)
}

/// Every Organization row, removed ones included, by name.
pub async fn organizations(pool: &PgPool) -> Result<Vec<OrganizationRow>, sqlx::Error> {
    sqlx::query_as("SELECT name, organization_id, host, state FROM organizations ORDER BY name")
        .fetch_all(pool)
        .await
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
/// `p.organization` must be an `active` Organization row. A host outside the installation's
/// domain must lie under the domain of the minted id `p` names, and that id must be the row's
/// (the one its manifest in `mirror` derives while the row has none).
pub async fn propose(
    pool: &PgPool,
    mirror: &Mirror,
    rules: &Rules<'_>,
    p: &Proposal<'_>,
) -> Result<(), ApiError> {
    let mut tx = pool.begin().await.map_err(db)?;
    let host = admit(&mut tx, mirror, rules, p).await?;
    insert(&mut tx, &host, p, "proposed", None).await?;
    tx.commit().await.map_err(db)
}

/// The refusal [`propose`] would answer for `p`, writing nothing: an operator Change is
/// refused when it is asked for, and checked again when it is approved ([`add_approved`]).
pub async fn check_proposal(
    pool: &PgPool,
    mirror: &Mirror,
    rules: &Rules<'_>,
    p: &Proposal<'_>,
) -> Result<(), ApiError> {
    let mut tx = pool.begin().await.map_err(db)?;
    admit(&mut tx, mirror, rules, p).await?;
    tx.rollback().await.map_err(db)
}

/// Adds `p` as `approved` by `approved_by` inside the caller's transaction, after the same
/// rules as [`propose`] against the rows there now (a host taken since the proposal is `409`).
pub async fn add_approved(
    tx: &mut sqlx::PgConnection,
    mirror: &Mirror,
    rules: &Rules<'_>,
    p: &Proposal<'_>,
    approved_by: &str,
) -> Result<(), ApiError> {
    let host = admit(tx, mirror, rules, p).await?;
    insert(tx, &host, p, "approved", Some(approved_by)).await
}

/// Locks the registry for the rest of the transaction and checks `p` against every row; the
/// host as it is stored.
async fn admit(
    tx: &mut sqlx::PgConnection,
    mirror: &Mirror,
    rules: &Rules<'_>,
    p: &Proposal<'_>,
) -> Result<String, ApiError> {
    let row: Option<Option<String>> = sqlx::query_scalar(
        "SELECT organization_id FROM organizations WHERE name = $1 AND state = 'active'",
    )
    .bind(p.organization)
    .fetch_optional(&mut *tx)
    .await
    .map_err(db)?;
    let Some(row_id) = row else {
        let m = format!("the installation has no Organization '{}'", p.organization);
        return Err(ApiError::Unprocessable {
            detail: m.clone(),
            errors: vec![format!("organization: {m}")],
        });
    };
    if let Some(id) = p.organization_id {
        // The row's id; an Organization seeded before jc.toml named one is proved by the id its
        // manifest's `spec.domain` derives, as before the row existed.
        let own =
            row_id.or_else(|| crate::reconciler::groups::organization_id(mirror, p.organization));
        if own.as_deref() != Some(id) {
            return Err(refused(Refusal::Invalid(format!(
                "organizationId '{id}' is not the id of Organization '{}'",
                p.organization
            ))));
        }
    }
    let host = p.host.trim().to_ascii_lowercase();
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
    Ok(host)
}

async fn insert(
    tx: &mut sqlx::PgConnection,
    host: &str,
    p: &Proposal<'_>,
    state: &str,
    approved_by: Option<&str>,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO organization_hosts \
         (host, organization, organization_id, role, state, created_by, approved_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(host)
    .bind(p.organization)
    .bind(p.organization_id)
    .bind(p.role)
    .bind(state)
    .bind(p.by)
    .bind(approved_by)
    .execute(&mut *tx)
    .await
    .map_err(db)?;
    Ok(())
}

/// Marks `host` removed; `false` when it has no row or is removed already. The row stays as the
/// tombstone that keeps the seed from adding the host again.
pub async fn remove(pool: &PgPool, host: &str) -> Result<bool, sqlx::Error> {
    let mut conn = pool.acquire().await?;
    remove_in(&mut conn, host).await
}

/// [`remove`] inside the caller's transaction.
pub async fn remove_in(conn: &mut sqlx::PgConnection, host: &str) -> Result<bool, sqlx::Error> {
    Ok(sqlx::query(
        "UPDATE organization_hosts SET state = 'removed', updated_at = now() \
         WHERE host = $1 AND state <> 'removed'",
    )
    .bind(host)
    .execute(conn)
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
