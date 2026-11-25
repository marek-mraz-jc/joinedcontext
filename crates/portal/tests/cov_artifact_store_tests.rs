//! The object calls and the prefix move of the artifact store, against an in-memory bucket that
//! can be made to fail one call at a time (TEN-15, T-7823).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use jc_types::{OrgId, ProjectId, ProjectKey};
use joinedcontext_portal::artifact_store::{
    move_artifacts, move_prefix, Client, Error, Legacy, Settings,
};
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const BUCKET: &str = "jc-artifacts";

#[derive(Default)]
struct Faults {
    read: Option<u16>,
    copy: Option<u16>,
    put: Option<u16>,
    delete: Option<u16>,
    /// The nth listing (1-based) answers 500.
    list_fails_on: Option<usize>,
    /// Every read of a key under `keyed/` answers other bytes than were written.
    corrupt_keyed: bool,
    /// Keys under the listed prefix the listing names though the bucket holds none of them.
    ghosts: Vec<String>,
    /// Keys the listing names whatever the prefix asked for.
    strays: Vec<String>,
    /// Objects per listing page; 0 is one page.
    page: usize,
}

#[derive(Clone, Default)]
struct Bucket {
    objects: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    faults: Arc<Mutex<Faults>>,
    lists: Arc<Mutex<usize>>,
}

impl Bucket {
    fn put(&self, key: &str, body: &[u8]) {
        self.objects
            .lock()
            .expect("objects")
            .insert(key.to_owned(), body.to_vec());
    }

    fn keys(&self) -> Vec<String> {
        self.objects
            .lock()
            .expect("objects")
            .keys()
            .cloned()
            .collect()
    }

    fn fault(&self, change: impl FnOnce(&mut Faults)) {
        change(&mut self.faults.lock().expect("faults"));
    }

    fn list(&self, request: &Request) -> ResponseTemplate {
        let call = {
            let mut lists = self.lists.lock().expect("lists");
            *lists += 1;
            *lists
        };
        let faults = self.faults.lock().expect("faults");
        if faults.list_fails_on == Some(call) {
            return ResponseTemplate::new(500);
        }
        let query = |name: &str| {
            request
                .url
                .query_pairs()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.into_owned())
        };
        let prefix = query("prefix").unwrap_or_default();
        let mut keys: Vec<String> = self
            .objects
            .lock()
            .expect("objects")
            .keys()
            .filter(|key| key.starts_with(&prefix))
            .cloned()
            .collect();
        keys.extend(
            faults
                .ghosts
                .iter()
                .filter(|g| g.starts_with(&prefix))
                .cloned(),
        );
        keys.extend(faults.strays.iter().cloned());
        keys.sort();
        let from: usize = query("continuation-token")
            .and_then(|token| token.parse().ok())
            .unwrap_or(0);
        let per_page = if faults.page == 0 {
            keys.len().max(1)
        } else {
            faults.page
        };
        let page: Vec<&String> = keys.iter().skip(from).take(per_page).collect();
        let more = from + per_page < keys.len();
        let objects = self.objects.lock().expect("objects");
        let mut xml = format!("<ListBucketResult><IsTruncated>{more}</IsTruncated>");
        for key in page {
            let size = objects.get(key).map_or(0, Vec::len);
            xml.push_str(&format!(
                "<Contents><Key>{key}</Key><Size>{size}</Size>\
                 <LastModified>2026-01-01T00:00:00Z</LastModified></Contents>"
            ));
        }
        if more {
            xml.push_str(&format!(
                "<NextContinuationToken>{}</NextContinuationToken>",
                from + per_page
            ));
        }
        xml.push_str("</ListBucketResult>");
        ResponseTemplate::new(200).set_body_string(xml)
    }
}

