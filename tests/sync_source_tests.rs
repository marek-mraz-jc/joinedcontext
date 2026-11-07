//! The `SyncSource` loop end to end: origin → decision → branch → merge request → answer
//! (T-0449, MF-27…MF-32).
//!
//! The origin is a fake [`SyncRemote`] rather than a mock HTTP server, because a real origin is
//! `https` before the transport will read it and no local mock offers that. What the real
//! transport does with a plaintext URL, a redirect, an escaping archive entry and a `secretRef`
//! it cannot resolve is asserted in its own unit tests next to it. What is asserted here is the
//! loop around it: when a run happens, where its change lands, and what a reviewer's answer
//! does to the source afterwards.

use std::path::Path;
use std::sync::{Arc, Mutex};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use jc_core::kinds::SyncOrigin;
use jcctl::sync::{RemoteError, SyncRemote};
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::sync::driver::{Driver, Remote};
use joinedcontext_portal::sync::state::States;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, path_regex, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

/// The head of the repository the Portal reconciles.
const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// Where the source stands. Its first seven characters are the branch name's suffix.
const MOVED: &str = "9f1c0de1234567";

const NOW: u64 = 1_757_000_000;

/// What the source publishes: a sandbox space, which is a green-lane create.
const SANDBOX: &str = r#"apiVersion: joinedcontext.com/v1alpha1
kind: ContextSpace
metadata:
  name: regional-sandbox
  namespace: regional
spec:
  isSandbox: true
"#;

/// An origin that answers one revision and writes the documents it was built with.
struct Origin {
    revision: Mutex<String>,
    documents: Vec<(&'static str, String)>,
    calls: Mutex<Vec<String>>,
}

impl Origin {
    fn at(revision: &str) -> Arc<Self> {
        Arc::new(Self {
            revision: Mutex::new(revision.to_owned()),
            documents: vec![("space.yaml", SANDBOX.to_owned())],
            calls: Mutex::new(Vec::new()),
        })
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().expect("the call log").clone()
    }
}

impl SyncRemote for Origin {
    fn revision(&self, _origin: &SyncOrigin) -> Result<String, RemoteError> {
        self.calls
            .lock()
            .expect("the call log")
            .push("revision".into());
        Ok(self.revision.lock().expect("the revision").clone())
    }

    fn checkout(
        &self,
        _origin: &SyncOrigin,
        revision: &str,
        into: &Path,
    ) -> Result<(), RemoteError> {
        self.calls
            .lock()
            .expect("the call log")
            .push(format!("checkout {revision}"));
        for (name, body) in &self.documents {
            std::fs::write(into.join(name), body)
                .map_err(|err| RemoteError::Unavailable(err.to_string()))?;
        }
        Ok(())
    }
}

/// What the forge was asked to do, and what it answers when asked what is open.
#[derive(Default)]
struct Forge {
    branches: Mutex<Vec<String>>,
    writes: Mutex<Vec<(String, String)>>,
    created: Mutex<Vec<(String, String)>>,
    pulls: Mutex<Vec<Value>>,
}

impl Forge {
    fn branches(&self) -> Vec<String> {
        self.branches.lock().expect("the branch log").clone()
    }

    fn created(&self) -> Vec<(String, String)> {
        self.created.lock().expect("the pull log").clone()
    }

    fn writes(&self) -> Vec<(String, String)> {
        self.writes.lock().expect("the write log").clone()
    }

    /// A reviewer's answer to the one open merge request.
    fn answer(&self, merged: bool) {
        for pull in self.pulls.lock().expect("the pull log").iter_mut() {
            pull["state"] = json!("closed");
            pull["merged"] = json!(merged);
        }
    }
}

/// The SyncSource the repository carries.
fn source(schedule: Value) -> String {
    // A webhook-driven source names the secret its origin signs with, or jc-core refuses the
    // manifest (MF-44); every other schedule carries none.
    let webhook = if schedule["webhook"] == json!(true) {
        "  webhook: { secretRef: { name: regional-hook, key: secret } }\n"
    } else {
        ""
    };
    format!(
        "apiVersion: joinedcontext.com/v1alpha1\n\
         kind: SyncSource\n\
         metadata:\n  name: regional\n  namespace: bb\n\
         spec:\n\
         \x20 source:\n    git: {{ url: https://git.region.sk/udp/models.git, ref: main }}\n\
         \x20 schedule: {schedule}\n\
         \x20 mode: mirror\n\
         \x20 conflictPolicy: replace\n\
         {webhook}"
    )
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
                .set_body_json(json!({"name": "main", "commit": {"id": HEAD}})),
        )
        .mount(&server)
        .await;

