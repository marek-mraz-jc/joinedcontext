//! Bringing back what a merged removal took (T-3247, API/01 §5): every file the change deleted,
//! read at the commit it was cut from, proposed again as one change; refused for a change that is
//! not merged, removed nothing, or whose resource is back, and for a caller who may not propose it.

mod common;

use serde_json::{json, Value};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{encode, envelope, forge, person, send, state_on, REPO};
use joinedcontext_portal::forge::shako::committed_text;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;

const PROJECT: &str = "ovzdusie";
const MANIFEST: &str = "projects/ovzdusie/spaces/air/pipelines/air-ingest/pipeline.yaml";
const NATIVE: &str = "projects/ovzdusie/spaces/air/pipelines/air-ingest/bento.yaml";
/// A model's LinkML source: no manifest, so it answers to the kind its folder names.
const LINKML: &str = "projects/ovzdusie/datamodels/air/model.linkml.yaml";
/// A file under no kind's folder.
const NOTE: &str = "projects/ovzdusie/notes/readme.md";
/// An Endpoint of space `water`, which a steward bound to space `air` may not propose.
const WATER: &str = "projects/ovzdusie/spaces/water/endpoints/water-api/endpoint.yaml";
const WATER_YAML: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: Endpoint\nmetadata:\n  name: water-api\n  namespace: ovzdusie\nspec:\n  contextSpaceRef: water\n";
const YAML: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: Pipeline\nmetadata:\n  name: air-ingest\n  namespace: ovzdusie\nspec:\n  contextSpaceRef:\n    name: air\n";

fn pull(number: u64, branch: &str, merged: bool) -> Value {
    json!({
        "Number": number,
        "HtmlUrl": format!("https://shako.example/pulls/{number}"),
        "State": if merged { "closed" } else { "open" },
        "Title": branch,
        "Head": branch,
        "HeadSha": format!("head-{number}"),
        "Base": "main",
        "Merged": merged,
        "MergeBase": "base-7",
        "Author": "jana"
    })
}

/// Change 7 removed the pipeline `air-ingest` and its bento file and is merged; change 8 is still
/// open; change 6 changed a file and removed nothing.
async fn world() -> (MockServer, AppState) {
    let shako = forge().await;
    Mock::given(method("GET"))
        .and(path(REPO))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "DefaultBranch": "main" })))
        .mount(&shako)
        .await;
    for (number, branch, merged, files) in [
        (
            7,
            "portal/delete-pipeline-air-ingest-0a1b2c3d",
            true,
            json!([
                { "Filename": MANIFEST, "Status": "deleted" },
                { "Filename": NATIVE, "Status": "deleted" },
                { "Filename": "users/someone.yaml", "Status": "deleted" }
            ]),
        ),
        (
            8,
            "portal/delete-pipeline-air-ingest-0a1b2c3d_1a2b3c4d",
            false,
            json!([{ "Filename": MANIFEST, "Status": "deleted" }]),
        ),
        (
            6,
            "portal/update-pipeline-air-ingest-0a1b2c3d",
            true,
            json!([{ "Filename": MANIFEST, "Status": "modified" }]),
        ),
        (
            5,
            "portal/delete-datamodel-air-0a1b2c3d",
            true,
            json!([{ "Filename": LINKML, "Status": "deleted" }]),
        ),
        (
            4,
            "portal/import-notes-0a1b2c3d",
            true,
            json!([{ "Filename": NOTE, "Status": "deleted" }]),
        ),
        (
            3,
            "portal/delete-endpoint-water-api-0a1b2c3d",
            true,
            json!([{ "Filename": WATER, "Status": "deleted" }]),
        ),
        (
            2,
            "portal/update-endpoint-water-api-0a1b2c3d",
            true,
            json!([{ "Filename": WATER, "Status": "modified" }]),
        ),
    ] {
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/PullRequests({number})")))
            .respond_with(ResponseTemplate::new(200).set_body_json(pull(number, branch, merged)))
            .mount(&shako)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/PullRequests({number})/Files")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "value": files })))
            .mount(&shako)
            .await;
    }
    for (file, content) in [
        (MANIFEST, YAML),
        (NATIVE, "input:\n  generate: {}\n"),
        (LINKML, "id: https://example.org/air\nname: air\n"),
        (NOTE, "# notes\n"),
        (WATER, WATER_YAML),
    ] {
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/Shako.File(Path=@p,Ref=@r)")))
            .and(query_param("@p", format!("'{file}'")))
            .and(query_param("@r", "'base-7'"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "Sha": "blob", "Content": encode(content) })),
            )
            .mount(&shako)
            .await;
    }
    let state = state_on(&shako);
    for (role, verbs) in [
        ("restorer", json!(["read", "propose"])),
        ("reader", json!(["read"])),
    ] {
        state.mirror.upsert(envelope(
            "Role",
            role,
            ORG_NAMESPACE,
            json!({ "rules": [{ "kinds": ["Pipeline"], "verbs": verbs }] }),
        ));
        state.mirror.upsert(envelope(
            "RoleBinding",
            &format!("{role}s"),
            ORG_NAMESPACE,
            json!({ "subjects": [{ "user": format!("{role}@hel.fi") }], "role": role, "scope": { "project": PROJECT } }),
        ));
    }
    (shako, state)
}

