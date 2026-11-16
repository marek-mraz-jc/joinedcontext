//! An Organization's own backup to its own versioned S3 bucket (T-7154, T-7155).
//!
//! The configuration backup is files, never a database dump: the Organization's settings as
//! manifests with their ids, the configuration export of each of its projects (`export.rs`, the
//! same bundle a whole-project export is, minus code), the JSON Schema of every kind it holds and
//! a README saying what is in it and what is left out on purpose. One Organization per bundle,
//! selected by [`belongs`]: nothing of another Organization or of the installation's own settings.
//!
//! The bucket is the Organization's: versioning must be on (GetBucketVersioning answers
//! `Enabled`) before a target is saved and again before every run, so a delete only leaves a
//! delete marker and every earlier bundle stays readable. Noncurrent versions older than the
//! target's `keepDays` are removed by the run itself, which works on any S3 and never rewrites
//! a bucket lifecycle the Organization set for other prefixes; Object Lock, where the bucket has
//! it, wins over that.
//!
//! Every run, delete and restore is written to `organization_backup_log` with who and when: that
//! table is the audit, kept as long as the Organization's target exists.

pub mod data;
pub mod restore;
pub mod s3;

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::RwLock;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use zeroize::Zeroizing;

use crate::error::ApiError;
use crate::permissions::ORG_NAMESPACE;
use crate::resource::ResourceEnvelope;
use crate::state::AppState;
use crate::store::{ListOptions, Mirror};

/// The configuration backup: manifests, schemas, no database rows.
pub const CONFIGURATION: &str = "configuration";
/// The data backup: the Organization's database rows, age-encrypted ([`data`], T-7168).
pub const DATA: &str = "data";

/// Hours between scheduled runs: at least one, at most four weeks.
pub const EVERY_HOURS: std::ops::RangeInclusive<i32> = 1..=672;
/// Days a noncurrent version is kept: at least one, at most ten years.
pub const KEEP_DAYS: std::ops::RangeInclusive<i32> = 1..=3650;

/// Where an Organization's backup goes and when it runs. The secret access key is sealed with
/// the Portal's age key and never leaves this struct: it is not serialised.
#[derive(Debug, Clone, Serialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub organization: String,
    pub kind: String,
    /// `https://s3.eu-central-1.amazonaws.com`, a MinIO or Ceph address.
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
    /// The folder of the bucket every object of this backup lands under, e.g. `jc/`.
    pub prefix: String,
    pub access_key_id: String,
    #[serde(skip)]
    pub sealed_secret: String,
    pub enabled: bool,
    pub every_hours: i32,
    pub keep_days: i32,
    /// The age recipients (`age1…`, public keys) a data archive is encrypted to: the working
    /// one first, then the recovery one. Empty for the configuration backup.
    pub recipients: Vec<String>,
    pub updated_by: String,
    /// RFC 3339.
    pub updated_at: String,
    /// RFC 3339; the start of the last scheduled or manual run.
    pub last_run_at: Option<String>,
}

impl Target {
    /// The folder of this Organization's bundles: `{prefix}{organization}/{kind}/`.
    pub fn folder(&self) -> String {
        format!("{}{}/{}/", self.prefix, self.organization, self.kind)
    }

    /// The one object every run writes a new version of.
    pub fn object_key(&self) -> String {
        if self.kind == DATA {
            format!("{}archive.zip.age", self.folder())
        } else {
            format!("{}bundle.zip", self.folder())
        }
    }
}

/// One audited act on an Organization's backup.
#[derive(Debug, Clone, Serialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub id: i64,
    pub organization: String,
    pub kind: String,
    /// `run`, `delete`, `restore-plan` or `restore`.
    pub action: String,
    /// Who: a username, or `schedule`.
    pub actor: String,
    /// RFC 3339.
    pub at: String,
    pub ok: bool,
    pub bytes: i64,
    pub object_key: String,
    pub version_id: String,
    /// What happened, in words; never a credential.
    pub message: String,
    /// The SHA-256 (hex) of the object a run uploaded; empty for every other act. A restore
    /// opens only these bytes.
    #[serde(skip)]
    pub sha256: String,
}

/// What a new log entry says; the store stamps `id` and `at`.
pub struct NewEntry<'a> {
    pub organization: &'a str,
    pub kind: &'a str,
    pub action: &'a str,
    pub actor: &'a str,
    pub ok: bool,
    pub bytes: i64,
    pub object_key: &'a str,
    pub version_id: &'a str,
    pub message: &'a str,
    pub sha256: &'a str,
}

fn rfc3339(at: time::OffsetDateTime) -> String {
    at.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

/// The targets by (organization, kind) and the log, when the Portal runs without a database.
type MemoryRows = (BTreeMap<(String, String), Target>, Vec<LogEntry>);

enum Inner {
    Postgres(sqlx::PgPool),
    Memory(RwLock<MemoryRows>),
}

/// The targets and the audit log, in Postgres (`0058_organization_backups.sql`) or in memory.
pub struct BackupStore {
    inner: Inner,
}

type TargetRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    bool,
    i32,
    i32,
    Vec<String>,
    String,
    time::OffsetDateTime,
    Option<time::OffsetDateTime>,
);

