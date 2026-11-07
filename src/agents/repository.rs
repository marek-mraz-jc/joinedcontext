//! One forge repository per generated `static` application (AP-75…AP-78).
//!
//! The application's whole source lives at the root of `{project}_{app}` in the organization of
//! the configuration repository; each run commits to its own branch there, publish opens the
//! merge request into `main`, and the approval of the `App` Change merges it.

use std::time::Duration;

use super::run::AgentRun;
use crate::git::{GitError, GiteaClient, MergeStyle, PullRequest};

/// The longest repository name the forge accepts.
pub const MAX_NAME: usize = 100;

/// The file the Portal writes beside the application's own files, saying how to run it.
pub const README: &str = "README.md";

/// The application's build, the Portal's and never the run's (AP-100, ADR-N-028 §3.2).
pub const WORKFLOW: &str = ".gitea/workflows/build.yml";
/// The folder the forge reads workflows from; a run commit never touches it.
const BUILD_FOLDER: &str = ".gitea/";

/// The workflow every application repository carries, as the SDK template holds it.
pub const WORKFLOW_TEXT: &str = include_str!("../../sdk/template/.gitea/workflows/build.yml");
/// The workflow of a `wasm` App, on the runner with the Rust toolchain (T-3575, AP-151).
pub const WASM_WORKFLOW_TEXT: &str =
    include_str!("../../sdk/template-wasm/.gitea/workflows/build.yml");

/// Why a run commit cannot land, when it would touch the application's build (AP-100); `None`
/// when it may. The first commit of a run carries the template's workflow and nothing else under
/// `.gitea/`; every later commit leaves the folder as the first one wrote it, so only a workflow
/// a reviewer merged ever runs, and a run cannot reach the lane's secret.
pub fn build_refusal(
    files: &std::collections::BTreeMap<String, String>,
    committed: &std::collections::BTreeMap<String, String>,
) -> Option<String> {
    let under = |path: &&String| path.starts_with(BUILD_FOLDER);
    let refused = |path: &str| {
        Some(format!(
            "{path}: the application's build is written by the Portal in the first commit and \
             a run never changes it (AP-100)"
        ))
    };
    if committed.is_empty() {
        if let Some(path) = files
            .keys()
            .filter(under)
            .find(|path| path.as_str() != WORKFLOW)
        {
            return refused(path);
        }
        return match files.get(WORKFLOW) {
            Some(text) if text != WORKFLOW_TEXT && text != WASM_WORKFLOW_TEXT => refused(WORKFLOW),
            _ => None,
        };
    }
    files
        .keys()
        .chain(committed.keys())
        .filter(under)
        .find(|path| files.get(*path) != committed.get(*path))
        .and_then(|path| refused(path))
}

/// `{project}_{app}`: no project or application name holds a `_` (both are DNS-1123 labels),
/// so two projects can never land on one repository (AP-75).
pub fn name(project: &str, app: &str) -> String {
    format!("{project}_{app}")
}

/// Why an application's repository cannot be named, as a person fixes it; `None` when it can.
pub fn name_refusal(project: &str, app: &str) -> Option<String> {
    let name = name(project, app);
    (name.len() > MAX_NAME).then(|| {
        format!(
            "the application's repository '{name}' would be {} characters long and the forge \
             allows {MAX_NAME}; choose an application name of at most {} characters (AP-75)",
            name.len(),
            MAX_NAME.saturating_sub(project.len() + 1)
        )
    })
}

/// Whether runs of this class and kind commit to the application's own repository: a `ui`
/// application (`static` before AP-124) or a `wasm` one (T-3575), which the Portal writes itself. A workspace run reaches the forge only through
/// the proxy's `/v1/forge` route, whose rules name the configuration repository's folder.
pub fn owns_repository(app_class: &str, kind: &str) -> bool {
    matches!(
        jc_core::kinds::AppClass::parse(app_class),
        Ok(jc_core::kinds::AppClass::Ui | jc_core::kinds::AppClass::Wasm)
    ) && kind == "application"
}

