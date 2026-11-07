//! `status.build` has one writer: the build lane, the role whose `propose` on `App` is
//! constrained to that field (AP-13a, AP-73). Everything else about `status` is the platform's
//! own computation and no manifest carries it (MF-04). And what the lane writes is checked: the
//! commit is its repository's head, the bundle an artifact of a run of it hashing to the digest,
//! and the Portal publishes the package itself before the build is written (AP-101, AP-104).

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{checked_send as send, envelope, forge, person};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::API_VERSION;
use joinedcontext_portal::state::AppState;

const APPS: &str = "/api/v1/projects/ovzdusie/apps";

/// `builder@hel.fi` is the build lane: propose on App, constrained to `status.build`.
/// `jana@hel.fi` proposes Apps like anybody else.
fn state_with(gitea: &MockServer) -> AppState {
    let state = common::state_on(gitea);
    state.mirror.upsert(envelope(
        "Role",
        "app-builder",
        ORG_NAMESPACE,
        json!({ "rules": [{
            "kinds": ["App"],
            "verbs": ["propose"],
            "constraints": [{ "field": "status.build" }],
        }]}),
    ));
    state.mirror.upsert(envelope(
        "Role",
        "app-editor",
        ORG_NAMESPACE,
        // ContextSpace too, so the refusal of a build on another kind is the content check
        // and not the verb check that runs before the manifest is read (T-2576).
        json!({ "rules": [{ "kinds": ["App", "ContextSpace"], "verbs": ["propose"] }] }),
    ));
    // The space the app's `dataNeeds` names: a manifest naming a resource that is not in the
    // project is refused before the lane is judged at all (MF-13, T-2268), and this project really
    // does hold it.
    state.mirror.upsert(envelope(
        "ContextSpace",
        "ovzdusie",
        "ovzdusie",
        json!({ "dataModelRef": "air" }),
    ));
    for (who, role) in [("builder", "app-builder"), ("jana", "app-editor")] {
        state.mirror.upsert(envelope(
            "RoleBinding",
            &format!("{who}-{role}"),
            ORG_NAMESPACE,
            json!({
                "subjects": [{ "user": format!("{who}@hel.fi") }],
                "role": role,
                "scope": { "project": "ovzdusie" },
            }),
        ));
    }
    state
}

/// What the lane sends (T-2636): it reads the App on `main` and writes it back, unchanged, with
/// `status.build`, so the App is put on `main` first and the write is a `PUT` of that App.
async fn as_lane(state: &AppState, body: Value) -> common::Answer {
    let mut on_main: joinedcontext_portal::resource::ResourceEnvelope =
        serde_json::from_value(body.clone()).expect("an App manifest");
    on_main.status = None;
    state.mirror.upsert(on_main);
    send(
        state,
        person("builder"),
        "PUT",
        &format!("{APPS}/air-quality"),
        Some(body),
    )
    .await
}

fn app(status: Option<Value>, annotation: Option<&str>) -> Value {
    let mut metadata = json!({ "name": "air-quality", "namespace": "ovzdusie" });
    if let Some(key) = annotation {
        metadata["annotations"] = json!({ key: "sha256:deadbeef" });
    }
    let mut manifest = json!({
        "apiVersion": API_VERSION,
        "kind": "App",
        "metadata": metadata,
        "spec": {
            "kind": "static",
            // A published static App names its repository (AP-87).
            "source": { "git": {
                "url": "https://git.example/joinedcontext/ovzdusie_air-quality.git",
                "ref": COMMIT,
            }},
            "build": { "node": "22" },
            "visibility": "public",
            "lifecycle": "published",
            "dataNeeds": [{
                "contextSpaceRef": { "kind": "ContextSpace", "name": "ovzdusie" },
                "types": ["AirQualityObserved"],
                "operations": ["queryEntity"],
            }],
        },
    });
    if let Some(status) = status {
        manifest["status"] = status;
    }
    manifest
}

/// The App's own repository, `{project}_{app}` (AP-75), and the head of its default branch.
const APP_REPO: &str = "/api/v1/repos/test-owner/ovzdusie_air-quality";
const COMMIT: &str = "8c56954a1f0e3d2c1b0a99887766554433221100";
const PACKAGE: &str = "/api/packages/test-owner/generic/app-air-quality";
const BUNDLE: &[u8] = b"the bundle the workflow built";
const SBOM: &[u8] = b"{\"bomFormat\":\"CycloneDX\"}";

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

/// The package version of a build of `COMMIT` whose digest is `digest` (AP-101, T-2671).
fn version(digest: &str) -> String {
    format!(
        "{COMMIT}-{}",
        &digest["sha256:".len().."sha256:".len() + 12]
    )
}

/// The forge after a run of `COMMIT` uploaded `bundle` and the SBOM as run 5's artifacts, and
/// with the default branch at `head`. The artifact download redirects to the forge's public
/// ROOT_URL, which the Portal cannot reach, as Gitea does.
async fn built_on_forge(gitea: &MockServer, head: &str, bundle: &[u8], size: Option<u64>) {
    built_on_forge_as(gitea, head, "bundle", bundle, size).await;
}

/// The forge after a run uploaded `built` as the artifact `{artifact}-{COMMIT}`, as above.
async fn built_on_forge_as(
    gitea: &MockServer,
    head: &str,
    artifact: &str,
    bundle: &[u8],
    size: Option<u64>,
) {
    built_on_forge_in(gitea, APP_REPO, head, artifact, bundle, size).await;
}

