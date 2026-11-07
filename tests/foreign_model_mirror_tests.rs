//! The foreign-model mirror end to end: reference → surface → peer → merge request
//! (T-0460, DM-48, DM-49).
//!
//! The peer is a fake `SchemaApi` rather than a mock HTTP server, because a schema surface has
//! to be `https` before `jcctl` will read it and no local mock offers that. What the real
//! transport does with a plaintext URL, a redirect and an oversized body is asserted in its own
//! unit tests next to it; what is asserted here is the pass over the repository: who is due,
//! what is fetched, and where the result lands.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use jcctl::foreign_models::{FetchError, SchemaApi};
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::sync::mirror::{run_once, LastFetch, Peers};
use serde_json::json;
use sha2::{Digest, Sha256};
use wiremock::matchers::{method, path, path_regex, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const REVISION: &str = "c0ffee1234567890";
const PUBLIC_URL: &str = "https://portal.example";
const SLUG: &str = "k7m2qz4tv6xh3n5jb2ryd3wcfa";
const LINKML: &str = "id: https://peer.example/air\nname: air\n";
const CONTEXT: &str = "{\"@context\":{\"pm10\":\"https://peer.example/air#pm10\"}}";

fn digest(body: &str) -> String {
    Sha256::digest(body.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The peer, answering exactly what a schema surface answers.
struct FakePeer {
    documents: HashMap<String, Vec<u8>>,
    fetches: AtomicUsize,
}

impl FakePeer {
    fn new() -> Self {
        let base = format!("{PUBLIC_URL}/api/endpoint/{SLUG}");
        let mut documents = HashMap::new();
        documents.insert(
            format!("{base}/schema/index.json"),
            json!({
                "models": [{
                    "name": "air",
                    "semver": "1.2.0",
                    "version": 1,
                    "types": ["AirQualityObserved"],
                    "artifacts": {
                        "model.linkml.yaml": { "sha256": digest(LINKML) },
                        "context.jsonld": { "sha256": digest(CONTEXT) },
                    },
                }]
            })
            .to_string()
            .into_bytes(),
        );
        documents.insert(
            format!("{base}/schema/v1/model.linkml.yaml"),
            LINKML.as_bytes().to_vec(),
        );
        documents.insert(
            format!("{base}/schema/v1/context.jsonld"),
            CONTEXT.as_bytes().to_vec(),
        );
        Self {
            documents,
            fetches: AtomicUsize::new(0),
        }
    }
}

impl SchemaApi for FakePeer {
    fn get(&self, url: &str) -> Result<Option<Vec<u8>>, FetchError> {
        self.fetches.fetch_add(1, Ordering::Relaxed);
        Ok(self.documents.get(url).cloned())
    }
}

/// What the forge was asked to do, in the order it was asked.
#[derive(Default)]
struct Forge {
    branches: Mutex<Vec<String>>,
    writes: Mutex<Vec<(String, String)>>,
    pulls: Mutex<Vec<(String, String)>>,
}

/// A forge serving one revision of a repository and recording what is written to it.
async fn forge(files: &[(&str, String)]) -> (MockServer, Arc<Forge>) {
    let server = MockServer::start().await;
    let recorded = Arc::new(Forge::default());

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"default_branch": "main"})))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches/main"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"name": "main", "commit": {"id": REVISION}})),
        )
        .mount(&server)
        .await;

    let tree: Vec<serde_json::Value> = files
        .iter()
        .map(|(p, _)| json!({"path": p, "type": "blob"}))
        .collect();
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/repos/test-owner/test-repo/git/trees/{REVISION}"
        )))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"sha": "tree-sha", "truncated": false, "tree": tree})),
        )
        .mount(&server)
        .await;

    for (file_path, content) in files {
        Mock::given(method("GET"))
            .and(path(format!(
                "/api/v1/repos/test-owner/test-repo/contents/{file_path}"
            )))
            .and(query_param("ref", REVISION))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": "blob-sha",
                "content": STANDARD.encode(content.as_bytes()),
            })))
            .mount(&server)
            .await;
    }

    // Anything else the mirror reads from the branch it is building: the branch is new, so the
    // file is not there yet and the write is a create.
    Mock::given(method("GET"))
        .and(path_regex(
            r"^/api/v1/repos/test-owner/test-repo/contents/.*",
        ))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let branches = Arc::clone(&recorded);
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches"))
        .respond_with(move |request: &Request| {
            let body: serde_json::Value = request.body_json().unwrap_or_default();
            branches.branches.lock().expect("the branch log").push(
                body["new_branch_name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            );
            ResponseTemplate::new(201).set_body_json(json!({"name": "branch"}))
        })
        .mount(&server)
        .await;

    let writes = Arc::clone(&recorded);
    Mock::given(method("PUT"))
        .and(path_regex(
            r"^/api/v1/repos/test-owner/test-repo/contents/.*",
        ))
        .respond_with(move |request: &Request| {
            let body: serde_json::Value = request.body_json().unwrap_or_default();
            let file = request.url.path().rsplit("/contents/").next().unwrap_or("");
            writes.writes.lock().expect("the write log").push((
                file.to_owned(),
                body["branch"].as_str().unwrap_or_default().to_owned(),
            ));
            ResponseTemplate::new(201)
                .set_body_json(json!({"commit": {"sha": "commit-sha"}, "sha": "new-sha"}))
        })
        .mount(&server)
        .await;

    let pulls = Arc::clone(&recorded);
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .respond_with(move |request: &Request| {
            let body: serde_json::Value = request.body_json().unwrap_or_default();
            pulls.pulls.lock().expect("the pull log").push((
                body["head"].as_str().unwrap_or_default().to_owned(),
                body["base"].as_str().unwrap_or_default().to_owned(),
            ));
            ResponseTemplate::new(201).set_body_json(json!({
                "number": 7,
                "html_url": "https://forge.example/pulls/7",
                "state": "open",
                "title": body["title"],
            }))
        })
        .mount(&server)
        .await;

    (server, recorded)
}

fn client(server: &MockServer) -> GiteaClient {
    GiteaClient::new(
        server.uri().parse().expect("a url"),
        "test-owner",
        "test-repo",
        "token",
    )
    .expect("a client")
}

fn reference() -> (&'static str, String) {
    (
        "projects/bb/shared/peer.yaml",
        format!(
            "apiVersion: joinedcontext.com/v1alpha1\n\
             kind: SharedSpaceReference\n\
             metadata:\n  name: peer\n  namespace: bb\n\
             spec:\n  endpointSlug: {SLUG}\n  alias: partner\n"
        ),
    )
}

/// The mirror the repository would hold after a previous run pinned this exact document.
fn already_mirrored(sha256: &str) -> (&'static str, String) {
    (
        "projects/bb/spaces/partner/datamodels/peer-air.yaml",
        format!(
            "apiVersion: joinedcontext.com/v1alpha1\n\
             kind: DataModel\n\
             metadata:\n  name: peer-air\n  namespace: bb\n\
             spec:\n\
             \x20 contextSpaceRef: partner\n\
             \x20 linkml: peer-air/model.linkml.yaml\n\
             \x20 version: 1.2.0\n\
             \x20 lifecycle: mirrored\n\
             \x20 classes: [AirQualityObserved]\n\
             \x20 artifacts:\n    context: peer-air/context.jsonld\n\
             \x20 source:\n    remote:\n\
             \x20     url: {PUBLIC_URL}/api/endpoint/{SLUG}/schema/v1/model.linkml.yaml\n\
             \x20     version: 1.2.0\n\
             \x20     sha256: {sha256}\n\
             \x20     fetchedAt: 2026-09-01T00:00:00Z\n"
        ),
    )
}

fn peers(peer: &Arc<FakePeer>) -> Peers {
    Arc::clone(peer) as Peers
}

#[tokio::test]
async fn a_peer_that_publishes_a_model_reaches_a_merge_request() {
    let (server, recorded) = forge(&[reference()]).await;
    let peer = Arc::new(FakePeer::new());

    let run = run_once(
        &client(&server),
        PUBLIC_URL,
        &peers(&peer),
        &LastFetch::default(),
        1_757_000_000,
    )
    .await
    .expect("the pass runs");

    assert_eq!(run.proposed.len(), 1, "flags: {:?}", run.flags);
    assert_eq!(run.proposed[0], "https://forge.example/pulls/7");

    let writes = recorded.writes.lock().expect("the write log").clone();
    let paths: Vec<&str> = writes.iter().map(|(path, _)| path.as_str()).collect();
    assert!(
        paths.contains(&"projects/bb/spaces/partner/datamodels/peer-air.yaml"),
        "the mirrored DataModel: {paths:?}"
    );
    assert!(
        paths.contains(&"projects/bb/spaces/partner/datamodels/peer-air/model.linkml.yaml"),
        "the document the digest is over: {paths:?}"
    );
    assert!(
        paths.contains(&"projects/bb/spaces/partner/datamodels/peer-air/context.jsonld"),
        "the @context beside it: {paths:?}"
    );
}

#[tokio::test]
async fn nothing_a_peer_publishes_lands_on_the_branch_that_is_applied() {
    let (server, recorded) = forge(&[reference()]).await;
    let peer = Arc::new(FakePeer::new());

    run_once(
        &client(&server),
        PUBLIC_URL,
        &peers(&peer),
        &LastFetch::default(),
        1_757_000_000,
    )
    .await
    .expect("the pass runs");

    let branch = recorded.branches.lock().expect("the branch log").clone();
    assert_eq!(branch.len(), 1, "one branch for one peer: {branch:?}");
    assert!(branch[0].starts_with("mirror/"), "{branch:?}");

    for (path, target) in recorded.writes.lock().expect("the write log").iter() {
        assert_eq!(
            target, &branch[0],
            "{path} was written somewhere other than the review branch"
        );
    }

    let pulls = recorded.pulls.lock().expect("the pull log").clone();
    assert_eq!(pulls, vec![(branch[0].clone(), "main".to_owned())]);
}

#[tokio::test]
async fn a_peer_whose_digest_the_repository_already_pins_produces_no_commit() {
    let (server, recorded) = forge(&[reference(), already_mirrored(&digest(LINKML))]).await;
    let peer = Arc::new(FakePeer::new());

    let run = run_once(
        &client(&server),
        PUBLIC_URL,
        &peers(&peer),
        &LastFetch::default(),
        1_757_000_000,
    )
    .await
    .expect("the pass runs");

    assert!(run.proposed.is_empty(), "flags: {:?}", run.flags);
    assert_eq!(run.unchanged, 1);
    assert!(recorded.branches.lock().expect("the branch log").is_empty());
    assert!(recorded.writes.lock().expect("the write log").is_empty());
    assert!(recorded.pulls.lock().expect("the pull log").is_empty());
}

#[tokio::test]
async fn a_peer_the_repository_pins_at_another_digest_is_proposed_again() {
    let (server, _) = forge(&[reference(), already_mirrored(&"0".repeat(64))]).await;
    let peer = Arc::new(FakePeer::new());

    let run = run_once(
        &client(&server),
        PUBLIC_URL,
        &peers(&peer),
        &LastFetch::default(),
        1_757_000_000,
    )
    .await
    .expect("the pass runs");

    assert_eq!(run.proposed.len(), 1, "flags: {:?}", run.flags);
}

#[tokio::test]
async fn the_schedule_decides_whether_a_peer_is_fetched_at_all() {
    let (server, _) = forge(&[reference()]).await;
    let peer = Arc::new(FakePeer::new());
    let last_fetch = LastFetch::default();
    let gitea = client(&server);
    let now = 1_757_000_000;

    run_once(&gitea, PUBLIC_URL, &peers(&peer), &last_fetch, now)
        .await
        .expect("the first pass runs");
    let after_first = peer.fetches.load(Ordering::Relaxed);
    assert!(after_first > 0, "the first pass fetches");

    run_once(&gitea, PUBLIC_URL, &peers(&peer), &last_fetch, now + 3_600)
        .await
        .expect("the second pass runs");
    assert_eq!(
        peer.fetches.load(Ordering::Relaxed),
        after_first,
        "an hour later is not a day later, and the default mirror schedule is a day"
    );

    run_once(&gitea, PUBLIC_URL, &peers(&peer), &last_fetch, now + 90_000)
        .await
        .expect("the third pass runs");
    assert!(
        peer.fetches.load(Ordering::Relaxed) > after_first,
        "a day later the peer is read again"
    );
}

#[tokio::test]
async fn a_peer_that_publishes_no_surface_is_flagged_and_nothing_else() {
    let (server, recorded) = forge(&[reference()]).await;
    // A peer with no documents at all: `schema/index.json` answers 404.
    let peer = Arc::new(FakePeer {
        documents: HashMap::new(),
        fetches: AtomicUsize::new(0),
    });

    let run = run_once(
        &client(&server),
        PUBLIC_URL,
        &peers(&peer),
        &LastFetch::default(),
        1_757_000_000,
    )
    .await
    .expect("the pass runs");

    assert!(run.proposed.is_empty());
    assert!(recorded.pulls.lock().expect("the pull log").is_empty());
    assert_eq!(
        run.unchanged, 1,
        "a peer with nothing to mirror is not an error"
    );
}
