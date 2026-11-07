//! Workspace previews (CC-78, CC-81, PF-83; API/01 §22, Architecture/06 §7.2).
//!
//! A preview is the workspace's branch rendered by the loader with the prefix `ws-{name}-`.
//! On `dev` it runs inside the shared services: the Portal holds the render, cached by the
//! branch head, and lists every running preview on its internal listener; the gateway loads
//! that list beside `main` and serves the preview's Endpoints on minted slugs. Nothing runs
//! as a workload of its own, and every pipeline stays paused.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use utoipa::ToSchema;

use crate::auth::session::Identity;
use crate::error::ApiError;
use crate::ops::workspaces::{self, PreviewAccess, PreviewState, Workspace};
use crate::state::AppState;

/// At most this many previews run on the node at once (ADR-N-024 §10).
pub const MAX_ON_NODE: usize = 2;

/// What the loader puts in front of every organization-unique name of the preview.
pub fn prefix_of(workspace: &str) -> String {
    format!("ws-{workspace}-")
}

/// A fresh salt for a preview's slugs: 128 bits of the OS RNG in base64url, 22 characters, what
/// the gateway accepts as a salt (CC-92).
fn mint_salt() -> String {
    use argon2::password_hash::rand_core::{OsRng, RngCore};
    use base64::Engine as _;
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// The salt `workspace`'s preview mints its slugs with; empty for a preview started before
/// salts existed, which [`served`] leaves out.
fn salt_of(workspace: &Workspace) -> &str {
    workspace
        .preview
        .as_ref()
        .map_or("", |access| access.salt.as_str())
}

/// One Endpoint of a preview and where it answers.
#[derive(Debug, Clone, Serialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PreviewEndpoint {
    /// The Endpoint's name, as in the workspace.
    pub name: String,
    /// The slug minted for the preview; never the origin's.
    pub slug: String,
    pub url: String,
    /// The slug `main` serves the same Endpoint on, the source of a copy; absent for an
    /// Endpoint the workspace adds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_slug: Option<String>,
}

/// A workspace's preview as a person, an agent or an MCP client reads it.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub state: PreviewState,
    pub prefix: String,
    /// The Endpoints of the workspace's project in the preview; empty unless it runs.
    pub endpoints: Vec<PreviewEndpoint>,
    /// Every pipeline of the project, paused until a person starts it (PL-40).
    pub paused_pipelines: Vec<String>,
    /// Why the loader refused the render, when it did (CC-78).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// One running preview as the gateway reads it.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Served {
    pub prefix: String,
    /// The branch's manifest files by path: the organization's files and the project's own.
    pub files: BTreeMap<String, String>,
    /// The Keycloak user ids the gateway answers the preview for: the person who started it
    /// and whoever reviews a Change of its project (CC-91, ADR-N-061).
    pub readers: BTreeSet<String>,
    /// The salt the preview's slugs are minted with (CC-92).
    pub salt: String,
}

/// The list on the internal listener.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ServedList {
    pub items: Vec<Served>,
}

/// What a render found in the project: its Endpoints as `(name, slug)`, and its pipelines.
type Found = (Vec<(String, String)>, Vec<String>);

/// A render of one branch head.
#[derive(Debug)]
pub struct Render {
    files: BTreeMap<String, String>,
    outcome: Result<Found, String>,
}

/// The renders the Portal holds, one per workspace, keyed by the branch head it rendered and
/// the salt it minted the slugs with.
#[derive(Debug, Default)]
pub struct Renders(Mutex<HashMap<String, (String, Arc<Render>)>>);

impl Renders {
    fn get(&self, workspace: &str, head: &str) -> Option<Arc<Render>> {
        let held = self.0.lock().unwrap_or_else(|p| p.into_inner());
        held.get(workspace)
            .filter(|(revision, _)| revision == head)
            .map(|(_, render)| Arc::clone(render))
    }

    fn put(&self, workspace: &str, head: String, render: Arc<Render>) {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(workspace.to_owned(), (head, render));
    }

    pub(crate) fn forget(&self, workspace: &str) {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(workspace);
    }
}