    let tree: Vec<Value> = files
        .iter()
        .map(|(p, _)| json!({"path": p, "type": "blob"}))
        .collect();
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/repos/test-owner/test-repo/git/trees/{HEAD}"
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
            .and(query_param("ref", HEAD))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": "blob-sha",
                "content": STANDARD.encode(content.as_bytes()),
            })))
            .mount(&server)
            .await;
    }

    // Anything else a run reads from the branch it is building: the branch is new, so the file
    // is not there yet and the write is a create.
    Mock::given(method("GET"))
        .and(path_regex(
            r"^/api/v1/repos/test-owner/test-repo/contents/.*",
        ))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let listing = Arc::clone(&recorded);
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .respond_with(move |_: &Request| {
            ResponseTemplate::new(200)
                .set_body_json(listing.pulls.lock().expect("the pull log").clone())
        })
        .mount(&server)
        .await;

    let branches = Arc::clone(&recorded);
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches"))
        .respond_with(move |request: &Request| {
            let body: Value = request.body_json().unwrap_or_default();
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
            let body: Value = request.body_json().unwrap_or_default();
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
            let body: Value = request.body_json().unwrap_or_default();
            let head = body["head"].as_str().unwrap_or_default().to_owned();
            let base = body["base"].as_str().unwrap_or_default().to_owned();
            pulls
                .created
                .lock()
                .expect("the pull log")
                .push((head.clone(), base.clone()));
            let opened = json!({
                "number": 7,
                "html_url": "https://forge.example/pulls/7",
                "state": "open",
                "title": body["title"],
                "head": { "ref": head },
                "base": { "ref": base },
                "merged": false,
            });
            pulls
                .pulls
                .lock()
                .expect("the pull log")
                .push(opened.clone());
            ResponseTemplate::new(201).set_body_json(opened)
        })
        .mount(&server)
        .await;

    (server, recorded)
}

fn client(server: &MockServer) -> Arc<GiteaClient> {
    Arc::new(
        GiteaClient::new(
            server.uri().parse().expect("a url"),
            "test-owner",
            "test-repo",
            "token",
        )
        .expect("a client"),
    )
}

/// The loop over one repository, one origin and a memory the test can keep across restarts.
fn driver(server: &MockServer, origin: &Arc<Origin>, states: &Arc<States>) -> Driver {
    Driver::new(
        client(server),
        Arc::clone(states),
        Arc::clone(origin) as Remote,
    )
}

async fn repository(schedule: Value) -> (MockServer, Arc<Forge>) {
    forge(&[("projects/bb/sync/regional.yaml", source(schedule))]).await
}

#[tokio::test]
async fn a_source_that_moved_reaches_a_reviewable_merge_request() {
    let (server, recorded) = repository(json!({"interval": "30m"})).await;
    let origin = Origin::at(MOVED);
    let states = Arc::new(States::new(None));

    let run = driver(&server, &origin, &states)
        .tick(NOW)
        .await
        .expect("the pass runs");

    assert_eq!(run.proposed.len(), 1, "flags: {:?}", run.flags);
    assert_eq!(
        run.proposed[0]["status"]["mergeRequest"],
        json!("https://forge.example/pulls/7"),
        "the envelope carries the merge request a reviewer opens"
    );
    assert_eq!(run.proposed[0]["status"]["lane"], json!("green"));

    let branches = recorded.branches();
    assert_eq!(
        branches,
        vec!["chg-sync-regional-9f1c0de".to_owned()],
        "one branch, named after the revision it carries"
    );
    assert_eq!(
        recorded.created(),
        vec![("chg-sync-regional-9f1c0de".to_owned(), "main".to_owned())]
    );

    let writes = recorded.writes();
    assert_eq!(
        writes,
        vec![(
            "projects/bb/spaces/regional-sandbox/space.yaml".to_owned(),
            "chg-sync-regional-9f1c0de".to_owned()
        )],
        "the source's resource lands in the syncing project, on the review branch and nowhere \
         else"
    );

    let status = states.get("bb", "regional").await;
    assert_eq!(status.phase().as_str(), "PendingApproval");
    assert_eq!(
        status.state.observed_revision, None,
        "nothing is imported until somebody merges it"
    );
    assert_eq!(
        status.merge_request.as_deref(),
        Some("https://forge.example/pulls/7")
    );
}

