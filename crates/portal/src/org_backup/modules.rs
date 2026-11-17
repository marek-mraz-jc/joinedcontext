//! The data backup's module exporters: Antares (T-7170), kaisatsu, shako, daiya and dataroom
//! (T-7169), each read through a role of its own on the module's database.
//!
//! The reader role holds SELECT on the columns of [`KAISATSU_TABLES`] and the other lists only
//! (the module's migration grants them, `crates/<module>/store-pg/migrations/*backup_reader*`),
//! never BYPASSRLS: the module's own row-level security still decides what a read sees. Each
//! read sets the session settings that security reads to the Organization's tenant, and each
//! table's filter names the same keys again, so a row of another Organization would have to
//! pass both to leave.
//!
//! The keys are resolved once, at the start of the run, from what this Portal knows of the
//! Organization (its realm, its projects, the repositories of those projects) and from what jc
//! tells it of the modules' tenants ([`BackupModules`], `JC_PORTAL_BACKUP_TENANTS`).

use std::collections::BTreeMap;

use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::data::{Source, Table};
use crate::state::AppState;

/// The module databases this Portal reads for a data backup, and jc's tenant of each module.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct BackupModules {
    /// Connection string of each module's backup reader, by module (`antares`, `kaisatsu`,
    /// `shako`, `daiya`, `dataroom`). Secrets: they carry a password, never printed.
    pub readers: BTreeMap<&'static str, String>,
    pub tenants: Tenants,
}

impl std::fmt::Debug for BackupModules {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackupModules")
            .field("readers", &self.readers.keys().collect::<Vec<_>>())
            .field("tenants", &self.tenants)
            .finish()
    }
}

/// What jc tells the Portal about the modules' tenants (`JC_PORTAL_BACKUP_TENANTS`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Tenants {
    /// shako's one tenant in jc: `[instance id, org id]`.
    #[serde(default)]
    pub shako: Option<[uuid::Uuid; 2]>,
    /// daiya's installation id: each project's daiya organization derives from it.
    #[serde(default)]
    pub daiya_installation: Option<uuid::Uuid>,
    /// dataroom's OrgId and catalogue id of each Organization, by its name.
    #[serde(default)]
    pub dataroom: BTreeMap<String, DataroomTenant>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataroomTenant {
    pub org: uuid::Uuid,
    pub catalogue: uuid::Uuid,
}

/// The module readers in the order the archive lists them, with the variable naming each.
const READERS: &[(&str, &str)] = &[
    ("antares", "JC_PORTAL_BACKUP_ANTARES_DATABASE_URL"),
    ("kaisatsu", "JC_PORTAL_BACKUP_KAISATSU_DATABASE_URL"),
    ("shako", "JC_PORTAL_BACKUP_SHAKO_DATABASE_URL"),
    ("daiya", "JC_PORTAL_BACKUP_DAIYA_DATABASE_URL"),
    ("dataroom", "JC_PORTAL_BACKUP_DATAROOM_DATABASE_URL"),
];

impl BackupModules {
    /// Reads the module readers and jc's tenants:
    /// - `JC_PORTAL_BACKUP_ANTARES_DATABASE_URL`, `JC_PORTAL_BACKUP_KAISATSU_DATABASE_URL`,
    ///   `JC_PORTAL_BACKUP_SHAKO_DATABASE_URL`,
    ///   `JC_PORTAL_BACKUP_DAIYA_DATABASE_URL`, `JC_PORTAL_BACKUP_DATAROOM_DATABASE_URL`: the
    ///   connection string of the module's backup reader (`<module>_backup`, a secret: it carries
    ///   the password). Unset, the data backup lists the module as not exported, with why.
    /// - `JC_PORTAL_BACKUP_TENANTS`: jc's tenant of each module as JSON
    ///   (`{"shako": [instance, org], "daiyaInstallation": uuid, "dataroom": {"<Organization>":
    ///   {"org": uuid, "catalogue": uuid}}}`); jc fills it, a standalone Portal leaves it unset.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let mut readers = BTreeMap::new();
        for (module, var) in READERS {
            if let Some(url) = lookup(var).filter(|url| !url.trim().is_empty()) {
                if !url.starts_with("postgres://") && !url.starts_with("postgresql://") {
                    return Err(format!("{var}: not a postgres:// connection string"));
                }
                readers.insert(*module, url);
            }
        }
        let tenants = match lookup("JC_PORTAL_BACKUP_TENANTS").filter(|v| !v.trim().is_empty()) {
            Some(text) => {
                serde_json::from_str(&text).map_err(|e| format!("JC_PORTAL_BACKUP_TENANTS: {e}"))?
            }
            None => Tenants::default(),
        };
        Ok(Self { readers, tenants })
    }
}

