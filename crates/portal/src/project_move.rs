//! Moving a project to another Organization (T-7305): what the operators' `ProjectMove` Change
//! plans and applies. The project keeps its id; its registry row and every row keyed by
//! (old Organization, project id) take the target Organization's key in one transaction, through
//! migration 0089's `project_move`. The Organization a request places the project in comes
//! from its Project manifest (`spec.organizationRef`, in Git), so the move applies only once the
//! manifest names the target: before that the registry would key the project under an
//! Organization the mirror does not place it in, and every keyed read would answer 404.
//! The rights follow the manifest: an Organization-scoped binding of the old Organization no
//! longer reaches the project, and the project's own bindings move with it.

use serde::Serialize;
use sqlx::PgPool;
use utoipa::ToSchema;

use crate::permissions::{installation_organization, organization_of, ORG_NAMESPACE};
use crate::store::{ListOptions, Mirror};

tokio::task_local! {
    /// The domains of the Organizations a proposed `ProjectMove` takes the judged project to: a
    /// policyRef on one of them binds once the project moves, so an Endpoint Change may list it
    /// beside the references that bind now (T-7462). The Endpoint door and a Change's approval
    /// set it around their judge ([`moving_domains`]).
    pub static MOVING: Vec<String>;
}

/// The rows of one keyed table that move.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct TableRows {
    pub table: String,
    pub rows: u64,
}

/// The approved `ProjectMove` Change a move applies: its id, the operator approving it and the
/// address the approval came from, for the audit row `project_move` writes.
pub struct Approval<'a> {
    pub id: &'a str,
    pub approver: &'a str,
    pub source_ip: Option<&'a str>,
}

/// What a `ProjectMove` of `project` to `to` would do now, as the operators read it before they
/// approve.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MovePlan {
    pub project: String,
    /// The Organization the registry files the project under now; `None` before it has one.
    pub from: Option<String>,
    pub to: String,
    /// The Organization the project's manifest names (or the installation's when it names none).
    pub manifest_organization: Option<String>,
    /// Rows keyed by the project under `from`, per table; only tables holding any are listed.
    pub tables: Vec<TableRows>,
    pub rows: u64,
    /// The project's own `RoleBinding`s, which move with it.
    pub bindings: Vec<String>,
    /// The project sits under `to` already: approving changes nothing.
    pub done: bool,
    /// Why the move cannot be asked for now, each naming its next step; empty when it can.
    pub blockers: Vec<String>,
    /// What must finish before a second operator approves, each naming its next step: the
    /// manifest naming `to`, the project's open Changes merged or closed (T-7462). A proposal may
    /// come first, and it is what lets the Project Change name `to` ([`check_manifest`]).
    pub waits: Vec<String>,
}

/// The registry's record of a live project: its id and the Organization's name and key.
struct Recorded {
    id: String,
    organization: Option<String>,
    key: Option<String>,
}

async fn recorded(pool: &PgPool, project: &str) -> Result<Option<Recorded>, sqlx::Error> {
    let row: Option<(String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT p.id::text, p.organization, o.key::text FROM project_ids p \
         LEFT JOIN organizations o ON o.name = p.organization \
         WHERE p.parent IS NULL AND NOT p.reserved AND p.name = $1",
    )
    .bind(project)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(id, organization, key)| Recorded {
        id,
        organization,
        key,
    }))
}

/// The key of an active Organization of the registry.
async fn active_key(
    conn: impl sqlx::PgExecutor<'_>,
    organization: &str,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT key::text FROM organizations WHERE name = $1 AND state = 'active'")
        .bind(organization)
        .fetch_optional(conn)
        .await
}