fn target_of(row: TargetRow) -> Target {
    Target {
        organization: row.0,
        kind: row.1,
        endpoint: row.2,
        region: row.3,
        bucket: row.4,
        prefix: row.5,
        access_key_id: row.6,
        sealed_secret: row.7,
        enabled: row.8,
        every_hours: row.9,
        keep_days: row.10,
        recipients: row.11,
        updated_by: row.12,
        updated_at: rfc3339(row.13),
        last_run_at: row.14.map(rfc3339),
    }
}

type LogRow = (
    i64,
    String,
    String,
    String,
    String,
    time::OffsetDateTime,
    bool,
    i64,
    String,
    String,
    String,
    String,
);

fn db(err: sqlx::Error) -> ApiError {
    ApiError::Internal(format!("the backup store failed: {err}"))
}

impl BackupStore {
    pub fn new(db: Option<sqlx::PgPool>) -> Self {
        Self {
            inner: match db {
                Some(pool) => Inner::Postgres(pool),
                None => Inner::Memory(RwLock::default()),
            },
        }
    }

    pub async fn target(&self, organization: &str, kind: &str) -> Result<Option<Target>, ApiError> {
        match &self.inner {
            Inner::Memory(rows) => Ok(rows
                .read()
                .map_err(|_| ApiError::Internal("the backup store is poisoned".into()))?
                .0
                .get(&(organization.to_owned(), kind.to_owned()))
                .cloned()),
            Inner::Postgres(pool) => {
                let row: Option<TargetRow> = sqlx::query_as(
                    "SELECT organization, kind, endpoint, region, bucket, prefix, access_key_id, \
                     sealed_secret, enabled, every_hours, keep_days, recipients, updated_by, \
                     updated_at, last_run_at FROM organization_backup_targets \
                     WHERE organization = $1 AND kind = $2",
                )
                .bind(organization)
                .bind(kind)
                .fetch_optional(pool)
                .await
                .map_err(db)?;
                Ok(row.map(target_of))
            }
        }
    }

    /// Writes the target whole; `last_run_at` is the store's and stays as it was.
    pub async fn save(&self, target: &Target) -> Result<(), ApiError> {
        match &self.inner {
            Inner::Memory(rows) => {
                let mut rows = rows
                    .write()
                    .map_err(|_| ApiError::Internal("the backup store is poisoned".into()))?;
                let key = (target.organization.clone(), target.kind.clone());
                let last = rows.0.get(&key).and_then(|t| t.last_run_at.clone());
                let mut saved = target.clone();
                saved.last_run_at = last;
                saved.updated_at = rfc3339(time::OffsetDateTime::now_utc());
                rows.0.insert(key, saved);
                Ok(())
            }
            Inner::Postgres(pool) => {
                sqlx::query(
                    "INSERT INTO organization_backup_targets (organization, kind, endpoint, \
                     region, bucket, prefix, access_key_id, sealed_secret, enabled, every_hours, \
                     keep_days, updated_by, recipients, updated_at) \
                     VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,now()) \
                     ON CONFLICT (organization, kind) DO UPDATE SET endpoint = $3, region = $4, \
                     bucket = $5, prefix = $6, access_key_id = $7, sealed_secret = $8, \
                     enabled = $9, every_hours = $10, keep_days = $11, updated_by = $12, \
                     recipients = $13, updated_at = now()",
                )
                .bind(&target.organization)
                .bind(&target.kind)
                .bind(&target.endpoint)
                .bind(&target.region)
                .bind(&target.bucket)
                .bind(&target.prefix)
                .bind(&target.access_key_id)
                .bind(&target.sealed_secret)
                .bind(target.enabled)
                .bind(target.every_hours)
                .bind(target.keep_days)
                .bind(&target.updated_by)
                .bind(&target.recipients)
                .execute(pool)
                .await
                .map_err(db)?;
                Ok(())
            }
        }
    }

    /// Claims the scheduled run of every enabled target whose last run is older than its
    /// interval, stamping `last_run_at` in the same statement: of several Portal replicas only
    /// the one whose UPDATE took the row runs it.
    pub async fn claim_due(&self) -> Result<Vec<Target>, ApiError> {
        match &self.inner {
            Inner::Memory(rows) => {
                let mut rows = rows
                    .write()
                    .map_err(|_| ApiError::Internal("the backup store is poisoned".into()))?;
                let now = time::OffsetDateTime::now_utc();
                let mut due = Vec::new();
                for target in rows.0.values_mut() {
                    let last = target.last_run_at.as_deref().and_then(|at| {
                        time::OffsetDateTime::parse(
                            at,
                            &time::format_description::well_known::Rfc3339,
                        )
                        .ok()
                    });
                    let ripe = last.is_none_or(|last| {
                        now - last >= time::Duration::hours(i64::from(target.every_hours))
                    });
                    if target.enabled && ripe {
                        target.last_run_at = Some(rfc3339(now));
                        due.push(target.clone());
                    }
                }
                Ok(due)
            }
            Inner::Postgres(pool) => {
                let rows: Vec<TargetRow> = sqlx::query_as(
                    "UPDATE organization_backup_targets SET last_run_at = now() \
                     WHERE enabled AND (last_run_at IS NULL \
                     OR last_run_at <= now() - make_interval(hours => every_hours)) \
                     RETURNING organization, kind, endpoint, region, bucket, prefix, \
                     access_key_id, sealed_secret, enabled, every_hours, keep_days, recipients, \
                     updated_by, updated_at, last_run_at",
                )
                .fetch_all(pool)
                .await
                .map_err(db)?;
                Ok(rows.into_iter().map(target_of).collect())
            }
        }
    }