impl AgentRun {
    /// Whether this run's source is the application's own repository (AP-75). A run recorded
    /// before the application had one keeps its folder in the configuration repository.
    pub fn in_own_repository(&self) -> bool {
        owns_repository(&self.app_class, &self.kind) && self.path_prefix.is_empty()
    }
}

/// The README a clone opens on: what the application is, where it came from, how to run it.
pub fn readme(project: &str, app: &str, title: Option<&str>, clone_url: &str) -> String {
    let heading = title.filter(|t| !t.trim().is_empty()).unwrap_or(app);
    format!(
        "# {heading}\n\
         \n\
         The application `{app}` of project `{project}`, generated in the joinedcontext Portal. \
         This repository holds its whole source; the Portal commits every version of a run to \
         the run's branch, and publishing merges that branch into `main`.\n\
         \n\
         ## Run it\n\
         \n\
         ```sh\n\
         git clone {clone_url}\n\
         cd {repo}\n\
         pnpm install\n\
         pnpm test\n\
         pnpm dev\n\
         ```\n\
         \n\
         `pnpm install` needs the `@joinedcontext/sdk` version `package.json` pins. `pnpm build` \
         writes the static bundle to `dist/`.\n\
         \n\
         ## Where it reads\n\
         \n\
         The application reads and writes only through the project's endpoints, with the \
         signed-in person's own grants. It holds no credential: the page it is served in names \
         the endpoints in `#jc-config`.\n",
        repo = name(project, app),
    )
}

/// The open merge request from the run's branch into the repository's default branch, or a new
/// one when none is open (AP-77).
pub async fn open_merge_request(
    repo: &GiteaClient,
    run: &AgentRun,
) -> Result<PullRequest, GitError> {
    let base = repo.default_branch().await?;
    if let Some(open) = repo
        .list_pull_requests("open")
        .await?
        .into_iter()
        .find(|pull| pull.head_branch == run.branch && pull.base_branch == base)
    {
        return Ok(open);
    }
    let title = format!(
        "Publish {}",
        run.title.as_deref().unwrap_or(run.app_name.as_str())
    );
    let body = format!(
        "The version run `{}` built for application `{}` of project `{}`. It merges when the \
         Change that publishes the application is approved in the Portal.",
        run.id, run.app_name, run.project
    );
    repo.create_pull_request(&run.branch, &base, &title, &body)
        .await
}

/// Whether the tree at `sha` holds the application: a file that is neither the Portal's README
/// nor the build folder (AP-77). A run that never committed a version has a branch that is `main`
/// as the repository was created, and merging it would leave `main` with the README alone, which
/// is how a published application once served nothing (T-2603).
pub async fn holds_application(repo: &GiteaClient, sha: &str) -> Result<bool, GitError> {
    Ok(repo
        .list_tree(sha)
        .await?
        .iter()
        .any(|path| path != README && !path.starts_with(BUILD_FOLDER)))
}

