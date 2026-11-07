//! An application's repository kept on GitHub as well (AP-79).
//!
//! The forge stays the repository of record: runs commit there and the merge request is opened
//! and merged there. GitHub holds a copy that the forge keeps current with a push mirror. The
//! Portal only creates the GitHub repository and hands the forge the mirror, so every commit,
//! branch and merge reaches GitHub without the Portal pushing anything itself.

use std::time::Duration;

use serde::Deserialize;
use url::Url;

use super::gitea::{GitError, GiteaClient};

const DEFAULT_API: &str = "https://api.github.com";
const DEFAULT_WEB: &str = "https://github.com";
/// GitHub reads a token as the password of any user name; this one names what it is.
const MIRROR_USER: &str = "x-access-token";

/// Where application repositories are copied to on GitHub, and the credential that creates and
/// pushes them.
#[derive(Clone)]
pub struct GithubMirror {
    api: Url,
    web: Url,
    owner: String,
    token: String,
    http: reqwest::Client,
}

impl std::fmt::Debug for GithubMirror {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GithubMirror")
            .field("api", &self.api.as_str())
            .field("owner", &self.owner)
            .field("token", &"[redacted]")
            .finish()
    }
}

/// What setting up the copy of one repository did, for the sentence a run shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mirrored {
    /// The mirror was already there and its last sync did not fail.
    Current,
    /// The mirror was added now and pushed once.
    Added,
    /// The mirror's last sync had failed, so it was set up again with today's credential and
    /// pushed once; the failure it had is kept for the run to say.
    Repaired(String),
}

#[derive(Deserialize)]
struct Login {
    login: String,
}

impl GithubMirror {
    /// Reads the copy's configuration from the environment.
    ///
    /// `JC_APP_MIRROR_GITHUB_OWNER` (the organization or user the copies are created under) and
    /// `JC_APP_MIRROR_GITHUB_TOKEN` (a secret: creates the repositories and is the push mirror's
    /// credential) are set together or not at all. `JC_APP_MIRROR_GITHUB_API` and
    /// `JC_APP_MIRROR_GITHUB_URL` name a GitHub Enterprise server; github.com when unset.
    ///
    /// Fail-closed: `Ok(None)` when neither of the two is set, an error when only one is.
    pub fn from_env(lookup: impl Fn(&str) -> Option<String>) -> Result<Option<Self>, GitError> {
        let owner = lookup("JC_APP_MIRROR_GITHUB_OWNER").filter(|v| !v.trim().is_empty());
        let token = lookup("JC_APP_MIRROR_GITHUB_TOKEN").filter(|v| !v.trim().is_empty());
        let (owner, token) = match (owner, token) {
            (None, None) => return Ok(None),
            (Some(owner), Some(token)) => (owner.trim().to_owned(), token.trim().to_owned()),
            _ => return Err(GitError::Config(
                "JC_APP_MIRROR_GITHUB_OWNER and JC_APP_MIRROR_GITHUB_TOKEN must be set together"
                    .to_owned(),
            )),
        };
        if !is_login(&owner) {
            return Err(GitError::Config(format!(
                "JC_APP_MIRROR_GITHUB_OWNER '{owner}' is not a GitHub account name: letters, digits \
                 and single hyphens, at most 39 characters"
            )));
        }
        let parse = |name: &str, default: &str| {
            let raw = lookup(name).unwrap_or_else(|| default.to_owned());
            let url =
                Url::parse(&raw).map_err(|e| GitError::Config(format!("invalid {name}: {e}")))?;
            if url.scheme() != "https" {
                return Err(GitError::Config(format!(
                    "{name} must be an https address: the token travels with every call"
                )));
            }
            Ok(url)
        };
        let api = parse("JC_APP_MIRROR_GITHUB_API", DEFAULT_API)?;
        let web = parse("JC_APP_MIRROR_GITHUB_URL", DEFAULT_WEB)?;
        Self::new(api, web, owner, token).map(Some)
    }

