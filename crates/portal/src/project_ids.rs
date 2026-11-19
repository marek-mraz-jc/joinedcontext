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
pub const KEYED_BY_NAME: &[&str] = &[
    // T-7203 PK-1j, migration 0074: an installation's connector event (project '') takes the
    // installation Organization's reserved project key, see [`installation`]. Phase one until
    // their own forcing migration.
    "notifications",
    "app_builds",
];

/// The column naming the project whose key a row of `table` takes: the project of the Endpoint
/// an access request asks for, whose owners decide it (T-7202); `project` everywhere else.
pub fn key_column(table: &str) -> &'static str {
    match table {
        "access_requests" => "endpoint_project",
        "sync_source_state" => "namespace",
        _ => "project",
    }
}

/// Tables of a project's data that stay installation-level, with the reason (T-7202). A
/// person's invitation exists before the person belongs to any project and is read by person
/// alone: its `project` is only the place the invitation offers, nullable.
pub const LEFT_UNKEYED: &[(&str, &str)] = &[
    (
        "person_invitations",
        "an invitation is the person's, read by person id before they join; its project is optional",
    ),
    (
        "project_rows_set_aside",
        "rows 0080 found of a name no Project ever had; no project holds them, an operator reads them",
    ),
];

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

/// The key of the installation Organization's reserved project (TEN-11): what an installation's
/// own rows take, a connector event with project '' (T-7203). `None` while the mirror names no
/// installation Organization or the registry holds no active one by that name.
pub async fn installation(
    pool: &PgPool,
    mirror: &Mirror,
) -> Result<Option<ProjectKey>, sqlx::Error> {
    match crate::permissions::installation_organization(mirror) {
        Some(organization) => reserved(pool, &organization).await,
        None => Ok(None),
    }
}

/// The Organization a person's own rows belong to (connectors, connector calls, preferences;
/// T-7282): the one the request is pinned to (its Organization host), else the installation's
/// from the mirror, else `configured` (`JC_PORTAL_INSTALLATION_ORGANIZATION`, under which
/// [`key_person_rows`] keyed the rows written before). `None` on a deployment host of an
/// installation that names no Organization: such a person keeps no row there.
pub fn person_organization(mirror: &Mirror, configured: Option<&str>) -> Option<String> {
    crate::organization_host::organization()
        .or_else(|| crate::permissions::installation_organization(mirror))
        .or_else(|| configured.map(str::to_owned))
}

/// A `db::tenant_tx` of the reserved project of [`person_organization`] (TEN-11), where a person's
/// own rows live: one person on two Organization hosts keeps two sets. `None` when the request is
/// in no active Organization of the registry: the caller reads nothing and writes nothing.
pub async fn person_tx(
    pool: &PgPool,
    mirror: &Mirror,
    configured: Option<&str>,
) -> Result<Option<sqlx::Transaction<'static, sqlx::Postgres>>, sqlx::Error> {
    let Some(organization) = person_organization(mirror, configured) else {
        return Ok(None);
    };
    match reserved(pool, &organization).await? {
        Some(key) => Ok(Some(crate::db::tenant_tx(pool, key).await?)),
        None => Ok(None),
    }
}

/// What a person's write answers on a host that is in no active Organization ([`person_tx`]
/// `None`): 409 naming the next step.
pub fn person_unplaced(what: &str) -> crate::error::ApiError {
    crate::error::ApiError::Conflict(format!(
        "this address belongs to no active Organization, so it keeps no {what}; open the Portal \
         on your Organization's own address, or an operator sets the installation Organization"
    ))
}

/// The tables migration 0088 forces with a person's Organization key (T-7282), in the order
/// [`key_person_rows`] keys them: a call takes its connector's key.
const PERSON_TABLES: &[&str] = &["connectors", "user_preferences"];

