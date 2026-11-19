//! The files routes of a `kind: Portal` (T-7417): upload, list, rename, replace and delete.
//! Another Organization's portal is the same 404 as an unknown one, a member without propose is
//! 403 on every write, a file in use is not deleted, and each call reaches only its own portal.

mod common;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use axum::body::Body;
use axum::http::StatusCode;
use jc_core::kinds::PortalSpec;
use serde_json::{json, Value};

use common::doors::http;
use common::envelope;
use joinedcontext_portal::api::portal_files::{
    hand_file_desk_to, FileDesk, FileFuture, FileLimits, FileRefusal, PortalFile,
};
use joinedcontext_portal::api::portal_sources::{DeskError, DeskFuture};
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;

/// Files by (organization, portal path), each named by its bytes.
#[derive(Default)]
struct Fake {
    files: Mutex<BTreeMap<(String, String), Vec<PortalFile>>>,
}

fn key(portal: &PortalSpec) -> (String, String) {
    (portal.organization_ref.clone(), portal.path.clone())
}

const LIMITS: FileLimits = FileLimits {
    files: 0,
    bytes: 0,
    max_files: 500,
    max_bytes: 200,
    max_file_bytes: 10,
};

impl FileDesk for Fake {
    fn list<'a>(&'a self, portal: &'a PortalSpec) -> DeskFuture<'a, (Vec<PortalFile>, FileLimits)> {
        let files = self
            .files
            .lock()
            .unwrap()
            .get(&key(portal))
            .cloned()
            .unwrap_or_default();
        Box::pin(async move { Ok((files, LIMITS)) })
    }
    fn upload<'a>(
        &'a self,
        portal: &'a PortalSpec,
        by: &'a str,
        name: &'a str,
        bytes: Vec<u8>,
    ) -> FileFuture<'a, PortalFile> {
        let answer = if name == "bad/name" {
            Err(FileRefusal::Invalid(
                "name: one line without / or \\".into(),
            ))
        } else {
            let sha = format!("{:064x}", bytes.len());
            let file = PortalFile {
                id: format!("f-{name}"),
                name: name.to_owned(),
                content_type: "text/csv".into(),
                size: bytes.len() as u64,
                path: format!("/files/{sha}/{name}"),
                sha256: sha,
                uploaded_by: by.to_owned(),
                uploaded_at: "2026-11-19T19:00:00Z".into(),
                used_by: Vec::new(),
            };
            self.files
                .lock()
                .unwrap()
                .entry(key(portal))
                .or_default()
                .push(file.clone());
            Ok(file)
        };
        Box::pin(async move { answer })
    }
    fn rename<'a>(
        &'a self,
        portal: &'a PortalSpec,
        _by: &'a str,
        id: &'a str,
        name: &'a str,
    ) -> FileFuture<'a, PortalFile> {
        let mut all = self.files.lock().unwrap();
        let answer = all
            .entry(key(portal))
            .or_default()
            .iter_mut()
            .find(|f| f.id == id)
            .map(|f| {
                f.name = name.to_owned();
                f.clone()
            })
            .ok_or(FileRefusal::Desk(DeskError::NotFound));
        Box::pin(async move { answer })
    }
    fn replace<'a>(
        &'a self,
        _portal: &'a PortalSpec,
        _by: &'a str,
        _id: &'a str,
        _bytes: Vec<u8>,
    ) -> FileFuture<'a, PortalFile> {
        Box::pin(async { Err(FileRefusal::Desk(DeskError::NotFound)) })
    }
    fn delete<'a>(
        &'a self,
        portal: &'a PortalSpec,
        _by: &'a str,
        id: &'a str,
    ) -> FileFuture<'a, ()> {
        let mut all = self.files.lock().unwrap();
        let list = all.entry(key(portal)).or_default();
        let before = list.len();
        list.retain(|f| f.id != id);
        let answer = (list.len() < before)
            .then_some(())
            .ok_or(FileRefusal::Desk(DeskError::NotFound));
        Box::pin(async move { answer })
    }
}

fn fake() -> &'static Arc<Fake> {
    static FAKE: OnceLock<Arc<Fake>> = OnceLock::new();
    FAKE.get_or_init(|| {
        let fake = Arc::new(Fake::default());
        hand_file_desk_to(fake.clone());
        fake
    })
}

