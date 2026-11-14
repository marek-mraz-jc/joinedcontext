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

use chrono::{DateTime, Utc};
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
            let mut last = self.last_pass.lock().unwrap_or_else(|p| p.into_inner());
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
        self.repos.lock().unwrap_or_else(|p| p.into_inner())
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
    pub fn digest(&self, at: DateTime<Utc>) -> crate::api::validation::Digest {
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
        let digest = serde_json::to_string(&mirror.digest(Utc::now())).expect("json");
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
        let digest = mirror.digest(Utc::now());
        assert_eq!((digest.counts.pass, digest.counts.fail), (0, 1));
    }
}
