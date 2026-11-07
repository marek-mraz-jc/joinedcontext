//! The secret form's routes (CC-06, MF-24, T-5971): a person enters a credential here, never in a
//! chat, a tool argument or a manifest.
//!
//! `GET /api/v1/projects/{project}/secrets` lists the names and keys the repository's encrypted
//! files declare (never a value), so the form offers the existing ones and a plan can say which
//! `secretRef` has no value yet. `POST` encrypts what the person typed with SOPS + age into
//! `projects/{project}/secrets/{name}.enc.yaml` (see [`crate::secret_files`]) and opens a change
//! request a person approves; the plaintext lives in this request's memory only. Both doors are
//! for a person in a Portal session with `propose` on DataSource: a connector's, an agent's or a
//! service account's token is refused, so no credential ever travels through a model.

use std::collections::BTreeMap;

use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use jc_core::kinds::Verb;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;
use zeroize::Zeroizing;

use crate::auth::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::secret_files;
use crate::state::AppState;

/// The kind whose `propose` lets a person write a project's credentials: a DataSource is what
/// reads them (PL-15).
pub(crate) const KIND: &str = "DataSource";
/// Keys one secret holds at most.
const MAX_KEYS: usize = 10;
/// Bytes one value holds at most: a PEM private key fits.
const MAX_VALUE: usize = 16 * 1024;

/// A secret the repository declares: its name and keys, never a value.
#[derive(Debug, Serialize, ToSchema, PartialEq, Eq)]
pub struct StoredSecret {
    /// The name a `secretRef` gives, e.g. `counters-api`.
    pub name: String,
    /// Its keys, e.g. `["apiKey"]`; empty for a secret holding one value.
    pub keys: Vec<String>,
    /// The repository file that holds it.
    pub path: String,
}

/// The project's secrets, by name.
#[derive(Debug, Serialize, ToSchema)]
pub struct SecretList {
    pub items: Vec<StoredSecret>,
}

/// What the form sends: the whole secret, every key of it.
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(example = json!({ "name": "counters-api", "values": { "apiKey": "typed-in-the-form" } }))]
pub struct SecretWrite {
    /// The secret's name, letters, digits, `-` and `_`, e.g. `counters-api`.
    pub name: String,
    /// Each key with its value, e.g. `{"apiKey": "…"}`. A secret that exists already is replaced
    /// whole, so every key it keeps is sent again.
    pub values: BTreeMap<String, String>,
}

/// The values stay out of every `{:?}`.
impl std::fmt::Debug for SecretWrite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretWrite")
            .field("name", &self.name)
            .field("keys", &self.values.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// The names and keys of every encrypted file in the project's own secrets folder, by name: in
/// layout 1 the repository holds every project, and another project's secrets are not this
/// one's to list or to collide with, as the reconciler resolves only this folder (CC-06).
pub async fn stored(
    state: &AppState,
    project: &str,
) -> Result<BTreeMap<String, StoredSecret>, ApiError> {
    let gitea = state
        .forge_for(project)
        .ok_or_else(|| ApiError::Unavailable("this Portal has no forge".into()))?;
    let base = gitea.default_branch().await?;
    let folder = format!("projects/{project}/secrets/");
    let mut found = BTreeMap::new();
    for (path, _) in gitea.list_tree_blobs(&base).await? {
        if !path.starts_with(&folder)
            || !jcctl::secrets::sops::is_encrypted_file(path.rsplit('/').next().unwrap_or_default())
        {
            continue;
        }
        let Some(file) = gitea.get_file(&path, &base).await? else {
            continue;
        };
        for (name, keys) in secret_files::names(&file.content).unwrap_or_default() {
            found.insert(
                name.clone(),
                StoredSecret {
                    name,
                    keys,
                    path: path.clone(),
                },
            );
        }
    }
    Ok(found)
}

/// The `secretRef`s a manifest names, as (name, key): every `secretRef`-like member (`secretRef`,
/// `apiTokenRef`, `previousSecretRef`) and a DataSource's `spec.secrets[]`.
pub fn references(manifest: &Value) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    collect(manifest, &mut out);
    out.sort();
    out.dedup();
    out
}

