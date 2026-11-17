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
/// migration refuses to run while one is unkeyed, then sets NOT NULL and forces row-level security,
/// and takes its tables off this list: migration 0080 (T-7201) forced the 31 tables of PK-1d to
/// PK-1h, from grafana_visualizations to agent_runs.
pub const KEYED_BY_NAME: &[&str] = &[];

/// The column naming the project whose key a row of `table` takes: the project of the Endpoint
/// an access request asks for, whose owners decide it (T-7202); `project` everywhere else.
pub fn key_column(table: &str) -> &'static str {
    match table {
        "access_requests" => "endpoint_project",
        _ => "project",
    }
}

/// Tables of a project's data that stay installation-level, with the reason (T-7202). A
/// person's invitation exists before the person belongs to any project and is read by person
/// alone: its `project` is only the place the invitation offers, nullable.
pub const LEFT_UNKEYED: &[(&str, &str)] = &[(
    "person_invitations",
    "an invitation is the person's, read by person id before they join; its project is optional",
)];

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
/// The Organization's own rows, filed under [`ORG_NAMESPACE`] (people, roles, an Organization
/// draft), take the installation Organization's reserved project (TEN-11; T-7205).
pub async fn key_of(
    pool: &PgPool,
    mirror: &Mirror,
    project: &str,
) -> Result<Option<ProjectKey>, sqlx::Error> {
    if project == ORG_NAMESPACE {
        return match crate::permissions::installation_organization(mirror) {
            Some(organization) => reserved(pool, &organization).await,
            None => Ok(None),
        };
    }
    if mirror.get(ORG_NAMESPACE, "Project", project).is_none() {
        return Ok(None);
    }
    let key = match organization_of(mirror, project) {
        Some(organization) => key(pool, &organization, project).await?,
        None => None,
    };
    if let Some(key) = key {
        crate::request_id::record_key(project, key);
    }
    Ok(key)
}

/// The key of `project` for a write of `what` (T-7200): a project whose Organization is not an
/// active one of the registry keeps none, which the caller answers as 404 naming the
/// Organizations page; a database that does not answer is 503.
pub(crate) async fn required(
    pool: &PgPool,
    mirror: &Mirror,
    project: &str,
    what: &str,
) -> Result<ProjectKey, crate::error::ApiError> {
    key_of(pool, mirror, project)
        .await
        .map_err(|err| {
            tracing::warn!(error = %err, %project, "the project key could not be read");
            crate::error::ApiError::Unavailable(format!("the {what} could not be read or written"))
        })?
        .ok_or_else(|| {
            crate::error::ApiError::NotFound(format!(
                "the project {project} belongs to no active Organization of this installation's \
                 registry, so it keeps no {what}; an operator adds its Organization on the \
                 Organizations page first"
            ))
        })
}

/// The key the registry recorded for `project`, whatever its Organization's state and whether the
/// mirror still holds the Project: what reaches rows already keyed, so an erasure of a project
/// that left the mirror still finishes (T-7202). `None` while the project has no Organization.
pub async fn recorded(pool: &PgPool, project: &str) -> Result<Option<ProjectKey>, sqlx::Error> {
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT o.key::text, p.id::text FROM project_ids p JOIN organizations o ON o.name = p.organization \
         WHERE p.parent IS NULL AND NOT p.reserved AND p.name = $1",
    )
    .bind(project)
    .fetch_optional(pool)
    .await?;
    Ok(row.and_then(|(org, project)| parsed(&org, &project)))
}

/// [`recorded`] with the project's Organization name: what the project MCP door names its server
/// by (`{org}.{ProjectId}.manage`, MCP-33).
pub async fn owned(
    pool: &PgPool,
    project: &str,
) -> Result<Option<(String, ProjectKey)>, sqlx::Error> {
    let row: Option<(String, String, String)> = sqlx::query_as(
        "SELECT p.organization, o.key::text, p.id::text FROM project_ids p \
         JOIN organizations o ON o.name = p.organization \
         WHERE p.parent IS NULL AND NOT p.reserved AND p.name = $1",
    )
    .bind(project)
    .fetch_optional(pool)
    .await?;
    Ok(row.and_then(|(organization, org, project)| {
        parsed(&org, &project).map(|key| (organization, key))
    }))
}

/// Every project the registry recorded a key for, as [`recorded`]: what a pass over every
/// project's rows fans out over, one `db::tenant_tx` each (STO-07).
pub async fn every_recorded(pool: &PgPool) -> Result<Vec<(String, ProjectKey)>, sqlx::Error> {
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT p.name, o.key::text, p.id::text FROM project_ids p JOIN organizations o ON o.name = p.organization \
         WHERE p.parent IS NULL AND NOT p.reserved ORDER BY p.name",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(name, org, project)| parsed(&org, &project).map(|key| (name, key)))
        .collect())
}

/// [`every_recorded`] with each project's Organization name: what a pass that still reads an old
/// name-built layout fans out over (TEN-15).
pub async fn every_recorded_owned(
    pool: &PgPool,
) -> Result<Vec<(String, String, ProjectKey)>, sqlx::Error> {
    let rows: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT p.organization, p.name, o.key::text, p.id::text FROM project_ids p \
         JOIN organizations o ON o.name = p.organization \
         WHERE p.parent IS NULL AND NOT p.reserved ORDER BY p.organization, p.name",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(organization, name, org, project)| {
            parsed(&org, &project).map(|key| (organization, name, key))
        })
        .collect())
}

