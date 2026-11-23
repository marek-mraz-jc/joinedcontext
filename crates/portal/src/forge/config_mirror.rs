//! The configuration kept off the pod (T-6892): the installation repository and every project
//! repository pushed to a private remote the operator names.
//!
//! The forge stays the repository of record. Each repository gets a push mirror scoped to
//! `main`: the forge pushes `main` and new tags after every commit, fast-forward only, so an open
//! Change's branch never leaves and a remote that moved elsewhere is reported, never
//! overwritten. The reconciler calls [`ConfigMirror::pass`] on every run: it adds what is
//! missing (the first run catches up on everything merged before), and pushes again, with
//! backoff, every mirror whose last push failed. A failed push never touches a merge.
//!
//! The installation repository holds every Organization, so it goes to the operator's remote
//! only: any other mirror on it is taken off. A per-Organization remote for its projects would
//! need a remote and a token per Organization, kept in its own Secret, and a rule that a project
//! repository's mirror follows its Organization's; this module has the operator's only.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use jc_time::TimestampTz;
use serde::Deserialize;
use url::Url;

use super::shako::{GitError, ShakoClient};

/// The one branch the remote receives.
const BRANCH: &str = "main";
/// The user name a token is sent under; GitHub reads any, this one says what it is.
const MIRROR_USER: &str = "x-access-token";
/// The longest wait between two pushes of a mirror that keeps failing.
const MAX_BACKOFF: Duration = Duration::from_secs(30 * 60);
pub const FAILURES_METRIC: &str = "jc_forge_mirror_failures_total";

/// Where the configuration goes and the file holding the token that pushes it.
pub struct ConfigMirror {
    /// `https://host/owner/prefix{repo}.git`: `{repo}` is the forge repository's name.
    remote: String,
    token_file: std::path::PathBuf,
    /// The host whose API tells a private repository from a public one, and that API.
    github: (String, Url),
    /// How often a run looks at the mirrors at all.
    every: Duration,
    last_pass: Mutex<Option<Instant>>,
    backoff: Duration,
    http: reqwest::Client,
    repos: Mutex<BTreeMap<String, RepoState>>,
}

impl std::fmt::Debug for ConfigMirror {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfigMirror")
            .field("remote", &self.remote)
            .field("token_file", &self.token_file)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Default)]
struct RepoState {
    /// Failed pushes in a row; 0 once one went through.
    failures: u32,
    next_try: Option<Instant>,
    last_error: String,
    checked_private: bool,
}

/// One repository's mirror, as the health page shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirrorStatus {
    pub repository: String,
    /// Empty when the last push went through.
    pub last_error: String,
}

#[derive(Deserialize)]
struct GithubRepo {
    private: bool,
}

impl ConfigMirror {
    /// `JC_CONFIG_MIRROR_REMOTE` (an https address with `{repo}`, no credential in it) and
    /// `JC_CONFIG_MIRROR_TOKEN_FILE` (a file on the secrets mount) are set together or not at
    /// all: `Ok(None)` when neither is, an error when one is or the address is unfit.
    pub fn from_env(lookup: impl Fn(&str) -> Option<String>) -> Result<Option<Self>, GitError> {
        let remote = lookup("JC_CONFIG_MIRROR_REMOTE").filter(|v| !v.trim().is_empty());
        let token_file = lookup("JC_CONFIG_MIRROR_TOKEN_FILE").filter(|v| !v.trim().is_empty());
        let (remote, token_file) = match (remote, token_file) {
            (None, None) => return Ok(None),
            (Some(r), Some(t)) => (r.trim().to_owned(), t.trim().to_owned()),
            _ => {
                return Err(GitError::Config(
                    "JC_CONFIG_MIRROR_REMOTE and JC_CONFIG_MIRROR_TOKEN_FILE must be set together"
                        .to_owned(),
                ));
            }
        };
        if !remote.starts_with("https://") {
            return Err(GitError::Config(
                "JC_CONFIG_MIRROR_REMOTE must be an https address: the token travels with every \
                 push"
                    .to_owned(),
            ));
        }
        let mirror = Self::new(&remote, token_file.into())?;
        // A token file that cannot be read is a startup error, not a mirror that never pushes.
        mirror.token()?;
        Ok(Some(mirror))
    }

