#![forbid(unsafe_code)]
//! The configuration off the pod (T-6892): the Portal's `ConfigMirror` against two real shako
//! servers over TCP, one the forge of record and one the operator's remote. A merged Change
//! reaching the remote's `main`, a Change branch staying home, a remote that is down while a
//! merge goes through, and a remote that moved elsewhere all run through real git pushes.

use jc_router::{JcRouterBuilder, Mount, MountSpec};
use joinedcontext_portal::forge::config_mirror::ConfigMirror;
use joinedcontext_portal::forge::shako::{Author, FileWrite, MergeStyle, ShakoClient};
use shako_core::{
    Action, InstanceId, LocalAuthorizer, OrgId, OutboundGuard, OutboundPolicy, RoleBinding,
    ShakoSigningKey, StaticTokenBinding, Subject, TokenVerifier,
};
use shako_server::{ShakoState, router};
use shako_store_memory::MemoryStore;
use std::sync::Arc;
use std::time::Duration;

const OWNER: &str = "joinedcontext";
const FORGE_TOKEN: &str = "forge-token-0123456789";
/// The remote's token: the mirror pushes with it, and it must never show.
const REMOTE_TOKEN: &str = "remote-token-0123456789";

const AUTHOR: Author<'static> = Author {
    name: "Alice Steward",
    email: "alice@city.example",
};

struct World {
    _tmp: tempfile::TempDir,
    base: String,
    token: &'static str,
}

impl World {
    /// A shako on `listener`, whose one principal holds every right with `token`.
    async fn start(listener: tokio::net::TcpListener, token: &'static str) -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let org = OrgId::from_u128(10);
        let authz = Arc::new(LocalAuthorizer::new(vec![RoleBinding {
            org_id: org,
            principal: "jc-portal".into(),
            actions: vec![Action::Admin, Action::Write, Action::Approve, Action::Publish],
            project_id: None,
            repo: None,
        }]));
        let verifier = TokenVerifier::new(ShakoSigningKey::generate().expect("key"))
            .with_static_tokens(vec![StaticTokenBinding::new(
                token,
                Subject::authenticated("jc-portal", "jc-portal", vec![org], vec![]),
            )]);
        let guard = OutboundGuard::new(OutboundPolicy {
            allowed_hosts: Some(vec!["127.0.0.1".into()]),
            allow_loopback_for_tests: true,
            ..OutboundPolicy::default()
        });
        let state = ShakoState::new(
            InstanceId::from_u128(1),
            org,
            tmp.path().to_path_buf(),
            Arc::new(MemoryStore::new()),
            authz,
            verifier,
            guard,
        );
        state.register_owner_org(OWNER, org);
        let host = listener.local_addr().unwrap().to_string();
        let git_mount = Mount::new("shako", "http", host.clone(), "/git").unwrap();
        let jc = JcRouterBuilder::new("http", host.clone())
            .mount(
                MountSpec {
                    module: "shako".into(),
                    host: None,
                    path_prefix: Some("/git".into()),
                },
                router(state, git_mount),
            )
            .build()
            .expect("mount table");
        tokio::spawn(async move {
            axum::serve(listener, jc.into_axum_router()).await.unwrap();
        });
        World {
            _tmp: tmp,
            base: format!("http://{host}/git/"),
            token,
        }
    }

    async fn up(token: &'static str) -> Self {
        Self::start(tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(), token).await
    }

    fn client(&self, repo: &str) -> ShakoClient {
        let base = self.base.clone();
        let token = self.token;
        let repo = repo.to_owned();
        ShakoClient::from_env(move |key| match key {
            "JC_SHAKO_URL" => Some(base.clone()),
            "JC_SHAKO_OWNER" => Some(OWNER.into()),
            "JC_SHAKO_REPO" => Some(repo.clone()),
            "JC_SHAKO_TOKEN" => Some(token.into()),
            _ => None,
        })
        .expect("config")
        .expect("forge configured")
    }
}

fn token_file(dir: &tempfile::TempDir) -> std::path::PathBuf {
    let file = dir.path().join("token");
    std::fs::write(&file, REMOTE_TOKEN).expect("token file");
    file
}

/// What a Change does: a branch, a commit on it, a merge request merged into `main`.
async fn merge_change(forge: &ShakoClient, branch: &str, path: &str) {
    forge.create_branch(branch, "main").await.expect("branch");
    forge
        .put_file(&FileWrite {
            path,
            branch,
            message: "Add an Endpoint",
            content: "apiVersion: joinedcontext.com/v1\nkind: Endpoint\n",
            sha: None,
            author: AUTHOR,
        })
        .await
        .expect("commit");
    let pr = forge
        .create_pull_request(branch, "main", "Add an Endpoint", "")
        .await
        .expect("merge request");
    forge
        .merge(pr.number, MergeStyle::Merge, "Merge the Change", None, None)
        .await
        .expect("merged");
}

/// `refs/heads/*` of a remote repository, as `git ls-remote` reads it with the token.
fn heads(remote: &World, repo: &str) -> Vec<(String, String)> {
    let url = format!("{}{OWNER}/{repo}.git", remote.base);
    let out = std::process::Command::new("git")
        .args(["-c", &format!("http.extraHeader=Authorization: Bearer {}", remote.token)])
        .args(["ls-remote", "--heads", "--tags", &url])
        .output()
        .expect("git");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .map(|(sha, name)| (name.to_owned(), sha.to_owned()))
        .collect()
}