fn collect(value: &Value, out: &mut Vec<(String, Option<String>)>) {
    let named = |v: &Value| {
        v["name"]
            .as_str()
            .map(|name| (name.to_owned(), v["key"].as_str().map(str::to_owned)))
    };
    match value {
        Value::Object(map) => {
            for (member, child) in map {
                let lower = member.to_ascii_lowercase();
                // `headerRef` is a DataSource's credential header (MF-35); the others name what
                // they hold (`secretRef`, `apiTokenRef`, `previousSecretRef`).
                let credential = lower == "headerref"
                    || lower
                        .strip_suffix("ref")
                        .is_some_and(crate::secret_scan::secret_name);
                if credential {
                    out.extend(named(child));
                } else if member == "secrets" {
                    out.extend(child.as_array().into_iter().flatten().filter_map(named));
                }
                collect(child, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|item| collect(item, out)),
        _ => {}
    }
}

/// The form's address for one reference, on this Portal.
pub fn form_url(state: &AppState, project: &str, name: &str, key: Option<&str>) -> String {
    let mut url = state.config.public_base_url.clone();
    url.set_path(&format!("/projects/{project}/secrets/new"));
    url.query_pairs_mut().append_pair("name", name);
    if let Some(key) = key {
        url.query_pairs_mut().append_pair("key", key);
    }
    url.to_string()
}

/// A person in a Portal session who may propose a DataSource here, or the refusal saying why.
fn authorize(
    state: &AppState,
    identity: &crate::auth::Identity,
    project: &str,
) -> Result<(), ApiError> {
    if identity.client.is_some() {
        return Err(ApiError::Denied(
            "a person enters a credential in the Portal's secret form; a connector's, an agent's or a service account's token never does (MF-24)".into(),
        ));
    }
    crate::permissions::for_request(state, identity, project).check(KIND, Verb::Propose, None)
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/secrets",
    summary = "List The Project's Secrets",
    description = "Names and keys of every secret the repository's encrypted files declare, never a value (CC-06, T-5971). Only a person in a Portal session with `propose` on DataSource; a token of a connector, an agent or a service account is refused.",
    tag = "access",
    params(("project" = String, Path, description = "Project name")),
    responses(
        (status = 200, description = "The secrets, by name", body = SecretList),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "A bearer token, or no `propose` on DataSource here", body = ProblemDetails),
        (status = 503, description = "No forge", body = ProblemDetails),
    )
)]
pub(crate) async fn list_secrets(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> Result<Json<SecretList>, ApiError> {
    authorize(&state, &user.0.identity, &project)?;
    Ok(Json(SecretList {
        items: stored(&state, &project).await?.into_values().collect(),
    }))
}

/// Every field the form refuses, named in one 422, before any read or write.
fn refuse_invalid(body: &SecretWrite) -> Result<(), ApiError> {
    let mut errors = Vec::new();
    if !secret_files::valid_name(&body.name) {
        errors.push("name: letters, digits, '-' and '_', 1 to 63, e.g. counters-api".to_owned());
    }
    if body.values.is_empty() || body.values.len() > MAX_KEYS {
        errors.push(format!(
            "values: 1 to {MAX_KEYS} keys, e.g. {{\"apiKey\": \"…\"}}"
        ));
    }
    for (key, value) in &body.values {
        if !secret_files::valid_name(key) {
            errors.push(format!(
                "values.{key}: a key is letters, digits, '-' and '_', e.g. apiKey"
            ));
        } else if value.trim().is_empty() || value.len() > MAX_VALUE {
            errors.push(format!("values.{key}: 1 to {MAX_VALUE} bytes"));
        }
    }
    if errors.is_empty() {
        return Ok(());
    }
    Err(ApiError::Unprocessable {
        detail: "the secret cannot be written as sent; nothing was kept".into(),
        errors,
    })
}

/// The Portal's own recipient first, then every recipient the replaced file had, once each,
/// so a replace never locks out a reader the old file admitted.
fn recipients(own: String, replaced: Option<&str>) -> Vec<String> {
    let mut recipients = vec![own];
    for kept in replaced
        .map(secret_files::recipients_of)
        .unwrap_or_default()
    {
        if !recipients.contains(&kept) {
            recipients.push(kept);
        }
    }
    recipients
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/secrets",
    summary = "Propose A Secret",
    description = "Encrypts the values with SOPS + age for the Portal's own recipient (and every recipient the replaced file had) into `projects/{project}/secrets/{name}.enc.yaml` and opens a change request a person approves (CC-06, T-5971). The values are never stored in plain text, logged or answered. A secret that exists replaces whole: send every key. Only a person in a Portal session with `propose` on DataSource; a token of a connector, an agent or a service account is refused.",
    tag = "access",
    params(("project" = String, Path, description = "Project name")),
    request_body = SecretWrite,
    responses(
        (status = 202, description = "The change request that adds the secret", body = crate::change::Change),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "A bearer token, or no `propose` on DataSource here", body = ProblemDetails),
        (status = 409, description = "Another file declares the name, or a change of it is open already", body = ProblemDetails),
        (status = 422, description = "A name, key or value the form refuses, each named in errors", body = ProblemDetails),
        (status = 503, description = "No forge, or this Portal holds no age identity", body = ProblemDetails),
    )
)]
pub(crate) async fn propose_secret(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
    Json(mut body): Json<SecretWrite>,
) -> Result<Response, ApiError> {
    let identity = &user.0.identity;
    authorize(&state, identity, &project)?;
    refuse_invalid(&body)?;

    let age_key_file = match state.config.pipeline_secrets.as_ref() {
        Some(crate::pipeline_secrets::Backend::Sops { age_key_file }) => age_key_file.clone(),
        _ => {
            return Err(ApiError::Unavailable(
                "this Portal resolves secrets without an age identity (JC_PORTAL_SOPS_AGE_KEY_FILE), so it cannot encrypt one; ask an administrator".into(),
            ))
        }
    };
    let own = secret_files::own_recipient(&age_key_file).map_err(ApiError::Unavailable)?;
    let path = secret_files::path_of(&project, &body.name);
    if let Some(held) = stored(&state, &project).await?.get(&body.name) {
        if held.path != path {
            return Err(ApiError::Conflict(format!(
                "secret '{}' is declared in {} already; change it there, or pick another name",
                body.name, held.path
            )));
        }
    }

    let gitea = state
        .forge_for(&project)
        .ok_or_else(|| ApiError::Unavailable("this Portal has no forge".into()))?;
    let base = gitea.default_branch().await?;
    let existing = gitea.get_file(&path, &base).await?;
    let recipients = recipients(own, existing.as_ref().map(|f| f.content.as_str()));
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let values: BTreeMap<String, Zeroizing<String>> = std::mem::take(&mut body.values)
        .into_iter()
        .map(|(key, value)| (key, Zeroizing::new(value)))
        .collect();
    let content = secret_files::encrypt(&body.name, &values, &recipients, &now)
        .map_err(ApiError::Internal)?;
    drop(values);

    let branch = format!("portal/secret-{project}-{}", body.name);
    let held = crate::api::mutate::turn(&gitea, &branch).await;
    if let Some(pending) =
        crate::api::mutate::open_change_on(&state, &gitea, &branch, &project).await?
    {
        return Err(ApiError::Conflict(format!(
            "a change of secret '{}' is open already: {}; approve or reject it first",
            body.name, pending.name
        )));
    }
    let branch = crate::api::mutate::create_or_reuse_branch(&gitea, &held, &base).await?;
    let (author_name, author_email) = crate::api::mutate::author_credentials(identity, &project);
    let verb = if existing.is_some() { "replace" } else { "add" };
    gitea
        .put_file(&crate::git::FileWrite {
            path: &path,
            branch: &branch,
            message: &format!("{verb} secret {}", body.name),
            content: &content,
            sha: existing.as_ref().map(|f| f.sha.as_str()),
            author: crate::git::Author {
                name: &author_name,
                email: &author_email,
            },
        })
        .await?;
    let pr_body = format!(
        "{verb}s the secret `{}` in `{path}`, encrypted with SOPS + age (CC-06). The values are \
         in no plain text anywhere; a `secretRef` with name `{}` reads them once this merges.",
        body.name, body.name
    );
    let pull = gitea
        .create_pull_request(
            &branch,
            &base,
            &format!("{verb} secret {}", body.name),
            &pr_body,
        )
        .await?;
    tracing::info!(%project, secret_name = %body.name, pull = pull.number, "secret proposed");
    let mut response = (
        StatusCode::ACCEPTED,
        Json(crate::change::Change::new(
            crate::api::changes::change_meta(&state, &gitea, pull.number, &project),
            crate::change::ChangeStatus::new(
                crate::change::Lane::Yellow,
                crate::change::ChangePhase::PendingApproval,
                crate::change::PlanSummary::new(
                    usize::from(existing.is_none()),
                    usize::from(existing.is_some()),
                    0,
                ),
            )
            .in_repository(&pull.repository)
            .proposed_through(&pull.body)
            .with_merge_request(pull.url),
        )),
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

pub fn router() -> axum::Router<AppState> {
    axum::Router::new().route(
        "/projects/{project}/secrets",
        axum::routing::get(list_secrets).post(propose_secret),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_reference_a_manifest_names_is_found_once() {
        let manifest = json!({
            "kind": "DataSource",
            "spec": {
                "http": { "url": "https://x.example/c.csv", "headers": { "X-Api-Key": "${COUNTERS_KEY}" } },
                "secrets": [{ "name": "counters-api", "key": "apiKey", "envVar": "COUNTERS_KEY" }],
                "authorization": { "header": "X-Api-Key", "headerRef": { "name": "counters-api", "key": "apiKey" } },
                "webhook": { "secretRef": { "name": "hook" } },
                "ckan": { "apiTokenRef": { "name": "ckan" } },
                "contextSpaceRef": { "name": "doprava" }
            }
        });
        assert_eq!(
            references(&manifest),
            vec![
                ("ckan".to_owned(), None),
                ("counters-api".to_owned(), Some("apiKey".to_owned())),
                ("hook".to_owned(), None),
            ]
        );
    }

    /// Project ovzdusie, where pia@hel.fi may propose a DataSource; no forge.
    fn granted() -> AppState {
        use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let org = crate::permissions::ORG_NAMESPACE;
        for (kind, spec) in [
            (
                "Role",
                json!({ "rules": [{ "kinds": ["DataSource"], "verbs": ["read", "propose"] }] }),
            ),
            (
                "RoleBinding",
                json!({ "subjects": [{ "user": "pia@hel.fi" }], "role": "proposer", "scope": { "project": "ovzdusie" } }),
            ),
        ] {
            state.mirror.upsert(ResourceEnvelope {
                api_version: API_VERSION.to_owned(),
                kind: kind.to_owned(),
                metadata: ObjectMeta::new("proposer", org),
                spec,
                status: None,
            });
        }
        state
    }

    fn pia(client: Option<&str>) -> CurrentUser {
        use crate::auth::session::{Identity, Session};
        CurrentUser(Session {
            identity: Identity {
                client: client.map(str::to_owned),
                subject: "sub-pia".into(),
                username: "pia".into(),
                email: Some("pia@hel.fi".into()),
                name: None,
                roles: Vec::new(),
                groups: Vec::new(),
                second_factor: false,
            },
            expires_at: 9_999_999_999,
            issued_at: 1000,
            signed_in_at: None,
            id_token: "dummy".into(),
            access_expires_at: 9_999_999_999,
            refresh_token: None,
        })
    }

    /// MF-24: the person's own grant passes in a session, and the same person's token obtained
    /// by a connector, an agent or a service account is refused on both doors before any read
    /// or write.
    #[tokio::test]
    async fn a_token_of_a_client_never_reaches_the_secret_doors() {
        let state = granted();
        assert!(authorize(&state, &pia(None).0.identity, "ovzdusie").is_ok());
        for client in ["mcp-local", "agent-runner", "svc-ingest"] {
            let listed = list_secrets(
                pia(Some(client)),
                State(state.clone()),
                Path("ovzdusie".into()),
            )
            .await;
            assert!(
                matches!(&listed, Err(ApiError::Denied(m)) if m.contains("MF-24")),
                "{client}: {:?}",
                listed.err()
            );
            let body = SecretWrite {
                name: "counters-api".into(),
                values: [("apiKey".to_owned(), "typed-by-a-connector".to_owned())].into(),
            };
            let proposed = propose_secret(
                pia(Some(client)),
                State(state.clone()),
                Path("ovzdusie".into()),
                Json(body),
            )
            .await;
            assert!(
                matches!(&proposed, Err(ApiError::Denied(m)) if m.contains("MF-24")),
                "{client}: {:?}",
                proposed.err()
            );
        }
    }

    /// Each refused field is named once in one 422; a valid form passes.
    #[test]
    fn the_form_names_every_field_it_refuses() {
        let form = |name: &str, values: &[(&str, &str)]| SecretWrite {
            name: name.into(),
            values: values
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
        };
        assert!(refuse_invalid(&form("counters-api", &[("apiKey", "s3cret")])).is_ok());
        let long = "x".repeat(MAX_VALUE + 1);
        let refused = refuse_invalid(&form(
            "bad name",
            &[("ok", "  "), ("bad key", "v"), ("big", long.as_str())],
        ));
        let Err(ApiError::Unprocessable { errors, .. }) = refused else {
            panic!("expected 422, got {refused:?}");
        };
        assert_eq!(
            errors
                .iter()
                .map(|e| e.split(':').next().unwrap_or_default())
                .collect::<Vec<_>>(),
            vec!["name", "values.bad key", "values.big", "values.ok"]
        );
        let Err(ApiError::Unprocessable { errors, .. }) = refuse_invalid(&form("empty", &[]))
        else {
            panic!("an empty form passed");
        };
        assert!(
            matches!(errors.as_slice(), [only] if only.starts_with("values: 1 to")),
            "{errors:?}"
        );
    }

    /// A replace keeps every reader of the old file once, the Portal's own recipient first.
    #[test]
    fn a_replace_keeps_the_old_recipients_once() {
        assert_eq!(recipients("age1own".into(), None), vec!["age1own"]);
        let old = "sops:\n  age:\n    - recipient: age1other\n    - recipient: age1own\n";
        assert_eq!(
            recipients("age1own".into(), Some(old)),
            vec!["age1own", "age1other"]
        );
    }
}