    /// The mirror without the https rule `from_env` adds (a test remote speaks plain HTTP).
    pub fn new(remote: &str, token_file: std::path::PathBuf) -> Result<Self, GitError> {
        let probe = Url::parse(&remote.replace("{repo}", "r")).map_err(|e| {
            GitError::Config(format!(
                "JC_CONFIG_MIRROR_REMOTE is not an address: {e}; example: \
                 https://github.com/acme/jc-{{repo}}.git"
            ))
        })?;
        if !remote.contains("{repo}") {
            return Err(GitError::Config(
                "JC_CONFIG_MIRROR_REMOTE needs {repo}, one remote repository per forge \
                 repository; example: https://github.com/acme/jc-{repo}.git"
                    .to_owned(),
            ));
        }
        if !probe.username().is_empty()
            || probe.password().is_some()
            || probe.query().is_some()
            || probe.fragment().is_some()
        {
            return Err(GitError::Config(
                "JC_CONFIG_MIRROR_REMOTE carries no credential, query or fragment: the token \
                 comes from JC_CONFIG_MIRROR_TOKEN_FILE"
                    .to_owned(),
            ));
        }
        let http = reqwest::ClientBuilder::new()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| GitError::Transport(e.to_string()))?;
        Ok(Self {
            remote: remote.to_owned(),
            token_file,
            github: (
                "github.com".to_owned(),
                Url::parse("https://api.github.com").expect("a constant address"),
            ),
            every: Duration::from_secs(60),
            last_pass: Mutex::default(),
            backoff: Duration::from_secs(60),
            http,
            repos: Mutex::default(),
        })
    }

    /// How often a run looks at the mirrors (a minute), and the first wait after a failed push,
    /// which doubles up to 30 minutes (a minute).
    pub fn with_timing(mut self, every: Duration, backoff: Duration) -> Self {
        self.every = every;
        self.backoff = backoff;
        self
    }

    #[cfg(test)]
    fn with_github(mut self, host: &str, api: &str) -> Self {
        self.github = (host.to_owned(), Url::parse(api).expect("a stub address"));
        self
    }

    /// Read on every set-up, so a rotated token is used without a restart.
    fn token(&self) -> Result<String, GitError> {
        let token = std::fs::read_to_string(&self.token_file).map_err(|e| {
            GitError::Config(format!(
                "the configuration mirror's token file {} cannot be read: {}",
                self.token_file.display(),
                e.kind()
            ))
        })?;
        let token = token.trim().to_owned();
        if token.is_empty() {
            return Err(GitError::Config(format!(
                "the configuration mirror's token file {} is empty",
                self.token_file.display()
            )));
        }
        Ok(token)
    }

    /// Where the forge repository `repo` goes.
    pub fn remote_for(&self, repo: &str) -> String {
        self.remote.replace("{repo}", repo)
    }

    /// One run over the installation repository and the project repositories: each gets its
    /// mirror, and a mirror whose last push failed is pushed again once its backoff is over.
    /// Nothing here fails the run; what failed is kept for [`Self::statuses`] and counted.
    pub async fn pass(&self, installation: &ShakoClient, projects: &[ShakoClient]) {
        {
            let mut last = self
                .last_pass
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if last.is_some_and(|at| at.elapsed() < self.every) {
                return;
            }
            *last = Some(Instant::now());
        }
        let all = std::iter::once((installation, true)).chain(projects.iter().map(|p| (p, false)));
        for (forge, is_installation) in all {
            let outcome = self.keep(forge, is_installation).await;
            self.record(&forge.repo, outcome);
        }
    }

    /// The last outcome per repository, by name.
    pub fn statuses(&self) -> Vec<MirrorStatus> {
        self.lock()
            .iter()
            .map(|(repository, s)| MirrorStatus {
                repository: repository.clone(),
                last_error: s.last_error.clone(),
            })
            .collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, RepoState>> {
        self.repos
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// `Err`: what the set-up or the last push failed with.
    async fn keep(&self, forge: &ShakoClient, is_installation: bool) -> Result<(), String> {
        let target = self.remote_for(&forge.repo);
        let state = self.lock().get(&forge.repo).cloned().unwrap_or_default();
        if state.next_try.is_some_and(|at| Instant::now() < at) {
            return Err(state.last_error);
        }
        if !state.checked_private {
            self.refuse_public(&target).await?;
            self.lock()
                .entry(forge.repo.clone())
                .or_default()
                .checked_private = true;
        }
        let mirrors = forge.push_mirrors().await.map_err(|e| e.to_string())?;
        let mut ours = None;
        for m in mirrors {
            let same = m
                .remote_address
                .trim_end_matches('/')
                .eq_ignore_ascii_case(&target);
            if same && m.branch == BRANCH && ours.is_none() {
                ours = Some(m);
            } else if same || is_installation {
                // An unscoped copy of our remote, or anything else on the installation
                // repository: the installation repository holds every Organization and goes to
                // the operator's remote only.
                tracing::warn!(repository = %forge.repo, mirror = %m.remote_name, "a push mirror that is not the operator's scoped one is taken off");
                forge
                    .delete_push_mirror(&m.remote_name)
                    .await
                    .map_err(|e| e.to_string())?;
            }
        }
        match ours {
            Some(m) if m.last_error.trim().is_empty() => return Ok(()),
            // A failing mirror most often holds a token rotated since: set it up again with
            // today's, then push. The forge offers no way to change a mirror's credential.
            Some(m) => {
                metrics::counter!(FAILURES_METRIC, "repository" => forge.repo.clone()).increment(1);
                forge
                    .delete_push_mirror(&m.remote_name)
                    .await
                    .map_err(|e| e.to_string())?;
            }
            None => {}
        }
        let token = self.token().map_err(|e| e.to_string())?;
        forge
            .add_push_mirror(&target, MIRROR_USER, &token, Some(BRANCH))
            .await
            .map_err(|e| e.to_string())?;
        forge.sync_push_mirrors().await.map_err(|e| e.to_string())?;
        let pushed = forge.push_mirrors().await.map_err(|e| e.to_string())?;
        match pushed.into_iter().find(|m| {
            m.branch == BRANCH
                && m.remote_address
                    .trim_end_matches('/')
                    .eq_ignore_ascii_case(&target)
        }) {
            Some(m) if !m.last_error.trim().is_empty() => {
                metrics::counter!(FAILURES_METRIC, "repository" => forge.repo.clone()).increment(1);
                Err(m.last_error)
            }
            Some(_) => Ok(()),
            None => Err("the forge did not keep the push mirror".to_owned()),
        }
    }

    fn record(&self, repo: &str, outcome: Result<(), String>) {
        let mut repos = self.lock();
        let state = repos.entry(repo.to_owned()).or_default();
        match outcome {
            Ok(()) => {
                state.failures = 0;
                state.next_try = None;
                state.last_error.clear();
            }
            Err(error) => {
                // Still waiting out the backoff: the failure that started it stands.
                if state.next_try.is_some_and(|at| Instant::now() < at) {
                    return;
                }
                state.failures = state.failures.saturating_add(1);
                let wait = self
                    .backoff
                    .saturating_mul(1 << state.failures.saturating_sub(1).min(16))
                    .min(MAX_BACKOFF);
                state.next_try = Some(Instant::now() + wait);
                tracing::warn!(repository = %repo, %error, retry_in_secs = wait.as_secs(), "the configuration mirror did not push");
                state.last_error = error;
            }
        }
    }

    /// GitHub says whether a repository is public: a public one is refused. Another host
    /// cannot tell, so its privacy is the operator's word (docs).
    async fn refuse_public(&self, target: &str) -> Result<(), String> {
        let Ok(url) = Url::parse(target) else {
            return Err("the remote is not an address".to_owned());
        };
        let (host, github_api) = &self.github;
        if url.host_str() != Some(host.as_str()) {
            return Ok(());
        }
        let mut path = url
            .path()
            .trim_matches('/')
            .trim_end_matches(".git")
            .splitn(2, '/');
        let (Some(owner), Some(name)) = (path.next(), path.next()) else {
            return Err(format!("{target} names no owner/repository"));
        };
        let api = format!(
            "{}/repos/{owner}/{name}",
            github_api.as_str().trim_end_matches('/')
        );
        let token = self.token().map_err(|e| e.to_string())?;
        let res = self
            .http
            .get(&api)
            .bearer_auth(token)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .header(reqwest::header::USER_AGENT, "joinedcontext-portal")
            .send()
            .await
            .map_err(|e| format!("GitHub did not answer: {}", e.without_url()))?;
        if res.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(format!(
                "{target} does not exist or the token cannot read it: create it as a private \
                 repository and give the token contents write on it"
            ));
        }
        if !res.status().is_success() {
            return Err(format!("GitHub answered {} for {target}", res.status()));
        }
        let repo: GithubRepo = res
            .json()
            .await
            .map_err(|e| format!("GitHub's answer did not read: {}", e.without_url()))?;
        if !repo.private {
            return Err(format!(
                "{target} is public: the configuration goes to a private repository only"
            ));
        }
        Ok(())
    }

    /// The health page's row: every repository, green or with what its push failed with.
    pub fn digest(&self, at: TimestampTz) -> crate::api::validation::Digest {
        use crate::api::validation::{Counts, Digest, Failure, FailureVerdict};
        let statuses = self.statuses();
        let (failed, passed): (Vec<_>, Vec<_>) =
            statuses.into_iter().partition(|s| !s.last_error.is_empty());
        Digest {
            check: "configuration-mirror".to_owned(),
            at,
            every_hours: 1,
            run: None,
            counts: Counts {
                pass: u32::try_from(passed.len()).unwrap_or(u32::MAX),
                fail: u32::try_from(failed.len()).unwrap_or(u32::MAX),
                error: 0,
                skip: 0,
            },
            failures: failed
                .into_iter()
                .map(|s| Failure {
                    title: format!("{}: {}", s.repository, s.last_error),
                    key: s.repository,
                    verdict: FailureVerdict::Fail,
                    task: None,
                })
                .collect(),
            history: Vec::new(),
            passed: passed.into_iter().map(|s| s.repository).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const TOKEN: &str = "ghp_config-mirror-token-never-printed";

    /// A token file of its own per call: the tests run in parallel.
    fn token_file(token: &str) -> String {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let file =
            std::env::temp_dir().join(format!("jc-config-mirror-{}-{n}", std::process::id()));
        std::fs::write(&file, format!("{token}\n")).expect("token file");
        file.display().to_string()
    }

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        }
    }

    #[test]
    fn it_is_off_by_default_and_refuses_an_unfit_remote() {
        let file = token_file(TOKEN);
        assert!(ConfigMirror::from_env(env(&[])).expect("off").is_none());
        let refused = |remote: &str, file: &str| {
            ConfigMirror::from_env(env(&[
                ("JC_CONFIG_MIRROR_REMOTE", remote),
                ("JC_CONFIG_MIRROR_TOKEN_FILE", file),
            ]))
            .expect_err(remote)
            .to_string()
        };
        assert!(refused("http://github.com/acme/jc-{repo}.git", &file).contains("https"));
        assert!(refused("https://github.com/acme/configuration.git", &file).contains("{repo}"));
        assert!(
            refused("https://x:tok@github.com/acme/jc-{repo}.git", &file).contains("credential")
        );
        assert!(refused(
            "https://github.com/acme/jc-{repo}.git",
            "/nonexistent/token"
        )
        .contains("cannot be read"));
        let half = ConfigMirror::from_env(env(&[(
            "JC_CONFIG_MIRROR_REMOTE",
            "https://github.com/a/{repo}",
        )]));
        assert!(half.expect_err("half").to_string().contains("together"));

        let mirror = ConfigMirror::from_env(env(&[
            (
                "JC_CONFIG_MIRROR_REMOTE",
                "https://github.com/acme/jc-{repo}.git",
            ),
            ("JC_CONFIG_MIRROR_TOKEN_FILE", &file),
        ]))
        .expect("valid")
        .expect("on");
        assert_eq!(
            mirror.remote_for("configuration"),
            "https://github.com/acme/jc-configuration.git"
        );
    }

    #[tokio::test]
    async fn the_token_is_never_in_a_url_log_or_error() {
        let github = MockServer::start().await;
        let shako = MockServer::start().await;
        // GitHub refuses with a body that echoes the token: the error names the status only.
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(401).set_body_string(TOKEN))
            .mount(&github)
            .await;
        let mirror = ConfigMirror::new(
            "https://github.com/acme/jc-{repo}.git",
            token_file(TOKEN).into(),
        )
        .expect("mirror")
        .with_github("github.com", &github.uri())
        .with_timing(Duration::ZERO, Duration::ZERO);
        let forge = ShakoClient::new(
            Url::parse(&shako.uri()).expect("url"),
            "joinedcontext",
            "configuration",
            "forge-token",
        )
        .expect("client");
        mirror.pass(&forge, &[]).await;
        let statuses = mirror.statuses();
        assert_eq!(statuses.len(), 1);
        assert!(statuses[0].last_error.contains("401"), "{statuses:?}");
        assert!(!format!("{statuses:?}{mirror:?}").contains(TOKEN));
        assert!(!mirror.remote_for("configuration").contains(TOKEN));
        let digest = serde_json::to_string(&mirror.digest(TimestampTz::now())).expect("json");
        assert!(
            digest.contains("configuration") && !digest.contains(TOKEN),
            "{digest}"
        );
        // The token went in the header only, and nothing reached the forge.
        let calls = github.received_requests().await.expect("recorded");
        assert!(calls.iter().all(|c| !c.url.as_str().contains(TOKEN)));
        assert!(shako
            .received_requests()
            .await
            .expect("recorded")
            .is_empty());
    }

    const REPO: &str = "/odata/Repositories(Owner='joinedcontext',Name='configuration')";
    const TARGET: &str = "https://github.com/acme/jc-configuration.git";

    fn mirror_row(name: &str, address: &str, branch: &str, error: &str) -> serde_json::Value {
        serde_json::json!({ "Name": name, "RemoteAddress": address, "Branch": branch, "LastError": error })
    }

    /// GitHub answering `private` for the remote, a forge listing `before` then `after` as its
    /// push mirrors, and every action answered 204 and counted.
    async fn stubs(
        before: serde_json::Value,
        after: serde_json::Value,
    ) -> (MockServer, MockServer, ConfigMirror, ShakoClient) {
        let (github, shako) = (MockServer::start().await, MockServer::start().await);
        Mock::given(method("GET"))
            .and(path("/repos/acme/jc-configuration"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"private": true})),
            )
            .mount(&github)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/PushMirrors")))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "value": before })),
            )
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&shako)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/PushMirrors")))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "value": after })),
            )
            .mount(&shako)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&shako)
            .await;
        let mirror = ConfigMirror::new(
            "https://github.com/acme/jc-{repo}.git",
            token_file(TOKEN).into(),
        )
        .expect("mirror")
        .with_github("github.com", &github.uri())
        .with_timing(Duration::ZERO, Duration::from_secs(60));
        let forge = ShakoClient::new(
            Url::parse(&shako.uri()).expect("url"),
            "joinedcontext",
            "configuration",
            "forge-token",
        )
        .expect("client");
        (github, shako, mirror, forge)
    }

    /// The paths of the forge's POSTs, in order, with the body of each.
    async fn actions(shako: &MockServer) -> Vec<(String, String)> {
        shako
            .received_requests()
            .await
            .expect("recorded")
            .into_iter()
            .filter(|r| r.method == wiremock::http::Method::POST)
            .map(|r| {
                let at = r
                    .url
                    .path()
                    .rsplit('/')
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                (at, String::from_utf8_lossy(&r.body).into_owned())
            })
            .collect()
    }

    #[tokio::test]
    async fn the_installation_keeps_only_the_scoped_mirror_and_a_failing_one_is_set_up_again() {
        let (github, shako, mirror, forge) = stubs(
            serde_json::json!([
                mirror_row("ours", TARGET, "main", "authentication failed"),
                mirror_row(
                    "copy",
                    "https://GitHub.com/acme/jc-configuration.git/",
                    "",
                    ""
                ),
                mirror_row("else", "https://elsewhere.example/x.git", "main", ""),
            ]),
            serde_json::json!([mirror_row("new", TARGET, "main", "")]),
        )
        .await;
        mirror.pass(&forge, &[]).await;
        assert_eq!(
            mirror.statuses()[0].last_error,
            "",
            "{:?}",
            mirror.statuses()
        );
        let done = actions(&shako).await;
        let names: Vec<&str> = done.iter().map(|(at, _)| at.as_str()).collect();
        assert_eq!(
            names,
            [
                "Shako.RemovePushMirror",
                "Shako.RemovePushMirror",
                "Shako.RemovePushMirror",
                "Shako.AddPushMirror",
                "Shako.SyncPushMirrors"
            ]
        );
        for (gone, (_, body)) in ["copy", "else", "ours"].iter().zip(&done) {
            assert!(body.contains(&format!("\"Name\":\"{gone}\"")), "{body}");
        }
        let add = &done[3].1;
        assert!(
            add.contains(TARGET)
                && add.contains("x-access-token")
                && add.contains("\"Branch\":\"main\""),
            "{add}"
        );
        // The privacy of the remote is asked once; the next pass reads it from what it kept. A
        // project repository the forge does not know fails alone.
        let project = ShakoClient::new(
            Url::parse(&shako.uri()).expect("url"),
            "joinedcontext",
            "nowhere",
            "t",
        )
        .expect("client");
        mirror.pass(&forge, &[project]).await;
        assert_eq!(
            github.received_requests().await.expect("recorded").len(),
            2,
            "one per repository"
        );
        let digest = mirror.digest(TimestampTz::now());
        assert_eq!((digest.counts.pass, digest.counts.fail), (1, 1));
        assert_eq!(digest.passed, ["configuration"]);
        assert_eq!(digest.failures[0].key, "nowhere");
    }

    #[tokio::test]
    async fn a_project_keeps_other_mirrors_a_healthy_one_is_left_and_a_failed_push_is_reported() {
        // A project repository keeps a mirror that is not the operator's.
        let (_github, shako, mirror, forge) = stubs(
            serde_json::json!([
                mirror_row("ours", TARGET, "main", ""),
                mirror_row("theirs", "https://elsewhere.example/x.git", "main", ""),
            ]),
            serde_json::json!([]),
        )
        .await;
        assert_eq!(mirror.keep(&forge, false).await, Ok(()));
        assert!(actions(&shako).await.is_empty());

        let (_github, _shako, mirror, forge) = stubs(
            serde_json::json!([]),
            serde_json::json!([mirror_row("new", TARGET, "main", "push rejected")]),
        )
        .await;
        assert_eq!(
            mirror.keep(&forge, false).await,
            Err("push rejected".to_owned())
        );

        let (_github, _shako, mirror, forge) =
            stubs(serde_json::json!([]), serde_json::json!([])).await;
        assert_eq!(
            mirror.keep(&forge, false).await,
            Err("the forge did not keep the push mirror".to_owned())
        );
    }

    /// Each forge call that fails stops the set-up with its error; the token file read last.
    #[tokio::test]
    async fn a_forge_or_token_failure_at_any_step_is_the_repositorys_error() {
        let failing = TARGET.replace("github.com", "git.example");
        let row = |error: &str| serde_json::json!([mirror_row("ours", &failing, "main", error)]);
        // (the PushMirrors answers, the failing action or none, what the error names)
        for (lists, fails, want) in [
            (vec![ResponseTemplate::new(500)], None, "500"),
            (
                vec![ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"value": row("x")}))],
                Some("Shako.RemovePushMirror"),
                "500",
            ),
            // An unscoped copy of our remote that the forge will not take off.
            (
                vec![ResponseTemplate::new(200).set_body_json(
                    serde_json::json!({"value": [mirror_row("copy", &failing, "", "")]}),
                )],
                Some("Shako.RemovePushMirror"),
                "500",
            ),
            (
                vec![ResponseTemplate::new(200).set_body_json(serde_json::json!({"value": []}))],
                Some("Shako.AddPushMirror"),
                "500",
            ),
            (
                vec![ResponseTemplate::new(200).set_body_json(serde_json::json!({"value": []}))],
                Some("Shako.SyncPushMirrors"),
                "500",
            ),
            (
                vec![
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({"value": []})),
                    ResponseTemplate::new(500),
                ],
                None,
                "500",
            ),
        ] {
            let shako = MockServer::start().await;
            let mut lists = lists.into_iter().peekable();
            while let Some(answer) = lists.next() {
                let mock = Mock::given(method("GET"))
                    .and(path(format!("{REPO}/PushMirrors")))
                    .respond_with(answer);
                let mock = if lists.peek().is_some() {
                    mock.up_to_n_times(1).with_priority(1)
                } else {
                    mock
                };
                mock.mount(&shako).await;
            }
            if let Some(action) = fails {
                Mock::given(method("POST"))
                    .and(path(format!("{REPO}/{action}")))
                    .respond_with(ResponseTemplate::new(500))
                    .with_priority(1)
                    .mount(&shako)
                    .await;
            }
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(204))
                .mount(&shako)
                .await;
            let mirror = ConfigMirror::new(
                "https://git.example/acme/jc-{repo}.git",
                token_file(TOKEN).into(),
            )
            .expect("mirror");
            let forge = ShakoClient::new(
                Url::parse(&shako.uri()).expect("url"),
                "joinedcontext",
                "configuration",
                "t",
            )
            .expect("client");
            let error = mirror.keep(&forge, false).await.expect_err("a failure");
            assert!(error.contains(want), "{fails:?}: {error}");
            assert!(!error.contains(TOKEN), "{error}");
        }
        // The token file gone since start-up: nothing is set up without one.
        let shako = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/PushMirrors")))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"value": []})),
            )
            .mount(&shako)
            .await;
        let forge = ShakoClient::new(
            Url::parse(&shako.uri()).expect("url"),
            "joinedcontext",
            "configuration",
            "t",
        )
        .expect("client");
        let gone = ConfigMirror::new(
            "https://git.example/acme/jc-{repo}.git",
            "/nonexistent/jc-token".into(),
        )
        .expect("mirror");
        let error = gone.keep(&forge, false).await.expect_err("no token");
        assert!(error.contains("cannot be read"), "{error}");
        assert!(shako
            .received_requests()
            .await
            .expect("recorded")
            .iter()
            .all(|r| r.method != wiremock::http::Method::POST));
        // GitHub that does not answer at all: a bound socket that never listens.
        let socket = tokio::net::TcpSocket::new_v4().expect("socket");
        socket
            .bind("127.0.0.1:0".parse().expect("addr"))
            .expect("bind");
        let silent = ConfigMirror::new(
            "https://github.com/acme/{repo}.git",
            token_file(TOKEN).into(),
        )
        .expect("mirror")
        .with_github(
            "github.com",
            &format!("http://{}", socket.local_addr().expect("addr")),
        );
        let error = silent
            .refuse_public("https://github.com/acme/x.git")
            .await
            .expect_err("no answer");
        assert!(error.starts_with("GitHub did not answer"), "{error}");
        assert!(!error.contains(TOKEN), "{error}");
        drop(socket);
    }

    #[tokio::test]
    async fn a_failure_backs_off_doubling_and_a_success_clears_it() {
        let mirror = ConfigMirror::new(
            "https://github.com/acme/jc-{repo}.git",
            token_file(TOKEN).into(),
        )
        .expect("mirror")
        .with_timing(Duration::from_secs(3600), Duration::ZERO);
        mirror.record("r", Err("first".to_owned()));
        mirror.record("r", Err("second".to_owned()));
        assert_eq!(mirror.lock()["r"].failures, 2);
        assert_eq!(mirror.statuses()[0].last_error, "second");
        mirror.record("r", Ok(()));
        let state = mirror.lock()["r"].clone();
        assert_eq!(
            (
                state.failures,
                state.next_try.is_none(),
                state.last_error.as_str()
            ),
            (0, true, "")
        );
        // Within its backoff a later failure keeps the one that started it.
        let waiting = ConfigMirror::new(
            "https://github.com/acme/jc-{repo}.git",
            token_file(TOKEN).into(),
        )
        .expect("mirror")
        .with_timing(Duration::from_secs(3600), Duration::from_secs(60));
        waiting.record("r", Err("first".to_owned()));
        waiting.record("r", Err("second".to_owned()));
        assert_eq!(waiting.statuses()[0].last_error, "first");
        assert_eq!(waiting.lock()["r"].failures, 1);
        // A run inside `every` of the last one does nothing.
        let shako = MockServer::start().await;
        let forge = ShakoClient::new(
            Url::parse(&shako.uri()).expect("url"),
            "joinedcontext",
            "configuration",
            "t",
        )
        .expect("client");
        *waiting.last_pass.lock().unwrap() = Some(Instant::now());
        waiting.pass(&forge, &[]).await;
        assert!(shako
            .received_requests()
            .await
            .expect("recorded")
            .is_empty());
    }

    #[tokio::test]
    async fn githubs_answers_decide_the_privacy_check_and_another_host_is_trusted() {
        let github = MockServer::start().await;
        for (repo, answer) in [
            ("/repos/acme/gone", ResponseTemplate::new(404)),
            ("/repos/acme/down", ResponseTemplate::new(503)),
            (
                "/repos/acme/garbled",
                ResponseTemplate::new(200).set_body_string("not json"),
            ),
        ] {
            Mock::given(method("GET"))
                .and(path(repo))
                .respond_with(answer)
                .mount(&github)
                .await;
        }
        let mirror = ConfigMirror::new(
            "https://github.com/acme/{repo}.git",
            token_file(TOKEN).into(),
        )
        .expect("mirror")
        .with_github("github.com", &github.uri());
        let refused = |target: &'static str| {
            let mirror = &mirror;
            async move { mirror.refuse_public(target).await.expect_err(target) }
        };
        assert!(refused("https://github.com/acme/gone.git")
            .await
            .contains("does not exist"));
        assert!(refused("https://github.com/acme/down.git")
            .await
            .contains("GitHub answered 503"));
        assert!(refused("https://github.com/acme/garbled.git")
            .await
            .contains("did not read"));
        assert!(refused("https://github.com/only-owner")
            .await
            .contains("names no owner/repository"));
        assert!(refused("not an address").await.contains("not an address"));
        assert_eq!(
            mirror.refuse_public("https://git.example/acme/x.git").await,
            Ok(())
        );
        // A token file emptied since start-up is refused before anything is asked.
        let empty = ConfigMirror::new(
            "https://github.com/acme/{repo}.git",
            token_file("  ").into(),
        )
        .expect("mirror")
        .with_github("github.com", &github.uri());
        assert!(empty
            .refuse_public("https://github.com/acme/x.git")
            .await
            .expect_err("empty")
            .contains("is empty"));
        assert!(
            ConfigMirror::new("not a url {repo}", token_file(TOKEN).into())
                .expect_err("no address")
                .to_string()
                .contains("is not an address")
        );
    }

    #[tokio::test]
    async fn a_public_remote_is_refused() {
        let github = MockServer::start().await;
        let shako = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/jc-configuration"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"private": false})),
            )
            .mount(&github)
            .await;
        let mirror = ConfigMirror::new(
            "https://github.com/acme/jc-{repo}.git",
            token_file(TOKEN).into(),
        )
        .expect("mirror")
        .with_github("github.com", &github.uri())
        .with_timing(Duration::ZERO, Duration::from_secs(60));
        let forge = ShakoClient::new(
            Url::parse(&shako.uri()).expect("url"),
            "joinedcontext",
            "configuration",
            "forge-token",
        )
        .expect("client");
        mirror.pass(&forge, &[]).await;
        assert!(mirror.statuses()[0].last_error.contains("is public"));
        assert!(shako
            .received_requests()
            .await
            .expect("recorded")
            .is_empty());
        // Within its backoff a refused repository is not asked again.
        mirror.pass(&forge, &[]).await;
        assert_eq!(github.received_requests().await.expect("recorded").len(), 1);
        let digest = mirror.digest(TimestampTz::now());
        assert_eq!((digest.counts.pass, digest.counts.fail), (0, 1));
    }
}