    /// Stamps a manual run's start, which the schedule then counts from.
    pub async fn ran_now(&self, organization: &str, kind: &str) -> Result<(), ApiError> {
        match &self.inner {
            Inner::Memory(rows) => {
                let mut rows = rows
                    .write()
                    .map_err(|_| ApiError::Internal("the backup store is poisoned".into()))?;
                if let Some(target) = rows.0.get_mut(&(organization.to_owned(), kind.to_owned())) {
                    target.last_run_at = Some(rfc3339(time::OffsetDateTime::now_utc()));
                }
                Ok(())
            }
            Inner::Postgres(pool) => {
                sqlx::query(
                    "UPDATE organization_backup_targets SET last_run_at = now() \
                     WHERE organization = $1 AND kind = $2",
                )
                .bind(organization)
                .bind(kind)
                .execute(pool)
                .await
                .map_err(db)?;
                Ok(())
            }
        }
    }

    pub async fn log(&self, entry: NewEntry<'_>) -> Result<(), ApiError> {
        match &self.inner {
            Inner::Memory(rows) => {
                let mut rows = rows
                    .write()
                    .map_err(|_| ApiError::Internal("the backup store is poisoned".into()))?;
                let id = rows.1.last().map_or(1, |e| e.id + 1);
                rows.1.push(LogEntry {
                    id,
                    organization: entry.organization.to_owned(),
                    kind: entry.kind.to_owned(),
                    action: entry.action.to_owned(),
                    actor: entry.actor.to_owned(),
                    at: rfc3339(time::OffsetDateTime::now_utc()),
                    ok: entry.ok,
                    bytes: entry.bytes,
                    object_key: entry.object_key.to_owned(),
                    version_id: entry.version_id.to_owned(),
                    message: entry.message.to_owned(),
                    sha256: entry.sha256.to_owned(),
                });
                Ok(())
            }
            Inner::Postgres(pool) => {
                sqlx::query(
                    "INSERT INTO organization_backup_log (organization, kind, action, actor, ok, \
                     bytes, object_key, version_id, message, sha256) \
                     VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
                )
                .bind(entry.organization)
                .bind(entry.kind)
                .bind(entry.action)
                .bind(entry.actor)
                .bind(entry.ok)
                .bind(entry.bytes)
                .bind(entry.object_key)
                .bind(entry.version_id)
                .bind(entry.message)
                .bind(entry.sha256)
                .execute(pool)
                .await
                .map_err(db)?;
                Ok(())
            }
        }
    }

    /// The newest `limit` entries of one Organization's backup, newest first.
    pub async fn entries(
        &self,
        organization: &str,
        kind: &str,
        limit: i64,
    ) -> Result<Vec<LogEntry>, ApiError> {
        match &self.inner {
            Inner::Memory(rows) => Ok(rows
                .read()
                .map_err(|_| ApiError::Internal("the backup store is poisoned".into()))?
                .1
                .iter()
                .rev()
                .filter(|e| e.organization == organization && e.kind == kind)
                .take(usize::try_from(limit).unwrap_or(0))
                .cloned()
                .collect()),
            Inner::Postgres(pool) => {
                let rows: Vec<LogRow> = sqlx::query_as(
                    "SELECT id, organization, kind, action, actor, at, ok, bytes, object_key, \
                     version_id, message, sha256 FROM organization_backup_log \
                     WHERE organization = $1 AND kind = $2 ORDER BY id DESC LIMIT $3",
                )
                .bind(organization)
                .bind(kind)
                .bind(limit)
                .fetch_all(pool)
                .await
                .map_err(db)?;
                Ok(rows
                    .into_iter()
                    .map(|r| LogEntry {
                        id: r.0,
                        organization: r.1,
                        kind: r.2,
                        action: r.3,
                        actor: r.4,
                        at: rfc3339(r.5),
                        ok: r.6,
                        bytes: r.7,
                        object_key: r.8,
                        version_id: r.9,
                        message: r.10,
                        sha256: r.11,
                    })
                    .collect())
            }
        }
    }
}

/// How the backup reaches S3: the guarded client in production; a test hands its own client and
/// lets loopback addresses through, nothing else does.
pub struct Egress {
    pub http: reqwest::Client,
    pub public_only: bool,
}

impl Default for Egress {
    fn default() -> Self {
        Self {
            http: s3::guarded_client(),
            public_only: true,
        }
    }
}

impl Egress {
    /// Refuses an endpoint that is not `https` on a public host, before anything is sent. A name
    /// is judged again once it resolves, by the client's resolver.
    pub fn check(&self, endpoint: &url::Url) -> Result<(), String> {
        if !self.public_only {
            return Ok(());
        }
        if endpoint.scheme() != "https" {
            return Err("the endpoint must be an https address".into());
        }
        jc_core::outbound::check(endpoint).map_err(|r| r.to_string())?;
        if let Some(url::Host::Domain(host)) = endpoint.host() {
            let host = host.trim_end_matches('.').to_ascii_lowercase();
            let internal = !host.contains('.')
                || [".local", ".svc", ".internal", ".localdomain", ".home.arpa"]
                    .iter()
                    .any(|suffix| host.ends_with(suffix));
            if internal {
                return Err(format!(
                    "'{host}' is a name inside the cluster, not a public host"
                ));
            }
        }
        Ok(())
    }
}

