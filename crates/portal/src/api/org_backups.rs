//! Administration > Backups: an Organization's configuration backup (T-7154) and data backup
//! (T-7168) to its own S3 bucket. The two have the same doors, under `.../configuration` and
//! `.../data`; the data target adds the two age recipients its archive is encrypted to. The Organization is the request's (its own Portal host's, else the installation's),
//! never a path parameter, and every door answers its administrators only (403 to anyone else).
//! The bucket's secret key goes in and never comes back out.

use std::collections::BTreeMap;
use std::io::{Read, Write};

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use jc_core::kinds::Verb;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;
use zeroize::Zeroizing;

use crate::auth::session::Identity;
use crate::auth::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::org_backup::{self, s3, LogEntry, NewEntry, Target, CONFIGURATION, DATA};
use crate::permissions::ORG_NAMESPACE;
use crate::resource::ResourceEnvelope;
use crate::state::AppState;

/// The request's Organization (its own Portal host's, else the installation's) when the caller
/// administers it: the right to approve and to delete RoleBindings held at that Organization, never
/// through a binding of another Organization or of one of its projects. Anyone else is refused.
fn administered(state: &AppState, identity: &Identity) -> Result<String, ApiError> {
    let name = crate::organization_host::organization()
        .or_else(|| crate::permissions::installation_organization(&state.mirror));
    let holds = |organization: &str, verb| {
        crate::permissions::holds_at_organization(
            state,
            identity,
            organization,
            "RoleBinding",
            verb,
            None,
        )
    };
    match name {
        Some(organization)
            if holds(&organization, Verb::Approve) && holds(&organization, Verb::Delete) =>
        {
            Ok(organization)
        }
        _ => Err(ApiError::Denied(
            "the Organization's backups are for its administrators, on Administration > Backups"
                .into(),
        )),
    }
}

fn invalid(field: &str, message: &str) -> ApiError {
    ApiError::Unprocessable {
        detail: format!("{field}: {message}"),
        errors: vec![format!("{field}: {message}")],
    }
}

/// What an administrator sets. `secretAccessKey` is written only: absent keeps the saved one.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TargetInput {
    /// `https://s3.eu-central-1.amazonaws.com`
    pub endpoint: String,
    /// `eu-central-1`
    pub region: String,
    /// `bb-backups`
    pub bucket: String,
    /// `joinedcontext/`; empty for the bucket's root.
    #[serde(default)]
    pub prefix: String,
    pub access_key_id: String,
    #[serde(default)]
    pub secret_access_key: Option<String>,
    pub enabled: bool,
    pub every_hours: i32,
    pub keep_days: i32,
    /// The data backup's working age recipient, e.g. `age1ql3z7hjy54pw3hyw…`.
    #[serde(default)]
    pub working_recipient: Option<String>,
    /// The data backup's recovery age recipient, kept offline by another person.
    #[serde(default)]
    pub recovery_recipient: Option<String>,
}

/// The recipients a target of `kind` keeps: two for the data backup, none for the configuration
/// backup (which holds no rows and is not encrypted). A field that is wrong is named.
fn recipients_of(input: &TargetInput, kind: &str) -> Result<Vec<String>, ApiError> {
    let given = [&input.working_recipient, &input.recovery_recipient];
    if kind != DATA {
        return match given.iter().position(|r| r.is_some()) {
            Some(0) => Err(invalid(
                "workingRecipient",
                "only the data backup is encrypted",
            )),
            Some(_) => Err(invalid(
                "recoveryRecipient",
                "only the data backup is encrypted",
            )),
            None => Ok(Vec::new()),
        };
    }
    let recipients: Vec<String> = given
        .iter()
        .map(|r| r.as_deref().unwrap_or_default().trim().to_owned())
        .collect();
    let parsed = org_backup::data::parse_recipients(&recipients).map_err(|message| {
        let (field, text) = message
            .split_once(": ")
            .unwrap_or(("workingRecipient", &message));
        invalid(field, text)
    })?;
    if recipients[0] == recipients[1] {
        return Err(invalid(
            "recoveryRecipient",
            "a second key, held apart from the working one, so losing one key loses no backup",
        ));
    }
    org_backup::data::test_encrypt(&parsed).map_err(|e| {
        invalid(
            "workingRecipient",
            &format!("a test encryption failed: {e}"),
        )
    })?;
    Ok(recipients)
}

fn well_formed(input: &TargetInput) -> Result<url::Url, ApiError> {
    let endpoint = url::Url::parse(input.endpoint.trim())
        .ok()
        .filter(|url| {
            url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
        })
        .ok_or_else(|| {
            invalid(
                "endpoint",
                "an https address of the S3 service, e.g. https://s3.eu-central-1.amazonaws.com",
            )
        })?;
    let region_ok = (1..=32).contains(&input.region.len())
        && input
            .region
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if !region_ok {
        return Err(invalid(
            "region",
            "lower-case letters, digits and '-', e.g. eu-central-1",
        ));
    }
    let bucket_ok = (3..=63).contains(&input.bucket.len())
        && input
            .bucket
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.');
    if !bucket_ok {
        return Err(invalid(
            "bucket",
            "3 to 63 lower-case letters, digits, '.' and '-', e.g. bb-backups",
        ));
    }
    let prefix_ok = input.prefix.len() <= 200
        && (input.prefix.is_empty() || input.prefix.ends_with('/'))
        && !input.prefix.starts_with('/')
        && !input.prefix.split('/').any(|s| s == "..")
        && input
            .prefix
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./".contains(&b));
    if !prefix_ok {
        return Err(invalid(
            "prefix",
            "a folder ending in '/', letters, digits, '-', '_', '.', e.g. joinedcontext/",
        ));
    }
    let key_ok = (1..=128).contains(&input.access_key_id.len())
        && input.access_key_id.bytes().all(|b| b.is_ascii_graphic());
    if !key_ok {
        return Err(invalid(
            "accessKeyId",
            "the access key id, e.g. AKIAIOSFODNN7EXAMPLE",
        ));
    }
    if let Some(secret) = &input.secret_access_key {
        if !(1..=256).contains(&secret.len()) || !secret.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(invalid(
                "secretAccessKey",
                "the secret access key the bucket's owner issued",
            ));
        }
    }
    if !org_backup::EVERY_HOURS.contains(&input.every_hours) {
        return Err(invalid(
            "everyHours",
            "1 to 672 hours between runs, e.g. 24",
        ));
    }
    if !org_backup::KEEP_DAYS.contains(&input.keep_days) {
        return Err(invalid(
            "keepDays",
            "1 to 3650 days an earlier version is kept, e.g. 90",
        ));
    }
    Ok(endpoint)
}