/// Every table of the schema keyed by (org_id, project_id), from the catalog, so a table a later
/// migration keys moves without a list to keep.
async fn keyed_tables(conn: impl sqlx::PgExecutor<'_>) -> Result<Vec<String>, sqlx::Error> {
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT c.table_name::text FROM information_schema.columns c \
         JOIN information_schema.tables t ON t.table_schema = c.table_schema \
              AND t.table_name = c.table_name AND t.table_type = 'BASE TABLE' \
         WHERE c.table_schema = current_schema() AND c.column_name IN ('org_id', 'project_id') \
         GROUP BY c.table_name HAVING count(*) = 2 ORDER BY 1",
    )
    .fetch_all(conn)
    .await?;
    // Catalog names, quoted below all the same; anything but a plain name is left alone.
    Ok(tables
        .into_iter()
        .filter(|t| t.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'))
        .collect())
}

/// The rows each keyed table holds under (org, project), inside a transaction keyed with that pair.
async fn counts(
    tx: &mut sqlx::PgConnection,
    org: &str,
    project: &str,
) -> Result<Vec<TableRows>, sqlx::Error> {
    let mut found = Vec::new();
    for table in keyed_tables(&mut *tx).await? {
        let rows: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT count(*) FROM \"{table}\" WHERE org_id = $1::uuid AND project_id = $2::uuid"
        )))
        .bind(org)
        .bind(project)
        .fetch_one(&mut *tx)
        .await?;
        if rows > 0 {
            found.push(TableRows {
                table,
                rows: rows.unsigned_abs(),
            });
        }
    }
    Ok(found)
}

async fn keyed(
    pool: &PgPool,
    org: &str,
    project: &str,
) -> Result<sqlx::Transaction<'static, sqlx::Postgres>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "SELECT set_config('statement_timeout', $3, true), \
         set_config('jc.org', $1, true), set_config('jc.project', $2, true)",
    )
    .bind(org)
    .bind(project)
    .bind(crate::db::BULK_STATEMENT_TIMEOUT)
    .execute(&mut *tx)
    .await?;
    Ok(tx)
}

/// The project's own `RoleBinding`s in the mirror, by name.
fn bindings(mirror: &Mirror, project: &str) -> Vec<String> {
    mirror
        .list(ORG_NAMESPACE, "RoleBinding", &ListOptions::default())
        .items
        .into_iter()
        .filter(|b| b.spec["scope"]["project"].as_str() == Some(project))
        .map(|b| b.metadata.name)
        .collect()
}

/// The domain an Organization manifest of the mirror declares.
fn domain_of(mirror: &Mirror, kind: &str, name: &str) -> Option<String> {
    mirror.get(ORG_NAMESPACE, kind, name).and_then(|env| {
        env.spec["domain"]
            .as_str()
            .filter(|domain| !domain.is_empty())
            .map(str::to_owned)
    })
}

/// The policyRefs of `project`'s Endpoints that bind one of its Policies now and would bind none
/// once the project publishes under Organization `to` (T-7462): the gateway binds a Policy URN on
/// the domains of the project and of the Organization its manifest names, so a move from
/// `hel.fi` to `ctl-b.joinedcontext.com` leaves `urn:ngsi-ld:Policy:hel.fi:...` granting nothing
/// at the next load. Each line names the Endpoint and the URN to list beside the old one, which
/// keeps the Endpoint granting before and after the manifest names `to`. A reference binds now on
/// the project's domains as its manifest stands and on the domain of `from`, the Organization the
/// registry files it under, so a manifest that names `to` already still shows what the move
/// leaves behind.
pub fn stray_policy_refs(
    mirror: &Mirror,
    project: &str,
    from: Option<&str>,
    to: &str,
) -> Vec<String> {
    use jc_core::kinds::EndpointSpec;
    let mut now = crate::permissions::project_domains(mirror, project);
    now.extend(from.and_then(|from| domain_of(mirror, "Organization", from)));
    let target = domain_of(mirror, "Organization", to);
    let then: std::collections::BTreeSet<String> = domain_of(mirror, "Project", project)
        .into_iter()
        .chain(target.clone())
        .collect();
    let on = |domains: &std::collections::BTreeSet<String>, reference: &jc_core::Urn| {
        reference
            .org_domain()
            .is_some_and(|domain| domains.contains(domain))
    };
    let mut lines = Vec::new();
    for endpoint in mirror
        .list(project, "Endpoint", &ListOptions::default())
        .items
    {
        let Ok(spec) = serde_json::from_value::<EndpointSpec>(endpoint.spec) else {
            continue;
        };
        let Some(refs) = spec.policy_refs() else {
            continue;
        };
        let field = match spec.policy_ref {
            Some(_) => "spec.policyRef",
            None => "spec.policyRefs",
        };
        for reference in refs {
            // Without a domain of its own the project binds on the installation's: counted as
            // binding now, so nothing it grants today is missed.
            let binds_now = now.is_empty() || on(&now, reference);
            let kept = refs.iter().any(|other| {
                other.space() == reference.space()
                    && other.local_id() == reference.local_id()
                    && on(&then, other)
            });
            if !binds_now
                || kept
                || mirror
                    .get(project, "Policy", reference.local_id())
                    .is_none()
            {
                continue;
            }
            let name = &endpoint.metadata.name;
            lines.push(match &target {
                Some(domain) => format!(
                    "{field}: Endpoint '{name}' of project '{project}' names {reference}, which binds nothing under Organization '{to}' (domain {domain}); with the move asked for, list urn:ngsi-ld:Policy:{domain}:{}:{} beside it (an Endpoint Change), then name '{to}' in the Project",
                    reference.space().unwrap_or_default(),
                    reference.local_id()
                ),
                None => format!(
                    "{field}: Endpoint '{name}' of project '{project}' names {reference}, and Organization '{to}' declares no domain, so no policyRef binds under it; give the Organization its domain first"
                ),
            });
        }
    }
    lines
}