/// The files under `root`, by their path relative to it.
fn files_under(root: &Path) -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                stack.push(path);
            } else if name == "bento.yaml" || jcctl::secrets::sops::is_encrypted_file(&name) {
                // A mapping the gateway never reads, and the repository's encrypted secrets,
                // which a preview's reader has no use for: neither leaves the Portal.
                continue;
            } else if let (Ok(relative), Ok(text)) =
                (path.strip_prefix(root), std::fs::read_to_string(&path))
            {
                files.insert(relative.to_string_lossy().into_owned(), text);
            }
        }
    }
    files
}

/// Renders the branch as it is now, or hands back the render of the same head.
///
/// Only the organization's own files and the workspace project's travel: the preview is the
/// project's, and another project's Endpoints get no preview copy. A render the loader
/// refuses is kept too, with its reason, so asking again does not stage the branch again.
///
/// `JC_ENVIRONMENT` names the overlay the branch is rendered with, the same one the reconciler
/// applies for this installation (CC-73); unset renders the manifests as they are written.
async fn render(state: &AppState, workspace: &Workspace) -> Result<Arc<Render>, ApiError> {
    let gitea = state
        .forge_for(&workspace.project)
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;
    let gitea: &crate::git::GiteaClient = &gitea;
    let head = match gitea.branch_head(&workspace.branch()).await {
        Ok(head) => format!("{head}\n{}", salt_of(workspace)),
        Err(crate::git::GitError::NotFound) => {
            return Err(ApiError::NotFound(format!(
                "the branch of workspace '{}' is gone",
                workspace.name
            )))
        }
        Err(err) => return Err(err.into()),
    };
    if let Some(render) = state.previews.get(&workspace.name, &head) {
        return Ok(render);
    }
    let revision = head.split('\n').next().unwrap_or_default();
    let staged = match state.mirror.repository_of(&workspace.project) {
        Some(repository) => assembled(state, workspace, revision, &repository).await?,
        None => {
            let scratch = crate::reconciler::staging::stage(gitea, revision)
                .await
                .map_err(|err| ApiError::Unavailable(format!("the branch did not stage: {err}")))?;
            if let Ok(projects) = std::fs::read_dir(scratch.path().join("projects")) {
                for project in projects.flatten() {
                    if project.file_name().to_string_lossy() != workspace.project {
                        let _ = std::fs::remove_dir_all(project.path());
                    }
                }
            }
            let root = scratch.path().to_path_buf();
            (vec![scratch], root)
        }
    };
    let root = staged.1.as_path();
    let prefix = prefix_of(&workspace.name);
    let environment = std::env::var("JC_ENVIRONMENT")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let namespace = format!("{prefix}{}", workspace.project);
    let outcome = jcctl::loader::Repository::load_preview_salted(
        root,
        environment.as_deref(),
        &prefix,
        salt_of(workspace),
    )
    .map(|repository| {
        let mut endpoints = Vec::new();
        let mut pipelines = Vec::new();
        for (id, loaded) in repository.iter() {
            if id.namespace.as_deref() != Some(namespace.as_str()) {
                continue;
            }
            match id.kind.as_str() {
                "Endpoint" => endpoints.push((
                    id.name.clone(),
                    loaded.manifest.spec["slug"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                )),
                "Pipeline" => pipelines.push(id.name.clone()),
                _ => {}
            }
        }
        (endpoints, pipelines)
    })
    .map_err(|err| err.to_string());
    let render = Arc::new(Render {
        files: files_under(root),
        outcome,
    });
    state
        .previews
        .put(&workspace.name, head, Arc::clone(&render));
    Ok(render)
}

/// Layout 2 (CC-86, CC-87): the organization at its default branch with this project's own
/// repository mounted at the workspace branch head, assembled as the mirror is; no other
/// project travels. The staged checkouts are held until the render is read.
async fn assembled(
    state: &AppState,
    workspace: &Workspace,
    head: &str,
    repository: &str,
) -> Result<(Vec<crate::reconciler::staging::Scratch>, std::path::PathBuf), ApiError> {
    use crate::reconciler::staging::{stage, stage_project, Scratch};
    let unstaged = |err: crate::reconciler::SyncError| {
        ApiError::Unavailable(format!("the branch did not stage: {err}"))
    };
    let organization = state
        .gitea
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;
    let main = organization.default_branch().await?;
    let main_head = organization.branch_head(&main).await?;
    let org = stage(organization, &main_head).await.map_err(unstaged)?;
    let project = stage_project(&organization.for_repository(repository), head)
        .await
        .map_err(unstaged)?;
    let into = Scratch::new(head).map_err(unstaged)?;
    let root = into.path().join("render");
    let checkouts = jcctl::assemble::Directories(std::collections::BTreeMap::from([(
        workspace.project.clone(),
        project.path().to_path_buf(),
    )]));
    // The other projects have no checkout here, so the assembly leaves them out.
    jcctl::assemble::assemble(org.path(), &checkouts, &root, None)
        .map_err(|err| ApiError::Unavailable(format!("the workspace does not assemble: {err}")))?;
    Ok((vec![org, project, into], root))
}

fn view(state: &AppState, workspace: &Workspace, render: Option<&Render>) -> Preview {
    let mut preview = Preview {
        state: workspace.preview_state,
        prefix: prefix_of(&workspace.name),
        endpoints: Vec::new(),
        paused_pipelines: Vec::new(),
        reason: None,
    };
    match render.map(|render| &render.outcome) {
        Some(Ok((endpoints, pipelines))) => {
            preview.endpoints = endpoints
                .iter()
                .map(|(name, slug)| PreviewEndpoint {
                    name: name.clone(),
                    slug: slug.clone(),
                    url: state
                        .config
                        .public_base_url
                        .join(&format!("/api/endpoint/{slug}"))
                        .map(String::from)
                        .unwrap_or_default(),
                    origin_slug: state
                        .mirror
                        .get(&workspace.project, "Endpoint", name)
                        .and_then(|endpoint| endpoint.spec["slug"].as_str().map(str::to_owned)),
                })
                .collect();
            preview.paused_pipelines = pipelines.clone();
        }
        Some(Err(reason)) => preview.reason = Some(reason.clone()),
        None => {}
    }
    preview
}

fn live(state: PreviewState) -> bool {
    matches!(state, PreviewState::Starting | PreviewState::Running)
}

/// What a caller is told when the node's slots are taken (T-2294, PF-59).
///
/// The node is shared by every project, so the previews holding it are often work this caller may
/// not see. The count is of all of them, because that is what full means; the names are only of the
/// ones this project's door would show them, and the rest are a number. A person whose own previews
/// hold the node still reads the sentence that tells them what to stop.
fn full_node(
    state: &AppState,
    identity: &Identity,
    project: &str,
    running: &[Workspace],
) -> String {
    let effective = crate::permissions::for_request(state, identity, project);
    let mut named: Vec<&str> = Vec::new();
    for other in running {
        if other.project == project && workspaces::may_see(state, &effective, other, identity) {
            named.push(other.name.as_str());
        }
    }
    let hidden = running.len() - named.len();
    match (named.is_empty(), hidden) {
        (true, _) => format!(
            "{MAX_ON_NODE} previews run on the node already, none of them in this project; \
             the node takes {MAX_ON_NODE} at a time, so try again once one stops"
        ),
        (false, 0) => format!(
            "{MAX_ON_NODE} previews run on the node already ({}); stop one first",
            named.join(", ")
        ),
        (false, hidden) => format!(
            "{MAX_ON_NODE} previews run on the node already ({} in this project, {hidden} \
             elsewhere); stop one of yours first",
            named.join(", ")
        ),
    }
}

/// Starts the preview: one per workspace, [`MAX_ON_NODE`] on the node, the owner only.
pub async fn start(
    identity: &Identity,
    state: &AppState,
    project: &str,
    name: &str,
) -> Result<Preview, ApiError> {
    let workspace = workspaces::owned(state, identity, project, name, "start a preview of").await?;
    if live(workspace.preview_state) {
        return Err(ApiError::Conflict(format!(
            "a preview of workspace '{name}' runs already"
        )));
    }
    let now = chrono::Utc::now();
    let running: Vec<Workspace> = state
        .workspaces
        .previewing()
        .await?
        .into_iter()
        .filter(|other| !other.expired(now))
        .collect();
    // ponytail: counted, then set; two starts in the same instant may both pass. A row lock
    // when the node takes more than two.
    if running.len() >= MAX_ON_NODE {
        return Err(ApiError::Conflict(full_node(
            state, identity, project, &running,
        )));
    }
    // A new salt per start: the slugs of a stopped preview never come back (CC-92).
    let access = PreviewAccess {
        salt: mint_salt(),
        author: identity.subject.clone(),
    };
    state.workspaces.start_preview(name, access.clone()).await?;
    let workspace = Workspace {
        preview: Some(access),
        ..workspace
    };
    let render = match render(state, &workspace).await {
        Ok(render) => render,
        Err(err) => {
            state
                .workspaces
                .set_preview_state(name, PreviewState::Error)
                .await?;
            return Err(err);
        }
    };
    let next = match &render.outcome {
        Ok(_) => PreviewState::Running,
        Err(_) => PreviewState::Error,
    };
    state.workspaces.set_preview_state(name, next).await?;
    let workspace = Workspace {
        preview_state: next,
        ..workspace
    };
    let preview = view(state, &workspace, Some(&render));
    match &preview.reason {
        Some(reason) => Err(ApiError::Conflict(format!(
            "the preview of workspace '{name}' does not render: {reason}"
        ))),
        None => Ok(preview),
    }
}

/// The preview as it is: its addresses while it runs, the loader's reason when it failed.
pub async fn get(
    identity: &Identity,
    state: &AppState,
    project: &str,
    name: &str,
) -> Result<Preview, ApiError> {
    let workspace = workspaces::visible(state, identity, project, name).await?;
    let render = match workspace.preview_state {
        // A forge that cannot be read is that problem, not a preview with no addresses (JC1-56);
        // the loader's own refusal still comes back as the preview's reason.
        PreviewState::Running | PreviewState::Error => Some(render(state, &workspace).await?),
        _ => None,
    };
    Ok(view(state, &workspace, render.as_deref()))
}

/// Stops the preview; the gateway drops its Endpoints on its next fetch. Stopping one that
/// does not run changes nothing.
pub async fn stop(
    identity: &Identity,
    state: &AppState,
    project: &str,
    name: &str,
) -> Result<(), ApiError> {
    let workspace =
        workspaces::owned(state, identity, project, name, "stop the preview of").await?;
    if matches!(
        workspace.preview_state,
        PreviewState::None | PreviewState::Stopped
    ) {
        return Ok(());
    }
    state
        .workspaces
        .set_preview_state(name, PreviewState::Stopped)
        .await?;
    state.previews.forget(name);
    Ok(())
}

/// Who reads a preview (CC-91): its author and every person a binding in force lets approve in
/// its project, the reviewers of its Change, by Keycloak user id, as a binding names them
/// (ADR-N-061, PF-126). Built here from the row and the mirror alone: nothing a request carries
/// adds a reader.
///
/// ponytail: a reviewer bound only through a Keycloak group or realm role is not listed (the
/// mirror cannot name its members); list them once the Portal resolves group members.
fn readers(
    state: &AppState,
    project: &str,
    author: String,
    now: chrono::DateTime<chrono::Utc>,
) -> BTreeSet<String> {
    let mut readers = crate::permissions::project_members(&state.mirror, project, now).approvers;
    readers.insert(author);
    readers
}

/// Every running preview that has not expired, rendered, for the gateway (CC-81: an expired
/// or discarded workspace is not in the list, so its Endpoints go on the next fetch).
pub async fn served(state: &AppState) -> Result<ServedList, ApiError> {
    let now = chrono::Utc::now();
    let mut items = Vec::new();
    for workspace in state.workspaces.previewing().await? {
        if workspace.expired(now) || workspace.preview_state != PreviewState::Running {
            continue;
        }
        let Some(access) = workspace.preview.clone() else {
            tracing::warn!(workspace = %workspace.name, "a running preview has no salt or author; start it again to serve it");
            continue;
        };
        match render(state, &workspace).await {
            Ok(render) if render.outcome.is_ok() => items.push(Served {
                prefix: prefix_of(&workspace.name),
                files: render.files.clone(),
                readers: readers(state, &workspace.project, access.author, now),
                salt: access.salt,
            }),
            Ok(_) => {}
            Err(err) => {
                tracing::warn!(workspace = %workspace.name, error = %err, "a running preview did not render")
            }
        }
    }
    Ok(ServedList { items })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------------------------------------
    // T-2128 `prefix_of` (CC-78, CC-81, PF-83)
    //
    // The contract: the prefix is `ws-`, the workspace's name, and a hyphen — nothing else, for every
    // name a workspace can have. It is put in front of the project to make the namespace a preview's
    // manifests are rendered under (line 175), so it has to stay a legal name part: a prefix that
    // could carry a slash, a dot or an upper-case letter would put a rendered manifest somewhere
    // nobody meant. A workspace name is a DNS-1123 label of at most 20 characters, refused at
    // `workspaces.rs:223` before a preview exists, which is what keeps that true.
    // ---------------------------------------------------------------------------------------------

    #[test]
    fn the_prefix_is_the_workspace_name_between_two_fences() {
        for name in [
            "a",
            "feature",
            "feat-42",
            "a-b-c",
            "x9",
            &"a".repeat(MAX_NAME_FOR_TEST),
        ] {
            let prefix = prefix_of(name);
            assert_eq!(prefix, format!("ws-{name}-"));
            // What is rendered under it is still a name the loader and the mirror accept.
            let rendered = format!("{prefix}ovzdusie");
            assert!(
                crate::resource::is_dns1123(&rendered),
                "{name} rendered {rendered}, which is not a name",
            );
            assert!(prefix.starts_with("ws-") && prefix.ends_with('-'));
            assert!(!prefix.contains('/') && !prefix.contains('.') && !prefix.contains(' '));
        }
    }

    /// PF-83: the names that could break the prefix are refused before a workspace exists, so the
    /// function never sees them. The case asserts the refusal, not a defensive branch in `prefix_of`:
    /// two places checking the same thing is how one of them rots.
    #[test]
    fn a_name_that_could_break_the_prefix_is_refused_before_a_workspace_has_one() {
        for forged in [
            "../../etc",
            "a/b",
            "a.b",
            "A",
            "feat_42",
            "-leading",
            "trailing-",
            "",
            " space",
            "ws-",
            &"a".repeat(MAX_NAME_FOR_TEST + 1),
        ] {
            let too_long = forged.len() > crate::ops::workspaces::MAX_NAME;
            assert!(
                too_long || !crate::resource::is_dns1123(forged),
                "{forged:?} is a name a workspace may be opened with, so `prefix_of` sees it",
            );
        }
    }

    /// The prefix is `ws-` + name + `-` and the namespace is that prefix + the project, so the two
    /// are concatenated with the same separator the names may contain: workspace `a` with project
    /// `b-c` and workspace `a-b` with project `c` both render `ws-a-b-c`. Nothing mixes today — a
    /// render stages one branch, deletes every other project's directory and is stored under the
    /// workspace's own name (lines 164-206) — and it is written down in /workspace/chyby.md rather
    /// than pinned as if it were wanted.
    #[test]
    fn the_namespace_of_a_preview_is_the_prefix_and_the_project() {
        assert_eq!(
            format!("{}{}", prefix_of("a"), "b-c"),
            format!("{}{}", prefix_of("a-b"), "c"),
        );
        assert_eq!(
            format!("{}{}", prefix_of("feat"), "ovzdusie"),
            "ws-feat-ovzdusie"
        );
    }

    /// `MAX_NAME` is the workspace name's limit; kept beside these cases so a change to it is read.
    const MAX_NAME_FOR_TEST: usize = crate::ops::workspaces::MAX_NAME;

    /// JC1-56: a running preview whose branch cannot be read answers the problem, never an
    /// empty preview that looks like one serving no Endpoint.
    #[tokio::test]
    async fn a_preview_whose_forge_cannot_be_read_answers_that_problem() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        state
            .workspaces
            .create(crate::ops::workspaces::Opening {
                name: "air",
                title: None,
                project: "helsinki",
                owner: "test-user",
                base_revision: "b",
                scope: crate::ops::workspaces::Scope::Project {},
                ttl_hours: 24,
            })
            .await
            .expect("opened");
        state
            .workspaces
            .set_preview_state("air", PreviewState::Running)
            .await
            .expect("running");
        let identity = Identity {
            client: None,
            subject: "sub-test-user".into(),
            username: "test-user".into(),
            email: None,
            name: None,
            roles: vec![state.config.bootstrap_admins.clone()],
            groups: vec![],
            second_factor: false,
        };
        match get(&identity, &state, "helsinki", "air").await {
            Err(ApiError::Unavailable(why)) => assert!(why.contains("forge"), "{why}"),
            other => panic!("expected the forge's problem, got {other:?}"),
        }
    }
}