macro_rules! table {
    ($name:literal, $keys:literal, $filter:literal, [$($column:literal),+ $(,)?]) => {
        Table {
            name: $name,
            columns: &[$($column),+],
            keys: $keys,
            filter: $filter,
        }
    };
}

/// Antares' rows of the Organization's Context Spaces, one broker tenant per space: its
/// entities, their temporal history (the integer-keyed rows with the tenant's own IRI
/// dictionary, the older text-keyed instances and the superseded versions) and the
/// Organization's own @contexts. Left out: subscriptions and context source registrations
/// (their notification and forwarding endpoints may carry credentials in headers), the
/// broker's operational state (outbox, dead letters, snapshots, views, distributed
/// subscriptions, entity map documents) and the indexes it rebuilds from entities
/// (entity_types, entity_attrs, entity_values). Partitioned tables are read through their
/// parent, which holds the row-level security.
pub const ANTARES_TABLES: &[Table] = &[
    table!(
        "entities",
        "tenants",
        "tenant_id = ANY($1)",
        [
            "tenant_id",
            "id",
            "entity",
            "version",
            "types",
            "scopes",
            "location",
            "location_ambiguous",
            "created_at",
            "modified_at",
            "expires_at",
            "attrs_expire_at"
        ]
    ),
    table!(
        "temporal_entities",
        "tenants",
        "tenant_id = ANY($1)",
        [
            "tenant_id",
            "id",
            "types",
            "scopes",
            "meta",
            "created_at",
            "modified_at",
            "deleted_at",
            "dataset_attrs",
            "offset_until",
            "bare"
        ]
    ),
    table!(
        "iri_ids",
        "tenants",
        "tenant_id = ANY($1)",
        ["tenant_id", "id", "iri"]
    ),
    table!(
        "attr_rows",
        "tenants",
        "tenant_id = ANY($1)",
        [
            "tenant_id",
            "entity_no",
            "attr_no",
            "instance_no",
            "dataset_id",
            "observed_at",
            "created_at",
            "modified_at",
            "deleted_at",
            "data",
            "geo_value",
            "version",
            "provenance",
            "short"
        ]
    ),
    table!(
        "attr_instances",
        "tenants",
        "tenant_id = ANY($1)",
        [
            "tenant_id",
            "entity_id",
            "attr_id",
            "instance_id",
            "dataset_id",
            "observed_at",
            "created_at",
            "modified_at",
            "deleted_at",
            "data",
            "geo_value",
            "version",
            "provenance"
        ]
    ),
    table!(
        "attr_versions",
        "tenants",
        "tenant_id = ANY($1)",
        [
            "tenant_id",
            "entity_id",
            "attr_id",
            "instance_id",
            "dataset_id",
            "observed_at",
            "version",
            "data",
            "provenance",
            "superseded_at"
        ]
    ),
    table!(
        "entity_id_remap",
        "tenants",
        "tenant_id = ANY($1)",
        ["tenant_id", "old_id", "new_id", "remapped_at"]
    ),
    // The shared cached contexts (tenant_id NULL) are every tenant's: never on the list.
    table!(
        "jsonld_contexts",
        "tenants",
        "tenant_id = ANY($1)",
        ["id", "body", "kind", "created_at", "tenant_id"]
    ),
];