/// The page's state: the target (never its secret), the newest acts on it.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BackupOverview {
    pub target: Option<Target>,
    /// The fingerprint of each of the target's recipients, in their order.
    pub fingerprints: Vec<String>,
    pub log: Vec<LogEntry>,
}

#[utoipa::path(
    get,
    path = "/api/v1/organization/backups/configuration",
    operation_id = "organization_backup_overview",
    summary = "Read Configuration Backup",
    description = "The Organization's configuration backup target (never its secret key) and the newest runs, deletes and restores (T-7154).",
    tag = "system",
    responses(
        (status = 200, description = "Target and log", body = BackupOverview),
        (status = 403, description = "Not an administrator of the request's Organization", body = ProblemDetails),
    )
)]
pub async fn overview(
    user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<BackupOverview>, ApiError> {
    overview_of(&state, &user, CONFIGURATION).await
}

async fn overview_of(
    state: &AppState,
    user: &CurrentUser,
    kind: &str,
) -> Result<Json<BackupOverview>, ApiError> {
    let organization = administered(state, &user.0.identity)?;
    let target = state.org_backups.target(&organization, kind).await?;
    Ok(Json(BackupOverview {
        fingerprints: target
            .iter()
            .flat_map(|t| t.recipients.iter())
            .map(|r| org_backup::data::fingerprint(r))
            .collect(),
        target,
        log: state.org_backups.entries(&organization, kind, 20).await?,
    }))
}

#[utoipa::path(
    put,
    path = "/api/v1/organization/backups/configuration",
    operation_id = "organization_backup_save",
    summary = "Set Configuration Backup Target",
    description = "Saves the bucket, schedule and retention. The bucket must answer GetBucketVersioning with Enabled; the secret key is sealed and never returned (T-7154).",
    tag = "system",
    request_body(
        content = TargetInput,
        content_type = "application/json",
        example = json!({ "endpoint": "https://s3.eu-central-1.amazonaws.com", "region": "eu-central-1", "bucket": "bb-backups", "prefix": "joinedcontext/", "accessKeyId": "backup-writer", "enabled": true, "everyHours": 24, "keepDays": 30 })
    ),
    responses(
        (status = 200, description = "Saved", body = Target),
        (status = 403, description = "Not an administrator of the Organization", body = ProblemDetails),
        (status = 409, description = "The bucket's versioning is not on", body = ProblemDetails),
        (status = 422, description = "A field is not valid; `errors` names it", body = ProblemDetails),
        (status = 503, description = "No age key to seal the secret, or the bucket cannot be reached", body = ProblemDetails),
    )
)]
pub async fn save(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(input): Json<TargetInput>,
) -> Result<Json<Target>, ApiError> {
    save_of(&state, &user, input, CONFIGURATION).await
}

async fn save_of(
    state: &AppState,
    user: &CurrentUser,
    input: TargetInput,
    kind: &str,
) -> Result<Json<Target>, ApiError> {
    let identity = &user.0.identity;
    let organization = administered(state, identity)?;
    // A pending restore was checked against this target's bytes and keys: they stay until a
    // second administrator decides it.
    if kind == DATA && org_backup::restore::any_pending(state, &organization).await? {
        return Err(ApiError::Conflict(
            "a data restore request is pending: approve or decline it under Restore requests \
             before changing the data backup's bucket or keys"
                .into(),
        ));
    }
    let endpoint = well_formed(&input)?;
    let recipients = recipients_of(&input, kind)?;
    state
        .org_backup_egress
        .check(&endpoint)
        .map_err(|message| invalid("endpoint", &message))?;
    let saved = state.org_backups.target(&organization, kind).await?;
    let sealed_secret = match (&input.secret_access_key, &saved) {
        (Some(secret), _) => org_backup::seal(state, secret)?,
        (None, Some(saved)) => saved.sealed_secret.clone(),
        (None, None) => {
            return Err(invalid(
                "secretAccessKey",
                "the secret access key the bucket's owner issued",
            ))
        }
    };
    let target = Target {
        organization: organization.clone(),
        kind: kind.to_owned(),
        endpoint: endpoint.to_string().trim_end_matches('/').to_owned(),
        region: input.region,
        bucket: input.bucket,
        prefix: input.prefix,
        access_key_id: input.access_key_id,
        sealed_secret,
        enabled: input.enabled,
        every_hours: input.every_hours,
        keep_days: input.keep_days,
        recipients,
        updated_by: identity.username.clone(),
        updated_at: String::new(),
        last_run_at: None,
    };
    // The bucket is asked before anything is saved: an unversioned bucket is refused here, and
    // a wrong key or bucket name is said now rather than at 3 a.m.
    let bucket = org_backup::bucket(state, &target)?;
    org_backup::require_versioning(&state.org_backup_egress.http, &bucket).await?;
    state.org_backups.save(&target).await?;
    tracing::info!(organization = %organization, kind, by = %identity.username, "backup target saved");
    state
        .org_backups
        .target(&organization, kind)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::Internal("the saved target is gone".into()))
}

async fn saved_target(
    state: &AppState,
    organization: &str,
    kind: &str,
) -> Result<Target, ApiError> {
    state
        .org_backups
        .target(organization, kind)
        .await?
        .ok_or_else(|| {
            ApiError::Conflict(
                "no backup target is set: set the bucket on Administration > Backups first".into(),
            )
        })
}