/// The Portal's own age identity file, which seals the bucket's secret key at rest.
fn age_key_file(state: &AppState) -> Result<&std::path::Path, ApiError> {
    match &state.config.pipeline_secrets {
        Some(crate::pipeline_secrets::Backend::Sops { age_key_file }) => Ok(age_key_file),
        _ => Err(ApiError::Unavailable(
            "this Portal has no age key (JC_PORTAL_SOPS_AGE_KEY_FILE) to keep a bucket's secret \
             key sealed, so no backup target can be saved; the operator sets it"
                .into(),
        )),
    }
}

/// The secret access key sealed for the Portal's own recipient, ASCII-armored.
pub fn seal(state: &AppState, secret: &str) -> Result<String, ApiError> {
    let recipient =
        crate::secret_files::own_recipient(age_key_file(state)?).map_err(ApiError::Unavailable)?;
    let recipient = <age::x25519::Recipient as std::str::FromStr>::from_str(&recipient)
        .map_err(|_| ApiError::Unavailable("the Portal's age key is not an x25519 key".into()))?;
    crate::secret_files::wrap(secret.as_bytes(), &recipient).map_err(ApiError::Internal)
}

/// The secret access key back from its seal, for one run.
fn unseal(state: &AppState, sealed: &str) -> Result<Zeroizing<String>, ApiError> {
    let unreadable = || {
        ApiError::Unavailable("the bucket's secret key cannot be unsealed; save it again".into())
    };
    let identities =
        jcctl::secrets::identities_from_file(age_key_file(state)?).map_err(|_| unreadable())?;
    let decryptor = age::Decryptor::new(age::armor::ArmoredReader::new(sealed.as_bytes()))
        .map_err(|_| unreadable())?;
    let mut reader = decryptor
        .decrypt(identities.iter().map(|i| i as &dyn age::Identity))
        .map_err(|_| unreadable())?;
    let mut secret = Zeroizing::new(String::new());
    reader
        .read_to_string(&mut secret)
        .map_err(|_| unreadable())?;
    Ok(secret)
}

/// The bucket of a saved target, its endpoint checked again: a target saved before a rule
/// tightened is held to the rule of today.
pub fn bucket(state: &AppState, target: &Target) -> Result<s3::Bucket, ApiError> {
    let endpoint = url::Url::parse(&target.endpoint).map_err(|_| {
        ApiError::BadRequest("the saved endpoint does not parse; save it again".into())
    })?;
    state
        .org_backup_egress
        .check(&endpoint)
        .map_err(ApiError::BadRequest)?;
    Ok(s3::Bucket {
        endpoint,
        region: target.region.clone(),
        bucket: target.bucket.clone(),
        access_key_id: target.access_key_id.clone(),
        secret_access_key: unseal(state, &target.sealed_secret)?,
    })
}

/// Whether `envelope`, a manifest of the organization namespace, belongs to `organization`:
/// the Organization itself, a manifest whose `organizationRef` names it, or a RoleBinding whose
/// scope reaches it (its scope, a project of it or a space of such a project). Anything else is
/// another Organization's or the installation's and stays out.
pub fn belongs(mirror: &Mirror, organization: &str, envelope: &ResourceEnvelope) -> bool {
    match envelope.kind.as_str() {
        "Organization" => envelope.metadata.name == organization,
        "Project" => {
            crate::permissions::organization_of(mirror, &envelope.metadata.name).as_deref()
                == Some(organization)
        }
        "RoleBinding" => {
            crate::permissions::binding_organization(mirror, &envelope.spec).as_deref()
                == Some(organization)
        }
        _ => {
            envelope
                .spec
                .get("organizationRef")
                .and_then(crate::resource::reference_name)
                == Some(organization)
        }
    }
}

/// The projects of `organization`, by name, sorted.
pub fn projects_of(mirror: &Mirror, organization: &str) -> Vec<String> {
    mirror
        .list(ORG_NAMESPACE, "Project", &ListOptions::default())
        .items
        .into_iter()
        .map(|envelope| envelope.metadata.name)
        .filter(|project| {
            crate::permissions::organization_of(mirror, project).as_deref() == Some(organization)
        })
        .collect()
}

/// The manifests of the organization namespace that are `organization`'s, sanitised as an
/// export sanitises them (no status, no secret value, no code), sorted by kind and name.
pub fn organization_manifests(mirror: &Mirror, organization: &str) -> Vec<ResourceEnvelope> {
    let mut out = Vec::new();
    for kind in mirror.kinds_in(ORG_NAMESPACE) {
        for mut envelope in mirror
            .list(ORG_NAMESPACE, &kind, &ListOptions::default())
            .items
        {
            if !belongs(mirror, organization, &envelope)
                || crate::api::export::carries_code(&envelope)
            {
                continue;
            }
            envelope.strip_status();
            crate::api::export::strip_secret_values(&mut envelope.spec);
            out.push(envelope);
        }
    }
    out.sort_by(|a, b| (&a.kind, &a.metadata.name).cmp(&(&b.kind, &b.metadata.name)));
    out
}