/// kaisatsu's tables of the Organization's own realm. Left out: realms (settings, and the
/// realm's document carries its mail settings), credentials, identity_providers, signing_keys,
/// every session, code, token and device grant, login_failure, consent, email_outbox,
/// webauthn_credential and idp_links (second factors and linked accounts), and by column
/// `organization_domains.challenge_hash` and `applications.secret_enc`.
pub const KAISATSU_TABLES: &[Table] = &[
    table!(
        "organizations",
        "realm",
        "realm_id = ANY($1)",
        ["realm_id", "org_id", "alias", "doc"]
    ),
    table!(
        "organization_domains",
        "realm",
        "realm_id = ANY($1)",
        ["realm_id", "org_id", "domain_lower", "verified"]
    ),
    table!(
        "memberships",
        "realm",
        "realm_id = ANY($1)",
        ["realm_id", "org_id", "person_id", "doc"]
    ),
    table!(
        "groups",
        "realm",
        "realm_id = ANY($1)",
        ["realm_id", "id", "org_id", "path", "doc"]
    ),
    table!(
        "group_members",
        "realm",
        "realm_id = ANY($1)",
        ["realm_id", "group_id", "user_id", "org_id", "doc"]
    ),
    table!(
        "people",
        "realm",
        "realm_id = ANY($1)",
        [
            "realm_id",
            "id",
            "org_id",
            "username_lower",
            "email_lower",
            "transient",
            "doc"
        ]
    ),
    table!(
        "roles",
        "realm",
        "realm_id = ANY($1)",
        [
            "realm_id",
            "id",
            "container_id",
            "client_role",
            "name",
            "doc"
        ]
    ),
    table!(
        "role_mappings",
        "realm",
        "realm_id = ANY($1)",
        [
            "realm_id",
            "id",
            "principal_kind",
            "principal_id",
            "client_uuid",
            "role_name",
            "doc"
        ]
    ),
    table!(
        "applications",
        "realm",
        "realm_id = ANY($1)",
        ["realm_id", "id", "org_id", "client_id", "doc"]
    ),
    table!(
        "client_scopes",
        "realm",
        "realm_id = ANY($1)",
        ["realm_id", "id", "name", "doc"]
    ),
    table!(
        "required_actions",
        "realm",
        "realm_id = ANY($1)",
        ["realm_id", "alias", "doc"]
    ),
    table!(
        "branding_blocks",
        "realm",
        "realm_id = ANY($1)",
        ["realm_id", "org_key", "org_id", "doc"]
    ),
    table!(
        "events",
        "realm",
        "realm_id = ANY($1)",
        ["realm_id", "id", "org_id", "doc"]
    ),
    table!(
        "admin_events",
        "realm",
        "realm_id = ANY($1)",
        ["realm_id", "id", "org_id", "doc"]
    ),
];

/// shako's rows of the Organization's project repositories (`owner/name`). Left out: instance
/// and quota_usage (the installation's), webhook and push_mirror (secret references and
/// remote credentials), delivery, packages and blobs (registry content), outbox and store_log
/// (the installation's change feed).
pub const SHAKO_TABLES: &[Table] = &[
    table!(
        "repo",
        "repos",
        "owner || '/' || name = ANY($1)",
        [
            "id",
            "instance_id",
            "org_id",
            "project_id",
            "owner",
            "name",
            "description",
            "default_branch",
            "private",
            "archived",
            "size_bytes",
            "next_pr_number",
            "created_at",
            "updated_at"
        ]
    ),
    table!(
        "protection_rule",
        "repos",
        "repo_id IN (SELECT id FROM shako.repo WHERE owner || '/' || name = ANY($1))",
        [
            "id",
            "instance_id",
            "org_id",
            "repo_id",
            "kind",
            "rule_name",
            "doc",
            "created_at",
            "updated_at"
        ]
    ),
    table!(
        "pull_request",
        "repos",
        "repo_id IN (SELECT id FROM shako.repo WHERE owner || '/' || name = ANY($1))",
        [
            "id",
            "instance_id",
            "org_id",
            "repo_id",
            "number",
            "state",
            "title",
            "body",
            "head_ref",
            "head_sha",
            "base_ref",
            "base_sha",
            "merge_base",
            "mergeable",
            "merged",
            "merge_commit_sha",
            "author_login",
            "author_name",
            "author_email",
            "created_at",
            "updated_at",
            "closed_at"
        ]
    ),
    table!(
        "review",
        "repos",
        "repo_id IN (SELECT id FROM shako.repo WHERE owner || '/' || name = ANY($1))",
        [
            "id",
            "instance_id",
            "org_id",
            "repo_id",
            "pr_number",
            "verdict",
            "body",
            "author_login",
            "commit_id",
            "dismissed",
            "created_at"
        ]
    ),
    table!(
        "commit_status",
        "repos",
        "repo_id IN (SELECT id FROM shako.repo WHERE owner || '/' || name = ANY($1))",
        [
            "id",
            "instance_id",
            "org_id",
            "repo_id",
            "sha",
            "context",
            "state",
            "description",
            "target_url",
            "updated_at"
        ]
    ),
];