/// The domains of the Organizations a proposed `ProjectMove` takes `project` to, for [`MOVING`];
/// empty without a database or a proposal.
pub async fn moving_domains(
    state: &crate::state::AppState,
    project: &str,
) -> Result<Vec<String>, crate::error::ApiError> {
    let Some(pool) = state.db.as_ref() else {
        return Ok(Vec::new());
    };
    let targets: Vec<String> = sqlx::query_scalar(
        "SELECT payload->>'organization' FROM operator_changes WHERE kind = 'ProjectMove' \
         AND state = 'proposed' AND payload->>'project' = $1",
    )
    .bind(project)
    .fetch_all(pool)
    .await
    .map_err(|err| {
        tracing::warn!(error = %err, %project, "the operator changes could not be read");
        crate::error::ApiError::Unavailable(
            "the operator changes could not be read; try again".into(),
        )
    })?;
    Ok(targets
        .iter()
        .filter_map(|to| domain_of(&state.mirror, "Organization", to))
        .collect())
}

/// What moving `project` to `to` does now. `None` when the registry holds no live project of
/// that name.
pub async fn plan(
    pool: &PgPool,
    mirror: &Mirror,
    project: &str,
    to: &str,
) -> Result<Option<MovePlan>, sqlx::Error> {
    let Some(found) = recorded(pool, project).await? else {
        return Ok(None);
    };
    let manifest_organization = organization_of(mirror, project);
    let mut blockers = Vec::new();
    if active_key(pool, to).await?.is_none() {
        blockers.push(format!(
            "organization: '{to}' is no active Organization of the registry; an operator adds it on the Organizations page first"
        ));
    }
    if found.organization.is_none() {
        blockers.push(format!(
            "project: '{project}' has no Organization in the registry yet; it takes the one its manifest names at its first keyed write, no move needed"
        ));
    }
    if mirror.get(ORG_NAMESPACE, "Project", project).is_none() {
        blockers.push(format!(
            "project: the Portal's configuration holds no Project '{project}'"
        ));
    }
    let mut waits = Vec::new();
    if mirror.get(ORG_NAMESPACE, "Project", project).is_some()
        && manifest_organization.as_deref() != Some(to)
    {
        waits.push(format!(
            "organization: Project '{project}' names Organization '{}' in its manifest; with this move asked for, set its spec.organizationRef to '{to}' (a Project Change) and approve the move once it is synced",
            manifest_organization.as_deref().unwrap_or("none")
        ));
    }
    // The Project Change naming `to` waits for these, and so does the approval.
    waits.extend(stray_policy_refs(
        mirror,
        project,
        found.organization.as_deref(),
        to,
    ));
    let branches: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM project_ids p JOIN project_branches b ON b.id = p.id \
         WHERE p.parent = $1::uuid AND b.expires_at > now()",
    )
    .bind(&found.id)
    .fetch_one(pool)
    .await?;
    if branches > 0 {
        blockers.push(format!(
            "project: '{project}' has {branches} live branch(es); merge or close them, or wait until they expire"
        ));
    }
    let done = found.organization.as_deref() == Some(to);
    let tables = match (&found.key, done) {
        (Some(org), false) => {
            let mut tx = keyed(pool, org, &found.id).await?;
            let tables = counts(&mut tx, org, &found.id).await?;
            tx.rollback().await?;
            tables
        }
        _ => Vec::new(),
    };
    Ok(Some(MovePlan {
        project: project.to_owned(),
        from: found.organization,
        to: to.to_owned(),
        manifest_organization,
        rows: tables.iter().map(|t| t.rows).sum(),
        tables,
        bindings: bindings(mirror, project),
        done,
        blockers,
        waits,
    }))
}

