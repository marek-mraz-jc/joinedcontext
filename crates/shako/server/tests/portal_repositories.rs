#![forbid(unsafe_code)]
//! The Portal's repository pages against real shako: every `jc_repository_*` operation of the
//! Portal registry, called as a person, reads shako's OData service over TCP through the Portal's
//! forge client, with real git repositories on disk. A query shape shako does not parse, a member
//! it names otherwise, or a repository the project does not own leaking into its list fails here.

use jc_router::{JcRouterBuilder, Mount, MountSpec};
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::forge::shako::{Author, ReviewEvent, ShakoClient};
use joinedcontext_portal::ops::{self, Caller, Via};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::{API_VERSION, ObjectMeta, ResourceEnvelope};
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use serde_json::{Value, json};
use shako_core::{
    Action, InstanceId, LocalAuthorizer, OrgId, OutboundGuard, OutboundPolicy, RoleBinding,
    ShakoSigningKey, StaticTokenBinding, Subject, TokenVerifier,
};
use shako_server::{ShakoState, router};
use shako_store_memory::MemoryStore;
use std::collections::BTreeMap;
use std::sync::Arc;

const OWNER: &str = "joinedcontext";
const APPS_OWNER: &str = "joinedcontext-apps";
const PORTAL_TOKEN: &str = "portal-token-0123456789";
const APPS_TOKEN: &str = "apps-token-0123456789";
const PROJECT: &str = "ovzdusie";
/// Where people reach the forge; the Portal dials `World::base` (T-6893).
const PUBLIC_GIT: &str = "https://jc.city.example/git/";
const CONFIG: &str = "ovzdusie-config";

const AUTHOR: Author<'static> = Author {
    name: "Alice Steward",
    email: "alice@city.example",
};

struct World {
    _tmp: tempfile::TempDir,
    base: String,
}

impl World {
    async fn start() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let org = OrgId::from_u128(10);
        let apps_org = OrgId::from_u128(11);
        let all = vec![
            Action::Admin,
            Action::Write,
            Action::Approve,
            Action::Publish,
        ];
        let binding = |org_id, principal: &str| RoleBinding {
            org_id,
            principal: principal.into(),
            actions: all.clone(),
            project_id: None,
            repo: None,
        };
        let authz = Arc::new(LocalAuthorizer::new(vec![
            binding(org, "jc-portal"),
            binding(apps_org, "jc-apps"),
        ]));
        let verifier = TokenVerifier::new(ShakoSigningKey::generate().expect("key"))
            .with_static_tokens(vec![
                StaticTokenBinding::new(
                    PORTAL_TOKEN,
                    Subject::authenticated("jc-portal", "jc-portal", vec![org], vec![]),
                ),
                StaticTokenBinding::new(
                    APPS_TOKEN,
                    Subject::authenticated("jc-apps", "jc-apps", vec![apps_org], vec![]),
                ),
            ]);
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
        state.register_owner_org(APPS_OWNER, apps_org);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let host = listener.local_addr().unwrap().to_string();
        let git_mount = Mount::new("shako", "http", host.clone(), "/git").unwrap();
        let jc = JcRouterBuilder::new("http", host.clone())
            .mount(
                MountSpec {
                    module: "shako".into(),
                    host: None,
                    path_prefix: Some("/git".into()),
                },
                router(state.clone(), git_mount),
            )
            .build()
            .expect("mount table");
        tokio::spawn(async move {
            axum::serve(listener, jc.into_axum_router()).await.unwrap();
        });
        World {
            _tmp: tmp,
            base: format!("http://{host}/git/"),
        }
    }

    /// The Portal's organization client, as `jc` configures it (`JC_SHAKO_*`).
    fn client(&self) -> ShakoClient {
        let base = self.base.clone();
        ShakoClient::from_env(move |key| match key {
            "JC_SHAKO_URL" => Some(base.clone()),
            "JC_SHAKO_PUBLIC_URL" => Some(PUBLIC_GIT.into()),
            "JC_SHAKO_OWNER" => Some(OWNER.into()),
            "JC_SHAKO_REPO" => Some("configuration".into()),
            "JC_SHAKO_TOKEN" => Some(PORTAL_TOKEN.into()),
            "JC_SHAKO_APPS_OWNER" => Some(APPS_OWNER.into()),
            "JC_SHAKO_APPS_TOKEN" => Some(APPS_TOKEN.into()),
            _ => None,
        })
        .expect("config")
        .expect("forge configured")
    }
}

fn envelope(kind: &str, name: &str, spec: Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: kind.to_owned(),
        metadata: ObjectMeta::new(name, ORG_NAMESPACE),
        spec,
        status: None,
    }
}

fn person(name: &str, group: &str) -> Caller {
    Caller::new(
        Identity {
            client: None,
            subject: format!("f:1:{name}"),
            username: name.to_owned(),
            email: None,
            name: None,
            roles: vec![],
            groups: vec![group.to_owned()],
            second_factor: false,
        },
        Via::Session,
    )
}