    fn new(api: Url, web: Url, owner: String, token: String) -> Result<Self, GitError> {
        let http = reqwest::ClientBuilder::new()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| GitError::Transport(e.to_string()))?;
        Ok(Self {
            api,
            web,
            owner,
            token,
            http,
        })
    }

    /// A mirror against a stub GitHub, which speaks plain HTTP (tests only).
    #[cfg(test)]
    pub(crate) fn at(api: &str, owner: &str, token: &str) -> Self {
        Self::new(
            Url::parse(api).expect("a stub url"),
            Url::parse(DEFAULT_WEB).expect("github"),
            owner.to_owned(),
            token.to_owned(),
        )
        .expect("a client")
    }

    /// The address a person opens the copy at, on one branch.
    pub fn web_url(&self, repo: &str, branch: &str) -> String {
        let base = format!(
            "{}/{}/{repo}",
            self.web.as_str().trim_end_matches('/'),
            self.owner
        );
        if branch.is_empty() {
            base
        } else {
            format!("{base}/tree/{branch}")
        }
    }

    /// The address the forge pushes to. It carries no credential.
    fn push_url(&self, repo: &str) -> String {
        format!(
            "{}/{}/{repo}.git",
            self.web.as_str().trim_end_matches('/'),
            self.owner
        )
    }

    /// Makes the forge repository `forge` have its copy on GitHub: the GitHub repository exists,
    /// and the forge pushes to it on every commit (AP-79).
    pub async fn mirror(
        &self,
        forge: &GiteaClient,
        description: &str,
    ) -> Result<Mirrored, GitError> {
        self.ensure_repository(&forge.repo, description).await?;
        let target = self.push_url(&forge.repo);
        let existing = forge.push_mirrors().await?.into_iter().find(|m| {
            m.remote_address
                .trim_end_matches('/')
                .eq_ignore_ascii_case(&target)
        });
        let outcome = match existing {
            Some(m) if m.last_error.trim().is_empty() => return Ok(Mirrored::Current),
            // A mirror whose sync fails most often holds a token that was rotated since: the forge
            // offers no way to change a mirror's credential, so it is set up again.
            Some(m) => {
                forge.delete_push_mirror(&m.remote_name).await?;
                Mirrored::Repaired(m.last_error)
            }
            None => Mirrored::Added,
        };
        forge
            .add_push_mirror(&target, MIRROR_USER, &self.token)
            .await?;
        forge.sync_push_mirrors().await?;
        Ok(outcome)
    }

    /// Creates the GitHub repository unless it is there, private and empty: the first mirror push
    /// fills it, so it starts as the forge repository's history and not beside it.
    async fn ensure_repository(&self, repo: &str, description: &str) -> Result<(), GitError> {
        let found = self
            .send(
                self.http
                    .get(self.url(&format!("repos/{}/{repo}", self.owner))?),
            )
            .await?;
        match check(found).await {
            Ok(_) => return Ok(()),
            Err(GitError::NotFound) => {}
            Err(err) => return Err(err),
        }
        let body = serde_json::json!({
            "name": repo,
            "description": description,
            "private": true,
            "auto_init": false,
            "has_issues": false,
            "has_projects": false,
            "has_wiki": false,
        });
        let in_org = self
            .send(
                self.http
                    .post(self.url(&format!("orgs/{}/repos", self.owner))?)
                    .json(&body),
            )
            .await?;
        match check(in_org).await {
            Ok(_) => return Ok(()),
            // Another run created it in the meantime: GitHub answers 422 "name already exists".
            Err(GitError::Api { status: 422, .. }) => return Ok(()),
            Err(GitError::NotFound) => {}
            Err(err) => return Err(err),
        }
        // Not an organization: the owner may be the token's own account. `user/repos` creates in
        // whatever account the token belongs to, so it is only called once that is the owner.
        let me = check(self.send(self.http.get(self.url("user")?)).await?).await?;
        let me: Login = me
            .json()
            .await
            .map_err(|e| GitError::Transport(format!("failed to parse the GitHub user: {e}")))?;
        if !me.login.eq_ignore_ascii_case(&self.owner) {
            return Err(GitError::Config(format!(
                "JC_APP_MIRROR_GITHUB_OWNER '{}' is neither an organization the token may create \
                 repositories in nor the token's own account ('{}')",
                self.owner, me.login
            )));
        }
        let mine = self
            .send(self.http.post(self.url("user/repos")?).json(&body))
            .await?;
        match check(mine).await {
            Ok(_) | Err(GitError::Api { status: 422, .. }) => Ok(()),
            Err(err) => Err(err),
        }
    }

    fn url(&self, path: &str) -> Result<Url, GitError> {
        let full = format!("{}/{path}", self.api.as_str().trim_end_matches('/'));
        Url::parse(&full).map_err(|e| GitError::Config(format!("invalid url '{full}': {e}")))
    }

    async fn send(&self, builder: reqwest::RequestBuilder) -> Result<reqwest::Response, GitError> {
        builder
            .bearer_auth(&self.token)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            // GitHub refuses a request without one.
            .header(reqwest::header::USER_AGENT, "joinedcontext-portal")
            .send()
            .await
            .map_err(|e| GitError::Transport(e.to_string()))
    }
}