/// Keys every unkeyed row of a person's tables under `organization`'s reserved project, after
/// migration 0087 adds the key and before 0088 forces it ([`crate::db::connect_with`], T-7282):
/// before 0087 a person's rows had no Organization, and the installation's is the one the
/// deployment host shows them under. A call takes its connector's key. Answers the rows keyed.
pub async fn key_person_rows(
    pool: &PgPool,
    organization: &crate::host_registry::SeedOrganization,
) -> Result<u64, sqlx::Error> {
    crate::host_registry::seed_organizations(pool, std::slice::from_ref(organization)).await?;
    let Some(key) = reserved(pool, &organization.name).await? else {
        return Ok(0);
    };
    let mut keyed = 0;
    for table in PERSON_TABLES {
        keyed += sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE {table} SET org_id = $1::uuid, project_id = $2::uuid \
             WHERE org_id IS NULL OR project_id IS NULL"
        )))
        .bind(key.org.to_string())
        .bind(key.project.to_string())
        .execute(pool)
        .await?
        .rows_affected();
    }
    keyed += sqlx::query(
        "UPDATE connector_calls k SET org_id = c.org_id, project_id = c.project_id \
         FROM connectors c WHERE c.id = k.connector AND (k.org_id IS NULL OR k.project_id IS NULL)",
    )
    .execute(pool)
    .await?
    .rows_affected();
    Ok(keyed)
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
    // A name never resolves to a branch (RTE-05, TEN-12): only the request its own id address
    // placed reaches it, so a branch named in a body or a tool argument answers like an unknown
    // project. On an id address every pair present agrees: any other project's name gets no key.
    let placed = crate::request_id::placed_id();
    if jc_router::branch_name(project) {
        return match placed {
            Some(placed) if project.strip_prefix("b-") == Some(placed.as_str()) => {
                live_branch(pool, project).await
            }
            _ => Ok(None),
        };
    }
    if mirror.get(ORG_NAMESPACE, "Project", project).is_none() {
        return Ok(None);
    }
    let key = match organization_of(mirror, project) {
        Some(organization) => key(pool, &organization, project).await?,
        None => None,
    };
    let key = key.filter(|key| {
        placed
            .as_deref()
            .is_none_or(|placed| key.project.path_segment() == placed)
    });
    if let Some(key) = key {
        crate::request_id::record_key(project, key);
    }
    Ok(key)
}

/// The key of the live branch named `name` (TEN-12, CHG-15) whose Organization is an active one of
/// the registry; `None` once it expired, so a tombstone answers like an unknown project.
async fn live_branch(pool: &PgPool, name: &str) -> Result<Option<ProjectKey>, sqlx::Error> {
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT o.key::text, p.id::text FROM project_ids p JOIN project_branches b ON b.id = p.id \
         JOIN organizations o ON o.name = p.organization \
         WHERE p.parent IS NOT NULL AND p.name = $1 AND b.expires_at > now() AND o.state = 'active'",
    )
    .bind(name)
    .fetch_optional(pool)
    .await?;
    Ok(row.and_then(|(org, project)| parsed(&org, &project)))
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
/// [`every_recorded_owned`]. An expired branch stays listed until it is swept; the router refuses
/// it by the time, and a missing one alike.
pub async fn every_branch_owned(
    pool: &PgPool,
) -> Result<Vec<(String, String, ProjectKey, i64)>, sqlx::Error> {
    let rows: Vec<(String, String, String, String, i64)> = sqlx::query_as(
        "SELECT p.organization, p.name, o.key::text, p.id::text, \
         extract(epoch FROM b.expires_at)::bigint FROM project_branches b \
         JOIN project_ids p ON p.id = b.id JOIN organizations o ON o.name = p.organization \
         WHERE p.parent IS NOT NULL AND b.swept_at IS NULL ORDER BY p.organization, p.name",
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

/// A branch project as the registry holds it (TEN-12, CHG-15): its id, its name `b-{id}` (path
/// form), who opened it and when it stops answering, in Unix seconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchRow {
    pub id: ProjectId,
    pub name: String,
    pub created_by: String,
    pub expires_at: i64,
}

/// Opens a branch of the live project `parent` for `created_by`, living `lifetime_days` (TEN-12,
/// CHG-15): its `project_ids` row (name `b-{id}`, the parent, the parent's Organization) and its
/// `project_branches` row in one transaction, the parent's row locked so two requests count the
/// same live branches; [`NoBranch`] says why none was opened.
pub async fn open_branch(
    pool: &PgPool,
    parent: ProjectKey,
    created_by: &str,
    lifetime_days: u32,
    per_parent: u32,
) -> Result<Result<BranchRow, NoBranch>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let organization: Option<(String, String)> = sqlx::query_as(
        "SELECT organization, name FROM project_ids WHERE id = $1::uuid AND parent IS NULL \
         AND NOT reserved AND organization IS NOT NULL FOR UPDATE",
    )
    .bind(parent.project.to_string())
    .fetch_optional(&mut *tx)
    .await?;
    let Some((organization, parent_name)) = organization else {
        return Ok(Err(NoBranch::NotAParent));
    };
    let (live,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM project_ids p JOIN project_branches b ON b.id = p.id \
         WHERE p.parent = $1::uuid AND b.expires_at > now()",
    )
    .bind(parent.project.to_string())
    .fetch_one(&mut *tx)
    .await?;
    if live >= i64::from(per_parent) {
        return Ok(Err(NoBranch::AtLimit));
    }
    let id = ProjectId::mint();
    let name = format!("b-{}", id.path_segment());
    sqlx::query(
        "INSERT INTO project_ids (id, name, parent, organization) VALUES ($1::uuid, $2, $3::uuid, $4)",
    )
    .bind(id.to_string())
    .bind(&name)
    .bind(parent.project.to_string())
    .bind(&organization)
    .execute(&mut *tx)
    .await?;
    let (expires_at,): (i64,) = sqlx::query_as(
        "INSERT INTO project_branches (id, created_by, expires_at) \
         VALUES ($1::uuid, $2, now() + make_interval(days => $3)) \
         RETURNING extract(epoch FROM expires_at)::bigint",
    )
    .bind(id.to_string())
    .bind(created_by)
    .bind(i32::try_from(lifetime_days).unwrap_or(7))
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    note_branch(
        &name,
        BranchEntry {
            parent: parent_name,
            created_by: created_by.to_owned(),
            expires_at,
        },
    );
    Ok(Ok(BranchRow {
        id,
        name,
        created_by: created_by.to_owned(),
        expires_at,
    }))
}

