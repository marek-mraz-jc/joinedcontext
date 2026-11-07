//! The file representations an Endpoint no longer offers, dropped from the manifests a
//! repository still stores (owner 2026-10-18, EP-41, EP-43, EP-44, T-4600).
//!
//! CSV is the one file an Endpoint serves; `xlsx`, `parquet`, `json`, `zip` and `geojson` left
//! the vocabulary, and the gateway refuses a manifest that lists one. A run that finds such an
//! Endpoint, or an App whose `dataNeeds` name one (T-5305), rewrites its file without them, in
//! one commit per repository; a repository with none is not touched, so the second run changes
//! nothing.

use std::collections::BTreeSet;

use serde_norway::Value;

use crate::git::{Author, GitError, GiteaClient};

/// The representations an Endpoint manifest may no longer name.
pub const RETIRED: [&str; 5] = ["xlsx", "parquet", "json", "zip", "geojson"];

/// The two `spec` lists a representation is named in.
const LISTS: [&str; 2] = ["enabledRepresentations", "disabledRepresentations"];

/// Whether a manifest of `kind` names a retired representation: an Endpoint in either `spec`
/// list, an App in any `dataNeeds[].representations` (jc-core refuses both, T-5305).
pub fn names_retired(kind: &str, spec: &serde_json::Value) -> bool {
    let named = |items: Option<&serde_json::Value>| {
        items
            .and_then(serde_json::Value::as_array)
            .is_some_and(|items| items.iter().any(|item| is_retired(item.as_str())))
    };
    match kind {
        "Endpoint" => LISTS.iter().any(|list| named(spec.get(list))),
        "App" => spec
            .get("dataNeeds")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|needs| needs.iter().any(|need| named(need.get("representations")))),
        _ => false,
    }
}

fn is_retired(name: Option<&str>) -> bool {
    name.is_some_and(|name| RETIRED.contains(&name))
}