impl Respond for Bucket {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let path = request.url.path().to_owned();
        let Some(key) = path.strip_prefix(&format!("/{BUCKET}/")) else {
            return if request.method.as_str() == "GET" {
                self.list(request)
            } else {
                ResponseTemplate::new(400)
            };
        };
        let key = key.to_owned();
        let faults = self.faults.lock().expect("faults");
        match request.method.as_str() {
            "GET" => {
                if let Some(status) = faults.read {
                    return ResponseTemplate::new(status);
                }
                if faults.corrupt_keyed && key.starts_with("keyed/") {
                    return ResponseTemplate::new(200).set_body_bytes(b"corrupt".to_vec());
                }
                match self.objects.lock().expect("objects").get(&key) {
                    Some(body) => ResponseTemplate::new(200).set_body_bytes(body.clone()),
                    None => ResponseTemplate::new(404),
                }
            }
            "PUT" => {
                let copy_from = request
                    .headers
                    .get("x-amz-copy-source")
                    .and_then(|v| v.to_str().ok())
                    .map(|v| v.trim_start_matches(&format!("/{BUCKET}/")).to_owned());
                let mut objects = self.objects.lock().expect("objects");
                match copy_from {
                    Some(from) => {
                        if let Some(status) = faults.copy {
                            return ResponseTemplate::new(status);
                        }
                        if request.headers.contains_key("if-none-match")
                            && objects.contains_key(&key)
                        {
                            return ResponseTemplate::new(412);
                        }
                        match objects.get(&from).cloned() {
                            Some(body) => {
                                objects.insert(key, body);
                                ResponseTemplate::new(200)
                            }
                            None => ResponseTemplate::new(404),
                        }
                    }
                    None => {
                        if let Some(status) = faults.put {
                            return ResponseTemplate::new(status);
                        }
                        objects.insert(key, request.body.clone());
                        ResponseTemplate::new(200)
                    }
                }
            }
            "DELETE" => {
                if let Some(status) = faults.delete {
                    return ResponseTemplate::new(status);
                }
                self.objects.lock().expect("objects").remove(&key);
                ResponseTemplate::new(204)
            }
            _ => ResponseTemplate::new(405),
        }
    }
}

fn settings(endpoint: String) -> Settings {
    Settings {
        endpoint,
        bucket: BUCKET.to_owned(),
        region: "us-east-1".to_owned(),
        root_access_key: "jc-root".to_owned(),
        root_secret_key: "the-root-secret-nobody-else-holds".to_owned(),
    }
}

async fn store() -> (Client, Bucket, MockServer) {
    let bucket = Bucket::default();
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(bucket.clone())
        .mount(&server)
        .await;
    let client = Client::new(settings(server.uri())).expect("a client");
    (client, bucket, server)
}

fn dead() -> Client {
    Client::new(settings("http://127.0.0.1:1".to_owned())).expect("a client")
}

fn key() -> ProjectKey {
    ProjectKey::new(
        OrgId::parse_verified("0b0c6c2e-1d1e-4a40-9f0e-2a5a3c1f0b01").expect("an id"),
        ProjectId::parse_verified("5f1d2c3b-4a59-4b6c-8d7e-9f0a1b2c3d4e").expect("an id"),
    )
}

async fn move_of(client: &Client) -> Result<joinedcontext_portal::artifact_store::Moved, String> {
    move_prefix(
        client,
        BUCKET,
        ("legacy/", "keyed/"),
        "moved/rollback.json",
        serde_json::Map::new(),
    )
    .await
}

#[tokio::test]
async fn an_endpoint_that_is_no_url_or_names_no_host_is_refused() {
    assert!(matches!(
        Client::new(settings("not a url".to_owned())),
        Err(Error::Endpoint(_))
    ));
    let said = Client::new(settings("mailto:ops@example.org".to_owned()))
        .err()
        .expect("no host")
        .to_string();
    assert!(said.contains("names no host"), "{said}");
    let with_port = Client::new(settings("http://store.example:9000".to_owned())).expect("ok");
    assert_eq!(with_port.bucket(), BUCKET);
    assert!(Client::new(settings("http://store.example".to_owned())).is_ok());
}

#[tokio::test]
async fn an_object_call_to_a_store_that_is_down_is_a_transport_error_naming_it() {
    let client = dead();
    let said = client
        .object(
            reqwest::Method::GET,
            BUCKET,
            "a",
            Vec::new(),
            &[],
            "read an object",
        )
        .await
        .expect_err("down")
        .to_string();
    assert!(said.contains("read an object did not reach"), "{said}");
    assert!(matches!(
        client.list(BUCKET, "p/").await,
        Err(Error::Transport {
            operation: "list objects",
            ..
        })
    ));
    assert!(matches!(
        client.copy_object(BUCKET, "a", "b").await,
        Err(Error::Transport { .. })
    ));
    assert!(matches!(
        client.delete_object(BUCKET, "a").await,
        Err(Error::Transport { .. })
    ));
    assert!(matches!(
        client.put_object(BUCKET, "a", b"x".to_vec(), "write").await,
        Err(Error::Transport { .. })
    ));
}