/// Why [`open_branch`] opened no branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoBranch {
    /// The parent is no live project of an Organization: a reserved project, a branch or a copy.
    NotAParent,
    /// The parent already keeps as many live branches as the deployment allows.
    AtLimit,
}

/// The live branch `id` of `parent`; `None` once it expired or when it is another project's.
pub async fn live_branch_of(
    pool: &PgPool,
    parent: ProjectKey,
    id: ProjectId,
) -> Result<Option<BranchRow>, sqlx::Error> {
    let row: Option<(String, String, i64)> = sqlx::query_as(
        "SELECT p.name, b.created_by, extract(epoch FROM b.expires_at)::bigint \
         FROM project_ids p JOIN project_branches b ON b.id = p.id \
         WHERE p.id = $1::uuid AND p.parent = $2::uuid AND b.expires_at > now()",
    )
    .bind(id.to_string())
    .bind(parent.project.to_string())
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(name, created_by, expires_at)| BranchRow {
        id,
        name,
        created_by,
        expires_at,
    }))
}

/// Renews the live branch `id` of `parent` by 7 days, never past 30 days from its creation
/// (CHG-15), with the audit row of `renewed_by` in the same statement. `None` when the branch
/// is not a live one of `parent` or already lives to its 30 days.
pub async fn renew_branch(
    pool: &PgPool,
    parent: ProjectKey,
    id: ProjectId,
    renewed_by: &str,
) -> Result<Option<i64>, sqlx::Error> {
    let renewed: Option<(i64,)> = sqlx::query_as(
        "WITH renewed AS (UPDATE project_branches b \
         SET expires_at = least(b.expires_at + interval '7 days', b.created_at + interval '30 days') \
         FROM project_ids p WHERE p.id = b.id AND b.id = $1::uuid AND p.parent = $2::uuid \
         AND b.expires_at > now() AND b.expires_at < b.created_at + interval '30 days' \
         RETURNING b.id, b.expires_at), \
         audit AS (INSERT INTO project_branch_renewals (branch, renewed_by, expires_at) \
         SELECT id, $3, expires_at FROM renewed) \
         SELECT extract(epoch FROM expires_at)::bigint FROM renewed",
    )
    .bind(id.to_string())
    .bind(parent.project.to_string())
    .bind(renewed_by)
    .fetch_optional(pool)
    .await?;
    let expires_at = renewed.map(|(expires_at,)| expires_at);
    if let Some(expires_at) = expires_at {
        let name = format!("b-{}", id.path_segment());
        if let Some(entry) = branch_entry(&name) {
            note_branch(
                &name,
                BranchEntry {
                    expires_at,
                    ..entry
                },
            );
        }
    }
    Ok(expires_at)
}