#[utoipa::path(
    post,
    path = "/api/v1/organization/backups/configuration/run",
    operation_id = "organization_backup_run_now",
    summary = "Run Configuration Backup Now",
    description = "Builds the Organization's configuration bundle and uploads it as a new version, whether the schedule is on or off (T-7154).",
    tag = "system",
    responses(
        (status = 200, description = "The run's log entry", body = LogEntry),
        (status = 403, description = "Not an administrator of the Organization", body = ProblemDetails),
        (status = 409, description = "No target, or its bucket's versioning is off", body = ProblemDetails),
        (status = 503, description = "The bucket cannot be reached", body = ProblemDetails),
    )
)]
pub async fn run_now(
    user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<LogEntry>, ApiError> {
    run_of(&state, &user, CONFIGURATION).await
}

async fn run_of(
    state: &AppState,
    user: &CurrentUser,
    kind: &str,
) -> Result<Json<LogEntry>, ApiError> {
    let organization = administered(state, &user.0.identity)?;
    let target = saved_target(state, &organization, kind).await?;
    state.org_backups.ran_now(&organization, kind).await?;
    org_backup::run(state, &target, &user.0.identity.username)
        .await
        .map(Json)
}

/// The versions of the bundle in the bucket, and what the bucket says of itself.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct VersionList {
    /// `Enabled`, `Suspended` or empty (never versioned).
    pub versioning: String,
    pub object_lock: bool,
    pub versions: Vec<BundleVersion>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BundleVersion {
    pub version_id: String,
    /// RFC 3339.
    pub last_modified: String,
    pub size: u64,
    pub is_latest: bool,
    /// A soft delete: the bundle reads as gone until a version is restored.
    pub delete_marker: bool,
}

fn s3_unavailable(err: s3::S3Error) -> ApiError {
    ApiError::Unavailable(err.to_string())
}

#[utoipa::path(
    get,
    path = "/api/v1/organization/backups/configuration/versions",
    operation_id = "organization_backup_versions",
    summary = "List Configuration Backup Versions",
    description = "Every version and delete marker of the Organization's bundle in its bucket, newest first, with the bucket's versioning and Object Lock state (T-7154).",
    tag = "system",
    responses(
        (status = 200, description = "Versions", body = VersionList),
        (status = 403, description = "Not an administrator of the Organization", body = ProblemDetails),
        (status = 409, description = "No target", body = ProblemDetails),
        (status = 503, description = "The bucket cannot be reached", body = ProblemDetails),
    )
)]
pub async fn versions(
    user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<VersionList>, ApiError> {
    versions_of(&state, &user, CONFIGURATION).await
}

async fn versions_of(
    state: &AppState,
    user: &CurrentUser,
    kind: &str,
) -> Result<Json<VersionList>, ApiError> {
    let organization = administered(state, &user.0.identity)?;
    let target = saved_target(state, &organization, kind).await?;
    let http = &state.org_backup_egress.http;
    let bucket = org_backup::bucket(state, &target)?;
    let versioning = bucket.versioning(http).await.map_err(s3_unavailable)?;
    let object_lock = bucket.object_lock(http).await;
    let key = target.object_key();
    let versions = bucket
        .versions(http, &target.folder())
        .await
        .map_err(s3_unavailable)?
        .into_iter()
        .filter(|v| v.key == key)
        .map(|v| BundleVersion {
            version_id: v.version_id,
            last_modified: v.last_modified,
            size: v.size,
            is_latest: v.is_latest,
            delete_marker: v.delete_marker,
        })
        .collect();
    Ok(Json(VersionList {
        versioning,
        object_lock,
        versions,
    }))
}

fn valid_version(version: &str) -> Result<(), ApiError> {
    let ok = (1..=1024).contains(&version.len())
        && version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b));
    if ok {
        Ok(())
    } else {
        Err(ApiError::NotFound(format!("version '{version}' not found")))
    }
}

async fn fetch(state: &AppState, target: &Target, version: &str) -> Result<Vec<u8>, ApiError> {
    valid_version(version)?;
    let bucket = org_backup::bucket(state, target)?;
    bucket
        .get(&state.org_backup_egress.http, &target.object_key(), version)
        .await
        .map_err(|err| match err {
            s3::S3Error::Status { status: 404, .. } => {
                ApiError::NotFound(format!("version '{version}' not found"))
            }
            other => s3_unavailable(other),
        })
}

#[utoipa::path(
    get,
    path = "/api/v1/organization/backups/configuration/versions/{version}",
    operation_id = "organization_backup_download",
    summary = "Download Configuration Backup Version",
    description = "One version of the Organization's bundle, as the zip that was uploaded (T-7154).",
    tag = "system",
    params(
        ("version" = String, Path, description = "S3 version id"),
    ),
    responses(
        (status = 200, description = "The zip", content_type = "application/zip"),
        (status = 403, description = "Not an administrator of the Organization", body = ProblemDetails),
        (status = 404, description = "No such version", body = ProblemDetails),
    )
)]
pub async fn download(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(version): Path<String>,
) -> Result<Response, ApiError> {
    download_of(&state, &user, &version, CONFIGURATION).await
}

async fn download_of(
    state: &AppState,
    user: &CurrentUser,
    version: &str,
    kind: &str,
) -> Result<Response, ApiError> {
    let organization = administered(state, &user.0.identity)?;
    let target = saved_target(state, &organization, kind).await?;
    let bytes = fetch(state, &target, version).await?;
    let (content_type, file) = if kind == DATA {
        (
            "application/octet-stream",
            format!("{organization}-data.zip.age"),
        )
    } else {
        (
            "application/zip",
            format!("{organization}-configuration.zip"),
        )
    };
    Ok((
        [
            (header::CONTENT_TYPE, content_type.to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{file}\""),
            ),
        ],
        bytes,
    )
        .into_response())
}

#[utoipa::path(
    delete,
    path = "/api/v1/organization/backups/configuration/bundle",
    operation_id = "organization_backup_soft_delete",
    summary = "Delete Configuration Backup",
    description = "A soft delete: the bucket keeps every version behind a delete marker until retention removes them, and any version can still be restored (T-7154).",
    tag = "system",
    responses(
        (status = 204, description = "Delete marker written"),
        (status = 403, description = "Not an administrator of the Organization", body = ProblemDetails),
        (status = 409, description = "No target, or its versioning is off", body = ProblemDetails),
    )
)]
pub async fn soft_delete(
    user: CurrentUser,
    State(state): State<AppState>,
) -> Result<StatusCode, ApiError> {
    soft_delete_of(&state, &user, CONFIGURATION).await
}

async fn soft_delete_of(
    state: &AppState,
    user: &CurrentUser,
    kind: &str,
) -> Result<StatusCode, ApiError> {
    let identity = &user.0.identity;
    let organization = administered(state, identity)?;
    let target = saved_target(state, &organization, kind).await?;
    let http = &state.org_backup_egress.http;
    let bucket = org_backup::bucket(state, &target)?;
    // Without versioning a delete is for good: refused, as a save would be.
    org_backup::require_versioning(http, &bucket).await?;
    let key = target.object_key();
    let outcome = bucket.delete(http, &key, None).await;
    state
        .org_backups
        .log(NewEntry {
            organization: &organization,
            kind,
            action: "delete",
            actor: &identity.username,
            ok: outcome.is_ok(),
            bytes: 0,
            object_key: &key,
            version_id: "",
            sha256: "",
            message: &outcome
                .as_ref()
                .map(|()| "delete marker written".to_owned())
                .unwrap_or_else(|e| e.to_string()),
        })
        .await?;
    outcome.map_err(s3_unavailable)?;
    Ok(StatusCode::NO_CONTENT)
}

/// What restoring one version would change, per project and for the Organization's own
/// manifests: manifests that would be created or changed, and how many are as they are.
#[derive(Debug, Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RestorePlan {
    pub organization: String,
    pub version_id: String,
    /// When the bundle was taken.
    pub taken_at: String,
    pub groups: Vec<PlanGroup>,
    /// Manifests created or changed in all groups: 0 means the Organization is as it was then.
    pub changes: usize,
}

