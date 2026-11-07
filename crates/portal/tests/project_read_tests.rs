//! Every read of a project answers `404` to a caller who may not read it, the one answer for
//! "missing" and "not yours" (PF-59, R20): the catalogue status, the federation graph, the
//! catalog search (both operations since T-4453), a sync source's status, the agent-run list and the activity
//! operation (T-1401, T-1361, T-1368, T-1340). A reader of the project is answered.

mod common;

use axum::http::StatusCode;
use serde_json::json;

use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;

const READS: &[(&str, &str)] = &[
    ("GET", "/api/v1/projects/ovzdusie/ckan/status"),
    ("POST", "/api/v1/projects/ovzdusie/ops/jc_federation_graph"),
    ("POST", "/api/v1/projects/ovzdusie/ops/jc_catalog_search"),
    (
        "GET",
        "/api/v1/projects/ovzdusie/syncsources/upstream/status",
    ),
    ("GET", "/api/v1/projects/ovzdusie/agent-runs"),
    ("POST", "/api/v1/projects/ovzdusie/ops/jc_activity_list"),
];

/// `ovzdusie` holds a sync source, and `reader@hel.fi` reads the kinds these routes read there.
async fn state() -> AppState {
    let state = common::state_on_forge().await;
    state.mirror.upsert(common::envelope(
        "SyncSource",
        "upstream",
        "ovzdusie",
        json!({ "url": "https://git.example.org/city/config.git" }),
    ));
    state.mirror.upsert(common::envelope(
        "Role",
        "reads-everything",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["CkanInstance", "SyncSource", "Pipeline"], "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(common::envelope(
        "RoleBinding",
        "reader-ovzdusie",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "user": "reader@hel.fi" }],
            "role": "reads-everything",
            "scope": { "project": "ovzdusie" },
        }),
    ));
    state
}

/// An operation's input: the search asks one word, every other read nothing.
fn body(http: &str, uri: &str) -> Option<serde_json::Value> {
    (http == "POST").then(|| {
        if uri.ends_with("jc_catalog_search") {
            json!({ "q": "air" })
        } else {
            json!({})
        }
    })
}

#[tokio::test]
async fn a_project_the_caller_may_not_read_is_not_there() {
    let state = state().await;
    for (http, uri) in READS {
        let answer = common::send(
            &state,
            common::person("stranger"),
            http,
            uri,
            body(http, uri),
        )
        .await;
        assert_eq!(
            answer.status,
            StatusCode::NOT_FOUND,
            "{http} {uri}: {}",
            answer.text
        );
        let absent = common::send(
            &state,
            common::person("stranger"),
            http,
            &uri.replace("ovzdusie", "no-such-project"),
            body(http, uri),
        )
        .await;
        assert_eq!(
            (
                answer.status,
                answer.text.replace("ovzdusie", "no-such-project")
            ),
            (absent.status, absent.text),
            "{http} {uri}: a project the caller may not read reads like one that is not there"
        );
    }
}

#[tokio::test]
async fn a_reader_of_the_project_is_answered() {
    let state = state().await;
    for (http, uri) in READS {
        let answer =
            common::send(&state, common::person("reader"), http, uri, body(http, uri)).await;
        assert_ne!(
            answer.status,
            StatusCode::NOT_FOUND,
            "{http} {uri}: {}",
            answer.text
        );
        assert_ne!(
            answer.status,
            StatusCode::FORBIDDEN,
            "{http} {uri}: {}",
            answer.text
        );
    }
}

/// MF-11, MF-16 (T-2375): the resource API reads a project as it stood at a past commit.
mod at_a_revision {
    use super::*;
    use base64::Engine;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const PAST: &str = "4f2a9c1d0e8b7a6f5e4d3c2b1a0f9e8d7c6b5a49";
    const UNKNOWN: &str = "0000000000000000000000000000000000000000";

    fn endpoint(name: &str, project: &str, audience: &str) -> String {
        format!(
            "apiVersion: joinedcontext.com/v1alpha1\nkind: Endpoint\nmetadata:\n  name: {name}\n  namespace: {project}\nspec:\n  contextSpaceRef: air\n  audience: {audience}\n"
        )
    }