/// A branch as the Portal's rights and mirror read it (TEN-12, CHG-15): its parent's name, who
/// opened it (a token's `sub`) and when it stops answering, in Unix seconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchEntry {
    pub parent: String,
    pub created_by: String,
    pub expires_at: i64,
}

/// What this Portal knows of a `b-{id}` name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchState {
    /// The branch table has not loaded yet (a Portal without a database, or before startup
    /// read it): nothing is decided by it.
    Unread,
    Live(BranchEntry),
    /// Expired, or no branch of the registry: it answers like an unknown project.
    Gone,
}

/// Every branch of the registry by name, read at startup and every [`BRANCH_WATCH`] after, and
/// written at once by [`open_branch`] and [`renew_branch`]: the permissions and the mirror are
/// synchronous, so they read this table rather than the database.
static BRANCHES: std::sync::RwLock<Option<HashMap<String, BranchEntry>>> =
    std::sync::RwLock::new(None);

/// How often the branch table is read again and expired branches swept.
pub const BRANCH_WATCH: std::time::Duration = std::time::Duration::from_secs(30);

fn note_branch(name: &str, entry: BranchEntry) {
    let mut table = BRANCHES.write().unwrap_or_else(|p| p.into_inner());
    table
        .get_or_insert_with(HashMap::new)
        .insert(name.to_owned(), entry);
}

fn branch_entry(name: &str) -> Option<BranchEntry> {
    BRANCHES
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .and_then(|table| table.get(name).cloned())
}

/// The state of the branch `name` now.
pub fn branch_state(name: &str) -> BranchState {
    let table = BRANCHES.read().unwrap_or_else(|p| p.into_inner());
    let Some(table) = table.as_ref() else {
        return BranchState::Unread;
    };
    match table.get(name) {
        Some(entry) if entry.expires_at > crate::auth::session::now_unix() => {
            BranchState::Live(entry.clone())
        }
        _ => BranchState::Gone,
    }
}

/// Reads every branch of the registry into the table.
pub async fn read_branches(pool: &PgPool) -> Result<(), sqlx::Error> {
    let rows: Vec<(String, String, String, i64)> = sqlx::query_as(
        "SELECT p.name, parent.name, b.created_by, extract(epoch FROM b.expires_at)::bigint \
         FROM project_branches b JOIN project_ids p ON p.id = b.id \
         JOIN project_ids parent ON parent.id = p.parent WHERE b.swept_at IS NULL",
    )
    .fetch_all(pool)
    .await?;
    let table = rows
        .into_iter()
        .map(|(name, parent, created_by, expires_at)| {
            (
                name,
                BranchEntry {
                    parent,
                    created_by,
                    expires_at,
                },
            )
        })
        .collect();
    *BRANCHES.write().unwrap_or_else(|p| p.into_inner()) = Some(table);
    Ok(())
}

/// The expiry job (CHG-15): every branch expired more than an hour ago and not swept yet loses
/// (the reconciler's runs in that hour revoke its clients first: an expired branch's manifests
/// leave the mirror at once, so their clients are removed and their tokens ended)
/// its rows in each Portal table of a project's data, under its own key and by that table's own
/// filter, and is marked swept once every delete committed; a branch whose delete failed is
/// tried again at the next pass. Its `project_ids` and `project_branches` rows stay as the
/// tombstone. Returns the branches swept.
pub async fn sweep_expired_branches(pool: &PgPool) -> Result<Vec<String>, sqlx::Error> {
    let due: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT p.name, o.key::text, p.id::text FROM project_branches b \
         JOIN project_ids p ON p.id = b.id JOIN organizations o ON o.name = p.organization \
         WHERE b.swept_at IS NULL AND b.expires_at < now() - interval '1 hour'",
    )
    .fetch_all(pool)
    .await?;
    let mut swept = Vec::new();
    for (name, org, project) in due {
        let Some(key) = parsed(&org, &project) else {
            continue;
        };
        let mut complete = true;
        for table in crate::org_backup::data::PORTAL_TABLES {
            // The branch's own `project_ids` row is the tombstone that stays.
            if table.keys != "projects" || table.name == "project_ids" {
                continue;
            }
            let filter = match key_column(table.name) {
                "project" => table.filter.to_owned(),
                column => format!("{column} = ANY($1)"),
            };
            let mut tx = crate::db::tenant_tx(pool, key).await?;
            let statement = format!("DELETE FROM {} WHERE {filter}", table.name);
            // The table name and its filter are PORTAL_TABLES constants; the branch name is bound.
            match sqlx::query(sqlx::AssertSqlSafe(statement))
                .bind(std::slice::from_ref(&name))
                .execute(&mut *tx)
                .await
            {
                Ok(_) => tx.commit().await?,
                Err(err) => {
                    tracing::warn!(branch = %name, table = table.name, error = %err, "a branch's rows could not be deleted; the next pass tries again");
                    tx.rollback().await?;
                    complete = false;
                }
            }
        }
        if !complete {
            continue;
        }
        sqlx::query("UPDATE project_branches SET swept_at = now() WHERE id = $1::uuid")
            .bind(&project)
            .execute(pool)
            .await?;
        tracing::info!(branch = %name, "an expired branch's data was deleted; its id stays a tombstone");
        swept.push(name);
    }
    Ok(swept)
}