async fn restore(state: &AppState, who: &str, id: &str) -> (u16, Value) {
    let answer = send(
        state,
        person(who),
        "POST",
        &format!("/api/v1/projects/{PROJECT}/changes/{id}/restore"),
        None,
    )
    .await;
    (
        answer.status.as_u16(),
        serde_json::from_str(&answer.text).unwrap_or(Value::Null),
    )
}

async fn writes(shako: &MockServer) -> Vec<(String, String, String)> {
    shako
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|request| request.method.as_str() != "GET")
        .map(|request| {
            (
                request.method.to_string(),
                request.url.path().to_owned(),
                String::from_utf8_lossy(&request.body).into_owned(),
            )
        })
        .collect()
}

#[tokio::test]
async fn a_merged_removal_is_proposed_again_as_one_change_with_every_file_it_removed() {
    let (shako, state) = world().await;
    let (status, body) = restore(&state, "restorer", "chg-00000007").await;
    assert_eq!(status, 202, "{body}");
    assert_eq!(body["metadata"]["name"], json!("chg-00000009"), "{body}");
    assert_eq!(body["status"]["phase"], json!("PendingApproval"));
    assert_eq!(body["status"]["plan"]["create"], json!(1));

    let sent = writes(&shako).await;
    let branch = sent
        .iter()
        .find(|(_, path, _)| path.ends_with("/Shako.CreateBranch"))
        .expect("a branch");
    assert!(branch.2.contains("portal/restore-00000007"), "{branch:?}");
    let commit = sent
        .iter()
        .find(|(_, path, _)| path.ends_with("/Shako.CommitFiles"))
        .expect("one commit");
    let files: Value = serde_json::from_str(&commit.2).expect("a json commit");
    let paths: Vec<&str> = files["Files"]
        .as_array()
        .expect("files")
        .iter()
        .filter_map(|f| f["path"].as_str())
        .collect();
    // The organization's file is no project file: a project's restore brings back its own alone.
    assert_eq!(paths, vec![MANIFEST, NATIVE], "{files}");
    assert!(files["Files"]
        .as_array()
        .expect("files")
        .iter()
        .all(|f| f["operation"] == "upload"));
    let opened = sent
        .iter()
        .find(|(_, path, _)| path.ends_with("/Shako.OpenPullRequest"))
        .expect("a merge request");
    assert!(
        opened.2.contains("restore Pipeline air-ingest"),
        "{opened:?}"
    );
}