/// The same, with the App's repository at `APP_REPO`'s place `app_repo`.
async fn built_on_forge_in(
    gitea: &MockServer,
    app_repo: &str,
    head: &str,
    artifact: &str,
    bundle: &[u8],
    size: Option<u64>,
) {
    Mock::given(method("GET"))
        .and(path(app_repo))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(gitea)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{app_repo}/branches/main")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "commit": { "id": head } })))
        .mount(gitea)
        .await;
    // Any other name: none, as the forge answers.
    Mock::given(method("GET"))
        .and(path(format!("{app_repo}/actions/artifacts")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "artifacts": [] })))
        .with_priority(10)
        .mount(gitea)
        .await;
    for (id, name, bytes) in [(11, artifact, bundle), (12, "sbom", SBOM)] {
        let artifact = |id: u64, run: u64, sha: &str| {
            json!({
                "id": id, "name": format!("{name}-{COMMIT}"),
                "size_in_bytes": if name == artifact { size.unwrap_or(bytes.len() as u64) } else { bytes.len() as u64 },
                "workflow_run": { "id": run, "head_sha": sha },
            })
        };
        Mock::given(method("GET"))
            .and(path(format!("{app_repo}/actions/artifacts")))
            .and(query_param("name", format!("{name}-{COMMIT}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                // An artifact of this name from a run of another commit never counts.
                "artifacts": [artifact(id, 5, COMMIT), artifact(id + 90, 9, "0000000000000000000000000000000000000000")],
            })))
            .mount(gitea)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("{app_repo}/actions/artifacts/{id}/zip")))
            .respond_with(ResponseTemplate::new(302).insert_header(
                "Location",
                // The forge signs its public ROOT_URL, which carries a path on dev
                // (`https://host/git/`); the Portal dials the API base, where no `/git` exists.
                format!("https://forge.public.example/git{app_repo}/actions/artifacts/{id}/zip/raw?sig=s{id}&expires=9"),
            ))
            .mount(gitea)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("{app_repo}/actions/artifacts/{id}/zip/raw")))
            .and(query_param("sig", format!("s{id}")))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes.to_vec()))
            .mount(gitea)
            .await;
    }
}

/// The package files the Portal wrote, in order.
async fn published(gitea: &MockServer) -> Vec<String> {
    gitea
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.method.as_str() == "PUT" && r.url.path().starts_with(PACKAGE))
        .map(|r| r.url.path().trim_start_matches(PACKAGE).to_owned())
        .collect()
}

async fn package_takes(gitea: &MockServer, status: u16) {
    Mock::given(method("PUT"))
        .and(wiremock::matchers::path_regex(format!("^{PACKAGE}/.*")))
        .respond_with(ResponseTemplate::new(status))
        .mount(gitea)
        .await;
}

fn build() -> Value {
    json!({ "build": {
        "digest": digest(BUNDLE),
        "commit": COMMIT,
        "sdkVersion": "0.4.1",
        "builtAt": "2026-09-17T06:00:00Z",
    }})
}

fn build_of(commit: &str) -> Value {
    let mut build = build();
    build["build"]["commit"] = json!(commit);
    build
}

/// What the merge request wrote, decoded.
async fn committed(gitea: &MockServer) -> Vec<String> {
    use base64::Engine;
    gitea
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.method.as_str() == "PUT" && r.url.path().contains("/contents/"))
        .filter_map(|r| {
            let body: Value = serde_json::from_slice(&r.body).ok()?;
            let encoded = body.get("content")?.as_str()?;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .ok()?;
            String::from_utf8(bytes).ok()
        })
        .collect()
}

#[tokio::test]
async fn only_the_build_lane_writes_the_build_and_the_refusal_says_whose_field_it_is() {
    let gitea = forge().await;
    built_on_forge(&gitea, COMMIT, BUNDLE, None).await;
    package_takes(&gitea, 201).await;
    let state = state_with(&gitea);

    let refused = send(
        &state,
        person("jana"),
        "POST",
        APPS,
        Some(app(Some(build()), None)),
    )
    .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.text);
    assert!(refused.text.contains("build lane"), "{}", refused.text);
    assert!(
        committed(&gitea).await.is_empty(),
        "nothing was written for a refused proposal"
    );

    let accepted = as_lane(&state, app(Some(build()), None)).await;
    assert_eq!(accepted.status, StatusCode::ACCEPTED, "{}", accepted.text);
    let written = committed(&gitea).await;
    assert!(
        written.iter().any(|file| file.contains("status:")
            && file.contains("digest:")
            && file.contains(COMMIT)),
        "the build the lane wrote is what lands in the repository: {written:?}"
    );
    assert_eq!(
        published(&gitea).await,
        vec![
            format!("/{}/bundle.tar.gz", version(&digest(BUNDLE))),
            format!("/{}/sbom.cdx.json", version(&digest(BUNDLE)))
        ],
        "the Portal publishes the package itself (AP-101)"
    );
    let raw: Vec<_> = gitea
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.url.path().ends_with("/zip/raw"))
        .collect();
    assert_eq!(
        raw.len(),
        2,
        "both artifacts are fetched from the forge the Portal dials"
    );
    assert!(
        raw.iter().all(|r| !r.headers.contains_key("authorization")),
        "the signed address is the grant; the Portal's token never follows the redirect"
    );
}

