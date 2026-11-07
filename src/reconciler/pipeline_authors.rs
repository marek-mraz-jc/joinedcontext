//! A pipeline keeps only the reach of its people (PL-95, T-4917): at each render the reconciler
//! re-asks PL-94 ([`crate::pipeline_rights::missing`]) for the person who last proposed the
//! Pipeline and the one who approved it, and a reference either of them lost holds the pipeline
//! out of the runner with the reason `author lost access to {reference}`. `spec.enabled` stays
//! as the manifest says; a proposal by a person who holds the rights brings it back.
//!
//! Both people are read from the commit that brought the manifest to the default branch, as the
//! approval wrote it ("Approved in the Portal by …", "Proposed in the Portal by …"). A commit the
//! Portal did not write names nobody and is not judged here: whoever pushed it held the forge's
//! own write. Bindings and `Group` manifests come from the mirror; with the realm's admin client
//! the person's realm groups and roles count too, the bootstrap group included.
//!
//! The forge is asked once per manifest content (two calls), `LOOKUPS_AT_ONCE` Pipelines at a
//! time. A path the forge has no commit for is remembered with the ref it was asked at, so an
//! unchanged revision asks nothing again (T-5292: ~640 lookups, ~23 s, on every run before).

use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use futures_util::StreamExt;
use jc_core::kinds::Verb;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::auth::session::Identity;
use crate::git::{GitError, GiteaClient};
use crate::people::{Admin, People, PeopleError};
use crate::store::Mirror;

/// Who proposed and who approved one Pipeline, as its merge commit names them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Authors {
    pub proposer: Option<String>,
    pub approver: Option<String>,
}

impl Authors {
    pub fn of_message(message: &str) -> Self {
        Self {
            proposer: crate::api::changes::proposer_in(message),
            approver: crate::api::changes::approver_in(message),
        }
    }
}

/// Forge lookups in flight at once, so a first run over hundreds of Pipelines does not take
/// one round trip each in a row, nor open hundreds of requests to the forge.
const LOOKUPS_AT_ONCE: usize = 8;

/// What the forge said about one Pipeline's manifest.
#[derive(Debug, Clone)]
enum Seen {
    Read(Authors),
    /// No commit holds the path at this ref: asked again only when the ref or the manifest moves.
    Missing {
        git_ref: String,
    },
}

impl Seen {
    /// Whether this answer still holds at `git_ref` for the manifest it was asked for.
    fn holds_at(&self, git_ref: &str) -> bool {
        match self {
            Seen::Read(_) => true,
            Seen::Missing { git_ref: at } => at == git_ref,
        }
    }
}

/// One Pipeline of this render and where its manifest sits in the forge.
pub struct Placed<'a> {
    pub namespace: String,
    pub name: String,
    pub manifest: Value,
    pub forge: &'a GiteaClient,
    pub git_ref: &'a str,
    pub path: String,
}

pub struct AuthorRights {
    bootstrap_group: String,
    people: Option<Arc<People>>,
    /// What the forge said of each Pipeline, by the hash of the manifest it was asked for.
    known: Mutex<HashMap<(String, String), (u64, Seen)>>,
}

impl AuthorRights {
    pub fn new(bootstrap_group: String, people: Option<Arc<People>>) -> Self {
        Self {
            bootstrap_group,
            people,
            known: Mutex::new(HashMap::new()),
        }
    }

    /// The Pipelines of `pipelines` an author of which lost a reference, with the reason.
    pub async fn lost(
        &self,
        mirror: &Mirror,
        pipelines: Vec<Placed<'_>>,
    ) -> BTreeMap<(String, String), String> {
        let mut known = self.known.lock().await;
        let answered = read_all(&known, pipelines).await;
        let mut kept = HashMap::new();
        let mut realm = Realm::default();
        let mut lost = BTreeMap::new();
        for (key, hash, placed, answer) in answered {
            let Some(seen) = answer else { continue };
            kept.insert(key.clone(), (hash, seen.clone()));
            let Seen::Read(authors) = seen else { continue };
            if let Some(reason) = self.judge(&mut realm, mirror, &placed, &authors).await {
                tracing::warn!(project = %placed.namespace, pipeline = %placed.name, %reason, "the pipeline is paused: an author no longer reaches what it reads or writes (PL-95)");
                lost.insert(key, reason);
            }
        }
        *known = kept;
        lost
    }