/// Reads the branch table once, then keeps it fresh and sweeps expired branches every
/// [`BRANCH_WATCH`] in the background.
pub async fn watch_branches(pool: PgPool) {
    if let Err(err) = read_branches(&pool).await {
        tracing::warn!(error = %err, "the branch table could not be read; branches wait for the next read");
    }
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(BRANCH_WATCH);
        tick.tick().await;
        loop {
            tick.tick().await;
            if let Err(err) = read_branches(&pool).await {
                tracing::warn!(error = %err, "the branch table could not be read");
            }
            if let Err(err) = sweep_expired_branches(&pool).await {
                tracing::warn!(error = %err, "expired branches could not be swept");
            }
        }
    });
}

/// What the mirror does with a manifest of a branch's namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchManifest {
    /// Taken as any project's.
    Kept,
    /// Kept in the refused set (CHG-05): shown to the branch's readers, rendered nowhere.
    Refused,
    /// Not taken at all: the reconciler then revokes the branch's clients and removes its objects.
    Dropped,
}

/// The mirror's rule for `env` in a branch whose state is `state` (TEN-12, CHG-15): an expired or
/// unknown branch keeps nothing; anything that would reach beyond its Organization or the public
/// is refused whether the branch lives or the table is not read yet (unknown means refused, so
/// sharing never fails open); the rest is kept while the branch lives or the table is unread.
pub fn branch_manifest(
    state: &BranchState,
    env: &crate::resource::ResourceEnvelope,
) -> BranchManifest {
    match state {
        BranchState::Gone => BranchManifest::Dropped,
        _ if shares_beyond_branch(env) => BranchManifest::Refused,
        _ => BranchManifest::Kept,
    }
}