#[derive(Debug, Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PlanGroup {
    /// `organization`, `organization/people`, or a project's name.
    pub name: String,
    pub created: Vec<String>,
    pub changed: Vec<String>,
    pub unchanged: usize,
    /// The Change the restore proposed for this project, once applied; a second administrator
    /// approves it as any other Change.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change: Option<Value>,
    /// What this group needs done by hand: an Organization-level manifest is restored through
    /// the Organization's own repository, never by this door.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// The files of a bundle, by path.
fn unpack(bytes: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, ApiError> {
    let unreadable = || ApiError::Unprocessable {
        detail: "this version is not a configuration bundle".into(),
        errors: vec!["version: not a configuration bundle zip".into()],
    };
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|_| unreadable())?;
    let mut files = BTreeMap::new();
    let mut total = 0usize;
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(|_| unreadable())?;
        if !file.is_file() || file.enclosed_name().is_none() {
            continue;
        }
        let mut content = Vec::new();
        file.by_ref()
            .take(s3::MAX_OBJECT_BYTES as u64 + 1)
            .read_to_end(&mut content)
            .map_err(|_| unreadable())?;
        total += content.len();
        if total > s3::MAX_OBJECT_BYTES {
            return Err(ApiError::TooLarge("the bundle unpacks past 256 MiB".into()));
        }
        files.insert(file.name().map_err(|_| unreadable())?.to_string(), content);
    }
    Ok(files)
}

/// The spec a live manifest has as a bundle carries it, for a fair comparison.
fn comparable(mut envelope: ResourceEnvelope) -> Value {
    crate::api::export::strip_secret_values(&mut envelope.spec);
    envelope.spec
}

fn compare(state: &AppState, envelope: &ResourceEnvelope, namespace: &str, group: &mut PlanGroup) {
    let label = format!("{}/{}", envelope.kind, envelope.metadata.name);
    match state
        .mirror
        .get(namespace, &envelope.kind, &envelope.metadata.name)
    {
        None => group.created.push(label),
        Some(live) if comparable(live.clone()) != envelope.spec => group.changed.push(label),
        Some(_) => group.unchanged += 1,
    }
}

/// The project's files of a bundle, back at the paths its configuration export had.
fn project_archive(files: &BTreeMap<String, Vec<u8>>, project: &str) -> Result<Vec<u8>, ApiError> {
    let folder = format!("projects/{project}/");
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    let failed = |e: zip::result::ZipError| ApiError::Internal(format!("archive failed: {e}"));
    for (path, content) in files.iter().filter(|(p, _)| p.starts_with(&folder)) {
        let rest = &path[folder.len()..];
        let original = if rest == "bundle.yaml"
            || rest == crate::api::export::README_PATH
            || rest.starts_with(crate::api::export::SCHEMAS_DIR)
        {
            rest.to_owned()
        } else {
            path.clone()
        };
        writer.start_file(original, options).map_err(failed)?;
        writer
            .write_all(content)
            .map_err(|e| ApiError::Internal(format!("archive failed: {e}")))?;
    }
    Ok(writer.finish().map_err(failed)?.into_inner())
}

/// The plan group of the people a bundle names; the slash keeps it apart from any project name.
const PEOPLE: &str = "organization/people";