    /// A forge whose commit `PAST` held `files`, and which knows no `UNKNOWN`.
    async fn forge_at(files: &[(&str, String)]) -> MockServer {
        let forge = common::forge().await;
        let tree: Vec<serde_json::Value> = files
            .iter()
            .map(|(p, _)| json!({ "path": p, "type": "blob", "sha": format!("blob-{p}") }))
            .collect();
        Mock::given(method("GET"))
            .and(path(format!("{}/git/trees/{PAST}", common::REPO)))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "sha": PAST, "truncated": false, "tree": tree })),
            )
            .with_priority(1)
            .mount(&forge)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("{}/git/trees/{UNKNOWN}", common::REPO)))
            .respond_with(
                ResponseTemplate::new(404).set_body_json(json!({ "message": "sha not found" })),
            )
            .with_priority(1)
            .mount(&forge)
            .await;
        for (file, content) in files {
            Mock::given(method("GET"))
                .and(path(format!("{}/contents/{file}", common::REPO)))
                .and(query_param("ref", PAST))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "sha": format!("blob-{file}"),
                    "content": base64::engine::general_purpose::STANDARD.encode(content),
                })))
                .with_priority(1)
                .mount(&forge)
                .await;
        }
        forge
    }

    /// `reader@hel.fi` reads Endpoints of `ovzdusie`; today the project has `air`, and at `PAST`
    /// it had `old-air` as public and `other` held one of its own.
    async fn state() -> AppState {
        let forge = forge_at(&[
            (
                "projects/ovzdusie/endpoints/old-air.yaml",
                endpoint("old-air", "ovzdusie", "public"),
            ),
            (
                "projects/other/endpoints/theirs.yaml",
                endpoint("theirs", "other", "internal"),
            ),
            ("projects/ovzdusie/README.md", "# not a manifest".to_owned()),
        ])
        .await;
        let state = common::state_on(&forge);
        state.mirror.upsert(common::envelope(
            "Endpoint",
            "air",
            "ovzdusie",
            json!({ "contextSpaceRef": "air", "audience": "internal" }),
        ));
        state.mirror.upsert(common::envelope(
            "Role",
            "reads-endpoints",
            ORG_NAMESPACE,
            json!({ "rules": [{ "kinds": ["Endpoint"], "verbs": ["read"] }] }),
        ));
        state.mirror.upsert(common::envelope(
            "RoleBinding",
            "reader-ovzdusie",
            ORG_NAMESPACE,
            json!({
                "subjects": [{ "user": "reader@hel.fi" }],
                "role": "reads-endpoints",
                "scope": { "project": "ovzdusie" },
            }),
        ));
        state
    }

    fn reader() -> joinedcontext_portal::auth::session::Identity {
        common::person("reader@hel.fi")
    }

    fn json_of(text: &str) -> serde_json::Value {
        serde_json::from_str(text).unwrap_or(serde_json::Value::Null)
    }

    #[tokio::test]
    async fn a_list_and_a_read_at_a_past_commit_answer_what_it_held() {
        let state = state().await;
        let base = "/api/v1/projects/ovzdusie/endpoints";

        let list = common::send(
            &state,
            reader(),
            "GET",
            &format!("{base}?revision={PAST}"),
            None,
        )
        .await;
        assert_eq!(list.status, StatusCode::OK, "{}", list.text);
        let names: Vec<String> = json_of(&list.text)["items"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|i| i["metadata"]["name"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            names,
            ["old-air"],
            "another project's file or today's mirror leaked"
        );

        let read = common::send(
            &state,
            reader(),
            "GET",
            &format!("{base}/old-air?revision={PAST}"),
            None,
        )
        .await;
        assert_eq!(read.status, StatusCode::OK, "{}", read.text);
        let manifest = json_of(&read.text);
        assert_eq!(manifest["spec"]["audience"], "public");
        assert!(
            manifest
                .get("status")
                .is_none_or(serde_json::Value::is_null),
            "a past commit has no live status: {manifest}"
        );

        // Without a revision the answer is today's, as before.
        let now = common::send(&state, reader(), "GET", &format!("{base}/air"), None).await;
        assert_eq!(now.status, StatusCode::OK, "{}", now.text);
        let gone = common::send(&state, reader(), "GET", &format!("{base}/old-air"), None).await;
        assert_eq!(gone.status, StatusCode::NOT_FOUND);
    }

    /// An unknown commit, a resource the commit did not hold in this project, and a caller who may
    /// not read the kind all answer the one 404 of a missing resource (R20).
    #[tokio::test]
    async fn what_the_commit_did_not_hold_or_the_caller_may_not_read_is_not_there() {
        let state = state().await;
        let base = "/api/v1/projects/ovzdusie/endpoints";
        let missing = common::send(&state, reader(), "GET", &format!("{base}/nothing"), None).await;
        assert_eq!(missing.status, StatusCode::NOT_FOUND);

        for (who, uri) in [
            (reader(), format!("{base}/old-air?revision={UNKNOWN}")),
            (reader(), format!("{base}/theirs?revision={PAST}")),
            (
                common::person("stranger"),
                format!("{base}/old-air?revision={PAST}"),
            ),
        ] {
            let answer = common::send(&state, who, "GET", &uri, None).await;
            assert_eq!(
                answer.status,
                StatusCode::NOT_FOUND,
                "{uri}: {}",
                answer.text
            );
            assert!(
                !answer.text.contains("sha not found"),
                "the forge's words leaked: {}",
                answer.text
            );
        }
        let list = common::send(
            &state,
            reader(),
            "GET",
            &format!("{base}?revision={UNKNOWN}"),
            None,
        )
        .await;
        assert_eq!(list.status, StatusCode::NOT_FOUND, "{}", list.text);
    }

    /// A revision is a commit id: a branch name would read a workspace's unmerged edits past the
    /// workspace's own door, and anything else is not a revision at all.
    #[tokio::test]
    async fn a_revision_that_is_not_a_commit_id_is_refused() {
        let state = state().await;
        for revision in ["main", "agent/worker/x", "../etc", "XYZ", ""] {
            let uri = format!("/api/v1/projects/ovzdusie/endpoints?revision={revision}");
            let answer = common::send(&state, reader(), "GET", &uri, None).await;
            assert_eq!(
                answer.status,
                StatusCode::BAD_REQUEST,
                "{revision}: {}",
                answer.text
            );
        }
    }
}
