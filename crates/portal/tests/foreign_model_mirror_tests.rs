//! The foreign-model mirror end to end: reference → surface → peer → merge request
//! (T-0460, DM-48, DM-49).
//!
//! The peer is a fake `SchemaApi` rather than a mock HTTP server, because a schema surface has
//! to be `https` before `jcctl` will read it and no local mock offers that. What the real
//! transport does with a plaintext URL, a redirect and an oversized body is asserted in its own
//! unit tests next to it; what is asserted here is the pass over the repository: who is due,
//! what is fetched, and where the result lands.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use jcctl::foreign_models::{FetchError, SchemaApi};
use jcctl::loader::Repository;
use joinedcontext_portal::forge::ShakoClient;
use joinedcontext_portal::sync::mirror::{run_once, LastFetch, Peers, Run, Transports, Tree};
use serde_json::json;
use sha2::{Digest, Sha256};
use wiremock::matchers::{method, path, query_param};
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
    forge_named("test-repo", files).await
}

/// [`forge`] for the repository `repo` of `test-owner`.
async fn forge_named(repo: &str, files: &[(&str, String)]) -> (MockServer, Arc<Forge>) {
    let at = |rest: &str| format!("/odata/Repositories(Owner='test-owner',Name='{repo}'){rest}");
    let server = MockServer::start().await;
    let recorded = Arc::new(Forge::default());

    Mock::given(method("GET"))
        .and(path(at("")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "DefaultBranch": "main" })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path(at("/Branches('main')")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "Name": "main", "HeadSha": REVISION })),
        )
        .mount(&server)
        .await;

    let tree: Vec<serde_json::Value> = files
        .iter()
        .map(|(p, _)| json!({ "Path": p, "Type": "blob" }))
        .collect();
    Mock::given(method("GET"))
        .and(path(at(
            "/Shako.Tree(Ref=@r,Recursive=true,Skip=@s,Top=@t)",
        )))
        .and(query_param("@r", format!("'{REVISION}'")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "Sha": "tree-sha", "Truncated": false, "Tree": tree })),
        )
        .mount(&server)
        .await;

    for (file_path, content) in files {
        Mock::given(method("GET"))
            .and(path(at("/Shako.File(Path=@p,Ref=@r)")))
            .and(query_param("@p", format!("'{file_path}'")))
            .and(query_param("@r", format!("'{}'", REVISION)))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "Sha": "blob-sha",
                "Content": STANDARD.encode(content.as_bytes())
            })))
            .mount(&server)
            .await;
    }

    // Anything else the mirror reads from the branch it is building: the branch is new, so the
    // file is not there yet and the write is a create.
    Mock::given(method("GET"))
        .and(path(at("/Shako.File(Path=@p,Ref=@r)")))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let branches = Arc::clone(&recorded);
    Mock::given(method("POST"))
        .and(path(at("/Shako.CreateBranch")))
        .respond_with(move |request: &Request| {
            let body: serde_json::Value = request.body_json().unwrap_or_default();
            branches.branches.lock().expect("the branch log").push(
                body["NewBranchName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            );
            ResponseTemplate::new(201).set_body_json(json!({ "Name": "branch" }))
        })
        .mount(&server)
        .await;

    let writes = Arc::clone(&recorded);
    Mock::given(method("POST"))
        .and(path(at("/Shako.CommitFiles")))
        .respond_with(move |request: &Request| {
            let body: serde_json::Value = request.body_json().unwrap_or_default();
            let branch = body["Branch"].as_str().unwrap_or_default().to_owned();
            let mut log = writes.writes.lock().expect("the write log");
            for file in body["Files"].as_array().into_iter().flatten() {
                log.push((
                    file["path"].as_str().unwrap_or_default().to_owned(),
                    branch.clone(),
                ));
            }
            ResponseTemplate::new(201).set_body_json(json!({ "Sha": "commit-sha" }))
        })
        .mount(&server)
        .await;

    let pulls = Arc::clone(&recorded);
    Mock::given(method("POST"))
        .and(path(at("/Shako.OpenPullRequest")))
        .respond_with(move |request: &Request| {
            let body: serde_json::Value = request.body_json().unwrap_or_default();
            pulls.pulls.lock().expect("the pull log").push((
                body["Head"].as_str().unwrap_or_default().to_owned(),
                body["Base"].as_str().unwrap_or_default().to_owned(),
            ));
            ResponseTemplate::new(201).set_body_json(json!({
                "Number": 7,
                "HtmlUrl": "https://forge.example/pulls/7",
                "State": "open",
                "Title": body["Title"]
            }))
        })
        .mount(&server)
        .await;

    (server, recorded)
}

fn client(server: &MockServer) -> ShakoClient {
    ShakoClient::new(
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

/// `files` as the last sync accepted them, loaded from a directory of their own; `admitted`
/// says whether the Endpoint the reference names shares with project `bb`.
fn tree(files: &[(&str, String)], admitted: bool) -> Tree {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after the epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "portal-mirror-{nanos}-{:?}",
        std::thread::current().id()
    ));
    for (path, content) in files {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().expect("a parent")).expect("a directory");
        std::fs::write(file, content).expect("the file");
    }
    std::fs::create_dir_all(&dir).expect("the repository");
    Tree {
        repository: Arc::new(Repository::load(&dir).expect("the repository loads")),
        repositories: BTreeMap::new(),
        admitted: if admitted {
            BTreeSet::from([("bb".to_owned(), "peer".to_owned())])
        } else {
            BTreeSet::new()
        },
    }
}