/// The bundle's people missing from the Organization today, by id: a restore never creates a
/// person, so they are listed to invite by hand (T-7164). `None` when the bundle has no people or
/// the Organization's people cannot be read here.
async fn people_plan(
    state: &AppState,
    organization: &str,
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<Option<PlanGroup>, ApiError> {
    let Some(raw) = files.get(org_backup::PEOPLE_PATH) else {
        return Ok(None);
    };
    let saved: Vec<org_backup::Person> =
        serde_norway::from_slice(raw).map_err(|_| ApiError::Unprocessable {
            detail: "the bundle's organization/people.yaml is not a list of people".into(),
            errors: vec!["version: unreadable organization/people.yaml".into()],
        })?;
    let Some(today) = org_backup::people_of(state, organization).await? else {
        return Ok(None);
    };
    Ok(Some(missing_people(&saved, &today)))
}

fn missing_people(saved: &[org_backup::Person], today: &[org_backup::Person]) -> PlanGroup {
    let (present, missing): (Vec<_>, Vec<_>) = saved
        .iter()
        .partition(|person| today.iter().any(|now| now.id == person.id));
    PlanGroup {
        name: PEOPLE.into(),
        created: missing
            .iter()
            .map(|person| match &person.email {
                Some(email) => format!("{} <{email}>", person.username),
                None => person.username.clone(),
            })
            .collect(),
        unchanged: present.len(),
        note: (!missing.is_empty())
            .then(|| "a restore never creates people: invite these by hand on People".into()),
        ..Default::default()
    }
}

async fn plan(
    state: &AppState,
    identity: &Identity,
    organization: &str,
    version: &str,
    apply: bool,
) -> Result<RestorePlan, ApiError> {
    let target = saved_target(state, organization, CONFIGURATION).await?;
    let bytes = fetch(state, &target, version).await?;
    let files = unpack(&bytes)?;
    let index: org_backup::Index = files
        .get("index.yaml")
        .and_then(|raw| serde_norway::from_slice(raw).ok())
        .ok_or_else(|| ApiError::Unprocessable {
            detail: "this version carries no index.yaml".into(),
            errors: vec!["version: no index.yaml".into()],
        })?;
    // A bundle restores into the Organization it was taken of and no other (T-7154).
    if index.organization != organization {
        return Err(ApiError::Unprocessable {
            detail: format!(
                "this bundle is of Organization '{}', not '{organization}': a backup restores only into its own Organization",
                index.organization
            ),
            errors: vec!["version: a bundle of another Organization".into()],
        });
    }
    let mine = org_backup::projects_of(&state.mirror, organization);
    let mut groups: BTreeMap<String, PlanGroup> = BTreeMap::new();
    for item in &index.items {
        let Some(envelope) = files
            .get(&item.path)
            .and_then(|raw| std::str::from_utf8(raw).ok())
            .and_then(crate::store::envelope_of)
        else {
            continue;
        };
        if envelope.kind == "Bundle" {
            continue;
        }
        if item.path.starts_with("organization/") {
            // An Organization-level manifest of the bundle must still be this Organization's.
            if !org_backup::belongs(&state.mirror, organization, &envelope)
                && state
                    .mirror
                    .get(ORG_NAMESPACE, &envelope.kind, &envelope.metadata.name)
                    .is_some()
            {
                return Err(ApiError::Unprocessable {
                    detail: format!(
                        "{}/{} belongs to another Organization now; the bundle is not restored",
                        envelope.kind, envelope.metadata.name
                    ),
                    errors: vec!["version: names another Organization's manifest".into()],
                });
            }
            let group = groups.entry("organization".into()).or_default();
            group.name = "organization".into();
            compare(state, &envelope, ORG_NAMESPACE, group);
            continue;
        }
        let Some(project) = item
            .path
            .strip_prefix("projects/")
            .and_then(|rest| rest.split('/').next())
        else {
            continue;
        };
        if !mine.iter().any(|p| p == project) {
            return Err(ApiError::Unprocessable {
                detail: format!(
                    "project '{project}' of the bundle is not a project of {organization} now; the bundle is not restored"
                ),
                errors: vec![format!("version: project '{project}' is not this Organization's")],
            });
        }
        // The import writes into the folder's project, so the plan reads that project's mirror
        // only: a namespace naming another project would read across Organizations (T-7154).
        if envelope
            .metadata
            .namespace
            .as_deref()
            .is_some_and(|namespace| namespace != project)
        {
            return Err(ApiError::Unprocessable {
                detail: format!(
                    "{} sits in project '{project}' of the bundle but names another namespace; the bundle is not restored",
                    item.path
                ),
                errors: vec![format!(
                    "version: {} names a namespace other than '{project}'",
                    item.path
                )],
            });
        }
        let group = groups.entry(project.to_owned()).or_default();
        group.name = project.to_owned();
        compare(state, &envelope, project, group);
    }
    if let Some(group) = people_plan(state, organization, &files).await? {
        groups.insert(PEOPLE.into(), group);
    }
    if let Some(group) = groups.get_mut("organization") {
        if !group.created.is_empty() || !group.changed.is_empty() {
            group.note = Some(
                "the Organization's own manifests are put back through the organization \
                 repository: download this version and propose its organization/ files there"
                    .into(),
            );
        }
    }
    if apply {
        for group in groups
            .values_mut()
            .filter(|g| g.name != "organization" && g.name != PEOPLE)
        {
            if group.created.is_empty() && group.changed.is_empty() {
                continue;
            }
            let archive = project_archive(&files, &group.name)?;
            let options = || crate::api::import::ImportOptions {
                conflict_policy: crate::api::import::ConflictPolicy::Replace,
                dry_run: true,
                ..Default::default()
            };
            // The import's own dry run first: the apply that follows is held to it.
            crate::api::import::import_bundle(state, identity, &group.name, &archive, options())
                .await?;
            let (_, change) = crate::api::import::import_bundle(
                state,
                identity,
                &group.name,
                &archive,
                crate::api::import::ImportOptions {
                    dry_run: false,
                    ..options()
                },
            )
            .await?;
            group.change = Some(change);
        }
    }
    let changes = groups
        .values()
        .filter(|g| g.name != PEOPLE)
        .map(|g| g.created.len() + g.changed.len())
        .sum();
    Ok(RestorePlan {
        organization: organization.to_owned(),
        version_id: version.to_owned(),
        taken_at: index.taken_at,
        groups: groups.into_values().collect(),
        changes,
    })
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RestoreInput {
    /// `false` (the default) plans only; `true` proposes one Change per project, which another
    /// administrator approves.
    #[serde(default)]
    pub apply: bool,
}

#[utoipa::path(
    post,
    path = "/api/v1/organization/backups/configuration/versions/{version}/restore",
    operation_id = "organization_backup_restore",
    summary = "Restore Configuration Backup Version",
    description = "Plans what restoring one version would change in this Organization (the dry run), or with `apply` proposes it as one Change per project for a second administrator to approve. A bundle of another Organization is refused (T-7154).",
    tag = "system",
    params(
        ("version" = String, Path, description = "S3 version id"),
    ),
    request_body(
        content = RestoreInput,
        content_type = "application/json",
        example = json!({ "apply": false })
    ),
    responses(
        (status = 200, description = "The plan, and the Changes once applied", body = RestorePlan),
        (status = 403, description = "Not an administrator of the Organization", body = ProblemDetails),
        (status = 404, description = "No such version", body = ProblemDetails),
        (status = 422, description = "A bundle of another Organization, or of a project it no longer has", body = ProblemDetails),
    )
)]
pub async fn restore(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(version): Path<String>,
    Json(input): Json<RestoreInput>,
) -> Result<Json<RestorePlan>, ApiError> {
    let identity = &user.0.identity;
    let organization = administered(&state, identity)?;
    let outcome = plan(&state, identity, &organization, &version, input.apply).await;
    let message = match &outcome {
        Ok(plan) => format!("{} change(s) planned", plan.changes),
        Err(err) => err.to_string(),
    };
    state
        .org_backups
        .log(NewEntry {
            organization: &organization,
            kind: CONFIGURATION,
            action: if input.apply {
                "restore"
            } else {
                "restore-plan"
            },
            actor: &identity.username,
            ok: outcome.is_ok(),
            bytes: 0,
            object_key: "",
            version_id: &version,
            sha256: "",
            message: &message,
        })
        .await?;
    outcome.map(Json)
}

#[utoipa::path(
    get,
    path = "/api/v1/organization/backups/data",
    summary = "Read Data Backup",
    description = "The Organization's data backup target (never its secret key), the fingerprints of its two age recipients and the newest runs, deletes and restores (T-7168).",
    tag = "system",
    responses(
        (status = 200, description = "Target, recipient fingerprints and log", body = BackupOverview),
        (status = 403, description = "Not an administrator of the request's Organization", body = ProblemDetails),
    )
)]
pub async fn data_overview(
    user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<BackupOverview>, ApiError> {
    overview_of(&state, &user, DATA).await
}

#[utoipa::path(
    put,
    path = "/api/v1/organization/backups/data",
    summary = "Set Data Backup Target",
    description = "Saves the bucket, schedule, retention and the working and recovery age recipients the archive is encrypted to (a test encryption runs first). The bucket must answer GetBucketVersioning with Enabled; the secret key is sealed and never returned (T-7168).",
    tag = "system",
    request_body(content = TargetInput, example = json!({ "endpoint": "https://s3.eu-central-1.amazonaws.com", "region": "eu-central-1", "bucket": "bb-backups", "prefix": "joinedcontext/", "accessKeyId": "AKIAIOSFODNN7EXAMPLE", "secretAccessKey": "the-key-the-bucket-owner-issued", "enabled": true, "everyHours": 24, "keepDays": 90, "workingRecipient": "age1mmaj3l7xsa7pjqmx…", "recoveryRecipient": "age1syrvza6xvtvsr9f0…" })),
    responses(
        (status = 200, description = "Saved", body = Target),
        (status = 403, description = "Not an administrator of the Organization", body = ProblemDetails),
        (status = 409, description = "The bucket's versioning is not on, or a restore request is pending", body = ProblemDetails),
        (status = 422, description = "A field is not valid, a recipient included; `errors` names it", body = ProblemDetails),
        (status = 503, description = "No age key to seal the secret, or the bucket cannot be reached", body = ProblemDetails),
    )
)]
pub async fn data_save(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(input): Json<TargetInput>,
) -> Result<Json<Target>, ApiError> {
    save_of(&state, &user, input, DATA).await
}