/// Whether `env`, a manifest of a branch, would reach beyond its Organization or the public
/// (TEN-12): a public Endpoint, an Endpoint with a catalogue record, a shared space, a context
/// source registration or a data space object. A branch renders none of them; turning one on is
/// a new approval in the parent.
pub fn shares_beyond_branch(env: &crate::resource::ResourceEnvelope) -> bool {
    match env.kind.as_str() {
        "SharedSpaceReference"
        | "ContextSourceRegistration"
        | "DataSpaceParticipant"
        | "DataOffer"
        | "DataAgreement" => true,
        "Endpoint" => {
            env.spec.get("audience").and_then(serde_json::Value::as_str) == Some("public")
                || env.spec.get("catalog").is_some_and(|c| !c.is_null())
        }
        _ => false,
    }
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
            let key = if name.is_empty() && *table == "notifications" {
                installation(pool, mirror).await?
            } else {
                key_of(pool, mirror, &name).await?
            };
            let Some(key) = key else {
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

/// The tables migration 0080 forces (T-7201), in its order: what [`key_under_installation`] keys.
const FORCED_BY_0080: &[&str] = &[
    "grafana_visualizations",
    "alert_subscriptions",
    "alert_incidents",
    "alert_notices",
    "health_check",
    "health_check_history",
    "space_quality",
    "endpoint_usage_daily",
    "link_check",
    "app_service_usage",
    "email_unsubscribes",
    "mqtt_capture",
    "entity_trash",
    "entity_comments",
    "data_views",
    "pipeline_log",
    "pipeline_runs",
    "pipeline_rejected",
    "pipeline_status",
    "pipeline_templates",
    "pipeline_record_hashes",
    "pipeline_source_ledger",
    "pipeline_source_holds",
    "pipeline_source_complete",
    "pipeline_source_removed",
    "erasures",
    "access_requests",
    "drafts",
    "activity",
    "workspaces",
    "agent_runs",
];

/// Keys every unkeyed row of the tables 0080 forces under `installation`, on a database from
/// before the project registry, after 0079 and before 0080 ([`crate::db::connect_with`], T-7264):
/// the Organization is seeded as jc seeds it, each named project gets its id and `installation`
/// as its Organization ([`key`]), and the Organization's own rows ([`ORG_NAMESPACE`]) its reserved
/// project. The Organization comes from the configuration alone, never from a project's name, so
/// a project named like another Organization stays the installation's. A row without a name, of
/// a branch, or of a project already recorded in another Organization is left for 0080 to set
/// aside or refuse. Answers the rows keyed.
pub async fn key_under_installation(
    pool: &PgPool,
    installation: &crate::host_registry::SeedOrganization,
) -> Result<u64, sqlx::Error> {
    crate::host_registry::seed_organizations(pool, std::slice::from_ref(installation)).await?;
    let organization = installation.name.as_str();
    let own = reserved(pool, organization).await?;
    let mut keyed = 0;
    for table in FORCED_BY_0080 {
        let column = key_column(table);
        let names: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT DISTINCT {column} FROM {table} \
             WHERE (org_id IS NULL OR project_id IS NULL) AND {column} <> ''"
        )))
        .fetch_all(pool)
        .await?;
        for name in names {
            let key = if name == ORG_NAMESPACE {
                own
            } else if jc_router::branch_name(&name) {
                None
            } else {
                key(pool, organization, &name).await?
            };
            let Some(key) = key else { continue };
            keyed += sqlx::query(sqlx::AssertSqlSafe(format!(
                "UPDATE {table} SET org_id = $1::uuid, project_id = $2::uuid \
                 WHERE {column} = $3 AND (org_id IS NULL OR project_id IS NULL)"
            )))
            .bind(key.org.to_string())
            .bind(key.project.to_string())
            .bind(&name)
            .execute(pool)
            .await?
            .rows_affected();
        }
    }
    Ok(keyed)
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

#[cfg(test)]
mod branch_manifest_tests {
    use super::*;
    use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};

    fn manifest(kind: &str, spec: serde_json::Value) -> ResourceEnvelope {
        let namespace = format!("b-{}", ProjectId::mint().path_segment());
        ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: kind.to_owned(),
            metadata: ObjectMeta::new("x", &namespace),
            spec,
            status: None,
        }
    }

    /// T-7215 bounce: before the branch table is read, a branch's public Endpoint is refused as
    /// while it lives, never rendered; its other manifests wait as they are.
    #[test]
    fn an_unread_branch_table_refuses_what_a_live_branch_refuses() {
        let public = manifest("Endpoint", serde_json::json!({ "audience": "public" }));
        let inside = manifest(
            "Endpoint",
            serde_json::json!({ "audience": "organization" }),
        );
        let live = BranchState::Live(BranchEntry {
            parent: "p".into(),
            created_by: "s".into(),
            expires_at: i64::MAX,
        });
        for state in [BranchState::Unread, live] {
            assert_eq!(
                branch_manifest(&state, &public),
                BranchManifest::Refused,
                "{state:?}"
            );
            assert_eq!(
                branch_manifest(&state, &inside),
                BranchManifest::Kept,
                "{state:?}"
            );
        }
        assert_eq!(
            branch_manifest(&BranchState::Gone, &inside),
            BranchManifest::Dropped
        );

        // Through the mirror, before any read of the table in this process.
        let mirror = Mirror::new();
        let namespace = public.metadata.namespace.clone().unwrap_or_default();
        assert_eq!(branch_state(&namespace), BranchState::Unread);
        mirror.upsert(public);
        assert!(mirror.get(&namespace, "Endpoint", "x").is_none());
        assert!(mirror.refused(&namespace, "Endpoint", "x").is_some());
    }
}