/// One pass over `files`, the reference admitted by its share, `peer` answering both hops.
async fn pass(
    shako: &ShakoClient,
    files: &[(&str, String)],
    peer: &Arc<FakePeer>,
    last_fetch: &LastFetch,
    now: u64,
) -> Run {
    let transports = Transports {
        peers: &peers(peer),
        local: &peers(peer),
    };
    run_once(
        shako,
        &tree(files, true),
        PUBLIC_URL,
        &transports,
        last_fetch,
        now,
    )
    .await
    .expect("the pass runs")
}

#[tokio::test]
async fn a_peer_that_publishes_a_model_reaches_a_merge_request() {
    let files = [reference()];
    let (server, recorded) = forge(&files).await;
    let peer = Arc::new(FakePeer::new());

    let run = pass(
        &client(&server),
        &files,
        &peer,
        &LastFetch::default(),
        1_757_000_000,
    )
    .await;

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
    let files = [reference()];
    let (server, recorded) = forge(&files).await;
    let peer = Arc::new(FakePeer::new());

    pass(
        &client(&server),
        &files,
        &peer,
        &LastFetch::default(),
        1_757_000_000,
    )
    .await;

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
    let files = [reference(), already_mirrored(&digest(LINKML))];
    let (server, recorded) = forge(&files).await;
    let peer = Arc::new(FakePeer::new());

    let run = pass(
        &client(&server),
        &files,
        &peer,
        &LastFetch::default(),
        1_757_000_000,
    )
    .await;

    assert!(run.proposed.is_empty(), "flags: {:?}", run.flags);
    assert_eq!(run.unchanged, 1);
    assert!(recorded.branches.lock().expect("the branch log").is_empty());
    assert!(recorded.writes.lock().expect("the write log").is_empty());
    assert!(recorded.pulls.lock().expect("the pull log").is_empty());
}

#[tokio::test]
async fn a_peer_the_repository_pins_at_another_digest_is_proposed_again() {
    let files = [reference(), already_mirrored(&"0".repeat(64))];
    let (server, _) = forge(&files).await;
    let peer = Arc::new(FakePeer::new());

    let run = pass(
        &client(&server),
        &files,
        &peer,
        &LastFetch::default(),
        1_757_000_000,
    )
    .await;

    assert_eq!(run.proposed.len(), 1, "flags: {:?}", run.flags);
}

#[tokio::test]
async fn the_schedule_decides_whether_a_peer_is_fetched_at_all() {
    let files = [reference()];
    let (server, _) = forge(&files).await;
    let peer = Arc::new(FakePeer::new());
    let last_fetch = LastFetch::default();
    let shako = client(&server);
    let now = 1_757_000_000;

    pass(&shako, &files, &peer, &last_fetch, now).await;
    let after_first = peer.fetches.load(Ordering::Relaxed);
    assert!(after_first > 0, "the first pass fetches");

    pass(&shako, &files, &peer, &last_fetch, now + 3_600).await;
    assert_eq!(
        peer.fetches.load(Ordering::Relaxed),
        after_first,
        "an hour later is not a day later, and the default mirror schedule is a day"
    );

    pass(&shako, &files, &peer, &last_fetch, now + 90_000).await;
    assert!(
        peer.fetches.load(Ordering::Relaxed) > after_first,
        "a day later the peer is read again"
    );
}

