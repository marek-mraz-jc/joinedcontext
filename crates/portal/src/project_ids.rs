//! The project id registry (ARCH/TEN-01, TEN-04; T-7190, migration 0066): a live project's
//! [`ProjectId`] is minted the first time [`ensure`] sees its name and read back on every later
//! call, so every carrier can key on (Organization, project id) instead of the name. Minting is
//! idempotent: two replicas asking at once agree on one id, because the live-name index decides.

use std::collections::HashMap;

use jc_types::{OrgId, ProjectId, ProjectKey};
use sqlx::PgPool;

use crate::permissions::{organization_of, ORG_NAMESPACE};
use crate::store::Mirror;

/// The project tables keyed by name until a project's Organization is known (T-7196 PK-1d, phase
/// one): `org_id` and `project_id` are nullable and default to the transaction's key, every
/// statement runs in a `db::tenant_tx`, and [`backfill`] keys the rows written before. A later
/// migration refuses to run while one is unkeyed, then sets NOT NULL and forces row-level security.
pub const KEYED_BY_NAME: &[&str] = &["grafana_visualizations"];

/// The ids of the live projects `names`, minting one for each name the registry has not seen.
/// A branch or copy (a row with a parent) never answers for its parent's name (TEN-12), and an
/// Organization's reserved project never answers for a project spelled like it (TEN-11).
pub async fn ensure(
    pool: &PgPool,
    names: &[String],
) -> Result<HashMap<String, ProjectId>, sqlx::Error> {
    if names.is_empty() {
        return Ok(HashMap::new());
    }
    // The Portal's sqlx speaks no uuid type: ids cross as text and the database casts them.
    let minted: Vec<String> = names
        .iter()
        .map(|_| ProjectId::mint().to_string())
        .collect();
    sqlx::query(
        "INSERT INTO project_ids (id, name) SELECT id::uuid, name FROM unnest($1::text[], $2::text[]) AS m(id, name) \
         ON CONFLICT (name) WHERE parent IS NULL AND NOT reserved DO NOTHING",
    )
    .bind(&minted)
    .bind(names)
    .execute(pool)
    .await?;
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT name, id::text FROM project_ids WHERE parent IS NULL AND NOT reserved AND name = ANY($1)",
    )
    .bind(names)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(name, id)| ProjectId::parse_verified(&id).ok().map(|id| (name, id)))
        .collect())
}

/// The tenancy key of `project` in `organization` (ARCH/TEN-04, T-7191 PK-1a): the Organization's
/// key from the registry and the project's id, minting the id on first sight and recording the
/// Organization the project belongs to the first time it is asked. `None` when the Organization
/// is not an active one of the registry (TEN-05: no default Organization) or the project already
/// belongs to another Organization: a key is never built across Organizations.
pub async fn key(
    pool: &PgPool,
    organization: &str,
    project: &str,
) -> Result<Option<ProjectKey>, sqlx::Error> {
    ensure(pool, &[project.to_owned()]).await?;
    sqlx::query(
        "UPDATE project_ids SET organization = $1 \
         WHERE parent IS NULL AND NOT reserved AND name = $2 AND organization IS NULL \
         AND EXISTS (SELECT 1 FROM organizations WHERE name = $1 AND state = 'active')",
    )
    .bind(organization)
    .bind(project)
    .execute(pool)
    .await?;
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT o.key::text, p.id::text FROM project_ids p JOIN organizations o ON o.name = p.organization \
         WHERE p.parent IS NULL AND NOT p.reserved AND p.name = $2 AND o.name = $1 AND o.state = 'active'",
    )
    .bind(organization)
    .bind(project)
    .fetch_optional(pool)
    .await?;
    Ok(row.and_then(|(org, project)| parsed(&org, &project)))
}

/// The key of `organization`'s reserved project (TEN-11), which holds the Organization's own rows;
/// `None` when the Organization is not an active one of the registry.
pub async fn reserved(
    pool: &PgPool,
    organization: &str,
) -> Result<Option<ProjectKey>, sqlx::Error> {
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT o.key::text, p.id::text FROM project_ids p JOIN organizations o ON o.name = p.organization \
         WHERE p.reserved AND o.name = $1 AND o.state = 'active'",
    )
    .bind(organization)
    .fetch_optional(pool)
    .await?;
    Ok(row.and_then(|(org, project)| parsed(&org, &project)))
}

/// The key of `project` as the mirror places it: its Organization from the Project's
/// `spec.organizationRef` or the installation's (ADR-N-058 section 2.3), then [`key`]. `None` for
/// a name the mirror holds no Project of, so a removed project's leftover rows never mint an id.
pub async fn key_of(
    pool: &PgPool,
    mirror: &Mirror,
    project: &str,
) -> Result<Option<ProjectKey>, sqlx::Error> {
    if mirror.get(ORG_NAMESPACE, "Project", project).is_none() {
        return Ok(None);
    }
    match organization_of(mirror, project) {
        Some(organization) => key(pool, &organization, project).await,
        None => Ok(None),
    }
}

/// Keys every row of [`KEYED_BY_NAME`] that has none yet with its project's key; answers the
/// project names it could not key (no Project in the mirror, or no active Organization of the
/// registry), sorted, so the caller says which ones wait.
pub async fn backfill(pool: &PgPool, mirror: &Mirror) -> Result<Vec<String>, sqlx::Error> {
    let mut waiting = std::collections::BTreeSet::new();
    for table in KEYED_BY_NAME {
        let names: Vec<String> = sqlx::query_scalar(&format!(
            "SELECT DISTINCT project FROM {table} WHERE project_id IS NULL OR org_id IS NULL"
        ))
        .fetch_all(pool)
        .await?;
        for name in names {
            let Some(key) = key_of(pool, mirror, &name).await? else {
                waiting.insert(name);
                continue;
            };
            sqlx::query(&format!(
                "UPDATE {table} SET org_id = $1::uuid, project_id = $2::uuid                  WHERE project = $3 AND (project_id IS NULL OR org_id IS NULL)"
            ))
            .bind(key.org.to_string())
            .bind(key.project.to_string())
            .bind(&name)
            .execute(pool)
            .await?;
        }
    }
    Ok(waiting.into_iter().collect())
}

/// Runs [`backfill`] each minute once the mirror holds a sync (a project's Organization lives
/// only in Git), on the replica that leads; says each change of the projects that wait.
pub(crate) fn spawn_backfill(state: crate::state::AppState) {
    let (Some(pool), Some(syncer)) = (state.db.clone(), state.syncer.clone()) else {
        return;
    };
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut said: Vec<String> = Vec::new();
        loop {
            tick.tick().await;
            if syncer.status().revision.is_none() || !state.leads() {
                continue;
            }
            match backfill(&pool, &state.mirror).await {
                Ok(waiting) if waiting != said => {
                    if !waiting.is_empty() {
                        tracing::warn!(projects = ?waiting, "project rows keep no key: their project is not in the mirror or its Organization is not an active one of the registry; an operator adds the Organization on the Organizations page");
                    }
                    said = waiting;
                }
                Ok(_) => {}
                Err(err) => tracing::warn!(error = %err, "project rows were not keyed this minute"),
            }
        }
    });
}

fn parsed(org: &str, project: &str) -> Option<ProjectKey> {
    Some(ProjectKey::new(
        OrgId::parse_verified(org).ok()?,
        ProjectId::parse_verified(project).ok()?,
    ))
}