    /// The reason `placed` is paused, or `None` when both authors reach it or the realm did
    /// not say who one of them is.
    async fn judge<'a>(
        &'a self,
        realm: &mut Realm<'a>,
        mirror: &Mirror,
        placed: &Placed<'_>,
        authors: &Authors,
    ) -> Option<String> {
        let mut people = Vec::new();
        for (verb, email) in [
            (Verb::Propose, &authors.proposer),
            (Verb::Approve, &authors.approver),
        ] {
            let Some(email) = email else { continue };
            match realm.person(self.people.as_deref(), mirror, email).await {
                Ok(identity) => people.push((verb, identity)),
                Err(error) => {
                    tracing::warn!(pipeline = %placed.name, error = ?error, "the realm did not say who an author of the pipeline is: their rights are checked next run");
                    return None;
                }
            }
        }
        reason(
            mirror,
            &self.bootstrap_group,
            &placed.namespace,
            &placed.manifest,
            &people,
        )
    }
}

type Answered<'a> = ((String, String), u64, Placed<'a>, Option<Seen>);

/// What the forge says of each of `pipelines`, `LOOKUPS_AT_ONCE` at a time, the answers of
/// `known` that still hold asking nothing.
async fn read_all<'a>(
    known: &HashMap<(String, String), (u64, Seen)>,
    pipelines: Vec<Placed<'a>>,
) -> Vec<Answered<'a>> {
    let asked: Vec<_> = pipelines
        .into_iter()
        .map(|placed| {
            let key = (placed.namespace.clone(), placed.name.clone());
            let hash = hash_of(&placed.manifest);
            let cached = known
                .get(&key)
                .filter(|(at, seen)| *at == hash && seen.holds_at(placed.git_ref))
                .map(|(_, seen)| seen.clone());
            (key, hash, placed, cached)
        })
        .collect();
    let started = std::time::Instant::now();
    let forge_asked = asked.iter().filter(|(.., cached)| cached.is_none()).count();
    // Collected first: a `map` closure inside the stream makes the sync future lose `Send`.
    let lookups: Vec<_> = asked
        .iter()
        .map(|(.., placed, cached)| seen_of(cached.clone(), placed))
        .collect();
    let answers: Vec<Option<Seen>> = futures_util::stream::iter(lookups)
        .buffered(LOOKUPS_AT_ONCE)
        .collect()
        .await;
    tracing::info!(
        pipelines = asked.len(),
        forge_asked,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "the authors of the pipelines were read (PL-95)"
    );
    asked
        .into_iter()
        .zip(answers)
        .map(|((key, hash, placed, _), answer)| (key, hash, placed, answer))
        .collect()
}

/// What the forge says of `placed`: the last run's answer when it still holds, else the
/// forge's; `None` when the forge failed, so the next run asks again.
async fn seen_of(cached: Option<Seen>, placed: &Placed<'_>) -> Option<Seen> {
    if cached.is_some() {
        return cached;
    }
    match authors_in_forge(placed).await {
        Ok(authors) => Some(Seen::Read(authors)),
        Err(GitError::NotFound) => {
            tracing::info!(project = %placed.namespace, pipeline = %placed.name, path = %placed.path, git_ref = %placed.git_ref, "the forge has no commit for the pipeline's manifest: its authors are not judged until the ref or the manifest changes");
            Some(Seen::Missing {
                git_ref: placed.git_ref.to_owned(),
            })
        }
        Err(error) => {
            tracing::warn!(project = %placed.namespace, pipeline = %placed.name, %error, "the authors of the pipeline were not read: their rights are checked next run");
            None
        }
    }
}