fn person(name: &str) -> Identity {
    Identity {
        client: None,
        subject: format!("sub-{name}"),
        username: name.to_owned(),
        email: Some(format!("{name}@example.org")),
        name: Some(name.to_owned()),
        roles: Vec::new(),
        groups: Vec::new(),
        second_factor: false,
    }
}

fn bind(m: &Mirror, name: &str, who: &str, role: &str, org: &str) {
    m.upsert(envelope(
        "RoleBinding",
        name,
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": format!("{who}@example.org") }], "role": role,
            "scope": { "organization": org } }),
    ));
}

/// Organizations `bbsk` and `praha`, each with an administrator and a portal `<org>-<path>`;
/// `bbsk` has a reader of its portals and a member who reads none.
fn world(path: &str) -> AppState {
    fake();
    let state = AppState::new(Config::for_tests(), None).with_mirror(Arc::new(Mirror::new()));
    let m = &state.mirror;
    for (name, spec) in [
        (
            "ops",
            json!({ "domain": "ops.example", "installation": true }),
        ),
        ("bbsk", json!({ "domain": "bbsk.sk" })),
        ("praha", json!({ "domain": "praha.eu" })),
    ] {
        m.upsert(envelope("Organization", name, ORG_NAMESPACE, spec));
    }
    let kinds: Vec<&str> = joinedcontext_portal::resource::kinds()
        .map(|i| i.kind)
        .collect();
    for (role, rule) in [
        (
            "org-admin",
            json!({ "kinds": kinds, "verbs": ["read", "propose", "approve", "delete"] }),
        ),
        (
            "portal-reader",
            json!({ "kinds": ["Portal"], "verbs": ["read"] }),
        ),
        (
            "group-reader",
            json!({ "kinds": ["Group"], "verbs": ["read"] }),
        ),
    ] {
        m.upsert(envelope(
            "Role",
            role,
            ORG_NAMESPACE,
            json!({ "rules": [rule] }),
        ));
    }
    for (who, org) in [("bbadmin", "bbsk"), ("prahaadmin", "praha")] {
        bind(m, &format!("{org}-admins"), who, "org-admin", org);
        m.upsert(envelope(
            "Portal",
            &format!("{org}-{path}"),
            ORG_NAMESPACE,
            json!({ "organizationRef": org, "path": path }),
        ));
    }
    bind(m, "bbsk-readers", "bbreader", "portal-reader", "bbsk");
    bind(m, "bbsk-others", "bbother", "group-reader", "bbsk");
    state
}

fn files(portal: &str) -> String {
    format!("/api/v1/projects/{ORG_NAMESPACE}/portals/{portal}/files")
}

async fn upload(state: &AppState, who: &str, portal: &str, name: &str) -> common::doors::Answer {
    let uri = format!("{}?name={}", files(portal), name.replace('/', "%2F"));
    let raw = common::send_body(
        state,
        person(who),
        "POST",
        &uri,
        "application/octet-stream",
        Body::from("a,b\n1,2\n"),
    )
    .await;
    common::doors::Answer {
        status: raw.status,
        body: serde_json::from_str(&raw.text).unwrap_or(Value::Null),
    }
}

#[tokio::test]
async fn an_admin_uploads_lists_renames_and_deletes_a_file_of_its_own_portal() {
    let state = world("files-a");
    let up = upload(&state, "bbadmin", "bbsk-files-a", "hero.csv").await;
    assert_eq!(up.status, StatusCode::CREATED, "{}", up.body);
    assert_eq!(up.body["uploadedBy"], "bbadmin");
    let id = up.body["id"].as_str().unwrap().to_owned();

    let list = http(
        &state,
        person("bbadmin"),
        "GET",
        &files("bbsk-files-a"),
        None,
    )
    .await;
    assert_eq!(list.status, StatusCode::OK, "{}", list.body);
    assert_eq!(list.body["items"][0]["name"], "hero.csv");
    assert_eq!(list.body["limits"]["maxFiles"], 500);
    let theirs = http(
        &state,
        person("prahaadmin"),
        "GET",
        &files("praha-files-a"),
        None,
    )
    .await;
    assert_eq!(
        theirs.body["items"],
        json!([]),
        "praha never lists bbsk's file"
    );

    let renamed = http(
        &state,
        person("bbadmin"),
        "PATCH",
        &format!("{}/{id}", files("bbsk-files-a")),
        Some(json!({ "name": "titulka.csv" })),
    )
    .await;
    assert_eq!(renamed.status, StatusCode::OK, "{}", renamed.body);
    assert_eq!(renamed.body["name"], "titulka.csv");

    let deleted = http(
        &state,
        person("bbadmin"),
        "DELETE",
        &format!("{}/{id}", files("bbsk-files-a")),
        None,
    )
    .await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT, "{}", deleted.body);
}