/// The registry's name of the Organization a manifest names: the name itself when the registry
/// holds it active, else the configured installation Organization when the name is the
/// repository's installation claim (jc-prod's forge says `hel`, its registry `smartcity`; as
/// `project_ids::key_of` resolves it, T-7414).
async fn registry_name(pool: &PgPool, mirror: &Mirror, named: &str) -> Result<String, sqlx::Error> {
    if active_key(pool, named).await?.is_some() {
        return Ok(named.to_owned());
    }
    Ok(match mirror.configured_installation() {
        Some(configured) if installation_organization(mirror).as_deref() == Some(named) => {
            configured.to_owned()
        }
        _ => named.to_owned(),
    })
}

/// Refuses a Project manifest whose `spec.organizationRef` places a project the registry files
/// under another Organization (T-7462), unless an operator asked to move it there: a
/// `ProjectMove` Change of `project` to that Organization is proposed. Without it the mirror
/// would place the project where its keyed rows are not, and every keyed read would answer 404;
/// a manifest naming the Organization the project moved away from (or the installation claim
/// after a move off it) is refused the same way. A project the registry files under none yet
/// takes the one its manifest names at its first keyed write.
pub async fn check_manifest(
    pool: &PgPool,
    mirror: &Mirror,
    project: &str,
    spec: &serde_json::Value,
) -> Result<Result<(), String>, sqlx::Error> {
    let Some(Recorded {
        organization: Some(filed),
        ..
    }) = recorded(pool, project).await?
    else {
        return Ok(Ok(()));
    };
    let named = spec
        .get("organizationRef")
        .and_then(crate::resource::reference_name)
        .map(str::to_owned)
        .or_else(|| installation_organization(mirror));
    let Some(named) = named else {
        return Ok(Ok(()));
    };
    let target = registry_name(pool, mirror, &named).await?;
    if target == filed {
        return Ok(Ok(()));
    }
    let asked: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM operator_changes WHERE kind = 'ProjectMove' \
         AND state = 'proposed' AND payload->>'project' = $1 AND payload->>'organization' = $2)",
    )
    .bind(project)
    .bind(&target)
    .fetch_one(pool)
    .await?;
    if asked {
        // Its Endpoints keep granting once the manifest names the target: the gateway binds
        // their policyRefs on the target's domain from the next load on.
        let stray = stray_policy_refs(mirror, project, Some(&filed), &named);
        return Ok(match stray.is_empty() {
            true => Ok(()),
            false => Err(format!("spec.organizationRef: {}", stray.join("; "))),
        });
    }
    Ok(Err(format!(
        "spec.organizationRef: the registry files project '{project}' under Organization \
         '{filed}', and '{named}' names another; an operator moves a project by a ProjectMove \
         Change to '{target}' (Operator, Changes), and this Project Change is accepted once that \
         move is asked for"
    )))
}