#[utoipa::path(
    post,
    path = "/api/v1/organization/backups/data/run",
    summary = "Run Data Backup Now",
    description = "Exports the Organization's database rows, encrypts the archive to its two recipients and uploads it as a new version, whether the schedule is on or off (T-7168).",
    tag = "system",
    responses(
        (status = 200, description = "The run's log entry", body = LogEntry),
        (status = 403, description = "Not an administrator of the Organization", body = ProblemDetails),
        (status = 409, description = "No target or recipients, or its bucket's versioning is off", body = ProblemDetails),
        (status = 503, description = "The bucket or a database cannot be reached", body = ProblemDetails),
    )
)]
pub async fn data_run(
    user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<LogEntry>, ApiError> {
    run_of(&state, &user, DATA).await
}

#[utoipa::path(
    get,
    path = "/api/v1/organization/backups/data/versions",
    summary = "List Data Backup Versions",
    description = "Every version and delete marker of the Organization's encrypted data archive in its bucket, newest first (T-7168).",
    tag = "system",
    responses(
        (status = 200, description = "Versions", body = VersionList),
        (status = 403, description = "Not an administrator of the Organization", body = ProblemDetails),
        (status = 409, description = "No target", body = ProblemDetails),
        (status = 503, description = "The bucket cannot be reached", body = ProblemDetails),
    )
)]
pub async fn data_versions(
    user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<VersionList>, ApiError> {
    versions_of(&state, &user, DATA).await
}

#[utoipa::path(
    get,
    path = "/api/v1/organization/backups/data/versions/{version}",
    summary = "Download Data Backup Version",
    description = "One version of the Organization's data archive, still age-encrypted: only the holder of the working or recovery identity can open it (T-7168).",
    tag = "system",
    params(
        ("version" = String, Path, description = "S3 version id"),
    ),
    responses(
        (status = 200, description = "The encrypted archive", content_type = "application/octet-stream"),
        (status = 403, description = "Not an administrator of the Organization", body = ProblemDetails),
        (status = 404, description = "No such version", body = ProblemDetails),
    )
)]
pub async fn data_download(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(version): Path<String>,
) -> Result<Response, ApiError> {
    download_of(&state, &user, &version, DATA).await
}

#[utoipa::path(
    delete,
    path = "/api/v1/organization/backups/data/archive",
    summary = "Delete Data Backup",
    description = "A soft delete: the bucket keeps every version behind a delete marker until retention removes them (T-7168).",
    tag = "system",
    responses(
        (status = 204, description = "Delete marker written"),
        (status = 403, description = "Not an administrator of the Organization", body = ProblemDetails),
        (status = 409, description = "No target, or its versioning is off", body = ProblemDetails),
    )
)]
pub async fn data_soft_delete(
    user: CurrentUser,
    State(state): State<AppState>,
) -> Result<StatusCode, ApiError> {
    soft_delete_of(&state, &user, DATA).await
}

/// The identity that opens a data archive, for one request. Never stored, logged or audited;
/// `Debug` is not derived so no log line can print it.
#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataRestoreInput {
    /// The working or the recovery age identity, one line, e.g. `AGE-SECRET-KEY-1… (the line in your key file)`.
    pub identity: String,
    /// `false` (the default) checks only; `true` also asks a second administrator to approve.
    #[serde(default)]
    pub request: bool,
}

/// The identity again, from the administrator who approves.
#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataApproveInput {
    /// The working or the recovery age identity, e.g. `AGE-SECRET-KEY-1… (the line in your key file)`.
    pub identity: String,
}

/// A dry run, and the request when one was asked for.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DataRestoreAnswer {
    pub plan: org_backup::restore::DataPlan,
    pub request: Option<org_backup::restore::DataRestoreRequest>,
}

/// Fetches `version` of the Organization's data archive and opens it with `identity`, after the
/// run log says this Organization's own backup wrote it and uploaded these very bytes; answers
/// their SHA-256 beside the archive.
async fn opened(
    state: &AppState,
    target: &Target,
    version: &str,
    identity: String,
) -> Result<(org_backup::restore::Opened, String), ApiError> {
    let identity = org_backup::restore::identity_of(&Zeroizing::new(identity))?;
    valid_version(version)?;
    let logged = org_backup::restore::taken_by(state, &target.organization, version).await?;
    let sealed = fetch(state, target, version).await?;
    let sha256 = org_backup::restore::same_bytes(&sealed, &logged)?;
    let archive = org_backup::restore::open(
        sealed,
        identity,
        &target.organization,
        org_backup::restore::CAPS,
    )
    .await?;
    Ok((archive, sha256))
}

async fn audit(
    state: &AppState,
    organization: &str,
    action: &str,
    actor: &str,
    version: &str,
    outcome: Result<String, &ApiError>,
) -> Result<(), ApiError> {
    let (ok, message) = match outcome {
        Ok(message) => (true, message),
        Err(err) => (false, err.to_string()),
    };
    state
        .org_backups
        .log(NewEntry {
            organization,
            kind: DATA,
            action,
            actor,
            ok,
            bytes: 0,
            object_key: "",
            version_id: version,
            message: &message,
            sha256: "",
        })
        .await
}

#[utoipa::path(
    post,
    path = "/api/v1/organization/backups/data/versions/{version}/restore",
    summary = "Check Data Backup Restore",
    description = "Opens one version of the Organization's data archive with the identity given for this request only, and answers what restoring it would create, update, leave and refuse per table (the dry run). With `request` it also asks a second administrator to approve. Only a version this Organization's own backup wrote is opened (T-7171).",
    tag = "system",
    params(
        ("version" = String, Path, description = "S3 version id"),
    ),
    request_body(content = DataRestoreInput, example = json!({ "identity": "AGE-SECRET-KEY-1…", "request": false })),
    responses(
        (status = 200, description = "The dry run, and the request when asked for", body = DataRestoreAnswer),
        (status = 403, description = "Not an administrator of the Organization", body = ProblemDetails),
        (status = 404, description = "No such version", body = ProblemDetails),
        (status = 409, description = "No target, or no database to restore into", body = ProblemDetails),
        (status = 422, description = "Another Organization's version, or an identity that does not open it; `errors` names the field", body = ProblemDetails),
    )
)]
pub async fn data_restore(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(version): Path<String>,
    Json(input): Json<DataRestoreInput>,
) -> Result<Json<DataRestoreAnswer>, ApiError> {
    let actor = &user.0.identity.username;
    let organization = administered(&state, &user.0.identity)?;
    let target = saved_target(&state, &organization, DATA).await?;
    let outcome = async {
        let (mut archive, sha256) = opened(&state, &target, &version, input.identity).await?;
        org_backup::restore::run(&state, &target, &version, &mut archive, None)
            .await
            .map(|plan| (plan, sha256))
    }
    .await;
    audit(
        &state,
        &organization,
        "restore-plan",
        actor,
        &version,
        outcome
            .as_ref()
            .map(|(plan, _)| format!("{} row(s) would be written", plan.writes())),
    )
    .await?;
    let (plan, sha256) = outcome?;
    if !input.request {
        return Ok(Json(DataRestoreAnswer {
            plan,
            request: None,
        }));
    }
    let request =
        org_backup::restore::request(&state, &organization, &user.0.identity, &plan, &sha256)
            .await?;
    audit(
        &state,
        &organization,
        "restore-request",
        actor,
        &version,
        Ok(format!(
            "restore {} asked; a second administrator approves it",
            request.id
        )),
    )
    .await?;
    Ok(Json(DataRestoreAnswer {
        plan,
        request: Some(request),
    }))
}