/// PF-106: where the applications have an organization of their own, the build is read from
/// the App's repository there and its package published there, both with the applications'
/// token; the configuration's organization sees only the App's manifest, written with its own.
#[tokio::test]
async fn a_build_is_read_and_published_in_the_applications_organization() {
    const APART: &str = "/api/v1/repos/test-owner-apps/ovzdusie_air-quality";
    const PACKAGE_APART: &str = "/api/packages/test-owner-apps/generic/app-air-quality";
    let gitea = forge().await;
    built_on_forge_in(&gitea, APART, COMMIT, "bundle", BUNDLE, None).await;
    Mock::given(method("PUT"))
        .and(wiremock::matchers::path_regex(format!(
            "^{PACKAGE_APART}/.*"
        )))
        .respond_with(ResponseTemplate::new(201))
        .mount(&gitea)
        .await;
    let mut state = state_with(&gitea);
    let uri = gitea.uri();
    let client = joinedcontext_portal::git::GiteaClient::from_env(|key| {
        match key {
            "JC_GITEA_URL" => Some(uri.as_str()),
            "JC_GITEA_OWNER" => Some("test-owner"),
            "JC_GITEA_REPO" => Some("test-repo"),
            "JC_GITEA_TOKEN" => Some("token-xyz"),
            "JC_GITEA_APPS_OWNER" => Some("test-owner-apps"),
            "JC_GITEA_APPS_TOKEN" => Some("apps-token"),
            _ => None,
        }
        .map(str::to_owned)
    })
    .expect("config")
    .expect("configured");
    state.gitea = Some(std::sync::Arc::new(client));

    let accepted = as_lane(&state, app(Some(build()), None)).await;
    assert_eq!(accepted.status, StatusCode::ACCEPTED, "{}", accepted.text);
    let requests = gitea.received_requests().await.unwrap_or_default();
    let token = |r: &wiremock::Request| {
        r.headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    };
    let apart: Vec<_> = requests
        .iter()
        .filter(|r| r.url.path().starts_with(APART) || r.url.path().starts_with(PACKAGE_APART))
        .filter(|r| !r.url.path().ends_with("/zip/raw"))
        .collect();
    assert!(
        !apart.is_empty(),
        "nothing was read in the applications' organization"
    );
    for r in &apart {
        assert_eq!(
            token(r),
            "token apps-token",
            "{} {}",
            r.method,
            r.url.path()
        );
    }
    let published: Vec<_> = requests
        .iter()
        .filter(|r| r.method.as_str() == "PUT" && r.url.path().starts_with(PACKAGE_APART))
        .collect();
    assert_eq!(
        published.len(),
        2,
        "the bundle and the SBOM are published there"
    );
    assert!(
        requests
            .iter()
            .all(|r| !r.url.path().starts_with(APP_REPO) && !r.url.path().starts_with(PACKAGE)),
        "the build touched the configuration's organization"
    );
    let manifest: Vec<_> = requests
        .iter()
        .filter(|r| r.method.as_str() == "PUT" && r.url.path().contains("/test-repo/contents/"))
        .collect();
    assert!(!manifest.is_empty(), "the App's build was not written");
    for r in manifest {
        assert_eq!(
            token(r),
            "token token-xyz",
            "the manifest is the configuration's to write"
        );
    }
}

/// AP-104: a build whose commit is not the head, whose bundle no run of it uploaded, or whose
/// bytes hash to another digest is refused, naming the check; nothing is published or written.
#[tokio::test]
async fn a_build_the_workflow_did_not_make_is_refused_and_nothing_is_published() {
    let other = "1111111111111111111111111111111111111111";
    let cases = [
        (
            "an older commit",
            other,
            BUNDLE,
            None,
            build(),
            StatusCode::CONFLICT,
            "the head of main",
        ),
        (
            "a commit no run built",
            other,
            BUNDLE,
            None,
            build_of(other),
            StatusCode::BAD_REQUEST,
            "holds no artifact bundle-1111",
        ),
        (
            "another digest",
            COMMIT,
            b"a bundle somebody else made".as_slice(),
            None,
            build(),
            StatusCode::BAD_REQUEST,
            "hashes to",
        ),
        (
            "a bundle too large",
            COMMIT,
            BUNDLE,
            Some(65 * 1024 * 1024),
            build(),
            StatusCode::BAD_REQUEST,
            "more than",
        ),
    ];
    for (what, head, bundle, size, status, code, says) in cases {
        let gitea = forge().await;
        built_on_forge(&gitea, head, bundle, size).await;
        package_takes(&gitea, 201).await;
        let state = state_with(&gitea);

        let refused = as_lane(&state, app(Some(status), None)).await;
        assert_eq!(refused.status, code, "{what}: {}", refused.text);
        assert!(refused.text.contains(says), "{what}: {}", refused.text);
        assert!(
            published(&gitea).await.is_empty(),
            "{what}: a package was published"
        );
        assert!(
            committed(&gitea).await.is_empty(),
            "{what}: the build was written"
        );
    }
}

