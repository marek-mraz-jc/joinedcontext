//! A forge with memory for the scenario lane (T-5997): branches, trees and file contents of the
//! one test repository, so what an apply commits into a workspace is what its comparison, its
//! preview and its bring-back read again, and what a project export reads is what main holds.
//!
//! Each branch is a map of path to base64 content; its head is `head-` + the branch name in hex,
//! so a tree or a file asked for by commit finds its branch again. Writes are the Gitea API's
//! three: `PUT/POST /contents/{path}`, `DELETE /contents/{path}` and `POST /contents` (several
//! files in one commit); `POST /branches` copies a branch and refuses one that is there.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use wiremock::{Request, Respond, ResponseTemplate};

type Branches = BTreeMap<String, BTreeMap<String, String>>;

#[derive(Clone, Default)]
pub struct Repo {
    /// The repository's API path, `/api/v1/repos/{owner}/{repo}`.
    root: String,
    branches: Arc<Mutex<Branches>>,
}

fn head(branch: &str) -> String {
    let hex: String = branch.bytes().map(|b| format!("{b:02x}")).collect();
    format!("head-{hex}")
}

fn branch_of(reference: &str) -> String {
    let decoded = reference.strip_prefix("head-").and_then(|hex| {
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
            .collect::<Option<Vec<u8>>>()
            .and_then(|bytes| String::from_utf8(bytes).ok())
    });
    decoded.unwrap_or_else(|| reference.to_owned())
}

fn blob(content: &str) -> String {
    Sha256::digest(content.as_bytes())
        .iter()
        .take(10)
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl Repo {
    pub fn new(root: &str) -> Self {
        Self {
            root: root.to_owned(),
            ..Self::default()
        }
    }

    /// A file on `branch` before the run, as a seeded repository holds it.
    pub fn seed(&self, branch: &str, path: &str, text: &str) {
        let content = base64::engine::general_purpose::STANDARD.encode(text);
        self.lock()
            .entry(branch.to_owned())
            .or_default()
            .insert(path.to_owned(), content);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Branches> {
        self.branches.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn write(&self, branch: &str, path: &str, content: Option<&str>) {
        let mut branches = self.lock();
        let files = branches.entry(branch.to_owned()).or_default();
        match content {
            Some(content) => files.insert(path.to_owned(), content.to_owned()),
            None => files.remove(path),
        };
    }

    fn query(request: &Request, key: &str) -> Option<String> {
        request
            .url
            .query_pairs()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.into_owned())
    }
}

fn committed() -> ResponseTemplate {
    ResponseTemplate::new(201).set_body_json(json!({ "commit": { "sha": "commit-1" } }))
}

impl Respond for Repo {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let Some(rest) = request.url.path().strip_prefix(&self.root) else {
            return ResponseTemplate::new(404);
        };
        let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
        let text = |key: &str| body[key].as_str().unwrap_or_default().to_owned();
        match (request.method.as_str(), rest) {
            ("GET", rest) if rest.starts_with("/branches/") => {
                let name = &rest["/branches/".len()..];
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "name": name, "commit": { "id": head(name) } }))
            }
            ("POST", "/branches") => {
                let mut branches = self.lock();
                // Gitea refuses a branch that is there already, as the Portal expects.
                if branches.contains_key(&text("new_branch_name")) {
                    return ResponseTemplate::new(409)
                        .set_body_json(json!({ "message": "branch already exists" }));
                }
                let copy = branches
                    .get(&text("old_branch_name"))
                    .cloned()
                    .unwrap_or_default();
                branches.insert(text("new_branch_name"), copy);
                ResponseTemplate::new(201).set_body_json(json!({}))
            }
            ("GET", rest) if rest.starts_with("/git/trees/") => {
                let reference = &rest["/git/trees/".len()..];
                let branches = self.lock();
                let tree: Vec<Value> = branches
                    .get(&branch_of(reference))
                    .into_iter()
                    .flatten()
                    .map(|(path, content)| json!({ "path": path, "type": "blob", "sha": blob(content) }))
                    .collect();
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "sha": reference, "tree": tree, "truncated": false }))
            }
            ("GET", rest) if rest.starts_with("/contents/") => {
                let path = &rest["/contents/".len()..];
                let branch =
                    branch_of(&Self::query(request, "ref").unwrap_or_else(|| "main".into()));
                match self.lock().get(&branch).and_then(|files| files.get(path)) {
                    Some(content) => ResponseTemplate::new(200)
                        .set_body_json(json!({ "sha": blob(content), "content": content })),
                    None => {
                        ResponseTemplate::new(404).set_body_json(json!({ "message": "not found" }))
                    }
                }
            }
            ("PUT" | "POST", rest) if rest.starts_with("/contents/") => {
                self.write(
                    &text("branch"),
                    &rest["/contents/".len()..],
                    Some(&text("content")),
                );
                committed()
            }
            ("DELETE", rest) if rest.starts_with("/contents/") => {
                self.write(&text("branch"), &rest["/contents/".len()..], None);
                committed()
            }
            ("POST", "/contents") => {
                for file in body["files"].as_array().into_iter().flatten() {
                    let path = file["path"].as_str().unwrap_or_default();
                    let content = (file["operation"] != json!("delete"))
                        .then(|| file["content"].as_str().unwrap_or_default());
                    self.write(&text("branch"), path, content);
                }
                committed()
            }
            _ => ResponseTemplate::new(404).set_body_json(json!({ "message": "not found" })),
        }
    }
}

#[test]
fn a_head_names_its_branch_again() {
    assert_eq!(branch_of(&head("workspace/s43")), "workspace/s43");
    assert_eq!(branch_of("main"), "main");
}