#[tokio::test]
async fn what_cannot_be_restored_is_refused_in_words_and_nothing_is_written() {
    let (shako, state) = world().await;
    let (status, body) = restore(&state, "restorer", "chg-00000008").await;
    assert_eq!(status, 409, "{body}");
    assert!(
        body["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("not merged"),
        "{body}"
    );

    let (status, body) = restore(&state, "restorer", "chg-00000006").await;
    assert_eq!(status, 409, "{body}");
    assert!(
        body["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("removed nothing"),
        "{body}"
    );

    // A reader who may not propose a Pipeline is told which verb it needs.
    let (status, body) = restore(&state, "reader", "chg-00000007").await;
    assert_eq!(status, 403, "{body}");
    assert!(
        body["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("propose"),
        "{body}"
    );

    // Nobody's project: the 404 of a change that is not there (R20).
    let (status, _) = restore(&state, "nobody", "chg-00000007").await;
    assert_eq!(status, 404);
    let (status, _) = restore(&state, "restorer", "not-a-change").await;
    assert_eq!(status, 400);

    assert!(
        writes(&shako).await.is_empty(),
        "a refusal wrote: {:?}",
        writes(&shako).await
    );
}

#[tokio::test]
async fn a_resource_that_is_back_on_main_is_not_restored_over() {
    let (shako, state) = world().await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/Shako.File(Path=@p,Ref=@r)")))
        .and(query_param("@p", format!("'{MANIFEST}'")))
        .and(query_param("@r", "'main'"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "Sha": "new", "Content": encode(YAML) })),
        )
        .mount(&shako)
        .await;
    let (status, body) = restore(&state, "restorer", "chg-00000007").await;
    assert_eq!(status, 409, "{body}");
    assert!(
        body["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("in the project again"),
        "{body}"
    );
    assert!(writes(&shako).await.is_empty());
}

/// Every file answers to a kind (integrator review of T-3247): a native file to its folder's, so a
/// caller who may propose Pipelines does not bring back a DataModel's LinkML; a file of no kind is
/// restored by nobody.
#[tokio::test]
async fn a_file_without_a_manifest_is_held_to_the_kind_of_its_folder() {
    let (shako, state) = world().await;
    let (status, body) = restore(&state, "restorer", "chg-00000005").await;
    assert_eq!(status, 403, "{body}");
    assert!(
        body["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("DataModel"),
        "{body}"
    );

    let (status, body) = restore(&state, "restorer", "chg-00000004").await;
    assert_eq!(status, 409, "{body}");
    assert!(
        body["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("no kind"),
        "{body}"
    );
    assert!(writes(&shako).await.is_empty());
}

/// T-3274: a merged update reads its "before" where the change was cut from, not on `main`, which
/// already holds what it made: its page and its history say which field it changed.
#[tokio::test]
async fn a_merged_update_shows_the_field_it_changed() {
    let (shako, state) = world().await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/Shako.File(Path=@p,Ref=@r)"))).and(query_param("@p", format!("'{MANIFEST}'")))
        .and(query_param("@r", "'head-6'"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({ "Sha": "blob", "Content": encode(&YAML.replace("name: air\n", "name: dust\n")) }),
        ))
        .mount(&shako)
        .await;
    // The pipeline lives under its space, so the change's head tree is where its file is found.
    Mock::given(method("GET"))
        .and(path(format!(
            "{REPO}/Shako.Tree(Ref=@r,Recursive=true,Skip=@s,Top=@t)"
        )))
        .and(query_param("@r", "'head-6'"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Truncated": false,
            "Tree": [{ "Path": MANIFEST, "Type": "blob", "Sha": "blob" }]
        })))
        .mount(&shako)
        .await;
    let answer = send(
        &state,
        person("reader"),
        "GET",
        &format!("/api/v1/projects/{PROJECT}/changes/chg-00000006"),
        None,
    )
    .await;
    assert_eq!(answer.status.as_u16(), 200, "{}", answer.text);
    let body: Value = serde_json::from_str(&answer.text).unwrap();
    let fields = body["planFields"].as_array().expect("plan fields");
    assert!(
        fields
            .iter()
            .any(|f| f["path"] == "spec.contextSpaceRef.name"
                && f["from"] == "air"
                && f["to"] == "dust"),
        "{fields:?}"
    );
}

async fn undo(state: &AppState, who: &str, id: &str) -> (u16, Value) {
    let answer = send(
        state,
        person(who),
        "POST",
        &format!("/api/v1/projects/{PROJECT}/changes/{id}/undo"),
        None,
    )
    .await;
    (
        answer.status.as_u16(),
        serde_json::from_str(&answer.text).unwrap_or(Value::Null),
    )
}

/// What change 6 made of the pipeline, and what `main` holds now.
async fn made_and_now(shako: &MockServer, now: &str) {
    let made = YAML.replace("name: air\n", "name: dust\n");
    for (git_ref, content) in [("head-6", made.as_str()), ("main", now)] {
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/Shako.File(Path=@p,Ref=@r)")))
            .and(query_param("@p", format!("'{MANIFEST}'")))
            .and(query_param("@r", format!("'{}'", git_ref)))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "Sha": git_ref, "Content": encode(content) })),
            )
            .mount(shako)
            .await;
    }
}