/// AP-101: the registry never replaces a file. The same bytes already there are the same build
/// published again; other bytes under the same version are refused.
#[tokio::test]
async fn a_version_already_published_is_the_same_build_or_a_refusal() {
    for (held, accepted) in [(BUNDLE, true), (b"other bytes".as_slice(), false)] {
        let gitea = forge().await;
        built_on_forge(&gitea, COMMIT, BUNDLE, None).await;
        package_takes(&gitea, 409).await;
        let at = version(&digest(BUNDLE));
        Mock::given(method("GET"))
            .and(path(format!("{PACKAGE}/{at}/bundle.tar.gz")))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(held.to_vec()))
            .mount(&gitea)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("{PACKAGE}/{at}/sbom.cdx.json")))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(SBOM.to_vec()))
            .mount(&gitea)
            .await;
        let state = state_with(&gitea);

        let answer = as_lane(&state, app(Some(build()), None)).await;
        if accepted {
            assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.text);
        } else {
            assert_eq!(answer.status, StatusCode::CONFLICT, "{}", answer.text);
            assert!(
                answer
                    .text
                    .contains("already holds a different bundle.tar.gz"),
                "{}",
                answer.text
            );
            assert!(committed(&gitea).await.is_empty());
        }
    }
}

/// AP-101, T-2671: a rebuild of the same commit on a newer platform release makes other bytes.
/// The build published before under the bare commit stays as it is, and the new one is a version
/// of its own instead of a refusal that would keep every platform fix from the App.
#[tokio::test]
async fn a_rebuild_of_the_same_commit_with_other_bytes_is_a_version_of_its_own() {
    let gitea = forge().await;
    built_on_forge(&gitea, COMMIT, BUNDLE, None).await;
    // The registry refuses a second file under the bare commit, where the old build sits.
    Mock::given(method("PUT"))
        .and(wiremock::matchers::path_regex(format!(
            "^{PACKAGE}/{COMMIT}/.*"
        )))
        .respond_with(ResponseTemplate::new(409))
        .mount(&gitea)
        .await;
    Mock::given(method("GET"))
        .and(wiremock::matchers::path_regex(format!(
            "^{PACKAGE}/{COMMIT}/.*"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"the old build".to_vec()))
        .mount(&gitea)
        .await;
    package_takes(&gitea, 201).await;
    let state = state_with(&gitea);

    let answer = as_lane(&state, app(Some(build()), None)).await;
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.text);
    let at = version(&digest(BUNDLE));
    assert_eq!(
        published(&gitea).await,
        vec![
            format!("/{at}/bundle.tar.gz"),
            format!("/{at}/sbom.cdx.json")
        ]
    );
}

/// T-2674, AP-73: the lane's rule is judged against the mirror, and the change is cut from main.
/// When main already holds the App at another commit (the forge bootstrap pinned a new one and
/// the mirror has not read it yet), the write is a 409 to read again: never a change that would
/// set the pin back, and nothing published for it.
#[tokio::test]
async fn a_build_judged_against_a_mirror_behind_main_is_refused_and_nothing_written() {
    let gitea = forge().await;
    built_on_forge(&gitea, COMMIT, BUNDLE, None).await;
    package_takes(&gitea, 201).await;
    let mut pinned = app(None, None);
    pinned["spec"]["source"]["git"]["ref"] = json!("2222222222222222222222222222222222222222");
    Mock::given(method("GET"))
        .and(wiremock::matchers::path_regex(format!(
            "^{}/contents/.*air-quality.*",
            common::REPO
        )))
        .and(query_param("ref", "main"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "blob-on-main",
            "content": common::encode(&serde_norway::to_string(&pinned).expect("yaml")),
        })))
        .with_priority(1)
        .mount(&gitea)
        .await;
    let state = state_with(&gitea);

    let refused = as_lane(&state, app(Some(build()), None)).await;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.text);
    assert!(refused.text.contains("read it again"), "{}", refused.text);
    assert!(
        published(&gitea).await.is_empty(),
        "a package was published"
    );
    assert!(
        committed(&gitea).await.is_empty(),
        "the stale App was written"
    );
}

#[tokio::test]
async fn every_other_part_of_status_is_refused_even_from_the_build_lane() {
    let gitea = forge().await;
    let state = state_with(&gitea);

    for status in [
        json!({ "phase": "Live" }),
        json!({ "build": null }),
        json!({ "phase": "Live", "build": build()["build"] }),
    ] {
        let refused = as_lane(&state, app(Some(status.clone()), None)).await;
        assert_eq!(
            refused.status,
            StatusCode::BAD_REQUEST,
            "{status}: {}",
            refused.text
        );
        assert!(refused.text.contains("MF-04"), "{}", refused.text);
    }
}

/// AP-13a: a digest in an annotation is a digest somebody typed, whoever they are.
#[tokio::test]
async fn an_image_annotation_is_refused_from_the_build_lane_too() {
    let gitea = forge().await;
    let state = state_with(&gitea);

    for key in ["joinedcontext.com/image", "joinedcontext.com/module"] {
        let refused = as_lane(&state, app(Some(build()), Some(key))).await;
        assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{}", refused.text);
        assert!(refused.text.contains(key), "{}", refused.text);
    }
}

/// A `status.build` on any other kind is the platform's computation, whoever asks: even a
/// person who may propose that kind is refused on the content.
#[tokio::test]
async fn a_build_on_another_kind_is_refused() {
    let gitea = forge().await;
    let state = state_with(&gitea);

    let refused = send(
        &state,
        person("jana"),
        "POST",
        "/api/v1/projects/ovzdusie/spaces",
        Some(json!({
            "apiVersion": API_VERSION,
            "kind": "ContextSpace",
            "metadata": { "name": "mhd", "namespace": "ovzdusie" },
            "spec": { "isSandbox": false },
            "status": build(),
        })),
    )
    .await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{}", refused.text);
}