/// One line of the bundle's `index.yaml`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IndexItem {
    pub kind: String,
    pub id: String,
    pub path: String,
    /// The project's commit the file was read at; empty for the Organization's own manifests,
    /// which are read from the Portal's synced view of the organization repository.
    pub revision: String,
}

/// `index.yaml` at the root of every bundle.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Index {
    pub format_version: u32,
    pub organization: String,
    /// RFC 3339, the start of the run.
    pub taken_at: String,
    pub taken_by: String,
    pub items: Vec<IndexItem>,
}

/// Where the Organization's people sit in a bundle (T-7164).
pub const PEOPLE_PATH: &str = "organization/people.yaml";

/// One person of `organization/people.yaml`: the id, username and e-mail and nothing else, so no
/// credential, second factor, session or attribute of the realm reaches a bundle (T-7164).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Person {
    pub id: String,
    pub username: String,
    pub email: Option<String>,
}

/// The people of `organization`: everyone in its own realm when it has one (T-7142), else the
/// members of `/{organization}/members` in the installation realm. `None` when this Portal has no
/// admin client for that realm, or the installation realm has no members group for it.
pub async fn people_of(
    state: &AppState,
    organization: &str,
) -> Result<Option<Vec<Person>>, ApiError> {
    let users = match state.organization_realms.get(organization) {
        Some(realm) => match realm.people.as_ref() {
            Some(people) => Some(people.admin().await?.everyone().await?),
            None => None,
        },
        None => match state.people.as_deref() {
            Some(people) => people.admin().await?.members_of(organization).await?,
            None => None,
        },
    };
    Ok(users.map(persons))
}

/// The column allow-list of a person, sorted by id, each person once.
fn persons(users: Vec<crate::people::KcUser>) -> Vec<Person> {
    let mut people: Vec<Person> = users
        .into_iter()
        .map(|user| Person {
            id: user.id,
            username: user.username,
            email: user.email,
        })
        .collect();
    people.sort_by(|a, b| a.id.cmp(&b.id));
    people.dedup_by(|a, b| a.id == b.id);
    people
}

/// Where an Organization-level manifest sits in the bundle.
fn organization_path(envelope: &ResourceEnvelope) -> String {
    let plural = crate::resource::by_kind(&envelope.kind)
        .map(|info| info.plural.to_owned())
        .unwrap_or_else(|| envelope.kind.to_ascii_lowercase());
    format!("organization/{plural}/{}.yaml", envelope.metadata.name)
}

const README_LEFT_OUT: &str = "\
## Left out on purpose

- Entity data and temporal history, database rows, files and uploads: the data backup holds those.
- Executable code: App source and builds, App functions, Script versions and Mapping native
  blocks, pipeline stream bodies (`bento.yaml`). The Pipeline and App manifests stay and name
  what they run by id and version.
- Secret values: a credential field stays with an empty value, a `secretRef` stays as it is.
- People's credentials, second factors, sessions and every other field of a person:
  `organization/people.yaml` holds each person's id, username and e-mail only. A restore never
  creates a person: the plan lists the people missing today, to invite by hand.
- Other Organizations, and the installation's own settings.
";

/// The README of a bundle: what each folder holds, and what is left out on purpose.
fn readme(index: &Index, projects: &[(String, String)], has_people: bool) -> String {
    let mut text = format!(
        "# Configuration backup of {}\n\nTaken {} by {}. `index.yaml` lists every manifest with \
         its kind, id, path and revision.\n\n## What is inside\n\n\
         - `organization/`: the Organization's own manifests (the Organization, its Portal hosts \
         and branding, roles, role bindings, groups, catalogue connections and the other \
         settings of the Administration page), one file per manifest named by its id.\n\
         - `organization/people.yaml`: the Organization's people, each as id, username and \
         e-mail.\n\
         - `schemas/kinds/`: the JSON Schema of every Organization-level kind in `organization/`.\n\
         - `projects/<name>/`: each project's configuration export at the commit `index.yaml` \
         names, with its own `bundle.yaml`, README and schemas (kinds and data models: LinkML \
         and JSON Schema).\n\n",
        index.organization, index.taken_at, index.taken_by
    );
    if !has_people {
        text.push_str(
            "This bundle holds no `organization/people.yaml`: the Portal that took it reads no \
             people of this Organization (no Keycloak admin client, or no members group).\n\n",
        );
    }
    if !projects.is_empty() {
        text.push_str("Projects:\n\n");
        for (project, commit) in projects {
            text.push_str(&format!("- `{project}` at `{commit}`\n"));
        }
        text.push('\n');
    }
    text.push_str(README_LEFT_OUT);
    text
}

fn zip_error(err: impl std::fmt::Display) -> ApiError {
    ApiError::Internal(format!("the backup archive failed: {err}"))
}