/// The identity that opens the archive, to list its project folders.
#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataProjectsInput {
    /// The working or the recovery age identity, e.g. `AGE-SECRET-KEY-1… (the line in your key file)`.
    pub identity: String,
}

/// One project folder restored into a new project.
#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataCopyInput {
    /// The working or the recovery age identity, e.g. `AGE-SECRET-KEY-1… (the line in your key file)`.
    pub identity: String,
    /// The source project's id, one of the version's project list, e.g. `0b9d6c1e-6f43-4a8e-9a51-3c1f4e2d7a10`.
    pub project: String,
    /// The new project's name, a DNS label no project holds, e.g. `air-quality-restored`.
    pub name: String,
    /// `false` (the default) checks only; `true` writes the new project.
    #[serde(default)]
    pub apply: bool,
}

#[utoipa::path(
    post,
    path = "/api/v1/organization/backups/data/versions/{version}/projects",
    summary = "List Projects of a Data Backup",
    description = "Opens one version of the Organization's data archive with the identity given for this request only and lists its project folders the caller may restore into a new project: live projects of the Organization they may read. Projects deleted since the backup are named apart and never restored (T-7208, TEN-16).",
    tag = "system",
    params(("version" = String, Path, description = "S3 version id")),
    request_body(content = DataProjectsInput, example = json!({ "identity": "AGE-SECRET-KEY-1…" })),
    responses(
        (status = 200, description = "The folders the caller may restore", body = org_backup::copy::ArchivedProjects),
        (status = 403, description = "Not an administrator of the Organization", body = ProblemDetails),
        (status = 404, description = "No such version", body = ProblemDetails),
        (status = 409, description = "No target, or no database", body = ProblemDetails),
        (status = 422, description = "Another Organization's version, or an identity that does not open it; `errors` names the field", body = ProblemDetails),
    )
)]
pub async fn data_projects(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(version): Path<String>,
    Json(input): Json<DataProjectsInput>,
) -> Result<Json<org_backup::copy::ArchivedProjects>, ApiError> {
    let organization = administered(&state, &user.0.identity)?;
    let target = saved_target(&state, &organization, DATA).await?;
    let (archive, _) = opened(&state, &target, &version, input.identity).await?;
    org_backup::copy::projects(&state, &user.0.identity, &organization, &archive)
        .await
        .map(Json)
}

#[utoipa::path(
    post,
    path = "/api/v1/organization/backups/data/versions/{version}/copy",
    summary = "Restore a Project into a New Project",
    description = "Restores one project's folder of the Organization's data archive into a new project: a new project id whose parent is the source, the folder's rows under the new name with new row ids, the source's approved erasures applied again. The source project is never written. `apply` false checks only. A project deleted since the backup is refused (TEN-16); a project the caller may not read answers 404 like an id the archive does not hold (T-7208).",
    tag = "system",
    params(("version" = String, Path, description = "S3 version id")),
    request_body(content = DataCopyInput, example = json!({ "identity": "AGE-SECRET-KEY-1…", "project": "0b9d6c1e-6f43-4a8e-9a51-3c1f4e2d7a10", "name": "air-quality-restored", "apply": false })),
    responses(
        (status = 200, description = "What the copy writes, or wrote", body = org_backup::copy::CopyPlan),
        (status = 403, description = "Not an administrator of the Organization", body = ProblemDetails),
        (status = 404, description = "No such version, or no project by that id the caller may read", body = ProblemDetails),
        (status = 409, description = "No target, or no database", body = ProblemDetails),
        (status = 422, description = "A deleted source project, a name taken or not a DNS label, a format 1 archive; `errors` names the field", body = ProblemDetails),
    )
)]
pub async fn data_copy(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(version): Path<String>,
    Json(input): Json<DataCopyInput>,
) -> Result<Json<org_backup::copy::CopyPlan>, ApiError> {
    let actor = &user.0.identity.username;
    let organization = administered(&state, &user.0.identity)?;
    let target = saved_target(&state, &organization, DATA).await?;
    let outcome = async {
        let (mut archive, _) = opened(&state, &target, &version, input.identity).await?;
        let request = org_backup::copy::CopyRequest {
            organization: &organization,
            version: &version,
            source: &input.project,
            name: &input.name,
            apply: input.apply,
            by: actor,
        };
        org_backup::copy::copy(&state, &user.0.identity, &mut archive, &request).await
    }
    .await;
    audit(
        &state,
        &organization,
        if input.apply { "copy" } else { "copy-plan" },
        actor,
        &version,
        outcome.as_ref().map(|plan| {
            if input.apply {
                format!(
                    "project {} restored into new project {} ({}): {} row(s)",
                    plan.source_name,
                    plan.name,
                    plan.project_id,
                    plan.writes()
                )
            } else {
                format!(
                    "{} row(s) of project {} would be restored into {}",
                    plan.writes(),
                    plan.source_name,
                    plan.name
                )
            }
        }),
    )
    .await?;
    outcome.map(Json)
}