#[tokio::test]
async fn a_restart_mid_flight_does_not_open_a_second_proposal_for_the_same_revision() {
    let (server, recorded) = repository(json!({"interval": "30m"})).await;
    let origin = Origin::at(MOVED);
    let states = Arc::new(States::new(None));

    driver(&server, &origin, &states)
        .tick(NOW)
        .await
        .expect("the first pass runs");

    // A new Driver over the same memory is what a restarted Portal is: nothing cached, the
    // repository read again, the run's state read from where it was recorded.
    let after_restart = driver(&server, &origin, &states);
    let run = after_restart
        .tick(NOW + 3_600)
        .await
        .expect("the pass after the restart runs");

    assert!(run.proposed.is_empty(), "flags: {:?}", run.flags);
    assert_eq!(
        recorded.created().len(),
        1,
        "the proposal a reviewer is already looking at is not opened a second time"
    );
    assert_eq!(
        recorded.branches().len(),
        1,
        "and no second branch is left behind"
    );
    assert_eq!(
        after_restart
            .status("bb", "regional")
            .await
            .phase()
            .as_str(),
        "PendingApproval"
    );
}

#[tokio::test]
async fn merging_the_proposal_is_what_marks_the_revision_observed() {
    let (server, recorded) = repository(json!({"interval": "30m"})).await;
    let origin = Origin::at(MOVED);
    let states = Arc::new(States::new(None));
    let driver = driver(&server, &origin, &states);

    driver.tick(NOW).await.expect("the first pass runs");
    recorded.answer(true);

    let run = driver
        .tick(NOW + 3_600)
        .await
        .expect("the pass after the merge runs");

    assert!(run.proposed.is_empty(), "flags: {:?}", run.flags);
    assert_eq!(run.unchanged, 1);
    assert_eq!(
        recorded.created().len(),
        1,
        "the source has not moved since, so there is nothing to propose"
    );

    let status = driver.status("bb", "regional").await;
    assert_eq!(status.phase().as_str(), "Synced");
    assert_eq!(
        status.state.observed_revision.as_deref(),
        Some(MOVED),
        "the full revision, not the seven characters the branch name carries"
    );
    assert_eq!(status.merge_request, None);
}

#[tokio::test]
async fn a_proposal_a_reviewer_closed_is_an_answer_and_not_a_queue() {
    let (server, recorded) = repository(json!({"interval": "30m"})).await;
    let origin = Origin::at(MOVED);
    let states = Arc::new(States::new(None));
    let driver = driver(&server, &origin, &states);

    driver.tick(NOW).await.expect("the first pass runs");
    recorded.answer(false);

    let run = driver
        .tick(NOW + 3_600)
        .await
        .expect("the pass after the refusal runs");

    assert_eq!(
        recorded.created().len(),
        1,
        "a refused revision is not proposed again every half hour"
    );
    assert!(
        run.flags
            .iter()
            .any(|flag| flag.contains("without merging")),
        "a reader is told why the source stopped: {:?}",
        run.flags
    );
    assert_eq!(
        driver.status("bb", "regional").await.phase().as_str(),
        "Synced"
    );
}