/// T-3274: a merged update is undone by one new change that puts back the file as it was before.
#[tokio::test]
async fn a_merged_update_is_undone_by_a_change_that_puts_the_file_back() {
    let (shako, state) = world().await;
    made_and_now(&shako, &YAML.replace("name: air\n", "name: dust\n")).await;
    let (status, body) = undo(&state, "restorer", "chg-00000006").await;
    assert_eq!(status, 202, "{body}");
    assert_eq!(body["status"]["plan"]["update"], json!(1), "{body}");
    let sent = writes(&shako).await;
    let commit = sent
        .iter()
        .find(|(_, path, _)| path.ends_with("/Shako.CommitFiles"))
        .expect("one commit");
    let files: Value = serde_json::from_str(&commit.2).expect("a json commit");
    assert_eq!(files["Files"][0]["path"], json!(MANIFEST));
    assert_eq!(
        files["Files"][0]["content"],
        json!(encode(&committed_text(MANIFEST, YAML))),
        "the file as it was before"
    );
    assert!(sent
        .iter()
        .any(|(_, path, body)| path.ends_with("/Shako.CreateBranch")
            && body.contains("portal/undo-00000006")));
}

/// T-3274: nothing is undone over a later change, nor a change that modified nothing, nor by a
/// person who may not propose the kind.
#[tokio::test]
async fn an_update_changed_again_since_or_a_removal_is_not_undone() {
    let (shako, state) = world().await;
    made_and_now(&shako, &YAML.replace("name: air\n", "name: smoke\n")).await;
    let (status, body) = undo(&state, "restorer", "chg-00000006").await;
    assert_eq!(status, 409, "{body}");
    assert!(
        body["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("changed again"),
        "{body}"
    );
    let (status, body) = undo(&state, "restorer", "chg-00000007").await;
    assert_eq!(status, 409, "{body}");
    assert!(
        body["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("modified nothing"),
        "{body}"
    );
    let (status, _) = undo(&state, "restorer", "chg-00000008").await;
    assert_eq!(status, 409);
    assert!(writes(&shako).await.is_empty(), "nothing is written");
}

#[tokio::test]
async fn undoing_needs_propose_on_the_kind() {
    let (shako, state) = world().await;
    made_and_now(&shako, &YAML.replace("name: air\n", "name: dust\n")).await;
    let (status, body) = undo(&state, "reader", "chg-00000006").await;
    assert_eq!(status, 403, "{body}");
    assert!(writes(&shako).await.is_empty());
}

/// T-5698: a restore or an undo is judged as a proposal of the manifest it puts back, its space
/// included: a steward bound to space `air` restores and undoes nothing of space `water`.
#[tokio::test]
async fn a_steward_of_one_space_restores_and_undoes_nothing_of_another() {
    let (shako, state) = world().await;
    for space in ["air", "water"] {
        state
            .mirror
            .upsert(envelope("ContextSpace", space, PROJECT, json!({})));
    }
    state.mirror.upsert(envelope(
        "Role",
        "endpoint-steward",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Endpoint"], "verbs": ["read", "propose"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "air-stewards",
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": "air@hel.fi" }], "role": "endpoint-steward", "scope": { "contextSpace": "air" } }),
    ));
    let (status, body) = restore(&state, "air", "chg-00000003").await;
    assert_eq!(status, 403, "{body}");
    for git_ref in ["head-2", "main"] {
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/Shako.File(Path=@p,Ref=@r)"))).and(query_param("@p", format!("'{WATER}'")))
            .and(query_param("@r", format!("'{}'", git_ref)))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({ "Sha": git_ref, "Content": encode(&WATER_YAML.replace("water\n", "water\n  audience: public\n")) }),
            ))
            .mount(&shako)
            .await;
    }
    let (status, body) = undo(&state, "air", "chg-00000002").await;
    assert_eq!(status, 403, "{body}");
    assert!(writes(&shako).await.is_empty(), "nothing is written");
}