/// Where the App's manifest lives in the organization repository.
fn manifest_path() -> String {
    joinedcontext_portal::resource::by_plural("apps")
        .expect("App is a kind")
        .repo_path("ovzdusie", "ovzdusie", "air-quality")
}

/// The forge's answer to "which files does merge request 9 change".
async fn change_holds(gitea: &MockServer, paths: &[&str]) {
    let files: Vec<Value> = paths
        .iter()
        .map(|path| json!({ "filename": path, "status": "changed" }))
        .collect();
    Mock::given(method("GET"))
        .and(path(format!("{}/pulls/9/files", common::REPO)))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!(files)))
        .mount(gitea)
        .await;
}

/// The merge messages the Portal sent the forge.
async fn merges(gitea: &MockServer) -> Vec<String> {
    gitea
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path().ends_with("/merge"))
        .filter_map(|r| {
            let body: Value = serde_json::from_slice(&r.body).ok()?;
            Some(body.get("merge_message_field")?.as_str()?.to_owned())
        })
        .collect()
}

/// AP-104, T-2661: a build the Portal checked and published does not wait for a person. A person
/// approved its source; the Portal merges the lane's Change itself and says so in the merge commit.
#[tokio::test]
async fn a_checked_build_is_merged_by_the_portal_and_the_commit_names_it() {
    let gitea = forge().await;
    built_on_forge(&gitea, COMMIT, BUNDLE, None).await;
    package_takes(&gitea, 201).await;
    change_holds(&gitea, &[&manifest_path()]).await;
    let state = state_with(&gitea);

    let accepted = as_lane(&state, app(Some(build()), None)).await;
    assert_eq!(accepted.status, StatusCode::ACCEPTED, "{}", accepted.text);
    let change: Value = serde_json::from_str(&accepted.text).expect("a Change");
    assert_eq!(change["status"]["phase"], "Deploying", "{change}");

    let merged = merges(&gitea).await;
    assert_eq!(merged.len(), 1, "one merge: {merged:?}");
    assert!(
        merged[0].contains("Approved by the Portal") && merged[0].contains("AP-104"),
        "the merge commit names the Portal as the approver: {}",
        merged[0]
    );
}

/// AP-73: the lane's field and nothing else. A Change that carries another file beside the App's
/// manifest is no longer the build alone, so it waits for a person and nothing is merged.
#[tokio::test]
async fn a_build_change_that_carries_another_file_waits_for_a_person() {
    let gitea = forge().await;
    built_on_forge(&gitea, COMMIT, BUNDLE, None).await;
    package_takes(&gitea, 201).await;
    change_holds(
        &gitea,
        &[
            &manifest_path(),
            "projects/ovzdusie/spaces/ovzdusie/policies/app-air-quality.yaml",
        ],
    )
    .await;
    let state = state_with(&gitea);

    let accepted = as_lane(&state, app(Some(build()), None)).await;
    assert_eq!(accepted.status, StatusCode::ACCEPTED, "{}", accepted.text);
    let change: Value = serde_json::from_str(&accepted.text).expect("a Change");
    assert_eq!(change["status"]["phase"], "PendingApproval", "{change}");
    assert!(merges(&gitea).await.is_empty(), "nothing is merged");
}

/// A merge the forge refuses (the branch moved, a conflict with `main`) leaves the lane's Change
/// waiting for a person, the way it waited before the Portal approved builds; the write stands.
#[tokio::test]
async fn a_build_change_the_forge_will_not_merge_waits_for_a_person() {
    let gitea = forge().await;
    built_on_forge(&gitea, COMMIT, BUNDLE, None).await;
    package_takes(&gitea, 201).await;
    change_holds(&gitea, &[&manifest_path()]).await;
    Mock::given(method("POST"))
        .and(path(format!("{}/pulls/9/merge", common::REPO)))
        .respond_with(ResponseTemplate::new(409).set_body_json(json!({ "message": "conflict" })))
        .with_priority(1)
        .mount(&gitea)
        .await;
    let state = state_with(&gitea);

    let accepted = as_lane(&state, app(Some(build()), None)).await;
    assert_eq!(accepted.status, StatusCode::ACCEPTED, "{}", accepted.text);
    let change: Value = serde_json::from_str(&accepted.text).expect("a Change");
    assert_eq!(change["status"]["phase"], "PendingApproval", "{change}");
}

/// A person's App change is theirs to have approved: the build lane's approval never reaches it.
#[tokio::test]
async fn a_persons_app_change_is_not_approved_by_the_portal() {
    let gitea = forge().await;
    change_holds(&gitea, &[&manifest_path()]).await;
    let state = state_with(&gitea);

    let proposed = send(&state, person("jana"), "POST", APPS, Some(app(None, None))).await;
    assert_eq!(proposed.status, StatusCode::ACCEPTED, "{}", proposed.text);
    assert!(merges(&gitea).await.is_empty(), "nothing is merged");
}