/// Why an apply did not move the project.
#[derive(Debug)]
pub enum MoveError {
    /// A rule of [`plan`] does not hold now; the message names the next step.
    Refused(String),
    Db(sqlx::Error),
}

impl From<sqlx::Error> for MoveError {
    fn from(err: sqlx::Error) -> Self {
        Self::Db(err)
    }
}

/// Moves `project` to `to` inside `tx`, which the caller commits with its decision and audit row:
/// the rules of [`plan`] are checked again under the registry row's lock, then every keyed row of
/// the project under its old Organization takes the target's key, and the registry row does. A project under `to` already moves nothing
/// (an apply is idempotent). Answers the plan as it was applied, with the rows moved per table.
pub async fn apply(
    tx: &mut sqlx::PgConnection,
    mirror: &Mirror,
    change: &Approval<'_>,
    project: &str,
    to: &str,
) -> Result<MovePlan, MoveError> {
    let row: Option<(String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT p.id::text, p.organization, o.key::text FROM project_ids p \
         LEFT JOIN organizations o ON o.name = p.organization \
         WHERE p.parent IS NULL AND NOT p.reserved AND p.name = $1 FOR UPDATE OF p",
    )
    .bind(project)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((id, Some(from), Some(_))) = row else {
        return Err(MoveError::Refused(format!(
            "project: '{project}' is no project of the registry with an Organization"
        )));
    };
    if active_key(&mut *tx, to).await?.is_none() {
        return Err(MoveError::Refused(format!(
            "organization: '{to}' is no active Organization of the registry"
        )));
    }
    let manifest_organization = organization_of(mirror, project);
    if manifest_organization.as_deref() != Some(to) {
        return Err(MoveError::Refused(format!(
            "organization: Project '{project}' names Organization '{}' in its manifest; set its spec.organizationRef to '{to}' first",
            manifest_organization.as_deref().unwrap_or("none")
        )));
    }
    let branches: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM project_ids p JOIN project_branches b ON b.id = p.id \
         WHERE p.parent = $1::uuid AND b.expires_at > now()",
    )
    .bind(&id)
    .fetch_one(&mut *tx)
    .await?;
    if branches > 0 {
        return Err(MoveError::Refused(format!(
            "project: '{project}' has {branches} live branch(es); merge or close them first"
        )));
    }
    // Migration 0089's `project_move` moves what the Change names, approved by another operator:
    // each table holding the project's rows as their owner, then the registry row (nothing when
    // the project is under `to` already); it marks the Change applied and writes its audit row.
    let rows: Vec<(String, i64)> =
        sqlx::query_as("SELECT moved_table, moved_rows FROM project_move($1::uuid, $2, $3)")
            .bind(change.id)
            .bind(change.approver)
            .bind(change.source_ip)
            .fetch_all(&mut *tx)
            .await?;
    let moved: Vec<TableRows> = rows
        .into_iter()
        .map(|(table, rows)| TableRows {
            table,
            rows: rows.unsigned_abs(),
        })
        .collect();
    Ok(MovePlan {
        project: project.to_owned(),
        done: from == to,
        from: Some(from),
        to: to.to_owned(),
        manifest_organization,
        rows: moved.iter().map(|t| t.rows).sum(),
        tables: moved,
        bindings: bindings(mirror, project),
        blockers: Vec::new(),
        waits: Vec::new(),
    })
}

/// [`check_manifest`] as a door asks it, for a Portal with a database: `refusal` makes the line
/// its answer (a form error on a door, a conflict at approval); a registry that does not answer
/// is 503.
pub async fn placed(
    state: &crate::state::AppState,
    project: &str,
    spec: &serde_json::Value,
    refusal: fn(String) -> crate::error::ApiError,
) -> Result<(), crate::error::ApiError> {
    let Some(pool) = state.db.as_ref() else {
        return Ok(());
    };
    check_manifest(pool, &state.mirror, project, spec)
        .await
        .map_err(|err| {
            tracing::warn!(error = %err, %project, "the project registry could not be read");
            crate::error::ApiError::Unavailable(
                "the project registry could not be read; try again".into(),
            )
        })?
        .map_err(refusal)
}