/// GitHub's own rule for an account name: alphanumerics and single inner hyphens, 1 to 39.
fn is_login(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 39
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// The status as an error, with GitHub's own `message` and never the request.
async fn check(res: reqwest::Response) -> Result<reqwest::Response, GitError> {
    let status = res.status();
    if status.is_success() {
        return Ok(res);
    }
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(GitError::NotFound);
    }
    #[derive(Deserialize)]
    struct Message {
        #[serde(default)]
        message: String,
    }
    let message = res
        .json::<Message>()
        .await
        .map(|m| m.message)
        .unwrap_or_default();
    Err(GitError::Api {
        status: status.as_u16(),
        message: format!("GitHub: {message}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const TOKEN: &str = "ghp_test-token-never-printed";

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        }
    }

    fn mirror(github: &MockServer, owner: &str) -> GithubMirror {
        GithubMirror::new(
            Url::parse(&github.uri()).expect("url"),
            Url::parse("https://github.com").expect("url"),
            owner.to_owned(),
            TOKEN.to_owned(),
        )
        .expect("client")
    }

    fn forge(server: &MockServer) -> GiteaClient {
        GiteaClient::new(
            Url::parse(&server.uri()).expect("url"),
            "joinedcontext",
            "helsinki_bikes",
            "forge-token",
        )
        .expect("client")
    }

    #[test]
    fn it_is_off_without_both_values_and_refuses_half_of_them() {
        assert!(GithubMirror::from_env(env(&[])).expect("off").is_none());
        assert!(
            GithubMirror::from_env(env(&[("JC_APP_MIRROR_GITHUB_OWNER", "hel-apps")])).is_err()
        );
        assert!(GithubMirror::from_env(env(&[("JC_APP_MIRROR_GITHUB_TOKEN", TOKEN)])).is_err());
        // A blank value is an unset one, not an owner called "".
        assert!(GithubMirror::from_env(env(&[
            ("JC_APP_MIRROR_GITHUB_OWNER", " "),
            ("JC_APP_MIRROR_GITHUB_TOKEN", TOKEN)
        ]))
        .is_err());
        let on = GithubMirror::from_env(env(&[
            ("JC_APP_MIRROR_GITHUB_OWNER", "hel-apps"),
            ("JC_APP_MIRROR_GITHUB_TOKEN", TOKEN),
        ]))
        .expect("valid")
        .expect("on");
        assert_eq!(
            on.web_url("helsinki_bikes", "agent/app-bikes/r1"),
            "https://github.com/hel-apps/helsinki_bikes/tree/agent/app-bikes/r1"
        );
    }

    #[test]
    fn it_refuses_an_owner_that_is_no_account_name_and_an_address_without_tls() {
        for owner in ["hel apps", "-hel", "hel--apps", "hel/apps", &"a".repeat(40)] {
            assert!(
                GithubMirror::from_env(env(&[
                    ("JC_APP_MIRROR_GITHUB_OWNER", owner),
                    ("JC_APP_MIRROR_GITHUB_TOKEN", TOKEN)
                ]))
                .is_err(),
                "{owner}"
            );
        }
        assert!(GithubMirror::from_env(env(&[
            ("JC_APP_MIRROR_GITHUB_OWNER", "hel-apps"),
            ("JC_APP_MIRROR_GITHUB_TOKEN", TOKEN),
            ("JC_APP_MIRROR_GITHUB_API", "http://github.internal/api/v3"),
        ]))
        .is_err());
    }

    #[test]
    fn its_debug_output_never_carries_the_token() {
        let on = GithubMirror::from_env(env(&[
            ("JC_APP_MIRROR_GITHUB_OWNER", "hel-apps"),
            ("JC_APP_MIRROR_GITHUB_TOKEN", TOKEN),
        ]))
        .expect("valid")
        .expect("on");
        assert!(!format!("{on:?}").contains(TOKEN));
    }

    #[tokio::test]
    async fn a_new_repository_is_created_private_in_the_organization_and_mirrored_without_the_token_in_its_address(
    ) {
        let github = MockServer::start().await;
        let gitea = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/hel-apps/helsinki_bikes"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&github)
            .await;
        Mock::given(method("POST"))
            .and(path("/orgs/hel-apps/repos"))
            .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
            .and(body_partial_json(serde_json::json!({
                "name": "helsinki_bikes", "private": true, "auto_init": false
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({})))
            .expect(1)
            .mount(&github)
            .await;
        Mock::given(method("GET"))
            .and(path(
                "/api/v1/repos/joinedcontext/helsinki_bikes/push_mirrors",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&gitea)
            .await;
        Mock::given(method("POST"))
            .and(path(
                "/api/v1/repos/joinedcontext/helsinki_bikes/push_mirrors",
            ))
            .and(body_partial_json(serde_json::json!({
                "remote_address": "https://github.com/hel-apps/helsinki_bikes.git",
                "remote_password": TOKEN,
                "sync_on_commit": true,
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .expect(1)
            .mount(&gitea)
            .await;
        Mock::given(method("POST"))
            .and(path(
                "/api/v1/repos/joinedcontext/helsinki_bikes/push_mirrors-sync",
            ))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&gitea)
            .await;

        let outcome = mirror(&github, "hel-apps")
            .mirror(&forge(&gitea), "Application bikes")
            .await
            .expect("mirrored");
        assert_eq!(outcome, Mirrored::Added);
    }

    #[tokio::test]
    async fn a_mirror_already_in_place_is_left_alone() {
        let github = MockServer::start().await;
        let gitea = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/hel-apps/helsinki_bikes"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&github)
            .await;
        Mock::given(method("GET"))
            .and(path(
                "/api/v1/repos/joinedcontext/helsinki_bikes/push_mirrors",
            ))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!([{
                    "remote_name": "remote_mirror_x",
                    "remote_address": "https://github.com/hel-apps/helsinki_bikes.git",
                    "last_error": ""
                }])),
            )
            .mount(&gitea)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&gitea)
            .await;

        let outcome = mirror(&github, "hel-apps")
            .mirror(&forge(&gitea), "Application bikes")
            .await
            .expect("mirrored");
        assert_eq!(outcome, Mirrored::Current);
    }

    #[tokio::test]
    async fn a_mirror_whose_sync_fails_is_set_up_again_and_says_what_failed() {
        let github = MockServer::start().await;
        let gitea = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/hel-apps/helsinki_bikes"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&github)
            .await;
        Mock::given(method("GET"))
            .and(path(
                "/api/v1/repos/joinedcontext/helsinki_bikes/push_mirrors",
            ))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!([{
                    "remote_name": "remote_mirror_x",
                    "remote_address": "https://github.com/hel-apps/helsinki_bikes.git",
                    "last_error": "authentication failed"
                }])),
            )
            .mount(&gitea)
            .await;
        Mock::given(method("DELETE"))
            .and(path(
                "/api/v1/repos/joinedcontext/helsinki_bikes/push_mirrors/remote_mirror_x",
            ))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&gitea)
            .await;
        Mock::given(method("POST"))
            .and(path(
                "/api/v1/repos/joinedcontext/helsinki_bikes/push_mirrors",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .expect(1)
            .mount(&gitea)
            .await;
        Mock::given(method("POST"))
            .and(path(
                "/api/v1/repos/joinedcontext/helsinki_bikes/push_mirrors-sync",
            ))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&gitea)
            .await;

        let outcome = mirror(&github, "hel-apps")
            .mirror(&forge(&gitea), "Application bikes")
            .await
            .expect("mirrored");
        assert_eq!(
            outcome,
            Mirrored::Repaired("authentication failed".to_owned())
        );
    }

    #[tokio::test]
    async fn a_personal_owner_is_created_under_the_token_s_account_and_only_when_it_is_that_account(
    ) {
        let github = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/jana/helsinki_bikes"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&github)
            .await;
        Mock::given(method("POST"))
            .and(path("/orgs/jana/repos"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&github)
            .await;
        Mock::given(method("GET"))
            .and(path("/user"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"login": "Jana"})),
            )
            .mount(&github)
            .await;
        Mock::given(method("POST"))
            .and(path("/user/repos"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({})))
            .expect(1)
            .mount(&github)
            .await;
        mirror(&github, "jana")
            .ensure_repository("helsinki_bikes", "Application bikes")
            .await
            .expect("created under the token's own account");

        // The same token asked to create under somebody else's name creates nothing.
        let other = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/someone/helsinki_bikes"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&other)
            .await;
        Mock::given(method("POST"))
            .and(path("/orgs/someone/repos"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&other)
            .await;
        Mock::given(method("GET"))
            .and(path("/user"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"login": "jana"})),
            )
            .mount(&other)
            .await;
        Mock::given(method("POST"))
            .and(path("/user/repos"))
            .respond_with(ResponseTemplate::new(201))
            .expect(0)
            .mount(&other)
            .await;
        let refused = mirror(&other, "someone")
            .ensure_repository("helsinki_bikes", "Application bikes")
            .await
            .expect_err("never created in the wrong account");
        assert!(refused.to_string().contains("someone"), "{refused}");
    }

    #[tokio::test]
    async fn a_refusal_names_github_s_message_and_never_the_token() {
        let github = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/hel-apps/helsinki_bikes"))
            .respond_with(
                ResponseTemplate::new(401)
                    .set_body_json(serde_json::json!({"message": "Bad credentials"})),
            )
            .mount(&github)
            .await;
        let err = mirror(&github, "hel-apps")
            .ensure_repository("helsinki_bikes", "Application bikes")
            .await
            .expect_err("refused");
        let said = err.to_string();
        assert!(said.contains("Bad credentials"), "{said}");
        assert!(!said.contains(TOKEN), "{said}");
    }
}