/// The OCI layout `lane.mjs image` writes, as a tar, and its manifest digest.
fn image_layout() -> (Vec<u8>, String, Vec<u8>) {
    let config = br#"{"architecture":"amd64","os":"linux"}"#.to_vec();
    let layer = b"the layer holding /app".to_vec();
    let manifest = serde_json::to_vec(&json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": { "mediaType": "application/vnd.oci.image.config.v1+json", "digest": digest(&config), "size": config.len() },
        "layers": [{ "mediaType": "application/vnd.oci.image.layer.v1.tar+gzip", "digest": digest(&layer), "size": layer.len() }],
    }))
    .expect("json");
    let image = digest(&manifest);
    let index = serde_json::to_vec(&json!({
        "schemaVersion": 2,
        "manifests": [{ "mediaType": "application/vnd.oci.image.manifest.v1+json", "digest": image, "size": manifest.len() }],
    }))
    .expect("json");
    let mut tar = tar::Builder::new(Vec::new());
    for (name, bytes) in [
        ("index.json".to_owned(), &index),
        (format!("blobs/sha256/{}", &digest(&config)[7..]), &config),
        (format!("blobs/sha256/{}", &digest(&layer)[7..]), &layer),
        (format!("blobs/sha256/{}", &image[7..]), &manifest),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, name, bytes.as_slice())
            .expect("append");
    }
    (tar.into_inner().expect("tar"), image, manifest)
}

/// The container registry of the forge: a token for the Portal's own token, no blob held yet,
/// and a manifest stored under the digest `stored`.
async fn registry_takes(gitea: &MockServer, stored: &str) {
    Mock::given(method("GET"))
        .and(path("/v2/token"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "token": "registry-bearer" })),
        )
        .mount(gitea)
        .await;
    Mock::given(method("HEAD"))
        .and(wiremock::matchers::path_regex(
            "^/v2/test-owner/app-air-quality/blobs/sha256:.*",
        ))
        .respond_with(ResponseTemplate::new(404))
        .mount(gitea)
        .await;
    Mock::given(method("POST"))
        .and(path("/v2/test-owner/app-air-quality/blobs/uploads/"))
        .respond_with(ResponseTemplate::new(201))
        .mount(gitea)
        .await;
    Mock::given(method("PUT"))
        .and(path(format!(
            "/v2/test-owner/app-air-quality/manifests/{COMMIT}"
        )))
        .respond_with(ResponseTemplate::new(201).insert_header("Docker-Content-Digest", stored))
        .mount(gitea)
        .await;
}

fn fullstack(status: Value) -> Value {
    let mut manifest = app(Some(status), None);
    manifest["spec"]["kind"] = json!("fullstack");
    manifest["spec"]["build"] = json!({ "rust": "1.90", "node": "22" });
    manifest["spec"]["visibility"] = json!("project");
    manifest
}

/// AP-105, AP-107: a fullstack App's build is the image in `image-{commit}`: the Portal pushes
/// each blob with its digest and the manifest's bytes unchanged as `app-{name}:{commit}`, with
/// the registry token its own token bought, and writes the build once the registry stored it.
#[tokio::test]
async fn a_fullstack_build_is_pushed_to_the_registry_as_checked_then_written() {
    let (layout, image, manifest) = image_layout();
    let gitea = forge().await;
    built_on_forge_as(&gitea, COMMIT, "image", &layout, None).await;
    registry_takes(&gitea, &image).await;
    package_takes(&gitea, 201).await;
    let state = state_with(&gitea);
    let mut status = build();
    status["build"]["digest"] = json!(image);

    let accepted = as_lane(&state, fullstack(status)).await;
    assert_eq!(accepted.status, StatusCode::ACCEPTED, "{}", accepted.text);

    let requests = gitea.received_requests().await.unwrap_or_default();
    let uploads: Vec<String> = requests
        .iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path().ends_with("/blobs/uploads/"))
        .map(|r| r.url.query().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(
        uploads.len(),
        2,
        "the config and the layer, each with its digest: {uploads:?}"
    );
    assert!(uploads.iter().all(|q| q.starts_with("digest=sha256%3A")));
    let pushed = requests
        .iter()
        .find(|r| r.method.as_str() == "PUT" && r.url.path().contains("/manifests/"))
        .expect("the manifest was pushed");
    assert_eq!(
        pushed.body, manifest,
        "the manifest's bytes, as they were hashed"
    );
    assert_eq!(
        pushed.headers["content-type"],
        "application/vnd.oci.image.manifest.v1+json"
    );
    assert_eq!(pushed.headers["authorization"], "Bearer registry-bearer");
    assert_eq!(
        published(&gitea).await,
        vec![format!("/{}/sbom.cdx.json", version(&image))],
        "the SBOM beside it; an image is no bundle"
    );
    assert!(committed(&gitea)
        .await
        .iter()
        .any(|file| file.contains(&image)));
}

/// AP-107: a registry that stores another digest than the one proposed is a refusal, and the
/// build is not written; so is a layout whose manifest is not the proposed digest.
#[tokio::test]
async fn a_fullstack_build_the_registry_stores_otherwise_is_refused() {
    let (layout, image, _) = image_layout();
    let other = format!("sha256:{}", "e".repeat(64));
    for (proposed, stored, says) in [
        (image.clone(), other.clone(), "the registry stored"),
        (other.clone(), image.clone(), "names no manifest"),
    ] {
        let gitea = forge().await;
        built_on_forge_as(&gitea, COMMIT, "image", &layout, None).await;
        registry_takes(&gitea, &stored).await;
        package_takes(&gitea, 201).await;
        let state = state_with(&gitea);
        let mut status = build();
        status["build"]["digest"] = json!(proposed);

        let refused = as_lane(&state, fullstack(status)).await;
        assert!(refused.status.is_client_error(), "{}", refused.text);
        assert!(refused.text.contains(says), "{}", refused.text);
        assert!(
            committed(&gitea).await.is_empty(),
            "the build was written: {says}"
        );
    }
}