#[tokio::test]
async fn a_put_a_copy_and_a_delete_the_store_refuses_are_refusals_with_the_status() {
    let (client, bucket, _server) = store().await;
    bucket.put("a", b"1");
    bucket.fault(|f| {
        f.put = Some(403);
        f.copy = Some(500);
        f.delete = Some(503);
    });
    let put = client
        .put_object(BUCKET, "b", b"x".to_vec(), "write b")
        .await;
    assert!(
        matches!(
            put,
            Err(Error::Refused {
                operation: "write b",
                status: 403
            })
        ),
        "{put:?}"
    );
    let copy = client.copy_object(BUCKET, "a", "c").await;
    assert!(
        matches!(
            copy,
            Err(Error::Refused {
                operation: "copy an object",
                status: 500
            })
        ),
        "{copy:?}"
    );
    let absent = client.copy_if_absent(BUCKET, "a", "c").await;
    assert!(
        matches!(absent, Err(Error::Refused { status: 500, .. })),
        "{absent:?}"
    );
    let delete = client.delete_object(BUCKET, "a").await;
    assert!(
        matches!(
            delete,
            Err(Error::Refused {
                operation: "delete an object",
                status: 503
            })
        ),
        "{delete:?}"
    );
    assert_eq!(bucket.keys(), vec!["a".to_owned()]);
}

#[tokio::test]
async fn a_copy_that_finds_its_target_leaves_it_and_a_delete_of_a_gone_object_is_done() {
    let (client, bucket, _server) = store().await;
    bucket.put("a", b"new");
    bucket.put("b", b"newer");
    assert!(!client
        .copy_if_absent(BUCKET, "a", "b")
        .await
        .expect("answered"));
    assert_eq!(bucket.objects.lock().expect("o")["b"], b"newer");
    assert!(client
        .copy_if_absent(BUCKET, "a", "c")
        .await
        .expect("copied"));
    client.copy_object(BUCKET, "a", "d").await.expect("copied");
    client
        .delete_object(BUCKET, "never-there")
        .await
        .expect("gone is deleted");
    assert_eq!(bucket.keys(), ["a", "b", "c", "d"]);
}

#[tokio::test]
async fn a_listing_follows_its_pages_and_a_refused_one_is_an_error() {
    let (client, bucket, _server) = store().await;
    for name in ["p/1", "p/2", "p/3", "q/1"] {
        bucket.put(name, b"xy");
    }
    bucket.fault(|f| f.page = 2);
    let listed = client.list(BUCKET, "p/").await.expect("listed");
    let names: Vec<&str> = listed.iter().map(|o| o.key.as_str()).collect();
    assert_eq!(names, ["p/1", "p/2", "p/3"]);
    assert!(listed
        .iter()
        .all(|o| o.size == 2 && o.modified == "2026-01-01T00:00:00Z"));
    assert!(client
        .list(BUCKET, "none/")
        .await
        .expect("empty")
        .is_empty());
    bucket.fault(|f| f.list_fails_on = Some(*bucket.lists.lock().expect("l") + 1));
    assert!(matches!(
        client.list(BUCKET, "p/").await,
        Err(Error::Refused {
            operation: "list objects",
            status: 500
        })
    ));
}

#[tokio::test]
async fn an_empty_old_prefix_is_done_with_nothing_moved() {
    let (client, bucket, _server) = store().await;
    let moved = move_of(&client).await.expect("moved");
    assert_eq!((moved.copied, moved.superseded, moved.done), (0, 0, true));
    assert!(bucket.keys().is_empty(), "no rollback entry for nothing");
}

#[tokio::test]
async fn a_move_copies_verifies_records_and_deletes_the_old_objects() {
    let (client, bucket, _server) = store().await;
    bucket.put("legacy/a.json", b"alpha");
    bucket.put("legacy/sub/b.json", b"beta");
    bucket.put("legacy/c.json", b"gamma");
    bucket.fault(|f| f.page = 1);
    let moved = move_of(&client).await.expect("moved");
    assert_eq!((moved.copied, moved.superseded, moved.done), (3, 0, true));
    assert_eq!(
        bucket.keys(),
        [
            "keyed/a.json",
            "keyed/c.json",
            "keyed/sub/b.json",
            "moved/rollback.json"
        ]
    );
    let entry = bucket.objects.lock().expect("o")["moved/rollback.json"].clone();
    let entry: serde_json::Value = serde_json::from_slice(&entry).expect("json");
    assert_eq!(entry["from"], "legacy/");
    assert_eq!(entry["to"], "keyed/");
    assert_eq!(entry["objects"].as_array().expect("objects").len(), 3);
    assert_eq!(entry["objects"][0]["outcome"], "copied");
    assert!(entry["objects"][0]["sha256"]
        .as_str()
        .is_some_and(|h| h.len() == 64));
}