/// The status reason when one of `people` lacks a reference of `manifest` with their verb.
pub fn reason(
    mirror: &Mirror,
    bootstrap_group: &str,
    project: &str,
    manifest: &Value,
    people: &[(Verb, Identity)],
) -> Option<String> {
    let mut missing: Vec<String> = Vec::new();
    for (verb, identity) in people {
        for reference in crate::pipeline_rights::missing(
            mirror,
            bootstrap_group,
            identity,
            project,
            manifest,
            *verb,
        ) {
            if !missing.contains(&reference) {
                missing.push(reference);
            }
        }
    }
    (!missing.is_empty()).then(|| format!("author lost access to {}", missing.join(", ")))
}

async fn authors_in_forge(placed: &Placed<'_>) -> Result<Authors, GitError> {
    let last = placed
        .forge
        .list_commits(placed.git_ref, &placed.path, 1)
        .await?;
    let Some(commit) = last.first() else {
        return Ok(Authors::default());
    };
    Ok(Authors::of_message(
        &placed.forge.commit_message(&commit.sha).await?,
    ))
}

fn hash_of(manifest: &Value) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    manifest.to_string().hash(&mut hasher);
    hasher.finish()
}

/// The realm's answers of one run: one admin token, each person asked once.
#[derive(Default)]
struct Realm<'a> {
    admin: Option<Admin<'a>>,
    names: HashMap<String, Vec<String>>,
}