/// The configuration bundle of `organization`, as a zip, with the index it carries.
pub async fn build_bundle(
    state: &AppState,
    organization: &str,
    actor: &str,
) -> Result<(Vec<u8>, Index), ApiError> {
    let taken_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut items = Vec::new();
    let mut kinds = std::collections::BTreeSet::new();

    for envelope in organization_manifests(&state.mirror, organization) {
        let path = organization_path(&envelope);
        let text = serde_norway::to_string(&envelope).map_err(zip_error)?;
        writer.start_file(&path, options).map_err(zip_error)?;
        writer.write_all(text.as_bytes()).map_err(zip_error)?;
        kinds.insert(envelope.kind.clone());
        items.push(IndexItem {
            kind: envelope.kind.clone(),
            id: envelope.metadata.name.clone(),
            path,
            revision: String::new(),
        });
    }
    let people = people_of(state, organization).await?;
    if let Some(people) = &people {
        let text = serde_norway::to_string(people).map_err(zip_error)?;
        writer.start_file(PEOPLE_PATH, options).map_err(zip_error)?;
        writer.write_all(text.as_bytes()).map_err(zip_error)?;
    }
    for kind in &kinds {
        if let Some(schema) = jc_core::registry::schema_of(kind) {
            let text = serde_json::to_string_pretty(&schema).map_err(zip_error)?;
            writer
                .start_file(format!("schemas/kinds/{kind}.schema.json"), options)
                .map_err(zip_error)?;
            writer.write_all(text.as_bytes()).map_err(zip_error)?;
        }
    }

    let mut projects = Vec::new();
    for project in projects_of(&state.mirror, organization) {
        let (bytes, commit) = crate::api::export::configuration_zip(state, &project, actor).await?;
        let mut inner = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(zip_error)?;
        for i in 0..inner.len() {
            let mut file = inner.by_index(i).map_err(zip_error)?;
            let mut content = Vec::new();
            file.read_to_end(&mut content).map_err(zip_error)?;
            // The export's paths are `projects/{project}/…` already, its index and README at
            // the root: those move into the project's folder.
            let name = file.name().map_err(zip_error)?.to_string();
            let path = if name.starts_with(&format!("projects/{project}/")) {
                name
            } else {
                format!("projects/{project}/{name}")
            };
            writer.start_file(&path, options).map_err(zip_error)?;
            writer.write_all(&content).map_err(zip_error)?;
            if let Some(envelope) = std::str::from_utf8(&content)
                .ok()
                .and_then(crate::store::envelope_of)
            {
                items.push(IndexItem {
                    kind: envelope.kind,
                    id: envelope.metadata.name,
                    path,
                    revision: commit.clone(),
                });
            }
        }
        projects.push((project, commit));
    }

    let index = Index {
        format_version: 1,
        organization: organization.to_owned(),
        taken_at,
        taken_by: actor.to_owned(),
        items,
    };
    writer
        .start_file("index.yaml", options)
        .map_err(zip_error)?;
    writer
        .write_all(
            serde_norway::to_string(&index)
                .map_err(zip_error)?
                .as_bytes(),
        )
        .map_err(zip_error)?;
    writer.start_file("README.md", options).map_err(zip_error)?;
    writer
        .write_all(readme(&index, &projects, people.is_some()).as_bytes())
        .map_err(zip_error)?;
    let cursor = writer.finish().map_err(zip_error)?;
    Ok((cursor.into_inner(), index))
}

/// Refuses a bucket whose versioning is not on: without it a delete or an overwrite loses the
/// bundle for good, which is what this backup exists to prevent.
pub async fn require_versioning(
    http: &reqwest::Client,
    bucket: &s3::Bucket,
) -> Result<(), ApiError> {
    let status = bucket
        .versioning(http)
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    if status == "Enabled" {
        return Ok(());
    }
    Err(ApiError::Conflict(format!(
        "versioning of bucket '{}' is {}: turn it on (GetBucketVersioning must answer Enabled), \
         so a delete leaves a delete marker and every earlier backup stays",
        bucket.bucket,
        if status.is_empty() { "off" } else { &status }
    )))
}

/// One run: versioning checked, the bundle built and uploaded, noncurrent versions past the
/// target's retention removed, the outcome logged. Answers the log entry's message.
pub async fn run(state: &AppState, target: &Target, actor: &str) -> Result<LogEntry, ApiError> {
    let outcome = upload(state, target, actor).await;
    let (ok, bytes, key, version, message, sha256) = match &outcome {
        Ok((bytes, key, version, pruned, sha256)) => (
            true,
            i64::try_from(*bytes).unwrap_or(i64::MAX),
            key.clone(),
            version.clone(),
            if *pruned > 0 {
                format!(
                    "uploaded; {pruned} version(s) past {} days removed",
                    target.keep_days
                )
            } else {
                "uploaded".to_owned()
            },
            sha256.clone(),
        ),
        Err(err) => (
            false,
            0,
            String::new(),
            String::new(),
            err.to_string(),
            String::new(),
        ),
    };
    if !ok {
        tracing::warn!(organization = %target.organization, %message, "organization backup failed");
    }
    state
        .org_backups
        .log(NewEntry {
            organization: &target.organization,
            kind: &target.kind,
            action: "run",
            actor,
            ok,
            bytes,
            object_key: &key,
            version_id: &version,
            message: &message,
            sha256: &sha256,
        })
        .await?;
    let entries = state
        .org_backups
        .entries(&target.organization, &target.kind, 1)
        .await?;
    outcome?;
    entries
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::Internal("the run was not logged".into()))
}