/// Merges the run's merge request once its `App` Change is approved, pinned to `sha`, the commit
/// the approved manifest names: a branch that moved after publish is refused, never merged
/// unread (AP-77, PF-57). A merge commit keeps `sha` in `main`'s history.
pub async fn merge_published(
    repo: &GiteaClient,
    run: &AgentRun,
    sha: &str,
    approver: &str,
) -> Result<(), GitError> {
    let pull = repo
        .list_pull_requests("open")
        .await?
        .into_iter()
        .find(|pull| pull.head_branch == run.branch)
        .ok_or(GitError::NotFound)?;
    let message = format!(
        "Publish {} from run {}\n\nApproved in the Portal by {approver}",
        run.app_name, run.id
    );
    // Gitea answers 405 until it has checked a fresh pull's mergeability; the approval that
    // follows a publish within seconds waits that out, as `approve_change` does.
    let mut attempt = 0;
    loop {
        match repo
            .merge(pull.number, MergeStyle::Merge, &message, Some(sha))
            .await
        {
            Err(GitError::Api { status: 405, .. }) if attempt < 15 => {
                attempt += 1;
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            other => return other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn tree(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(path, text)| ((*path).to_owned(), (*text).to_owned()))
            .collect()
    }

    /// AP-100: the template every run starts from carries the workflow, byte for byte the file
    /// the Portal commits, so a new repository builds itself on its first push.
    #[test]
    fn the_template_carries_the_workflow_the_first_commit_writes() {
        let template = super::super::preview::template_files();
        assert_eq!(
            template.get(WORKFLOW).map(String::as_str),
            Some(WORKFLOW_TEXT)
        );
        assert!(
            WORKFLOW_TEXT.contains("workflow_dispatch"),
            "Rebuild dispatches it (AP-103)"
        );
        assert!(WORKFLOW_TEXT.contains("branches: [main]"));
        // AP-80, T-1707: the build, and the HOME build-app gives the application, live under the
        // job's temp dir, which runner.sh wipes before the next job registers, so nothing the
        // application's code writes there reaches a later job or the propose job's token.
        assert!(
            WORKFLOW_TEXT.contains("JC_BUILD_DIR: ${{ runner.temp }}/jc-build"),
            "the build dir is per job"
        );
        assert!(
            WORKFLOW_TEXT.matches("secrets.").count() == 1
                && WORKFLOW_TEXT.contains("${{ secrets.JC_LANE_TOKEN }}"),
            "the lane's secret is in one step's environment and no other secret is (AP-80)"
        );
        let (build, propose) = WORKFLOW_TEXT
            .split_once("\n  propose:\n")
            .expect("a propose job after the build job");
        assert!(
            propose.contains("needs: build")
                && propose.contains("secrets.JC_LANE_TOKEN")
                && !propose.contains("build-app")
                && !build.contains("secrets."),
            "the job that runs the application's code never sees the lane's secret (AP-80)"
        );
        assert_eq!(build_refusal(&template, &BTreeMap::new()), None);
    }

    /// AP-100: a first commit carries the template's workflow and nothing else under `.gitea/`,
    /// and every later commit that adds, changes or deletes anything there is refused.
    #[test]
    fn a_run_commit_never_changes_the_build() {
        let first = tree(&[(WORKFLOW, WORKFLOW_TEXT), ("src/App.tsx", "a")]);
        assert_eq!(build_refusal(&first, &BTreeMap::new()), None);
        for bad in [
            tree(&[(WORKFLOW, "on: push\n"), ("src/App.tsx", "a")]),
            tree(&[
                (WORKFLOW, WORKFLOW_TEXT),
                (".gitea/workflows/steal.yml", "x"),
            ]),
        ] {
            let refusal = build_refusal(&bad, &BTreeMap::new()).expect("refused");
            assert!(
                refusal.contains("AP-100") && refusal.contains(".gitea/"),
                "{refusal}"
            );
        }

        let committed = first.clone();
        let edited = tree(&[(WORKFLOW, WORKFLOW_TEXT), ("src/App.tsx", "b")]);
        assert_eq!(
            build_refusal(&edited, &committed),
            None,
            "the app changes, the build does not"
        );
        let mut changed = edited.clone();
        changed.insert(WORKFLOW.to_owned(), "on: [push]\n".to_owned());
        let mut added = edited.clone();
        added.insert(".gitea/actions/x.yml".to_owned(), "x".to_owned());
        let mut deleted = edited.clone();
        deleted.remove(WORKFLOW);
        for bad in [changed, added, deleted] {
            assert!(
                build_refusal(&bad, &committed).is_some(),
                "{:?}",
                bad.keys()
            );
        }
    }

    /// AP-75: the name joins project and application with the one character neither can hold.
    #[test]
    fn two_projects_never_share_an_application_repository() {
        assert_eq!(name("helsinki", "city-bikes"), "helsinki_city-bikes");
        assert_ne!(name("a-b", "c"), name("a", "b-c"));
    }

    /// AP-75: a name the forge would refuse is refused at the request, with the length to aim for.
    #[test]
    fn a_repository_name_longer_than_the_forge_allows_is_refused_with_the_limit() {
        assert_eq!(name_refusal("helsinki", "city-bikes"), None);
        let project = "p".repeat(60);
        let app = "a".repeat(40);
        assert_eq!(name(&project, &app).len(), 101);
        let refusal = name_refusal(&project, &app).expect("101 characters is refused");
        assert!(refusal.contains("101 characters"), "{refusal}");
        assert!(refusal.contains("at most 39 characters"), "{refusal}");
        assert_eq!(name_refusal(&project, &"a".repeat(39)), None);
    }

    /// AP-75: only a `ui` application the Portal writes owns a repository; a run recorded as
    /// `static` before AP-124 still does.
    #[test]
    fn only_a_ui_application_run_owns_a_repository() {
        assert!(owns_repository("ui", "application"));
        assert!(owns_repository("static", "application"));
        assert!(!owns_repository("ui", "dashboard"));
        assert!(!owns_repository("ui", "analysis"));
        assert!(!owns_repository("ui-rust", "application"));
        assert!(!owns_repository("fullstack", "application"));
        assert!(!owns_repository("service", "application"));
        assert!(!owns_repository("ui-node", "application"));
    }

    /// AP-75: the README says how to run the application and names the repository.
    #[test]
    fn the_readme_says_how_to_run_the_application() {
        let text = readme(
            "helsinki",
            "city-bikes",
            Some("City bikes"),
            "https://forge.example/joinedcontext/helsinki_city-bikes.git",
        );
        assert!(text.starts_with("# City bikes\n"));
        assert!(
            text.contains("git clone https://forge.example/joinedcontext/helsinki_city-bikes.git")
        );
        assert!(text.contains("cd helsinki_city-bikes"));
        assert!(text.contains("pnpm install"));
        assert!(readme("helsinki", "city-bikes", Some("  "), "u").starts_with("# city-bikes\n"));
    }

    fn a_run() -> AgentRun {
        serde_json::from_value(serde_json::json!({
            "id": "run-1",
            "project": "helsinki",
            "appName": "city-bikes",
            "title": "City bikes",
            "endpointName": "helsinki-bikes",
            "endpointSlug": "s",
            "profile": "app-builder",
            "kind": "application",
            "unattended": false,
            "appClass": "ui",
            "visibility": "project",
            "prompt": "p",
            "promptDigest": "d",
            "dataNeeds": [],
            "allowsWrite": false,
            "branch": "agent/app-city-bikes/run-1",
            "pathPrefix": "",
            "status": "previewing",
            "ticketHash": "",
            "steps": 0,
            "tokensUsed": 0,
            "createdBy": "demo.builder",
            "createdAt": "2026-09-21T06:00:00Z",
            "expiresAt": "2099-09-21T06:00:00Z"
        }))
        .expect("a run")
    }

    fn pull(number: u64, head: &str, sha: &str) -> serde_json::Value {
        serde_json::json!({
            "number": number,
            "html_url": format!("http://forge/pulls/{number}"),
            "state": "open",
            "title": "t",
            "head": { "ref": head, "sha": sha },
            "base": { "ref": "main", "sha": "base" },
        })
    }

    async fn forge() -> (wiremock::MockServer, GiteaClient) {
        let server = wiremock::MockServer::start().await;
        let config = GiteaClient::new(
            server.uri().parse().expect("a url"),
            "joinedcontext",
            "configuration",
            "t",
        )
        .expect("a client");
        (server, config.for_repository("helsinki_city-bikes"))
    }

    const REPO: &str = "/api/v1/repos/joinedcontext/helsinki_city-bikes";

    /// AP-76: a run in its own repository writes there and nowhere else.
    #[test]
    fn a_static_application_run_is_in_its_own_repository_and_an_older_one_is_not() {
        let mut run = a_run();
        assert!(run.in_own_repository());
        run.path_prefix = "projects/helsinki/apps/city-bikes/".into();
        assert!(
            !run.in_own_repository(),
            "a run recorded before AP-75 moved"
        );
    }

    /// AP-77: publishing twice reuses the open merge request instead of opening a second one.
    #[tokio::test]
    async fn publishing_again_reuses_the_open_merge_request() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, ResponseTemplate};
        let (server, repo) = forge().await;
        Mock::given(method("GET"))
            .and(path(REPO))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "default_branch": "main" })),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/pulls")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                pull(3, "agent/app-city-bikes/other", "x"),
                pull(7, "agent/app-city-bikes/run-1", "abc"),
            ])))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("{REPO}/pulls")))
            .respond_with(ResponseTemplate::new(201).set_body_json(pull(9, "x", "y")))
            .expect(0)
            .mount(&server)
            .await;
        let open = open_merge_request(&repo, &a_run()).await.expect("reused");
        assert_eq!(open.number, 7);
    }

    /// AP-77: the first publish opens the merge request from the run's branch into `main`.
    #[tokio::test]
    async fn the_first_publish_opens_the_merge_request_from_the_runs_branch_into_main() {
        use wiremock::matchers::{body_partial_json, method, path};
        use wiremock::{Mock, ResponseTemplate};
        let (server, repo) = forge().await;
        Mock::given(method("GET"))
            .and(path(REPO))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "default_branch": "main" })),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/pulls")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("{REPO}/pulls")))
            .and(body_partial_json(serde_json::json!({
                "head": "agent/app-city-bikes/run-1",
                "base": "main",
                "title": "Publish City bikes",
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(pull(
                4,
                "agent/app-city-bikes/run-1",
                "abc",
            )))
            .expect(1)
            .mount(&server)
            .await;
        let opened = open_merge_request(&repo, &a_run()).await.expect("opened");
        assert_eq!(opened.number, 4);
    }

    /// AP-77, PF-57: the approval merges with a merge commit pinned to the published commit, and
    /// a run whose merge request is gone is an error, not a silent success.
    #[tokio::test]
    async fn the_approval_merges_the_published_commit_and_nothing_else() {
        use wiremock::matchers::{body_json, method, path};
        use wiremock::{Mock, ResponseTemplate};
        let (server, repo) = forge().await;
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/pulls")))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!([pull(
                    7,
                    "agent/app-city-bikes/run-1",
                    "abc"
                )])),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("{REPO}/pulls/7/merge")))
            .and(body_json(serde_json::json!({
                "Do": "merge",
                "merge_message_field": "Publish city-bikes from run run-1\n\nApproved in the Portal by approver@hel.fi",
                "head_commit_id": "abc",
            })))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        merge_published(&repo, &a_run(), "abc", "approver@hel.fi")
            .await
            .expect("merged");

        let mut other = a_run();
        other.branch = "agent/app-city-bikes/run-2".into();
        assert_eq!(
            merge_published(&repo, &other, "abc", "approver@hel.fi").await,
            Err(GitError::NotFound)
        );
    }

    fn tree_of(paths: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "sha": "abc",
            "truncated": false,
            "tree": paths.iter().map(|p| serde_json::json!({ "path": p, "type": "blob", "sha": "b" })).collect::<Vec<_>>(),
        })
    }

    /// AP-77, T-2603: a branch holding only the README and the build workflow is no application,
    /// so publishing it would leave `main` README-only; one file of its own is enough.
    #[tokio::test]
    async fn a_branch_with_only_the_readme_and_the_workflow_holds_no_application() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, ResponseTemplate};
        let (server, repo) = forge().await;
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/git/trees/empty")))
            .respond_with(ResponseTemplate::new(200).set_body_json(tree_of(&[README, WORKFLOW])))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/git/trees/built")))
            .respond_with(ResponseTemplate::new(200).set_body_json(tree_of(&[
                README,
                WORKFLOW,
                "src/App.tsx",
            ])))
            .mount(&server)
            .await;
        assert!(!holds_application(&repo, "empty").await.expect("read"));
        assert!(holds_application(&repo, "built").await.expect("read"));
    }
}