/// AP-73, T-2636: the lane's rule writes `status.build` of an App on `main` and authorizes
/// nothing else. A stolen lane token cannot create an App, change one's spec or labels beside a
/// build, or propose an App change with no build at all; and its build write commits no grant.
#[tokio::test]
async fn the_lanes_rule_writes_the_build_of_an_app_on_main_and_nothing_else() {
    let gitea = forge().await;
    built_on_forge(&gitea, COMMIT, BUNDLE, None).await;
    package_takes(&gitea, 201).await;
    let state = state_with(&gitea);

    let created = send(
        &state,
        person("builder"),
        "POST",
        APPS,
        Some(app(Some(build()), None)),
    )
    .await;
    assert_eq!(created.status, StatusCode::FORBIDDEN, "{}", created.text);
    assert!(created.text.contains("holds no App"), "{}", created.text);

    let on_main: joinedcontext_portal::resource::ResourceEnvelope =
        serde_json::from_value(app(None, None)).expect("an App");
    state.mirror.upsert(on_main);
    let one = format!("{APPS}/air-quality");

    let mut wider = app(Some(build()), None);
    wider["spec"]["visibility"] = json!("organization");
    let mut labelled = app(Some(build()), None);
    labelled["metadata"]["labels"] = json!({ "team": "lane" });
    for (what, body) in [("a spec change", wider), ("a label", labelled)] {
        let refused = send(&state, person("builder"), "PUT", &one, Some(body)).await;
        assert_eq!(
            refused.status,
            StatusCode::FORBIDDEN,
            "{what}: {}",
            refused.text
        );
        assert!(
            refused.text.contains("nothing else"),
            "{what}: {}",
            refused.text
        );
    }

    let mut plain = app(None, None);
    plain["spec"]["visibility"] = json!("organization");
    let refused = send(&state, person("builder"), "PUT", &one, Some(plain)).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.text);
    assert!(committed(&gitea).await.is_empty(), "nothing was written");

    let accepted = send(
        &state,
        person("builder"),
        "PUT",
        &one,
        Some(app(Some(build()), None)),
    )
    .await;
    assert_eq!(accepted.status, StatusCode::ACCEPTED, "{}", accepted.text);
    let commits: Vec<Value> = gitea
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path().ends_with("/contents"))
        .filter_map(|r| serde_json::from_slice(&r.body).ok())
        .collect();
    assert!(
        commits
            .iter()
            .all(|c| c["files"].as_array().is_none_or(Vec::is_empty)),
        "a build write carries no Endpoint or Policy: {commits:?}"
    );
}

// ---- wasm Apps (AP-149, AP-151, T-3360) ----

/// The wasm tests provision one App, whose roles are the server's and not a database's: two at
/// once would both create them.
static ONE_APP_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A component's preamble and one byte: what the lane packs at `.jc/component.wasm`.
const COMPONENT: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00, 0x07];

fn wasm_app(status: Option<Value>) -> Value {
    let mut manifest = app(status, None);
    manifest["spec"]["kind"] = json!("wasm");
    manifest["spec"]["build"] = json!({ "node": "22", "rust": "1.90" });
    manifest["spec"]["storage"] = json!({ "sql": { "migrations": "migrations" }, "blob": {} });
    manifest
}

/// The bundle a wasm App's lane uploads: the interface and the component.
fn wasm_bundle() -> Vec<u8> {
    let mut tar = tar::Builder::new(Vec::new());
    for (path, bytes) in [
        ("index.html", b"<html></html>".as_slice()),
        (".jc/component.wasm", COMPONENT),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, path, bytes).expect("entry");
    }
    let raw = tar.into_inner().expect("tar");
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    std::io::Write::write_all(&mut gz, &raw).expect("gz");
    gz.finish().expect("gz")
}

/// The App's repository at `COMMIT` holds `files` under `migrations/`.
async fn migrations_on_forge(gitea: &MockServer, files: &[(&str, &str)]) {
    use base64::Engine;
    let tree: Vec<Value> = files
        .iter()
        .map(|(name, _)| json!({ "path": format!("migrations/{name}"), "type": "blob", "sha": format!("blob-{name}") }))
        .chain([json!({ "path": "server/src/lib.rs", "type": "blob", "sha": "blob-lib" })])
        .collect();
    Mock::given(method("GET"))
        .and(path(format!("{APP_REPO}/git/trees/{COMMIT}")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "sha": COMMIT, "tree": tree, "truncated": false })),
        )
        .mount(gitea)
        .await;
    for (name, sql) in files {
        Mock::given(method("GET"))
            .and(path(format!("{APP_REPO}/contents/migrations/{name}")))
            .and(query_param("ref", COMMIT))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": format!("blob-{name}"),
                "content": base64::engine::general_purpose::STANDARD.encode(sql),
            })))
            .mount(gitea)
            .await;
    }
}

/// The object store: every PUT taken, as RustFS answers a write it may make.
async fn object_store() -> MockServer {
    let store = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&store)
        .await;
    store
}