/// daiya's rows of the daiya organizations of the Organization's projects (one per project).
/// Left out: cr_quota (the installation's limits), cr_query_cache (a cache), cr_import_report,
/// and by column `cr_datasource.secret_refs` and the whole cr_public_dashboard (its access
/// token hash is the share link).
pub const DAIYA_TABLES: &[Table] = &[
    table!(
        "cr_folder",
        "daiya_orgs",
        "org_id::text = ANY($1)",
        [
            "installation_id",
            "org_id",
            "instance_id",
            "project_id",
            "uid",
            "parent_uid",
            "title",
            "version",
            "managed_by",
            "created_by",
            "updated_by",
            "created_at",
            "updated_at"
        ]
    ),
    table!(
        "cr_dashboard",
        "daiya_orgs",
        "org_id::text = ANY($1)",
        [
            "installation_id",
            "org_id",
            "instance_id",
            "project_id",
            "uid",
            "folder_uid",
            "title",
            "slug",
            "tags",
            "panel_count",
            "datasource_uids",
            "version",
            "schema_version",
            "is_provisioned",
            "provisioned_external_id",
            "allow_ui_updates",
            "model",
            "created_by",
            "updated_by",
            "created_at",
            "updated_at"
        ]
    ),
    table!(
        "cr_dashboard_version",
        "daiya_orgs",
        "org_id::text = ANY($1)",
        [
            "installation_id",
            "org_id",
            "instance_id",
            "dashboard_uid",
            "version",
            "parent_version",
            "restored_from",
            "message",
            "model",
            "created_by",
            "created_at"
        ]
    ),
    table!(
        "cr_datasource",
        "daiya_orgs",
        "org_id::text = ANY($1)",
        [
            "installation_id",
            "org_id",
            "instance_id",
            "project_id",
            "uid",
            "name",
            "kind",
            "access",
            "url",
            "is_default",
            "read_only",
            "json_data",
            "allow_hosts",
            "wasm_digest",
            "version",
            "created_at",
            "updated_at"
        ]
    ),
    table!(
        "cr_library_panel",
        "daiya_orgs",
        "org_id::text = ANY($1)",
        [
            "installation_id",
            "org_id",
            "instance_id",
            "folder_uid",
            "uid",
            "name",
            "panel_type",
            "description",
            "model",
            "version",
            "created_at",
            "updated_at"
        ]
    ),
    table!(
        "cr_alert_rule",
        "daiya_orgs",
        "org_id::text = ANY($1)",
        [
            "installation_id",
            "org_id",
            "instance_id",
            "uid",
            "folder_uid",
            "title",
            "spec",
            "updated_at"
        ]
    ),
    table!(
        "cr_alert_state",
        "daiya_orgs",
        "org_id::text = ANY($1)",
        [
            "installation_id",
            "org_id",
            "instance_id",
            "rule_uid",
            "state",
            "updated_at"
        ]
    ),
];