#[tokio::test]
async fn an_old_object_whose_key_was_written_meanwhile_is_dropped_not_copied_over() {
    let (client, bucket, _server) = store().await;
    bucket.put("legacy/a.json", b"old");
    bucket.put("keyed/a.json", b"newer write");
    let moved = move_of(&client).await.expect("moved");
    assert_eq!((moved.copied, moved.superseded, moved.done), (0, 1, true));
    assert_eq!(
        bucket.objects.lock().expect("o")["keyed/a.json"],
        b"newer write"
    );
    assert!(!bucket.keys().iter().any(|k| k.starts_with("legacy/")));
}

#[tokio::test]
async fn a_listed_key_outside_the_prefix_or_gone_since_the_listing_is_skipped() {
    let (client, bucket, _server) = store().await;
    bucket.put("legacy/a.json", b"alpha");
    bucket.fault(|f| {
        f.ghosts = vec!["legacy/ghost.json".to_owned()];
        f.strays = vec!["elsewhere/stray.json".to_owned()];
    });
    let moved = move_of(&client).await.expect("moved");
    assert_eq!(
        moved.copied, 1,
        "neither the ghost nor the stray is counted"
    );
    assert!(bucket.keys().contains(&"keyed/a.json".to_owned()));
    assert!(!bucket.keys().contains(&"keyed/ghost.json".to_owned()));
}

#[tokio::test]
async fn a_move_stops_where_the_store_fails_and_says_which_step() {
    type Case = (&'static str, Box<dyn Fn(&mut Faults)>);
    let cases: Vec<Case> = vec![
        ("answered 500 to a read", Box::new(|f| f.read = Some(500))),
        (
            "an object could not be copied",
            Box::new(|f| f.copy = Some(500)),
        ),
        (
            "did not read back the same bytes",
            Box::new(|f| f.corrupt_keyed = true),
        ),
        (
            "the rollback entry could not be written",
            Box::new(|f| f.put = Some(500)),
        ),
        (
            "an old object could not be deleted",
            Box::new(|f| f.delete = Some(500)),
        ),
        (
            "the old prefix could not be listed",
            Box::new(|f| f.list_fails_on = Some(1)),
        ),
        (
            "the old prefix could not be listed",
            Box::new(|f| f.list_fails_on = Some(2)),
        ),
    ];
    for (expected, fault) in cases {
        let (client, bucket, _server) = store().await;
        bucket.put("legacy/a.json", b"alpha");
        bucket.fault(|f| fault(f));
        let said = move_of(&client).await.expect_err(expected);
        assert!(said.contains(expected), "{expected}: {said}");
        if expected.contains("read back")
            || expected.contains("copied")
            || expected.contains("a read")
        {
            assert!(
                bucket.keys().contains(&"legacy/a.json".to_owned()),
                "the old prefix stays: {:?}",
                bucket.keys()
            );
        }
    }
}

#[tokio::test]
async fn the_artifacts_of_a_project_move_area_by_area_and_a_failing_area_stops_the_move() {
    let (client, bucket, _server) = store().await;
    let legacy = Legacy::new("helsinki", "air").expect("labels");
    bucket.put("schemas/helsinki/air/a.json", b"one");
    bucket.put("dumps/helsinki/air/d.json", b"two");
    bucket.put("schemas/helsinki/other/skip.json", b"x");
    let moved = move_artifacts(&client, &key(), &legacy, "at1")
        .await
        .expect("moved");
    assert_eq!((moved.copied, moved.superseded, moved.done), (2, 0, true));
    let keyed = bucket.keys();
    assert!(
        keyed.iter().any(|k| k.ends_with("/schemas/a.json")),
        "{keyed:?}"
    );
    assert!(
        keyed.iter().any(|k| k.ends_with("/dumps/d.json")),
        "{keyed:?}"
    );
    assert!(keyed.contains(&"schemas/helsinki/other/skip.json".to_owned()));
    assert!(keyed
        .iter()
        .any(|k| k == "moved/schemas/helsinki/air/at1.json"));

    let (client, bucket, _server) = store().await;
    bucket.put("mappings/helsinki/air/m.json", b"three");
    bucket.fault(|f| f.copy = Some(500));
    let said = move_artifacts(&client, &key(), &legacy, "at")
        .await
        .expect_err("copy refused");
    assert!(said.contains("could not be copied"), "{said}");
}

#[test]
fn a_legacy_prefix_needs_two_labels() {
    assert!(Legacy::new("Hel Sinki", "air").is_none());
    assert!(Legacy::new("helsinki", "").is_none());
    let legacy = Legacy::new("helsinki", "air").expect("labels");
    assert_eq!(legacy.prefix("schemas"), "schemas/helsinki/air/");
}