async fn upload(
    state: &AppState,
    target: &Target,
    actor: &str,
) -> Result<(usize, String, String, usize, String), ApiError> {
    let http = &state.org_backup_egress.http;
    let bucket = bucket(state, target)?;
    require_versioning(http, &bucket).await?;
    let bytes = if target.kind == DATA {
        data::encrypted_archive(state, target, actor).await?
    } else {
        build_bundle(state, &target.organization, actor).await?.0
    };
    let size = bytes.len();
    let sha256 = sha256_hex(&bytes);
    // One key: every run is a new version of it, which is what "the versions" of the page lists.
    let key = target.object_key();
    let version = bucket
        .put(http, &key, bytes)
        .await
        .map_err(|e| ApiError::Unavailable(e.to_string()))?;
    let pruned = prune(http, &bucket, target).await.unwrap_or_else(|err| {
        tracing::warn!(organization = %target.organization, error = %err, "old backup versions not removed");
        0
    });
    Ok((size, key, version, pruned, sha256))
}

/// The SHA-256 of `bytes`, lower-case hex: what the run log binds a version's bytes by.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Removes the noncurrent versions (and delete markers) of the target's folder that became
/// noncurrent more than `keep_days` ago. The current version is never touched.
async fn prune(
    http: &reqwest::Client,
    bucket: &s3::Bucket,
    target: &Target,
) -> Result<usize, s3::S3Error> {
    let cutoff = (chrono::Utc::now() - chrono::Duration::days(i64::from(target.keep_days)))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let versions = bucket.versions(http, &target.folder()).await?;
    let mut removed = 0;
    for version in versions
        .iter()
        .filter(|v| !v.is_latest && v.last_modified < cutoff)
    {
        bucket
            .delete(http, &version.key, Some(&version.version_id))
            .await?;
        removed += 1;
    }
    Ok(removed)
}