/// Every branch project (TEN-12, CHG-15) with its Organization, its name `b-{ProjectId}` and when
/// it expires in Unix seconds: what the router's project table places on id addresses beside
/// [`every_recorded_owned`]. An expired branch stays listed; the router refuses it by the time.
pub async fn every_branch_owned(
    pool: &PgPool,
) -> Result<Vec<(String, String, ProjectKey, i64)>, sqlx::Error> {
    let rows: Vec<(String, String, String, String, i64)> = sqlx::query_as(
        "SELECT p.organization, p.name, o.key::text, p.id::text, \
         extract(epoch FROM b.expires_at)::bigint FROM project_branches b \
         JOIN project_ids p ON p.id = b.id JOIN organizations o ON o.name = p.organization \
         WHERE p.parent IS NOT NULL ORDER BY p.organization, p.name",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(organization, name, org, project, expires_at)| {
            parsed(&org, &project).map(|key| (organization, name, key, expires_at))
        })
        .collect())
}

/// A transaction in the key [`recorded`] for `project`, or an unkeyed one while it has none: it
/// reads no row of a forced table, and the NOT NULL key (0080, T-7201) refuses its writes, so a
/// handler answers [`required`]'s 404 before it writes.
pub async fn recorded_tx(
    pool: &PgPool,
    project: &str,
) -> Result<sqlx::Transaction<'static, sqlx::Postgres>, sqlx::Error> {
    match recorded(pool, project).await? {
        Some(key) => crate::db::tenant_tx(pool, key).await,
        None => pool.begin().await,
    }
}

/// A `db::tenant_tx` of `project`'s key as the mirror places it; `None` when it has none.
pub(crate) async fn tx_of(
    pool: &PgPool,
    mirror: &Mirror,
    project: &str,
) -> Result<Option<sqlx::Transaction<'static, sqlx::Postgres>>, sqlx::Error> {
    match key_of(pool, mirror, project).await? {
        Some(key) => Ok(Some(crate::db::tenant_tx(pool, key).await?)),
        None => Ok(None),
    }
}

/// Every Project of the mirror with its key, by name, then [`ORG_NAMESPACE`] with the
/// installation Organization's reserved key: what a read over every project fans out over, one
/// `db::tenant_tx` each (STO-07). A Project whose Organization has no key is left out.
pub async fn keyed_projects(
    pool: &PgPool,
    mirror: &Mirror,
) -> Result<Vec<(String, ProjectKey)>, sqlx::Error> {
    let projects = mirror.list(
        ORG_NAMESPACE,
        "Project",
        &crate::store::ListOptions::default(),
    );
    let mut keyed = Vec::with_capacity(projects.items.len());
    for project in &projects.items {
        let name = project.metadata.name.as_str();
        if name == ORG_NAMESPACE {
            continue;
        }
        if let Some(key) = key_of(pool, mirror, name).await? {
            keyed.push((name.to_owned(), key));
        }
    }
    if let Some(key) = key_of(pool, mirror, ORG_NAMESPACE).await? {
        keyed.push((ORG_NAMESPACE.to_owned(), key));
    }
    Ok(keyed)
}

/// Keys every row of [`KEYED_BY_NAME`] that has none yet with its project's key; answers the
/// project names it could not key (no Project in the mirror, or no active Organization of the
/// registry), sorted, so the caller says which ones wait.
pub async fn backfill(pool: &PgPool, mirror: &Mirror) -> Result<Vec<String>, sqlx::Error> {
    backfill_tables(pool, mirror, KEYED_BY_NAME).await
}

/// [`backfill`] over `tables`, phase-one tables of [`KEYED_BY_NAME`] or of an earlier list: what
/// a migration test keys a database of the release before a forcing migration with.
pub async fn backfill_tables(
    pool: &PgPool,
    mirror: &Mirror,
    tables: &[&str],
) -> Result<Vec<String>, sqlx::Error> {
    let mut waiting = std::collections::BTreeSet::new();
    // The table names are constants of this crate, never input, so the formatted SQL binds the
    // rest.
    for table in tables
        .iter()
        .filter(|t| t.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'))
    {
        let column = key_column(table);
        let names: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT DISTINCT {column} FROM {table} WHERE project_id IS NULL"
        )))
        .fetch_all(pool)
        .await?;
        for name in names {
            let Some(key) = key_of(pool, mirror, &name).await? else {
                waiting.insert(name);
                continue;
            };
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "UPDATE {table} SET org_id = $1::uuid, project_id = $2::uuid \
                 WHERE {column} = $3 AND project_id IS NULL"
            )))
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

/// Gives `project` an Organization of the registry, as tests/common `keyed_project` does, for the
/// crate's own tests of keyed stores.
#[cfg(test)]
pub(crate) async fn placed(pool: &PgPool, mirror: &Mirror, project: &str, organization: &str) {
    mirror.upsert(
        serde_json::from_value(serde_json::json!({
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": "Project",
            "metadata": { "name": project, "namespace": ORG_NAMESPACE },
            "spec": { "organizationRef": organization },
        }))
        .expect("a Project"),
    );
    sqlx::query(
        "INSERT INTO organizations (name, host, state, created_by) \
         VALUES ($1, $1 || '.example', 'active', 'test') ON CONFLICT DO NOTHING",
    )
    .bind(organization)
    .execute(pool)
    .await
    .expect("an Organization of the registry");
}