/// dataroom's rows of the Organization's own catalogue; the publisher is a child of it. Left
/// out: the search vectors (derived) and `cat_harvest_source.doc` (a source's settings may hold
/// its credentials).
pub const DATAROOM_TABLES: &[Table] = &[
    table!(
        "cat_catalogue",
        "catalogue",
        "id::text = ANY($1)",
        [
            "org_id",
            "id",
            "name",
            "host",
            "title",
            "description",
            "default_language",
            "main_profile",
            "modified",
            "doc"
        ]
    ),
    table!(
        "cat_publisher",
        "catalogue",
        "catalogue_id::text = ANY($1)",
        [
            "org_id",
            "catalogue_id",
            "id",
            "name",
            "title",
            "uri",
            "state",
            "merged_into",
            "modified",
            "doc"
        ]
    ),
    table!(
        "cat_dataset",
        "catalogue",
        "catalogue_id::text = ANY($1)",
        [
            "org_id",
            "catalogue_id",
            "id",
            "name",
            "title",
            "notes",
            "publisher_id",
            "visibility",
            "state",
            "licence_id",
            "frequency",
            "endpoint_url",
            "issued",
            "modified",
            "doc"
        ]
    ),
    table!(
        "cat_distribution",
        "catalogue",
        "catalogue_id::text = ANY($1)",
        [
            "org_id",
            "catalogue_id",
            "dataset_id",
            "id",
            "name",
            "access_url",
            "download_url",
            "format",
            "media_type",
            "byte_size",
            "doc"
        ]
    ),
    table!(
        "cat_view",
        "catalogue",
        "catalogue_id::text = ANY($1)",
        [
            "org_id",
            "catalogue_id",
            "distribution_id",
            "id",
            "view_type",
            "title",
            "view_order",
            "doc"
        ]
    ),
    table!(
        "cat_harvest_source",
        "catalogue",
        "catalogue_id::text = ANY($1)",
        [
            "org_id",
            "catalogue_id",
            "id",
            "name",
            "kind",
            "url",
            "publisher_id",
            "enabled",
            "modified"
        ]
    ),
    table!(
        "cat_harvest_run",
        "catalogue",
        "catalogue_id::text = ANY($1)",
        [
            "org_id",
            "catalogue_id",
            "id",
            "source_id",
            "state",
            "started_at",
            "finished_at",
            "created_count",
            "updated_count",
            "unchanged_count",
            "withdrawn_count",
            "doc"
        ]
    ),
    table!(
        "cat_activity",
        "catalogue",
        "catalogue_id::text = ANY($1)",
        [
            "org_id",
            "catalogue_id",
            "id",
            "at",
            "actor",
            "operation",
            "object_type",
            "object_id",
            "summary",
            "doc"
        ]
    ),
    table!(
        "cat_shacl_result",
        "catalogue",
        "catalogue_id::text = ANY($1)",
        [
            "org_id",
            "catalogue_id",
            "id",
            "dataset_id",
            "profile_id",
            "profile_version",
            "conforms",
            "violation_count",
            "warning_count",
            "checked_at",
            "doc"
        ]
    ),
    table!(
        "cat_profile",
        "catalogue",
        "catalogue_id::text = ANY($1)",
        ["org_id", "catalogue_id", "id", "version", "doc"]
    ),
    table!(
        "cat_page",
        "catalogue",
        "catalogue_id::text = ANY($1)",
        ["org_id", "catalogue_id", "slug", "lang", "modified", "doc"]
    ),
    table!(
        "cat_plugin",
        "catalogue",
        "catalogue_id::text = ANY($1)",
        ["org_id", "catalogue_id", "name", "modified", "doc"]
    ),
];

/// daiya's organization of `project` (daiya's `org_id_for_name`, `crates/daiya/api/src/
/// compat.rs`): the Portal provisions one daiya organization per project, named after it.
pub fn daiya_org(installation: uuid::Uuid, project: &str) -> uuid::Uuid {
    let digest = Sha256::digest(format!("daiya-org\n{installation}\n{project}"));
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    uuid::Builder::from_custom_bytes(bytes).into_uuid()
}

/// The realm of `organization` when it signs in through a realm of its own: the last segment
/// of its issuer (`…/realms/{name}`). The installation's shared realm holds every other
/// Organization's people, so it is never read for one of them.
fn own_realm(state: &AppState, organization: &str) -> Option<String> {
    let realm = state.organization_realms.get(organization)?;
    let (_, name) = realm
        .issuer()
        .trim_end_matches('/')
        .rsplit_once("/realms/")?;
    (!name.is_empty()).then(|| name.to_owned())
}

/// One connection of `module`'s reader for the run; the transaction is read only and the role
/// holds SELECT only.
async fn reader(state: &AppState, module: &str) -> Result<sqlx::PgPool, String> {
    let url = state
        .config
        .backup_modules
        .readers
        .get(module)
        .ok_or_else(|| {
            let var = READERS
                .iter()
                .find(|(m, _)| *m == module)
                .map_or("", |(_, var)| var);
            format!("this Portal holds no reader of the {module} database ({var} is not set)")
        })?;
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect(url)
        .await
        .map_err(|e| format!("the {module} database could not be reached: {e}"))
}