#[tokio::test]
async fn a_source_between_runs_is_not_asked_anything() {
    let (server, recorded) = repository(json!({"interval": "30m"})).await;
    let origin = Origin::at(MOVED);
    let states = Arc::new(States::new(None));
    let driver = driver(&server, &origin, &states);

    driver.tick(NOW).await.expect("the first pass runs");
    recorded.answer(true);
    driver.tick(NOW + 3_600).await.expect("the merge is read");
    let asked = origin.calls().len();

    driver
        .tick(NOW + 3_700)
        .await
        .expect("the pass a hundred seconds later runs");
    assert_eq!(
        origin.calls().len(),
        asked,
        "a schedule of half an hour is half an hour: {:?}",
        origin.calls()
    );
}

#[tokio::test]
async fn a_paused_source_runs_for_nobody_until_it_is_resumed() {
    let (server, recorded) = repository(json!({"interval": "30m"})).await;
    let origin = Origin::at(MOVED);
    let states = Arc::new(States::new(None));
    let driver = driver(&server, &origin, &states);

    driver.pause("bb", "regional", true).await;
    driver.tick(NOW).await.expect("the pass runs");
    assert!(origin.calls().is_empty(), "{:?}", origin.calls());
    assert_eq!(
        driver.status("bb", "regional").await.phase().as_str(),
        "Paused"
    );

    // Not even when somebody asks for it by hand: pause is the switch, and the button that
    // ignores the schedule does not ignore the switch.
    driver
        .run_now("bb", "regional", NOW)
        .await
        .expect("the forced run is judged");
    assert!(recorded.created().is_empty(), "{:?}", recorded.created());

    driver.pause("bb", "regional", false).await;
    let run = driver
        .tick(NOW)
        .await
        .expect("the pass after the resume runs");
    assert_eq!(run.proposed.len(), 1, "flags: {:?}", run.flags);
}

#[tokio::test]
async fn a_webhook_source_runs_when_it_is_told_to_and_never_on_a_timer() {
    let (server, recorded) = repository(json!({"webhook": true})).await;
    let origin = Origin::at(MOVED);
    let states = Arc::new(States::new(None));
    let driver = driver(&server, &origin, &states);

    driver.tick(NOW).await.expect("the pass runs");
    assert!(
        origin.calls().is_empty(),
        "a webhook schedule has no timer to be due on: {:?}",
        origin.calls()
    );

    let run = driver
        .run_now("bb", "regional", NOW)
        .await
        .expect("the run the origin asked for");
    assert_eq!(run.proposed.len(), 1, "flags: {:?}", run.flags);
    assert_eq!(
        recorded.created(),
        vec![("chg-sync-regional-9f1c0de".to_owned(), "main".to_owned())]
    );
}

#[tokio::test]
async fn an_origin_that_did_not_answer_says_so_and_leaves_the_repository_alone() {
    struct Silent;
    impl SyncRemote for Silent {
        fn revision(&self, _origin: &SyncOrigin) -> Result<String, RemoteError> {
            Err(RemoteError::Unavailable(
                "the origin did not answer in time".into(),
            ))
        }
        fn checkout(
            &self,
            _origin: &SyncOrigin,
            _revision: &str,
            _into: &Path,
        ) -> Result<(), RemoteError> {
            unreachable!("nothing is checked out when the revision is unknown")
        }
    }

    let (server, recorded) = repository(json!({"interval": "30m"})).await;
    let states = Arc::new(States::new(None));
    let driver = Driver::new(
        client(&server),
        Arc::clone(&states),
        Arc::new(Silent) as Remote,
    );

    let run = driver.tick(NOW).await.expect("the pass runs");
    assert!(run.proposed.is_empty());
    assert!(
        run.flags.iter().any(|flag| flag.contains("did not answer")),
        "{:?}",
        run.flags
    );
    assert!(recorded.branches().is_empty(), "nothing is written");

    let status = driver.status("bb", "regional").await;
    assert_eq!(status.phase().as_str(), "Error");
    assert!(
        status
            .last_error
            .as_deref()
            .is_some_and(|why| why.contains("did not answer")),
        "the project page says why, rather than leaving an operator to read the log"
    );
}