/// Runs every due backup once a minute, on every replica; [`BackupStore::claim_due`] hands each
/// due target to one of them.
pub fn spawn_schedule(state: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            tick.tick().await;
            let due = match state.org_backups.claim_due().await {
                Ok(due) => due,
                Err(err) => {
                    tracing::warn!(error = %err, "organization backups not scheduled this minute");
                    continue;
                }
            };
            for target in due {
                if let Err(err) = run(&state, &target, "schedule").await {
                    tracing::warn!(organization = %target.organization, error = %err, "scheduled organization backup failed");
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn envelope(kind: &str, name: &str, spec: Value) -> ResourceEnvelope {
        ResourceEnvelope {
            api_version: crate::resource::API_VERSION.to_owned(),
            kind: kind.to_owned(),
            metadata: crate::resource::ObjectMeta::new(name, ORG_NAMESPACE),
            spec,
            status: None,
        }
    }

    #[test]
    fn an_organization_holds_its_own_manifests_and_nothing_of_another() {
        let mirror = Mirror::new();
        for (kind, name, spec) in [
            (
                "Organization",
                "ba",
                serde_json::json!({ "domain": "ba.sk", "installation": true }),
            ),
            (
                "Organization",
                "bb",
                serde_json::json!({ "domain": "bb.sk" }),
            ),
            (
                "Project",
                "pa",
                serde_json::json!({ "organizationRef": "ba" }),
            ),
            (
                "Project",
                "pb",
                serde_json::json!({ "organizationRef": { "name": "bb" } }),
            ),
            (
                "Portal",
                "portal-bb",
                serde_json::json!({ "organizationRef": "bb" }),
            ),
            ("Role", "shared", serde_json::json!({ "rules": [] })),
            (
                "RoleBinding",
                "bb-admin",
                serde_json::json!({ "subjects": [{ "user": "x@bb.sk" }], "role": "shared",
                                    "scope": { "organization": "bb" } }),
            ),
            (
                "RoleBinding",
                "pb-reader",
                serde_json::json!({ "subjects": [{ "user": "y@bb.sk" }], "role": "shared",
                                    "scope": { "project": "pb" } }),
            ),
            (
                "RoleBinding",
                "pa-reader",
                serde_json::json!({ "subjects": [{ "user": "z@ba.sk" }], "role": "shared",
                                    "scope": { "project": "pa" } }),
            ),
        ] {
            mirror.upsert(envelope(kind, name, spec));
        }
        let names: Vec<String> = organization_manifests(&mirror, "bb")
            .into_iter()
            .map(|e| format!("{}/{}", e.kind, e.metadata.name))
            .collect();
        assert_eq!(
            names,
            [
                "Organization/bb",
                "Portal/portal-bb",
                "Project/pb",
                "RoleBinding/bb-admin",
                "RoleBinding/pb-reader"
            ]
        );
        assert_eq!(projects_of(&mirror, "bb"), ["pb"]);
        assert_eq!(projects_of(&mirror, "ba"), ["pa"]);
    }

    #[test]
    fn code_and_secret_values_stay_out_of_the_organization_manifests() {
        let mirror = Mirror::new();
        mirror.upsert(envelope(
            "Organization",
            "bb",
            serde_json::json!({ "domain": "bb.sk" }),
        ));
        mirror.upsert(envelope(
            "Script",
            "convert",
            serde_json::json!({ "organizationRef": "bb", "versions": [{ "code": "x()" }] }),
        ));
        mirror.upsert(envelope(
            "CatalogueConnection",
            "ckan",
            serde_json::json!({ "organizationRef": "bb", "password": "hunter2" }),
        ));
        let manifests = organization_manifests(&mirror, "bb");
        let text = serde_json::to_string(&manifests).unwrap_or_default();
        assert!(!text.contains("x()") && !text.contains("Script"), "{text}");
        assert!(!text.contains("hunter2"), "{text}");
    }

    #[test]
    fn an_endpoint_inside_the_cluster_is_refused() {
        let egress = Egress {
            http: reqwest::Client::new(),
            public_only: true,
        };
        for refused in [
            "http://s3.example.com",
            "https://10.0.0.5",
            "https://127.0.0.1:9000",
            "https://minio.backup.svc",
            "https://minio",
        ] {
            let url = url::Url::parse(refused).unwrap_or_else(|_| panic!("{refused}"));
            assert!(egress.check(&url).is_err(), "{refused}");
        }
        let fine = url::Url::parse("https://s3.eu-central-1.amazonaws.com").ok();
        assert!(fine.is_some_and(|url| egress.check(&url).is_ok()));
    }

    /// A realm of the installation with the members groups of two Organizations, `hel` and
    /// `esp`, and one person in both. Keycloak answers each user with more than the three fields.
    async fn two_organizations() -> wiremock::MockServer {
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let realm = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/realms/jc/protocol/openid-connect/token"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "access_token": "t" })),
            )
            .mount(&realm)
            .await;
        Mock::given(method("GET"))
            .and(path("/admin/realms/jc/groups"))
            .and(query_param("search", "members"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                { "id": "g-esp", "path": "/esp", "subGroups": [
                    { "id": "g-esp-members", "path": "/esp/members" } ] },
                { "id": "g-hel", "path": "/hel", "subGroups": [
                    { "id": "g-hel-members", "path": "/hel/members" } ] }
            ])))
            .mount(&realm)
            .await;
        let user = |id: &str, name: &str| {
            serde_json::json!({
                "id": id, "username": name, "email": format!("{name}@example.org"),
                "firstName": "First", "lastName": "Last", "enabled": true,
                "totp": true, "credentials": [{ "type": "password", "value": "hunter2" }],
                "attributes": { "phone": ["+358 40 123"] }, "requiredActions": ["CONFIGURE_TOTP"]
            })
        };
        for (group, users) in [
            (
                "g-hel-members",
                vec![user("u-hel", "aino"), user("u-both", "kai")],
            ),
            (
                "g-esp-members",
                vec![user("u-both", "kai"), user("u-esp", "eero")],
            ),
        ] {
            Mock::given(method("GET"))
                .and(path(format!("/admin/realms/jc/groups/{group}/members")))
                .respond_with(ResponseTemplate::new(200).set_body_json(Value::Array(users)))
                .mount(&realm)
                .await;
        }
        realm
    }

    async fn people_yaml(realm: &wiremock::MockServer, organization: &str) -> Option<String> {
        let people = crate::people::People::new(
            &format!("{}/realms/jc", realm.uri()),
            "portal".into(),
            crate::auth::client_auth::ClientAuth::Secret("s".into()),
        )
        .expect("a realm URL");
        let admin = people.admin().await.expect("a token");
        let users = admin
            .members_of(organization)
            .await
            .expect("the realm answers")?;
        Some(serde_norway::to_string(&persons(users)).expect("yaml"))
    }

    #[tokio::test]
    async fn an_organization_bundle_holds_its_people_as_id_username_and_email_only() {
        let realm = two_organizations().await;
        let hel = people_yaml(&realm, "hel")
            .await
            .expect("hel has a members group");
        let esp = people_yaml(&realm, "esp")
            .await
            .expect("esp has a members group");

        // No person of the other Organization, by id, username or e-mail.
        for other in ["u-esp", "eero"] {
            assert!(!hel.contains(other), "{other} in hel's people: {hel}");
        }
        assert!(!esp.contains("u-hel") && !esp.contains("aino"), "{esp}");

        // Each entry holds the three allowed keys and nothing else.
        for text in [&hel, &esp] {
            let entries: Vec<serde_json::Map<String, Value>> =
                serde_norway::from_str(text).expect("a list of people");
            assert_eq!(entries.len(), 2, "{text}");
            for entry in &entries {
                let keys: std::collections::BTreeSet<&str> =
                    entry.keys().map(String::as_str).collect();
                assert_eq!(keys, ["email", "id", "username"].into(), "{text}");
            }
            for secret in [
                "hunter2",
                "totp",
                "credentials",
                "phone",
                "First",
                "CONFIGURE_TOTP",
            ] {
                assert!(!text.contains(secret), "{secret} in {text}");
            }
        }

        // A member of both Organizations is in both bundles under the same id.
        let kai = |text: &str| -> Vec<Person> {
            serde_norway::from_str::<Vec<Person>>(text)
                .expect("people")
                .into_iter()
                .filter(|p| p.username == "kai")
                .collect()
        };
        assert_eq!(kai(&hel), kai(&esp));
        assert_eq!(kai(&hel)[0].id, "u-both");

        // An Organization without a members group has no people here: no file, never everyone.
        assert_eq!(people_yaml(&realm, "turku").await, None);
    }
}