#[tokio::test]
async fn a_peer_that_publishes_no_surface_is_flagged_and_nothing_else() {
    let files = [reference()];
    let (server, recorded) = forge(&files).await;
    // A peer with no documents at all: `schema/index.json` answers 404.
    let peer = Arc::new(FakePeer {
        documents: HashMap::new(),
        fetches: AtomicUsize::new(0),
    });

    let run = pass(
        &client(&server),
        &files,
        &peer,
        &LastFetch::default(),
        1_757_000_000,
    )
    .await;

    assert!(run.proposed.is_empty());
    assert!(recorded.pulls.lock().expect("the pull log").is_empty());
    assert_eq!(
        run.unchanged, 1,
        "a peer with nothing to mirror is not an error"
    );
}

/// EP-15, T-7425: a SharedSpaceReference whose Endpoint does not share with the project is
/// flagged and never read, and a flagged reference waits an hour, not one tick, to be tried again.
#[tokio::test]
async fn a_reference_the_endpoint_does_not_share_is_never_read() {
    let files = [reference()];
    let (server, recorded) = forge(&files).await;
    let peer = Arc::new(FakePeer::new());
    let transports = Transports {
        peers: &peers(&peer),
        local: &peers(&peer),
    };
    let last_fetch = LastFetch::default();
    let unshared = tree(&files, false);
    let now = 1_757_000_000;

    let run = run_once(
        &client(&server),
        &unshared,
        PUBLIC_URL,
        &transports,
        &last_fetch,
        now,
    )
    .await
    .expect("the pass runs");
    assert_eq!(run.flags.len(), 1, "{:?}", run.flags);
    assert!(run.flags[0].contains("EP-15"), "{:?}", run.flags);
    assert_eq!(peer.fetches.load(Ordering::Relaxed), 0, "nothing is read");
    assert!(recorded.pulls.lock().expect("the pull log").is_empty());

    let next_tick = run_once(
        &client(&server),
        &unshared,
        PUBLIC_URL,
        &transports,
        &last_fetch,
        now + 60,
    )
    .await
    .expect("the pass runs");
    assert!(
        next_tick.flags.is_empty(),
        "a minute later: {:?}",
        next_tick.flags
    );
    let an_hour = run_once(
        &client(&server),
        &unshared,
        PUBLIC_URL,
        &transports,
        &last_fetch,
        now + 3_600,
    )
    .await
    .expect("the pass runs");
    assert_eq!(an_hour.flags.len(), 1, "an hour later it is tried again");
}

/// SHR-16, T-7425: an Endpoint this instance serves is read over the in-process hop, never over
/// the one to other instances, on a loopback instance too (the train's gate); and in layout 2 the
/// merge request opens in the consumer project's own repository, at the path that repository
/// spells (CC-85).
#[tokio::test]
async fn a_shared_endpoint_is_read_in_process_and_proposed_in_the_project_repository() {
    const LOOPBACK: &str = "http://127.0.0.1:42101";
    let files = [reference()];
    let (server, recorded) = forge_named("bb-config", &files).await;
    let local = Arc::new(FakePeer::new());
    // The in-process hop sees the URLs of this instance; the fake keys them by PUBLIC_URL.
    struct Loopback(Arc<FakePeer>);
    impl SchemaApi for Loopback {
        fn get(&self, url: &str) -> Result<Option<Vec<u8>>, FetchError> {
            self.0.get(&url.replacen(LOOPBACK, PUBLIC_URL, 1))
        }
    }
    let remote = Arc::new(FakePeer {
        documents: HashMap::new(),
        fetches: AtomicUsize::new(0),
    });
    let transports = Transports {
        peers: &peers(&remote),
        local: &(Arc::new(Loopback(Arc::clone(&local))) as Peers),
    };
    let mut layout2 = tree(&files, true);
    layout2.repositories = BTreeMap::from([("bb".to_owned(), "bb-config".to_owned())]);

    let run = run_once(
        &client(&server),
        &layout2,
        &format!("{LOOPBACK}/"),
        &transports,
        &LastFetch::default(),
        1_757_000_000,
    )
    .await
    .expect("the pass runs");

    assert_eq!(run.proposed.len(), 1, "flags: {:?}", run.flags);
    assert_eq!(
        remote.fetches.load(Ordering::Relaxed),
        0,
        "no hop to another instance"
    );
    assert!(local.fetches.load(Ordering::Relaxed) > 0, "read in process");
    let writes = recorded.writes.lock().expect("the write log").clone();
    let paths: Vec<&str> = writes.iter().map(|(path, _)| path.as_str()).collect();
    assert!(
        paths.contains(&"spaces/partner/datamodels/peer-air.yaml"),
        "the project repository's own path: {paths:?}"
    );
    assert_eq!(recorded.pulls.lock().expect("the pull log").len(), 1);
}