fn wasm_apps(
    world: &common::apps_db::World,
    store: &MockServer,
) -> std::sync::Arc<joinedcontext_portal::apps::wasm_apps::WasmApps> {
    let client = joinedcontext_portal::artifact_store::Client::new(
        joinedcontext_portal::artifact_store::Settings {
            endpoint: store.uri(),
            bucket: "jc-artifacts".into(),
            region: "us-east-1".into(),
            root_access_key: "root".into(),
            root_secret_key: "root-secret".into(),
        },
    )
    .expect("store client");
    std::sync::Arc::new(joinedcontext_portal::apps::wasm_apps::WasmApps {
        db: world.db.clone(),
        store: std::sync::Arc::new(client),
        bucket: "apps".into(),
    })
}

/// AP-149, AP-151: the lane proposes a wasm App's build; the Portal takes the component from the
/// bundle it checked, stores it by digest, places the App, migrates its schema from the App's own
/// repository at the built commit, and only then publishes and commits the status it computed.
#[tokio::test]
async fn a_wasm_app_is_published_component_stored_shard_kept_schema_migrated() {
    let _one = ONE_APP_AT_A_TIME.lock().await;
    let gitea = forge().await;
    let bundle = wasm_bundle();
    built_on_forge(&gitea, COMMIT, &bundle, None).await;
    package_takes(&gitea, 201).await;
    migrations_on_forge(
        &gitea,
        &[(
            "0001_notes.sql",
            "CREATE TABLE notes (id bigserial PRIMARY KEY, body text NOT NULL)",
        )],
    )
    .await;
    let store = object_store().await;
    let world = common::apps_db::world("publish").await;
    let state = state_with(&gitea).with_wasm_apps(wasm_apps(&world, &store));

    let mut status = build();
    status["build"]["digest"] = json!(digest(&bundle));
    // A component the lane names is never what is committed (AP-151).
    status["build"]["component"] = json!(format!("sha256:{}", "0".repeat(64)));
    let accepted = as_lane(&state, wasm_app(Some(status))).await;
    assert_eq!(accepted.status, StatusCode::ACCEPTED, "{}", accepted.text);

    let component = format!("{:x}", Sha256::digest(COMPONENT));
    let stored: Vec<_> = store
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.method.as_str() == "PUT")
        .collect();
    assert_eq!(stored.len(), 1, "the component, once");
    assert_eq!(
        stored[0].url.path(),
        format!("/apps/components/sha256-{component}.wasm")
    );
    assert_eq!(stored[0].body, COMPONENT);

    let manifest = committed(&gitea).await.join("\n");
    assert!(
        manifest.contains(&format!("component: sha256:{component}")),
        "{manifest}"
    );
    assert!(
        !manifest.contains(&"0".repeat(64)),
        "the lane's component was committed: {manifest}"
    );
    assert!(
        manifest.contains("shard: 0") || manifest.contains("shard: 1"),
        "{manifest}"
    );
    assert_eq!(published(&gitea).await.len(), 2, "the bundle and the SBOM");

    let id = joinedcontext_portal::apps::apps_db::app_id("ovzdusie", "air-quality");
    let tables: i64 = sqlx::query_scalar("SELECT count(*) FROM information_schema.tables WHERE table_schema = $1 AND table_name = 'notes'")
        .bind(format!("app_{id}"))
        .fetch_one(&world.superuser)
        .await
        .expect("catalog");
    assert_eq!(tables, 1, "the App's migration ran in its own schema");
}

/// AP-149: a migration that fails holds the publication and names its file; nothing is
/// published and nothing committed.
#[tokio::test]
async fn a_failed_migration_holds_a_wasm_apps_publication() {
    let _one = ONE_APP_AT_A_TIME.lock().await;
    let gitea = forge().await;
    let bundle = wasm_bundle();
    built_on_forge(&gitea, COMMIT, &bundle, None).await;
    package_takes(&gitea, 201).await;
    migrations_on_forge(
        &gitea,
        &[(
            "0001_broken.sql",
            "CREATE TABLE notes (id bigserial PRIMARY KEY,",
        )],
    )
    .await;
    let store = object_store().await;
    let world = common::apps_db::world("held").await;
    let state = state_with(&gitea).with_wasm_apps(wasm_apps(&world, &store));

    let mut status = build();
    status["build"]["digest"] = json!(digest(&bundle));
    let refused = as_lane(&state, wasm_app(Some(status))).await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{}", refused.text);
    assert!(refused.text.contains("0001_broken.sql"), "{}", refused.text);
    assert!(published(&gitea).await.is_empty(), "nothing was published");
    assert!(committed(&gitea).await.is_empty(), "nothing was committed");
}

/// Without an apps database a wasm App is not published, and the answer says so (AP-149).
#[tokio::test]
async fn a_portal_without_an_apps_database_publishes_no_wasm_app() {
    let gitea = forge().await;
    let bundle = wasm_bundle();
    built_on_forge(&gitea, COMMIT, &bundle, None).await;
    package_takes(&gitea, 201).await;
    let state = state_with(&gitea);
    let mut status = build();
    status["build"]["digest"] = json!(digest(&bundle));
    let refused = as_lane(&state, wasm_app(Some(status))).await;
    assert_eq!(
        refused.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        refused.text
    );
    assert!(
        refused.text.contains("no apps database"),
        "{}",
        refused.text
    );
    assert!(published(&gitea).await.is_empty());
}