/// The file `text` with every retired representation dropped from its Endpoint documents and
/// from its Apps' `dataNeeds[].representations` (T-5305), or `None` when it names none (or is
/// not YAML this can read, which the loader reports itself). Every document of a changed file is written back as YAML values: comments in that file do
/// not survive, the manifests do.
pub fn strip(text: &str) -> Option<String> {
    let mut documents = Vec::new();
    for document in serde_norway::Deserializer::from_str(text) {
        documents.push(<Value as serde::Deserialize>::deserialize(document).ok()?);
    }
    let mut changed = false;
    for document in &mut documents {
        let kind = document
            .get("kind")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let Some(spec) = document.get_mut("spec").and_then(Value::as_mapping_mut) else {
            continue;
        };
        match kind.as_deref() {
            Some("Endpoint") => {
                for list in LISTS {
                    let Some(Value::Sequence(items)) = spec.get_mut(list) else {
                        continue;
                    };
                    if !drop_retired(items) {
                        continue;
                    }
                    changed = true;
                    if list == "enabledRepresentations" {
                        refill(items);
                    } else if items.is_empty() {
                        spec.remove(list);
                    }
                }
            }
            Some("App") => {
                let Some(Value::Sequence(needs)) = spec.get_mut("dataNeeds") else {
                    continue;
                };
                for need in needs {
                    if let Some(Value::Sequence(items)) = need.get_mut("representations") {
                        if drop_retired(items) {
                            refill(items);
                            changed = true;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    if !changed {
        return None;
    }
    let written: Vec<String> = documents
        .iter()
        .map(serde_norway::to_string)
        .collect::<Result<_, _>>()
        .ok()?;
    Some(written.join("---\n"))
}

/// `items` without the retired representations; answers whether one went.
fn drop_retired(items: &mut Vec<Value>) -> bool {
    let before = items.len();
    items.retain(|item| !is_retired(item.as_str()));
    items.len() != before
}

/// An emptied listed set names `ngsi-ld` alone: jc-core refuses an Endpoint's empty one,
/// NGSI-LD is served anyway (EP-10), so the Endpoint or App need stays as narrow as it was.
fn refill(items: &mut Vec<Value>) {
    if items.is_empty() {
        items.push(Value::String("ngsi-ld".to_owned()));
    }
}

/// The repository a loaded path belongs to and its path there: in layout 2 a project's files are
/// mounted at `projects/{slug}/` (CC-86), everything else is the organization's (`""`).
pub fn owner(layout: u32, path: &str) -> (String, String) {
    if layout == 2 {
        if let Some((slug, rest)) = path
            .strip_prefix("projects/")
            .and_then(|rest| rest.split_once('/'))
        {
            return (slug.to_owned(), rest.to_owned());
        }
    }
    (String::new(), path.to_owned())
}

/// Rewrites `paths` of the repository on `branch` without the retired representations, in one
/// commit. Answers how many files the commit changed; none changed is no commit.
pub async fn migrate(
    gitea: &GiteaClient,
    branch: &str,
    paths: &BTreeSet<String>,
) -> Result<usize, GitError> {
    let mut uploads = Vec::new();
    for path in paths {
        let Some(file) = gitea.get_file(path, branch).await? else {
            continue;
        };
        if let Some(content) = strip(&file.content) {
            // The blob read here, so a commit someone lands before this one is refused rather
            // than overwritten; the next sync reads the fresh file.
            uploads.push((path.clone(), content, file.sha));
        }
    }
    if uploads.is_empty() {
        return Ok(0);
    }
    gitea
        .update_files(
            branch,
            "endpoints and apps: CSV is the one file an Endpoint offers (T-4600)",
            Author {
                name: crate::sync::proposal::AUTHOR_NAME,
                email: crate::sync::proposal::AUTHOR_EMAIL,
            },
            &uploads,
        )
        .await?;
    Ok(uploads.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const ENDPOINT: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: Endpoint\nmetadata:\n  name: air\n  namespace: helsinki\nspec:\n  enabledRepresentations: [ngsi-ld, csv, xlsx, zip, geojson, ogc-features]\n  disabledRepresentations: [json, parquet, sta]\n";

    fn lists(text: &str) -> (Vec<String>, Vec<String>) {
        let value: Value = serde_norway::from_str(text).expect("yaml");
        let read = |list: &str| -> Vec<String> {
            value["spec"][list]
                .as_sequence()
                .expect("a list")
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        };
        (
            read("enabledRepresentations"),
            read("disabledRepresentations"),
        )
    }

    #[test]
    fn the_retired_files_go_and_everything_else_stays() {
        let stripped = strip(ENDPOINT).expect("the endpoint names retired files");

        let (enabled, disabled) = lists(&stripped);
        assert_eq!(enabled, ["ngsi-ld", "csv", "ogc-features"]);
        assert_eq!(disabled, ["sta"]);
        assert_eq!(strip(&stripped), None, "a second run changes nothing");
    }

    /// jc-core refuses a listed set that is empty, so an Endpoint that listed only retired files
    /// keeps the narrowest one there is, and an emptied `disabledRepresentations` goes away.
    #[test]
    fn a_list_of_only_retired_files_keeps_ngsi_ld_and_an_empty_disabled_list_goes() {
        let only = "kind: Endpoint\nmetadata:\n  name: air\nspec:\n  enabledRepresentations: [geojson, xlsx]\n";
        let value: Value = serde_norway::from_str(&strip(only).expect("changed")).expect("yaml");
        assert_eq!(
            value["spec"]["enabledRepresentations"]
                .as_sequence()
                .map(|items| items.iter().filter_map(Value::as_str).collect::<Vec<_>>()),
            Some(vec!["ngsi-ld"])
        );

        let disabled =
            "kind: Endpoint\nmetadata:\n  name: air\nspec:\n  disabledRepresentations: [zip]\n";
        let value: Value =
            serde_norway::from_str(&strip(disabled).expect("changed")).expect("yaml");
        assert!(value["spec"].get("disabledRepresentations").is_none());
    }

    #[test]
    fn a_clean_endpoint_and_other_kinds_are_not_rewritten() {
        let clean = "kind: Endpoint\nmetadata:\n  name: air\nspec:\n  enabledRepresentations: [ngsi-ld, csv]\n";
        // Only an Endpoint's own lists and an App's dataNeeds are read: another kind is left.
        let other = "kind: ContextSpace\nmetadata:\n  name: map\nspec:\n  enabledRepresentations: [geojson]\n";
        assert_eq!(strip(clean), None);
        assert_eq!(strip(other), None);
        assert_eq!(strip("kind: Endpoint\nspec: {}\n"), None);
        assert_eq!(strip("not: [valid"), None);
    }

    #[test]
    fn every_document_of_a_file_survives_the_rewrite() {
        let file = format!("kind: ContextSpace\nmetadata:\n  name: air\n---\n{ENDPOINT}");
        let stripped = strip(&file).expect("changed");
        let documents: Vec<Value> = serde_norway::Deserializer::from_str(&stripped)
            .map(|d| <Value as serde::Deserialize>::deserialize(d).expect("yaml"))
            .collect();
        assert_eq!(documents.len(), 2);
        assert_eq!(documents[0]["kind"].as_str(), Some("ContextSpace"));
        assert_eq!(
            documents[1]["spec"]["enabledRepresentations"]
                .as_sequence()
                .map(Vec::len),
            Some(3)
        );
    }

    /// A file someone changed after this run read it: the forge refuses the stale blob, the
    /// run reports it and writes nothing, and the next sync reads the fresh file.
    #[tokio::test]
    async fn a_file_that_moved_on_is_refused_by_the_forge_and_not_overwritten() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/repos/test-owner/test-repo/contents/a.yaml"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": "stale-sha",
                "content": base64::engine::general_purpose::STANDARD.encode(ENDPOINT),
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/repos/test-owner/test-repo/contents"))
            .respond_with(
                ResponseTemplate::new(409).set_body_json(json!({"message": "sha does not match"})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let gitea = GiteaClient::new(
            server.uri().parse().expect("forge url"),
            "test-owner",
            "test-repo",
            "token",
        )
        .expect("client");

        let paths = BTreeSet::from(["a.yaml".to_owned()]);
        assert!(migrate(&gitea, "main", &paths).await.is_err());
    }

    const APP: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: App\nmetadata:\n  name: air-quality\n  namespace: helsinki\nspec:\n  dataNeeds:\n  - contextSpaceRef: air\n    types: [AirQualityObserved]\n    operations: [queryEntities]\n    representations: [ngsi-ld, geojson]\n  - contextSpaceRef: air\n    types: [WeatherObserved]\n    operations: [queryEntities]\n    representations: [xlsx]\n  - contextSpaceRef: air\n    types: [Device]\n    operations: [queryEntities]\n";

    fn need_lists(text: &str) -> Vec<Option<Vec<String>>> {
        let value: Value = serde_norway::from_str(text).expect("yaml");
        value["spec"]["dataNeeds"]
            .as_sequence()
            .expect("needs")
            .iter()
            .map(|need| {
                need.get("representations")
                    .and_then(Value::as_sequence)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|item| item.as_str().map(str::to_owned))
                            .collect()
                    })
            })
            .collect()
    }

    /// jc-core refuses an App whose data need names a retired representation, so its Endpoint
    /// and Policies stop rendering (T-5305): the run drops the name, a need left with none
    /// names ngsi-ld alone, and a need that listed nothing stays without a list.
    #[test]
    fn an_app_data_need_loses_the_retired_files_and_keeps_ngsi_ld() {
        let stripped = strip(APP).expect("the app names retired files");

        assert_eq!(
            need_lists(&stripped),
            [
                Some(vec!["ngsi-ld".to_owned()]),
                Some(vec!["ngsi-ld".to_owned()]),
                None
            ]
        );
        assert_eq!(strip(&stripped), None, "a second run changes nothing");
        let value: Value = serde_norway::from_str(&stripped).expect("yaml");
        assert_eq!(
            value["spec"]["dataNeeds"][0]["types"][0].as_str(),
            Some("AirQualityObserved")
        );
    }

    #[test]
    fn a_clean_app_is_not_rewritten() {
        let clean = "kind: App\nmetadata:\n  name: map\nspec:\n  dataNeeds:\n  - contextSpaceRef: air\n    types: [Device]\n    operations: [queryEntities]\n    representations: [ngsi-ld, csv]\n";
        assert_eq!(strip(clean), None);
        assert_eq!(strip("kind: App\nspec: {}\n"), None);
        assert_eq!(strip("kind: App\nspec:\n  dataNeeds: []\n"), None);
    }

    #[test]
    fn names_retired_reads_both_lists_and_app_data_needs() {
        assert!(names_retired(
            "Endpoint",
            &json!({"enabledRepresentations": ["csv", "zip"]})
        ));
        assert!(names_retired(
            "Endpoint",
            &json!({"disabledRepresentations": ["geojson"]})
        ));
        assert!(!names_retired(
            "Endpoint",
            &json!({"enabledRepresentations": ["csv", "ngsi-ld"]})
        ));
        assert!(!names_retired("Endpoint", &json!({})));
        assert!(names_retired(
            "App",
            &json!({"dataNeeds": [{"representations": ["csv"]}, {"representations": ["ngsi-ld", "geojson"]}]})
        ));
        assert!(!names_retired(
            "App",
            &json!({"dataNeeds": [{"representations": ["csv"]}, {}]})
        ));
        assert!(!names_retired(
            "ContextSpace",
            &json!({"enabledRepresentations": ["geojson"]})
        ));
    }

    #[test]
    fn a_project_file_belongs_to_the_project_only_in_layout_two() {
        let file = "projects/helsinki/endpoints/air.yaml";
        assert_eq!(
            owner(2, file),
            ("helsinki".to_owned(), "endpoints/air.yaml".to_owned())
        );
        assert_eq!(owner(1, file), (String::new(), file.to_owned()));
        assert_eq!(
            owner(2, "org/organization.yaml"),
            (String::new(), "org/organization.yaml".to_owned())
        );
    }

    #[tokio::test]
    async fn one_commit_carries_every_changed_file_and_a_clean_one_is_left_alone() {
        let server = MockServer::start().await;
        let clean =
            "kind: Endpoint\nmetadata:\n  name: bikes\nspec:\n  enabledRepresentations: [csv]\n";
        for (file, content) in [("a.yaml", ENDPOINT), ("b.yaml", clean)] {
            Mock::given(method("GET"))
                .and(path(format!(
                    "/api/v1/repos/test-owner/test-repo/contents/{file}"
                )))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "sha": "blob-sha",
                    "content": base64::engine::general_purpose::STANDARD.encode(content),
                })))
                .mount(&server)
                .await;
        }
        Mock::given(method("POST"))
            .and(path("/api/v1/repos/test-owner/test-repo/contents"))
            .respond_with(
                ResponseTemplate::new(201).set_body_json(json!({"commit": {"sha": "c1"}})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let gitea = GiteaClient::new(
            server.uri().parse().expect("forge url"),
            "test-owner",
            "test-repo",
            "token",
        )
        .expect("client");

        let paths = BTreeSet::from(["a.yaml".to_owned(), "b.yaml".to_owned()]);
        assert_eq!(migrate(&gitea, "main", &paths).await.expect("migrates"), 1);

        let requests = server.received_requests().await.expect("recorded");
        let commit = requests
            .iter()
            .find(|r| r.method.as_str() == "POST")
            .expect("one commit");
        let body: serde_json::Value = serde_json::from_slice(&commit.body).expect("json");
        let files = body["files"].as_array().expect("files");
        assert_eq!(files.len(), 1, "the clean file is not in the commit");
        assert_eq!(files[0]["path"], "a.yaml");
        assert_eq!(
            files[0]["operation"], "update",
            "an update the forge refuses when the file moved on"
        );
        assert_eq!(files[0]["sha"], "blob-sha", "the blob this run read");

        let only_clean = BTreeSet::from(["b.yaml".to_owned()]);
        assert_eq!(migrate(&gitea, "main", &only_clean).await.expect("ok"), 0);
    }
}