#[tokio::test]
async fn a_file_in_use_is_not_deleted_and_the_refusal_names_where() {
    let state = world("files-b");
    let up = upload(&state, "bbadmin", "bbsk-files-b", "logo.csv").await;
    let (id, path) = (
        up.body["id"].as_str().unwrap().to_owned(),
        up.body["path"].as_str().unwrap().to_owned(),
    );
    state.mirror.upsert(envelope(
        "Portal",
        "bbsk-home",
        ORG_NAMESPACE,
        json!({ "organizationRef": "bbsk", "path": "home", "title": { "sk": format!("![x]({path})") } }),
    ));
    let list = http(
        &state,
        person("bbadmin"),
        "GET",
        &files("bbsk-files-b"),
        None,
    )
    .await;
    assert_eq!(list.body["items"][0]["usedBy"], json!(["bbsk-home"]));
    let refused = http(
        &state,
        person("bbadmin"),
        "DELETE",
        &format!("{}/{id}", files("bbsk-files-b")),
        None,
    )
    .await;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.body);
    assert!(
        refused.body["detail"]
            .as_str()
            .unwrap()
            .contains("used by bbsk-home"),
        "{}",
        refused.body
    );
    let replace = common::send_body(
        &state,
        person("bbadmin"),
        "PUT",
        &format!("{}/{id}", files("bbsk-files-b")),
        "application/octet-stream",
        Body::from("x"),
    )
    .await;
    assert_eq!(replace.status, StatusCode::CONFLICT, "{}", replace.text);
}

#[tokio::test]
async fn a_refused_name_or_type_is_422_naming_it() {
    let state = world("files-c");
    let refused = upload(&state, "bbadmin", "bbsk-files-c", "bad/name").await;
    assert_eq!(
        refused.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        refused.body
    );
    assert!(
        refused.body.to_string().contains("one line without /"),
        "{}",
        refused.body
    );
}

#[tokio::test]
async fn another_organizations_portal_is_the_same_404_and_a_member_without_propose_is_403() {
    let state = world("files-d");
    for portal in ["praha-files-d", "bbsk-nic"] {
        let up = upload(&state, "bbadmin", portal, "x.csv").await;
        assert_eq!(up.status, StatusCode::NOT_FOUND, "{portal}: {}", up.body);
        let list = http(&state, person("bbadmin"), "GET", &files(portal), None).await;
        assert_eq!(list.status, StatusCode::NOT_FOUND, "{portal}");
    }
    let reader_list = http(
        &state,
        person("bbreader"),
        "GET",
        &files("bbsk-files-d"),
        None,
    )
    .await;
    assert_eq!(reader_list.status, StatusCode::OK);
    let reader_up = upload(&state, "bbreader", "bbsk-files-d", "x.csv").await;
    assert_eq!(
        reader_up.status,
        StatusCode::FORBIDDEN,
        "{}",
        reader_up.body
    );
    let reader_delete = http(
        &state,
        person("bbreader"),
        "DELETE",
        &format!("{}/f-x", files("bbsk-files-d")),
        None,
    )
    .await;
    assert_eq!(reader_delete.status, StatusCode::FORBIDDEN);
    for (method, uri) in [
        ("GET", files("bbsk-files-d")),
        ("DELETE", format!("{}/f-x", files("bbsk-files-d"))),
    ] {
        let other = http(&state, person("bbother"), method, &uri, None).await;
        assert_eq!(other.status, StatusCode::FORBIDDEN, "{method} {uri}");
    }
    let other_up = upload(&state, "bbother", "bbsk-files-d", "x.csv").await;
    assert_eq!(other_up.status, StatusCode::FORBIDDEN);
}