fn mirror_for(remote: &World, dir: &tempfile::TempDir) -> ConfigMirror {
    ConfigMirror::new(&format!("{}{OWNER}/off-{{repo}}.git", remote.base), token_file(dir))
        .expect("mirror")
        .with_timing(Duration::ZERO, Duration::ZERO)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_merged_change_is_pushed_to_the_mirror_and_nothing_but_main() {
    let tmp = tempfile::tempdir().unwrap();
    let forge_world = World::up(FORGE_TOKEN).await;
    let remote = World::up(REMOTE_TOKEN).await;
    let installation = forge_world.client("configuration");
    assert!(installation.ensure_repository("Configuration").await.unwrap());
    let project = forge_world.client("ovzdusie-config");
    assert!(project.ensure_repository("Project").await.unwrap());
    for repo in ["off-configuration", "off-ovzdusie-config"] {
        remote.client(repo).create_empty_repository("Off the pod").await.unwrap();
    }
    // Merged before the mirror was on: the first pass catches up.
    merge_change(&installation, "change/1", "endpoints/one.yaml").await;
    let mirror = mirror_for(&remote, &tmp);
    mirror.pass(&installation, std::slice::from_ref(&project)).await;
    assert!(
        mirror.statuses().iter().all(|s| s.last_error.is_empty()),
        "{:?}",
        mirror.statuses()
    );
    let main = installation.branch_head("main").await.unwrap();
    assert_eq!(heads(&remote, "off-configuration"), [("refs/heads/main".to_owned(), main)]);

    // Merged after: the forge pushes on the merge, before any pass.
    merge_change(&installation, "change/2", "endpoints/two.yaml").await;
    let main = installation.branch_head("main").await.unwrap();
    assert_eq!(heads(&remote, "off-configuration"), [("refs/heads/main".to_owned(), main)]);

    // An open Change's branch and its commits stay home.
    installation.create_branch("change/3", "main").await.unwrap();
    assert!(heads(&remote, "off-configuration").iter().all(|(name, _)| name == "refs/heads/main"));
    assert_eq!(heads(&remote, "off-ovzdusie-config").len(), 1);

    // The mirror's address carries no credential, and nothing the forge answers holds the token.
    let listed = installation.push_mirrors().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].branch, "main");
    assert!(!format!("{listed:?}{mirror:?}").contains(REMOTE_TOKEN));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_installation_repository_goes_only_to_the_operator_remote() {
    let tmp = tempfile::tempdir().unwrap();
    let forge_world = World::up(FORGE_TOKEN).await;
    let remote = World::up(REMOTE_TOKEN).await;
    let installation = forge_world.client("configuration");
    installation.ensure_repository("Configuration").await.unwrap();
    let project = forge_world.client("ovzdusie-config");
    project.ensure_repository("Project").await.unwrap();
    remote.client("off-configuration").create_empty_repository("x").await.unwrap();
    remote.client("off-ovzdusie-config").create_empty_repository("x").await.unwrap();
    // Someone added copies elsewhere: an Organization's own remote on the installation
    // repository, and one on the project repository.
    let elsewhere = format!("{}{OWNER}/elsewhere.git", remote.base);
    installation.add_push_mirror(&elsewhere, "x", "y", None).await.unwrap();
    project.add_push_mirror(&elsewhere, "x", "y", None).await.unwrap();

    mirror_for(&remote, &tmp).pass(&installation, std::slice::from_ref(&project)).await;

    let on_installation: Vec<String> =
        installation.push_mirrors().await.unwrap().into_iter().map(|m| m.remote_address).collect();
    assert_eq!(on_installation, [format!("{}{OWNER}/off-configuration.git", remote.base)]);
    // A project repository keeps what else its people set up beside the operator's copy.
    assert_eq!(project.push_mirrors().await.unwrap().len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_down_remote_never_blocks_a_merge_and_catches_up() {
    let tmp = tempfile::tempdir().unwrap();
    let forge_world = World::up(FORGE_TOKEN).await;
    // Bound and not listening: every connection is refused until it listens.
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let addr = socket.local_addr().unwrap();
    let mirror = ConfigMirror::new(&format!("http://{addr}/git/{OWNER}/off-{{repo}}.git"), token_file(&tmp))
        .expect("mirror")
        .with_timing(Duration::ZERO, Duration::ZERO);
    let installation = forge_world.client("configuration");
    installation.ensure_repository("Configuration").await.unwrap();

    mirror.pass(&installation, &[]).await;
    assert!(!mirror.statuses()[0].last_error.is_empty());
    // The merge goes through with the remote down.
    merge_change(&installation, "change/1", "endpoints/one.yaml").await;
    let main = installation.branch_head("main").await.unwrap();
    assert!(!installation.push_mirrors().await.unwrap()[0].last_error.is_empty());

    let remote = World::start(socket.listen(64).unwrap(), REMOTE_TOKEN).await;
    remote.client("off-configuration").create_empty_repository("x").await.unwrap();
    mirror.pass(&installation, &[]).await;
    assert_eq!(mirror.statuses()[0].last_error, "", "{:?}", mirror.statuses());
    assert_eq!(heads(&remote, "off-configuration"), [("refs/heads/main".to_owned(), main)]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_force_push_ever() {
    let tmp = tempfile::tempdir().unwrap();
    let forge_world = World::up(FORGE_TOKEN).await;
    let remote = World::up(REMOTE_TOKEN).await;
    let installation = forge_world.client("configuration");
    installation.ensure_repository("Configuration").await.unwrap();
    // The remote holds a history of its own.
    let theirs = remote.client("off-configuration");
    theirs.ensure_repository("x").await.unwrap();
    let their_main = theirs.branch_head("main").await.unwrap();

    let mirror = mirror_for(&remote, &tmp);
    mirror.pass(&installation, &[]).await;
    let error = mirror.statuses()[0].last_error.clone();
    assert!(error.contains("not overwritten"), "{error}");
    assert!(!error.contains(REMOTE_TOKEN));
    assert_eq!(theirs.branch_head("main").await.unwrap(), their_main);
}