#[utoipa::path(
    get,
    path = "/api/v1/organization/backups/data/restores",
    summary = "List Data Restore Requests",
    description = "The Organization's newest data restore requests: pending ones wait for a second administrator (T-7171).",
    tag = "system",
    responses(
        (status = 200, description = "Requests, newest first", body = [org_backup::restore::DataRestoreRequest]),
        (status = 403, description = "Not an administrator of the Organization", body = ProblemDetails),
    )
)]
pub async fn data_restores(
    user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<Vec<org_backup::restore::DataRestoreRequest>>, ApiError> {
    let organization = administered(&state, &user.0.identity)?;
    org_backup::restore::requests(&state, &organization)
        .await
        .map(Json)
}

use org_backup::restore::no_request;

#[utoipa::path(
    post,
    path = "/api/v1/organization/backups/data/restores/{id}/approve",
    summary = "Approve Data Restore",
    description = "A second administrator approves a pending restore with the identity again: the rows are written in one transaction, re-keyed to the Organization, and its approved erasures applied again (T-7171).",
    tag = "system",
    params(
        ("id" = i64, Path, description = "Restore request id"),
    ),
    request_body(content = DataApproveInput, example = json!({ "identity": "AGE-SECRET-KEY-1…" })),
    responses(
        (status = 200, description = "The request, applied or failed with its reason", body = org_backup::restore::DataRestoreRequest),
        (status = 403, description = "Not an administrator, or the person who asked", body = ProblemDetails),
        (status = 404, description = "No such pending request", body = ProblemDetails),
        (status = 422, description = "An identity that does not open the version", body = ProblemDetails),
    )
)]
pub async fn data_restore_approve(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<DataApproveInput>,
) -> Result<Json<org_backup::restore::DataRestoreRequest>, ApiError> {
    let approver = &user.0.identity.username;
    let organization = administered(&state, &user.0.identity)?;
    let target = saved_target(&state, &organization, DATA).await?;
    let asked = org_backup::restore::pending(&state, &organization, id)
        .await?
        .ok_or_else(|| no_request(id))?;
    // Four eyes are two Keycloak subjects: a username can repeat across realms.
    if asked.requested_subject == user.0.identity.subject {
        return Err(ApiError::Denied(
            "a second administrator approves a restore: you asked for this one".into(),
        ));
    }
    // The key and the bytes are checked before anything is written, so a wrong key leaves the
    // request pending.
    let (mut archive, sha256) = opened(&state, &target, &asked.version_id, input.identity).await?;
    if sha256 != asked.sha256 {
        return Err(ApiError::Unprocessable {
            detail: "version: the object in the bucket is not the one this request was checked \
                     on: decline it and check the version again"
                .into(),
            errors: vec!["version: not the bytes this request was checked on".into()],
        });
    }
    let approval = org_backup::restore::Approval {
        id,
        by: approver,
        subject: &user.0.identity.subject,
    };
    let outcome = org_backup::restore::run(
        &state,
        &target,
        &asked.version_id,
        &mut archive,
        Some(&approval),
    )
    .await;
    let message = match &outcome {
        Ok(plan) => org_backup::restore::applied_message(plan),
        // Taken by another approval meanwhile: that one's outcome stands.
        Err(ApiError::NotFound(_)) => return Err(no_request(id)),
        Err(err) => {
            let message = err.to_string();
            // The rows were rolled back; the request ends as failed with the reason.
            org_backup::restore::decide(&state, &organization, id, approver, "failed", &message)
                .await?;
            tracing::warn!(organization = %organization, request = id, %message, "data restore failed");
            message
        }
    };
    audit(
        &state,
        &organization,
        "restore",
        approver,
        &asked.version_id,
        outcome.as_ref().map(|_| message.clone()),
    )
    .await?;
    org_backup::restore::requests(&state, &organization)
        .await?
        .into_iter()
        .find(|r| r.id == id)
        .map(Json)
        .ok_or_else(|| no_request(id))
}

#[utoipa::path(
    post,
    path = "/api/v1/organization/backups/data/restores/{id}/decline",
    summary = "Decline Data Restore",
    description = "Declines a pending restore; the person who asked may withdraw it the same way. Nothing is written (T-7171).",
    tag = "system",
    params(
        ("id" = i64, Path, description = "Restore request id"),
    ),
    responses(
        (status = 200, description = "The declined request", body = org_backup::restore::DataRestoreRequest),
        (status = 403, description = "Not an administrator of the Organization", body = ProblemDetails),
        (status = 404, description = "No such pending request", body = ProblemDetails),
    )
)]
pub async fn data_restore_decline(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<org_backup::restore::DataRestoreRequest>, ApiError> {
    let actor = &user.0.identity.username;
    let organization = administered(&state, &user.0.identity)?;
    let declined = org_backup::restore::decide(
        &state,
        &organization,
        id,
        actor,
        "declined",
        &format!("declined by {actor}"),
    )
    .await?
    .ok_or_else(|| no_request(id))?;
    audit(
        &state,
        &organization,
        "restore-decline",
        actor,
        &declined.version_id,
        Ok(format!("restore {id} declined")),
    )
    .await?;
    Ok(Json(declined))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/organization/backups/configuration",
            get(overview).put(save),
        )
        .route("/organization/backups/configuration/run", post(run_now))
        .route(
            "/organization/backups/configuration/versions",
            get(versions),
        )
        .route(
            "/organization/backups/configuration/versions/{version}",
            get(download),
        )
        .route(
            "/organization/backups/configuration/versions/{version}/restore",
            post(restore),
        )
        .route(
            "/organization/backups/configuration/bundle",
            delete(soft_delete),
        )
        .route(
            "/organization/backups/data",
            get(data_overview).put(data_save),
        )
        .route("/organization/backups/data/run", post(data_run))
        .route("/organization/backups/data/versions", get(data_versions))
        .route(
            "/organization/backups/data/versions/{version}",
            get(data_download),
        )
        .route(
            "/organization/backups/data/archive",
            delete(data_soft_delete),
        )
        .route(
            "/organization/backups/data/versions/{version}/restore",
            post(data_restore),
        )
        .route(
            "/organization/backups/data/versions/{version}/projects",
            post(data_projects),
        )
        .route(
            "/organization/backups/data/versions/{version}/copy",
            post(data_copy),
        )
        .route("/organization/backups/data/restores", get(data_restores))
        .route(
            "/organization/backups/data/restores/{id}/approve",
            post(data_restore_approve),
        )
        .route(
            "/organization/backups/data/restores/{id}/decline",
            post(data_restore_decline),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person(id: &str, username: &str, email: Option<&str>) -> org_backup::Person {
        org_backup::Person {
            id: id.into(),
            username: username.into(),
            email: email.map(Into::into),
        }
    }

    #[test]
    fn a_restore_lists_people_missing_today_to_invite_and_creates_none() {
        let saved = [
            person("u1", "aino", Some("aino@example.org")),
            person("u2", "kai", None),
            person("u3", "eero", Some("eero@example.org")),
        ];
        // u2 is there today under a new username: the same id is the same person.
        let today = [person("u2", "kai.k", None), person("u9", "new", None)];
        let group = missing_people(&saved, &today);
        assert_eq!(group.name, PEOPLE);
        assert_eq!(
            group.created,
            ["aino <aino@example.org>", "eero <eero@example.org>"]
        );
        assert!(group.changed.is_empty() && group.change.is_none());
        assert_eq!(group.unchanged, 1);
        assert!(group.note.is_some_and(|n| n.contains("by hand")));

        let all_there = missing_people(&saved[1..2], &today);
        assert!(all_there.created.is_empty() && all_there.note.is_none());
    }
}