impl<'a> Realm<'a> {
    async fn person(
        &mut self,
        people: Option<&'a People>,
        mirror: &Mirror,
        email: &str,
    ) -> Result<Identity, PeopleError> {
        let mut identity = crate::permissions::named_person(mirror, email);
        let Some(people) = people else {
            return Ok(identity);
        };
        let key = email.to_ascii_lowercase();
        if !self.names.contains_key(&key) {
            if self.admin.is_none() {
                self.admin = Some(people.admin().await?);
            }
            let Some(admin) = self.admin.as_ref() else {
                return Ok(identity);
            };
            let users = admin.list(email, 0, 20).await?;
            let names = match users.iter().find(|user| {
                user.email
                    .as_deref()
                    .is_some_and(|e| e.eq_ignore_ascii_case(email))
            }) {
                Some(user) => admin.groups_and_roles(&user.id).await?,
                // Gone from the realm: the bindings alone say what they reach.
                None => Vec::new(),
            };
            self.names.insert(key.clone(), names);
        }
        if let Some(names) = self.names.get(&key) {
            identity.groups.extend(names.iter().cloned());
        }
        Ok(identity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
    use serde_json::json;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn manifest(kind: &str, name: &str, namespace: &str, spec: Value) -> ResourceEnvelope {
        ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: kind.to_owned(),
            metadata: ObjectMeta::new(name, namespace),
            spec,
            status: None,
        }
    }

    /// Helsinki with the internal Endpoints `ep-a` (space `a`) and `ep-b` (space `b`), a Role
    /// that proposes and approves Pipelines and reads entities, and `dev@hel.fi` bound to it in
    /// space `a` alone.
    fn world() -> Mirror {
        let mirror = Mirror::new();
        for (name, space) in [("ep-a", "a"), ("ep-b", "b")] {
            mirror.upsert(manifest(
                "Endpoint",
                name,
                "helsinki",
                json!({ "contextSpaceRef": { "kind": "ContextSpace", "name": space },
                        "slug": name, "audience": "internal" }),
            ));
        }
        for space in ["a", "b"] {
            mirror.upsert(manifest(
                "ContextSpace",
                space,
                "helsinki",
                json!({ "projectRef": "helsinki" }),
            ));
        }
        mirror.upsert(manifest(
            "Role",
            "pipeline-author",
            "org",
            json!({ "rules": [
                { "kinds": ["Pipeline"], "verbs": ["propose", "approve"] },
                { "kinds": ["Entity", "ContextSpace"], "verbs": ["read"] }
            ] }),
        ));
        bind(&mirror, "dev-a", "dev@hel.fi", "a");
        mirror
    }

    fn bind(mirror: &Mirror, name: &str, user: &str, space: &str) {
        mirror.upsert(manifest(
            "RoleBinding",
            name,
            "org",
            json!({ "role": "pipeline-author", "subjects": [{ "user": user }],
                    "scope": { "contextSpace": space } }),
        ));
    }

    fn unbind(mirror: &Mirror, name: &str) {
        mirror.remove(&crate::resource::ResourceKey {
            namespace: "org".into(),
            kind: "RoleBinding".into(),
            name: name.into(),
        });
    }

    /// `route` reads `ep-a` and writes `ep-a`.
    fn route() -> Value {
        json!({
            "apiVersion": "joinedcontext.com/v1alpha2",
            "kind": "Pipeline",
            "metadata": { "name": "route", "namespace": "helsinki" },
            "spec": {
                "class": "resident",
                "sources": [{ "endpointRef": { "kind": "Endpoint", "name": "ep-a" },
                              "query": { "type": "Vehicle" } }],
                "outputs": [{ "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:a:ep-a" }],
            }
        })
    }

    async fn forge(message: &str) -> (MockServer, GiteaClient) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/repos/o/r/commits"))
            .and(query_param("path", "pipelines/route/pipeline.yaml"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                { "sha": "abc", "commit": { "message": "first line" } }
            ])))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/repos/o/r/git/commits/abc"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "sha": "abc", "commit": { "message": message } })))
            .mount(&server)
            .await;
        let client = stub_client(&server);
        (server, client)
    }

    fn stub_client(server: &MockServer) -> GiteaClient {
        let base = url::Url::parse(&server.uri()).expect("the stub's address");
        GiteaClient::new(base, "o", "r", "t").expect("a client for the stub")
    }

    async fn judged(
        mirror: &Mirror,
        rights: &AuthorRights,
        client: &GiteaClient,
    ) -> Option<String> {
        judged_at(mirror, rights, client, "main").await
    }

    async fn judged_at(
        mirror: &Mirror,
        rights: &AuthorRights,
        client: &GiteaClient,
        git_ref: &str,
    ) -> Option<String> {
        let placed = Placed {
            namespace: "helsinki".into(),
            name: "route".into(),
            manifest: route(),
            forge: client,
            git_ref,
            path: "pipelines/route/pipeline.yaml".into(),
        };
        rights
            .lost(mirror, vec![placed])
            .await
            .remove(&("helsinki".to_owned(), "route".to_owned()))
    }

    const MERGED: &str = "Merge change proposal chg-1: create Pipeline route\n\nApproved in the \
                          Portal by eva@hel.fi\nProposed in the Portal by dev@hel.fi";

    /// The task's own test: the proposer's binding goes, the pipeline pauses with the reference
    /// named; the binding back, it runs again.
    #[tokio::test]
    async fn a_proposer_who_lost_the_source_pauses_the_pipeline() {
        let mirror = world();
        bind(&mirror, "eva-a", "eva@hel.fi", "a");
        let (_server, client) = forge(MERGED).await;
        let rights = AuthorRights::new("portal-approver".into(), None);
        assert_eq!(judged(&mirror, &rights, &client).await, None);

        unbind(&mirror, "dev-a");
        let reason = judged(&mirror, &rights, &client).await.expect("paused");
        assert_eq!(
            reason,
            "author lost access to source Endpoint ep-a, target Endpoint ep-a"
        );
        assert!(!reason.contains(" a "), "the space stays unnamed: {reason}");

        bind(&mirror, "dev-a", "dev@hel.fi", "a");
        assert_eq!(judged(&mirror, &rights, &client).await, None);
    }

    /// A proposal by a person who holds the rights resumes it: the new merge names them, and the
    /// changed manifest is read again rather than from the last run's authors.
    #[tokio::test]
    async fn a_new_proposal_by_a_person_with_the_rights_resumes_it() {
        let mirror = world();
        let (_old, old) = forge(MERGED).await;
        let rights = AuthorRights::new("portal-approver".into(), None);
        let reason = judged(&mirror, &rights, &old)
            .await
            .expect("eva reads nothing");
        assert!(reason.starts_with("author lost access to"), "{reason}");

        bind(&mirror, "ana-a", "ana@hel.fi", "a");
        let (_new, new) = forge(
            "Merge change proposal chg-2: x\n\nApproved in the Portal by ana@hel.fi\nProposed in \
             the Portal by dev@hel.fi",
        )
        .await;
        let placed = Placed {
            namespace: "helsinki".into(),
            name: "route".into(),
            manifest: {
                let mut changed = route();
                changed["metadata"]["annotations"] = json!({ "note": "again" });
                changed
            },
            forge: &new,
            git_ref: "main",
            path: "pipelines/route/pipeline.yaml".into(),
        };
        assert!(rights.lost(&mirror, vec![placed]).await.is_empty());
    }

    /// The approver is held to `approve` on the target space, as at the approval (PL-94).
    #[tokio::test]
    async fn an_approver_without_approve_on_the_target_pauses_it() {
        let mirror = world();
        bind(&mirror, "eva-b", "eva@hel.fi", "b");
        let (_server, client) = forge(MERGED).await;
        let rights = AuthorRights::new("portal-approver".into(), None);
        assert_eq!(
            judged(&mirror, &rights, &client).await.as_deref(),
            Some("author lost access to source Endpoint ep-a, target Endpoint ep-a")
        );
    }

    /// A commit the Portal did not write names nobody: the forge's own write decided it.
    #[tokio::test]
    async fn a_commit_without_the_portals_lines_is_not_judged() {
        let mirror = world();
        unbind(&mirror, "dev-a");
        let (_server, client) = forge("seed the pipelines").await;
        let rights = AuthorRights::new("portal-approver".into(), None);
        assert_eq!(judged(&mirror, &rights, &client).await, None);
    }

    /// A forge that does not answer judges nobody this run, rather than pausing every pipeline.
    #[tokio::test]
    async fn a_forge_that_does_not_answer_pauses_nothing() {
        let mirror = world();
        unbind(&mirror, "dev-a");
        let server = MockServer::start().await;
        let client = stub_client(&server);
        let rights = AuthorRights::new("portal-approver".into(), None);
        assert_eq!(judged(&mirror, &rights, &client).await, None);
    }

    async fn asked(server: &MockServer) -> usize {
        server
            .received_requests()
            .await
            .map_or(0, |requests| requests.len())
    }

    /// An unchanged revision asks the forge nothing: the last run's authors still judge it.
    #[tokio::test]
    async fn an_unchanged_revision_asks_the_forge_nothing() {
        let mirror = world();
        let (server, client) = forge(MERGED).await;
        let rights = AuthorRights::new("portal-approver".into(), None);
        let first = judged(&mirror, &rights, &client).await;
        assert_eq!(asked(&server).await, 2, "list_commits and commit_message");
        assert_eq!(judged(&mirror, &rights, &client).await, first);
        assert_eq!(asked(&server).await, 2, "the second run asked again");
    }

    /// A path the forge has no commit for (404) is remembered with its ref: the same ref asks
    /// nothing, a new ref asks once more.
    #[tokio::test]
    async fn a_path_without_a_commit_is_asked_once_per_ref() {
        let mirror = world();
        unbind(&mirror, "dev-a");
        let server = MockServer::start().await;
        let client = stub_client(&server);
        let rights = AuthorRights::new("portal-approver".into(), None);
        assert_eq!(judged_at(&mirror, &rights, &client, "r1").await, None);
        assert_eq!(judged_at(&mirror, &rights, &client, "r1").await, None);
        assert_eq!(asked(&server).await, 1);
        assert_eq!(judged_at(&mirror, &rights, &client, "r2").await, None);
        assert_eq!(asked(&server).await, 2);
    }

    /// A forge error other than 404 is not remembered: the next run asks again.
    #[tokio::test]
    async fn a_failing_forge_is_asked_again_next_run() {
        let mirror = world();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        let client = stub_client(&server);
        let rights = AuthorRights::new("portal-approver".into(), None);
        assert_eq!(judged(&mirror, &rights, &client).await, None);
        assert_eq!(judged(&mirror, &rights, &client).await, None);
        assert_eq!(asked(&server).await, 2);
    }

    #[test]
    fn the_bootstrap_group_reaches_everything() {
        let mirror = world();
        let mut admin = crate::permissions::named_person(&mirror, "root@hel.fi");
        admin.groups.push("portal-approver".into());
        assert_eq!(
            reason(
                &mirror,
                "portal-approver",
                "helsinki",
                &route(),
                &[(Verb::Approve, admin)]
            ),
            None
        );
    }
}