/// The Portal on that forge: layout 2, `ovzdusie` in its own configuration repository, a steward
/// who approves on Project and a viewer who reads.
fn portal(world: &World) -> AppState {
    let state = AppState::new(Config::for_tests(), None)
        .with_shako(Arc::new(world.client()))
        .with_mirror(Arc::new(Mirror::new()));
    state.mirror.set_layout(2);
    state
        .mirror
        .set_repositories(BTreeMap::from([(PROJECT.into(), CONFIG.into())]));
    for (role, verbs) in [
        ("steward-role", json!(["read", "propose", "approve"])),
        ("viewer-role", json!(["read"])),
    ] {
        state.mirror.upsert(envelope(
            "Role",
            role,
            json!({ "rules": [{ "kinds": ["Project", "Endpoint"], "verbs": verbs }] }),
        ));
    }
    for (binding, group, role) in [
        ("steward-binding", "stewards", "steward-role"),
        ("viewer-binding", "viewers", "viewer-role"),
    ] {
        state.mirror.upsert(envelope(
            "RoleBinding",
            binding,
            json!({ "subjects": [{ "group": group }], "role": role, "scope": { "project": PROJECT } }),
        ));
    }
    state
}

async fn call(state: &AppState, caller: &Caller, name: &str, input: Value) -> Result<Value, u16> {
    let op = ops::find(name).expect(name);
    ops::call(op, caller, state, PROJECT, input)
        .await
        .map_err(|err| {
            use axum::response::IntoResponse as _;
            err.into_response().status().as_u16()
        })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_person_browses_the_projects_repositories_on_shako() {
    let world = World::start().await;
    let org = world.client();
    let forge = org.for_project(CONFIG, PROJECT);
    assert!(
        forge
            .ensure_repository("Air quality configuration")
            .await
            .unwrap()
    );
    let readme = "# Ovzdusie\n\n<script>alert(1)</script> [x](javascript:alert(1))\n";
    // auto_init leaves a README of its own: the first commit replaces it.
    let seeded = forge
        .get_file("projects/ovzdusie/README.md", "main")
        .await
        .unwrap()
        .map(|file| file.sha);
    let c1 = forge
        .write_files(
            "main",
            "First files",
            AUTHOR,
            &[
                ("projects/ovzdusie/README.md".into(), readme.into(), seeded),
                (
                    "projects/ovzdusie/spaces/air/endpoints/air.yaml".into(),
                    "a: 1\nb: 2\n".into(),
                    None,
                ),
            ],
        )
        .await
        .unwrap();
    let air = forge
        .get_file("projects/ovzdusie/spaces/air/endpoints/air.yaml", "main")
        .await
        .unwrap()
        .map(|file| file.sha);
    let c2 = forge
        .write_files(
            "main",
            "Change b",
            AUTHOR,
            &[(
                "projects/ovzdusie/spaces/air/endpoints/air.yaml".into(),
                "a: 1\nb: 3\n".into(),
                air,
            )],
        )
        .await
        .unwrap();
    forge.create_branch("portal/edit", "main").await.unwrap();
    forge
        .write_files(
            "portal/edit",
            "Propose c",
            AUTHOR,
            &[(
                "projects/ovzdusie/spaces/air/endpoints/c.yaml".into(),
                "c: 1\n".into(),
                None,
            )],
        )
        .await
        .unwrap();
    let pull = forge
        .create_pull_request("portal/edit", "main", "Add c", "Proposed in the Portal")
        .await
        .unwrap();
    forge
        .review(pull.number, ReviewEvent::Comment, "Looks right")
        .await
        .unwrap();
    // The project's application, another project's application, the organization's shared repo.
    let apps = org.for_application("ovzdusie_dash");
    assert!(apps.ensure_repository("Dashboard").await.unwrap());
    assert!(
        org.for_application("doprava_maps")
            .ensure_repository("Maps")
            .await
            .unwrap()
    );
    assert!(org.ensure_repository("Shared").await.unwrap());

    let state = portal(&world);
    let steward = person("jana", "stewards");
    let viewer = person("peter", "viewers");
    let stranger = person("eva", "elsewhere");

    // The list holds the project's own, never another project's or the shared repository.
    let list = call(&state, &viewer, "jc_repository_list", json!({}))
        .await
        .unwrap();
    let names: Vec<&str> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec![CONFIG, "ovzdusie_dash"], "{list}");
    assert_eq!(list["items"][0]["role"], "configuration");
    let filtered = call(
        &state,
        &viewer,
        "jc_repository_list",
        json!({ "filter": "dash" }),
    )
    .await
    .unwrap();
    assert_eq!(filtered["total"], 1);

    // The repository page: clone address, newest commit, the README as text.
    let repo = call(
        &state,
        &viewer,
        "jc_repository_get",
        json!({ "repository": CONFIG }),
    )
    .await
    .unwrap();
    // the_clone_url_is_the_public_address: never the address the Portal dials.
    assert_eq!(
        repo["cloneUrl"],
        format!("{PUBLIC_GIT}{OWNER}/{CONFIG}.git"),
        "{repo}"
    );
    assert_eq!(repo["lastCommit"]["sha"], c2, "{repo}");
    assert!(
        repo["readme"]["text"]
            .as_str()
            .unwrap()
            .contains("<script>"),
        "{repo}"
    );
    // The project's application, read by its own role; auto_init gave it one commit.
    let app = call(
        &state,
        &viewer,
        "jc_repository_get",
        json!({ "repository": "ovzdusie_dash" }),
    )
    .await
    .unwrap();
    assert_eq!(app["role"], "application", "{app}");
    assert_eq!(app["empty"], false, "{app}");

    // Folders by path, a file, a missing file, a traversal.
    let root = call(
        &state,
        &viewer,
        "jc_repository_tree",
        json!({ "repository": CONFIG }),
    )
    .await
    .unwrap();
    assert_eq!(root["entries"][0]["type"], "folder", "{root}");
    let deep = call(
        &state,
        &viewer,
        "jc_repository_tree",
        json!({ "repository": CONFIG, "path": "spaces/air/endpoints" }),
    )
    .await
    .unwrap();
    assert_eq!(
        deep["entries"][0]["path"], "spaces/air/endpoints/air.yaml",
        "{deep}"
    );
    let file = call(
        &state,
        &viewer,
        "jc_repository_file",
        json!({ "repository": CONFIG, "path": "spaces/air/endpoints/air.yaml", "ref": c1 }),
    )
    .await
    .unwrap();
    assert!(file["text"].as_str().unwrap().contains("b: 2"), "{file}");
    let missing = call(
        &state,
        &viewer,
        "jc_repository_file",
        json!({ "repository": CONFIG, "path": "spaces/nope.yaml" }),
    )
    .await;
    assert_eq!(missing, Err(404));
    let traversal = call(
        &state,
        &viewer,
        "jc_repository_file",
        json!({ "repository": CONFIG, "path": "../../etc/passwd" }),
    )
    .await;
    assert_eq!(traversal, Err(422));

    // Commits, one commit's diff, branches.
    let commits = call(
        &state,
        &viewer,
        "jc_repository_commits",
        json!({ "repository": CONFIG }),
    )
    .await
    .unwrap();
    assert_eq!(commits["items"][0]["sha"], c2, "{commits}");
    assert_eq!(commits["items"][0]["message"], "Change b");
    let of_path = call(
        &state,
        &viewer,
        "jc_repository_commits",
        json!({ "repository": CONFIG, "path": "README.md" }),
    )
    .await
    .unwrap();
    assert_eq!(of_path["items"][0]["sha"], c1, "{of_path}");
    let commit = call(
        &state,
        &viewer,
        "jc_repository_commit",
        json!({ "repository": CONFIG, "sha": c2 }),
    )
    .await
    .unwrap();
    assert_eq!(commit["parents"][0], c1, "{commit}");
    let patch = commit["files"][0]["patch"].as_str().unwrap();
    assert!(patch.contains("-b: 2\n+b: 3\n"), "{commit}");
    let refs = call(
        &state,
        &viewer,
        "jc_repository_refs",
        json!({ "repository": CONFIG }),
    )
    .await
    .unwrap();
    assert_eq!(refs["branches"][0]["name"], "main", "{refs}");
    assert_eq!(refs["branches"][0]["default"], true);

    // Pull requests, one with its files, its review and the Change it is.
    let pulls = call(
        &state,
        &viewer,
        "jc_repository_pulls",
        json!({ "repository": CONFIG }),
    )
    .await
    .unwrap();
    assert_eq!(pulls["items"][0]["title"], "Add c", "{pulls}");
    let one = call(
        &state,
        &viewer,
        "jc_repository_pull",
        json!({ "repository": CONFIG, "number": pull.number }),
    )
    .await
    .unwrap();
    assert_eq!(
        one["files"][0]["path"], "spaces/air/endpoints/c.yaml",
        "{one}"
    );
    assert_eq!(one["reviews"][0]["body"], "Looks right");
    assert_eq!(one["change"], format!("chg-{:08x}", pull.number));

    // Settings: a steward reads them, a viewer is refused.
    let settings = call(
        &state,
        &steward,
        "jc_repository_settings",
        json!({ "repository": CONFIG }),
    )
    .await
    .unwrap();
    assert!(settings["webhooks"].is_array(), "{settings}");
    assert_eq!(
        call(
            &state,
            &viewer,
            "jc_repository_settings",
            json!({ "repository": CONFIG })
        )
        .await,
        Err(403)
    );

    // Another project's repository and a stranger's read are not there.
    for (caller, name) in [
        (&viewer, "doprava_maps"),
        (&viewer, "configuration"),
        (&stranger, CONFIG),
    ] {
        assert_eq!(
            call(
                &state,
                caller,
                "jc_repository_get",
                json!({ "repository": name })
            )
            .await,
            Err(404),
            "{name}"
        );
    }
    assert_eq!(
        call(&state, &stranger, "jc_repository_list", json!({})).await,
        Err(404)
    );
}