fn one_key(name: &'static str, values: Vec<String>) -> BTreeMap<&'static str, Vec<String>> {
    BTreeMap::from([(name, values)])
}

/// The broker tenant of every Context Space of the Organization's projects: the space's
/// segment, as the gateway names it at the broker (`spaces::segment`).
/// ponytail: segment-named tenants only; a broker switched to `{organization id}__{segment}`
/// (JC_TENANT_NAMES=organization, PF-130) needs that name here too.
pub fn antares_tenants(state: &AppState, organization: &str) -> Vec<String> {
    let mut tenants: Vec<String> = super::projects_of(&state.mirror, organization)
        .iter()
        .flat_map(|project| {
            state
                .mirror
                .list(
                    project,
                    "ContextSpace",
                    &crate::store::ListOptions::default(),
                )
                .items
                .into_iter()
                .map(move |space| {
                    crate::spaces::segment(&state.mirror, project, &space.metadata.name)
                })
        })
        .collect();
    tenants.sort();
    tenants.dedup();
    tenants
}

async fn antares(state: &AppState, organization: &str) -> Result<Source, String> {
    let tenants = antares_tenants(state, organization);
    Ok(Source {
        pool: reader(state, "antares").await?,
        schema: "",
        scopes: tenants
            .iter()
            .map(|tenant| vec![("antares.tenant", tenant.clone())])
            .collect(),
        keys: one_key("tenants", tenants),
        tables: ANTARES_TABLES,
        folders: Vec::new(),
    })
}

async fn kaisatsu(state: &AppState, organization: &str) -> Result<Source, String> {
    let realm = own_realm(state, organization).ok_or_else(|| {
        "the Organization signs in through the installation's shared realm, which holds other \
         Organizations' people too; its people are in the configuration backup's people.yaml"
            .to_owned()
    })?;
    Ok(Source {
        pool: reader(state, "kaisatsu").await?,
        schema: "kaisatsu",
        scopes: vec![vec![
            ("kaisatsu.realm", realm.clone()),
            ("kaisatsu.orgs", "*".to_owned()),
        ]],
        keys: one_key("realm", vec![realm]),
        tables: KAISATSU_TABLES,
        folders: Vec::new(),
    })
}

async fn shako(state: &AppState, organization: &str) -> Result<Source, String> {
    let [instance, org] = state
        .config
        .backup_modules
        .tenants
        .shako
        .ok_or("jc named no shako tenant (JC_PORTAL_BACKUP_TENANTS)")?;
    let owner = state
        .shako
        .as_ref()
        .map(|client| client.owner.clone())
        .ok_or("this Portal has no forge")?;
    let repos = super::projects_of(&state.mirror, organization)
        .iter()
        .filter_map(|project| state.mirror.repository_of(project))
        .map(|repo| format!("{owner}/{repo}"))
        .collect();
    Ok(Source {
        pool: reader(state, "shako").await?,
        schema: "shako",
        scopes: vec![vec![
            ("shako.instance", instance.to_string()),
            ("shako.org", org.to_string()),
        ]],
        keys: one_key("repos", repos),
        tables: SHAKO_TABLES,
        folders: Vec::new(),
    })
}

async fn daiya(state: &AppState, organization: &str) -> Result<Source, String> {
    let installation = state
        .config
        .backup_modules
        .tenants
        .daiya_installation
        .ok_or("jc named no daiya installation (JC_PORTAL_BACKUP_TENANTS)")?;
    let orgs: Vec<String> = super::projects_of(&state.mirror, organization)
        .iter()
        .map(|project| daiya_org(installation, project).to_string())
        .collect();
    Ok(Source {
        pool: reader(state, "daiya").await?,
        schema: "",
        scopes: orgs
            .iter()
            .map(|org| vec![("cr.org", org.clone())])
            .collect(),
        keys: one_key("daiya_orgs", orgs),
        tables: DAIYA_TABLES,
        folders: Vec::new(),
    })
}

async fn dataroom(state: &AppState, organization: &str) -> Result<Source, String> {
    let tenant = state
        .config
        .backup_modules
        .tenants
        .dataroom
        .get(organization)
        .cloned()
        .ok_or("jc names no catalogue of the Organization's own (JC_PORTAL_BACKUP_TENANTS)")?;
    Ok(Source {
        pool: reader(state, "dataroom").await?,
        schema: "dataroom",
        scopes: vec![vec![
            ("jc.org", tenant.org.to_string()),
            ("jc.catalogue", tenant.catalogue.to_string()),
        ]],
        keys: one_key("catalogue", vec![tenant.catalogue.to_string()]),
        tables: DATAROOM_TABLES,
        folders: Vec::new(),
    })
}

/// Every module's source for `organization`, or why it is not exported.
pub async fn sources(
    state: &AppState,
    organization: &str,
) -> Vec<(&'static str, Result<Source, String>)> {
    vec![
        ("antares", antares(state, organization).await),
        ("kaisatsu", kaisatsu(state, organization).await),
        ("shako", shako(state, organization).await),
        ("daiya", daiya(state, organization).await),
        ("dataroom", dataroom(state, organization).await),
    ]
}

/// Every module table the backup reads, by module, for the grant checks.
pub const MODULE_TABLES: &[(&str, &str, &[Table])] = &[
    ("antares", "public", ANTARES_TABLES),
    ("kaisatsu", "kaisatsu", KAISATSU_TABLES),
    ("shako", "shako", SHAKO_TABLES),
    ("daiya", "public", DAIYA_TABLES),
    ("dataroom", "dataroom", DATAROOM_TABLES),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_daiya_organization_is_the_hash_daiya_derives() {
        // The value worked out apart from this code: SHA-256 of "daiya-org\n<installation>\n
        // <project>", the first 16 bytes with uuid's version 8 and variant bits.
        let one = uuid::Uuid::from_u128(1);
        let org = daiya_org(one, "helsinki");
        assert_eq!(org.to_string(), "bfd90e4a-5b80-822b-8831-52fe27995422");
        assert_ne!(org, daiya_org(one, "zilina"));
    }

    #[test]
    fn no_secret_column_of_a_module_is_on_an_allow_list() {
        let mut flagged = Vec::new();
        for (module, _, tables) in MODULE_TABLES {
            for table in *tables {
                for column in table.columns {
                    if [
                        "secret",
                        "token",
                        "hash",
                        "password",
                        "_enc",
                        "private_key",
                        "challenge",
                    ]
                    .iter()
                    .any(|s| column.contains(s))
                    {
                        flagged.push(format!("{module}.{}.{column}", table.name));
                    }
                }
                assert!(
                    ![
                        "credentials",
                        "identity_providers",
                        "signing_keys",
                        "webauthn_credential",
                        "idp_links",
                        "device_code",
                        "email_outbox",
                        "realms",
                        "webhook",
                        "push_mirror",
                        "cr_public_dashboard"
                    ]
                    .contains(&table.name),
                    "{module}.{} must not leave",
                    table.name
                );
            }
        }
        assert!(
            flagged.is_empty(),
            "looks like secret material: {flagged:?}"
        );
    }

    #[test]
    fn tenants_name_their_field_and_readers_must_be_postgres() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| (*v).to_owned())
            }
        };
        let parsed = BackupModules::from_lookup(env(&[
            (
                "JC_PORTAL_BACKUP_DAIYA_DATABASE_URL",
                "postgres://daiya_backup@db/jc_daiya",
            ),
            (
                "JC_PORTAL_BACKUP_TENANTS",
                r#"{"daiyaInstallation":"00000000-0000-0000-0000-000000000001"}"#,
            ),
        ]))
        .unwrap();
        assert_eq!(parsed.readers.keys().collect::<Vec<_>>(), vec![&"daiya"]);
        assert_eq!(
            parsed.tenants.daiya_installation,
            Some(uuid::Uuid::from_u128(1))
        );
        assert!(!format!("{parsed:?}").contains("daiya_backup@"));
        let refused =
            BackupModules::from_lookup(env(&[("JC_PORTAL_BACKUP_TENANTS", r#"{"shakoo":1}"#)]))
                .unwrap_err();
        assert!(refused.starts_with("JC_PORTAL_BACKUP_TENANTS"), "{refused}");
        let refused = BackupModules::from_lookup(env(&[(
            "JC_PORTAL_BACKUP_SHAKO_DATABASE_URL",
            "mysql://x",
        )]))
        .unwrap_err();
        assert!(
            refused.starts_with("JC_PORTAL_BACKUP_SHAKO_DATABASE_URL"),
            "{refused}"
        );
    }
}
