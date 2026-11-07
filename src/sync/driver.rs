//! The `SyncSource` loop: the tick, the merge request and what a reviewer's answer means
//! (MF-27…MF-32).
//!
//! [`jcctl::sync::poll`] is the whole judgement — whether a run is due, whether the source
//! moved, what the import gates make of it, which lane it lands in and what the proposal is.
//! This module is the three things that judgement deliberately does not have: a clock, a socket
//! (through [`super::remote`]) and a write path into the forge (through [`super::proposal`]).
//!
//! Two rules shape everything below.
//!
//! **A run never writes to the branch the platform applies.** Every run ends as a merge request
//! on a branch named after the revision it carries, so a re-run after a failed forge call
//! continues on the branch it started instead of opening a second review (CC-18).
//!
//! **A reviewer's answer is what unblocks the next run.** While a proposal is open the loop
//! proposes nothing else for that source, and the answer decides what the source is then
//! observed at: merged means the repository carries that revision, closed means a person
//! refused it and the loop moves on rather than re-proposing it every minute.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use jcctl::loader::{RawManifest, Repository};
use jcctl::sync::{self, SyncRemote};
use serde_json::Value;

use super::proposal::{self, Proposal};
use super::state::{States, Stored};
use crate::git::{GiteaClient, PullRequest};
use crate::reconciler::daemon::SyncError;
use crate::reconciler::staging::{stage, Scratch};

/// How often the loop looks at the repository.
///
/// The finest schedule a `SyncSource` may declare is one minute (`jc-core` refuses anything
/// shorter), so a minute is as often as this can usefully run.
const TICK: Duration = Duration::from_secs(60);

/// The kind this loop drives.
const KIND: &str = "SyncSource";

/// The hop to the sources, shared with the blocking task that does the fetching.
///
/// A trait object rather than a generic so a test can drive the whole loop over an origin it
/// controls: a real origin is `https` by the time [`super::remote`] accepts it, and no mock
/// HTTP server offers that.
pub type Remote = Arc<dyn SyncRemote + Send + Sync>;

/// `Arc<dyn SyncRemote>` as the sized value [`jcctl::sync::poll`] takes.
struct Shared(Remote);

impl SyncRemote for Shared {
    fn revision(&self, origin: &jc_core::kinds::SyncOrigin) -> Result<String, sync::RemoteError> {
        self.0.revision(origin)
    }

    fn checkout(
        &self,
        origin: &jc_core::kinds::SyncOrigin,
        revision: &str,
        into: &std::path::Path,
    ) -> Result<(), sync::RemoteError> {
        self.0.checkout(origin, revision, into)
    }
}

/// What one pass over the repository's sync sources did.
#[derive(Debug, Default, PartialEq)]
pub struct Run {
    /// The `kind: Change` envelope of every merge request this pass opened, each carrying the
    /// merge request's own URL (API/01 section 3).
    pub proposed: Vec<Value>,
    /// Sources that ran and changed nothing.
    pub unchanged: usize,
    /// What somebody has to be told: a source that did not answer, a resource the import gates
    /// refused, a proposal that vanished from the forge.
    pub flags: Vec<String>,
}

/// Why a pass could not run at all. One source failing is a flag, not one of these.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("the repository could not be read: {0}")]
    Repository(#[from] SyncError),
    #[error("the repository does not load: {0}")]
    Load(#[from] jcctl::loader::LoadError),
    #[error("forge: {0}")]
    Git(#[from] crate::git::GitError),
    #[error("the sync task did not finish: {0}")]
    Task(String),
    #[error("no sync source named {0} in project {1}")]
    NoSuchSource(String, String),
}

/// The loop: the repository's sync sources, their memory and the way out to their origins.
pub struct Driver {
    gitea: Arc<GiteaClient>,
    states: Arc<States>,
    remote: Remote,
    /// The sync sources of one revision of the repository, so a tick where nothing is due
    /// costs one request instead of a staged copy of the whole repository.
    cached: Mutex<Option<(String, Vec<RawManifest>)>>,
}

impl Driver {
    pub fn new(gitea: Arc<GiteaClient>, states: Arc<States>, remote: Remote) -> Self {
        Self {
            gitea,
            states,
            remote,
            cached: Mutex::new(None),
        }
    }

    /// Every source whose schedule says it is due (MF-28).
    pub async fn tick(&self, now: u64) -> Result<Run, RunError> {
        self.pass(now, None).await
    }

    /// One source, now, whatever its schedule says (MF-28, the **Sync now** button and the
    /// webhook route).
    pub async fn run_now(&self, namespace: &str, name: &str, now: u64) -> Result<Run, RunError> {
        self.pass(now, Some((namespace, name))).await
    }

    /// One pass: the sources that are due, or the one named.
    async fn pass(&self, now: u64, only: Option<(&str, &str)>) -> Result<Run, RunError> {
        let default_branch = self.gitea.default_branch().await?;
        let revision = self.gitea.branch_head(&default_branch).await?;
        let (sources, staged) = self.sources(&revision).await?;

        let mut wanted: Vec<(RawManifest, bool)> = Vec::new();
        for source in &sources {
            let Some(namespace) = source.metadata.namespace.as_deref() else {
                continue;
            };
            let name = source.metadata.name.as_str();
            if let Some((project, only)) = only {
                if project != namespace || only != name {
                    continue;
                }
                // Asked for, so the schedule is not consulted. What is still consulted is the
                // memory: a source with a proposal already open proposes nothing else (CC-18),
                // and a paused source stays paused until somebody resumes it.
                wanted.push((source.clone(), true));
                continue;
            }
            let stored = self.states.get(namespace, name).await;
            if sync::due(&schedule_of(source), &stored.state, now) {
                wanted.push((source.clone(), false));
            }
        }

        if let Some((project, name)) = only {
            if wanted.is_empty() {
                return Err(RunError::NoSuchSource(name.to_owned(), project.to_owned()));
            }
        }
        if wanted.is_empty() {
            return Ok(Run::default());
        }

        // The import gates read this repository, so a run needs it as files. Staged once for
        // the whole pass, and only now that something is going to run.
        let scratch = match staged {
            Some(scratch) => scratch,
            None => stage(&self.gitea, &revision).await?,
        };
        let pulls = self.gitea.list_pull_requests("all").await?;

        let mut run = Run::default();
        for (source, asked) in &wanted {
            let namespace = source.metadata.namespace.clone().unwrap_or_default();
            let name = source.metadata.name.clone();
            if let Err(err) = self
                .run_one(
                    source,
                    &namespace,
                    &name,
                    *asked,
                    &pulls,
                    scratch.path(),
                    now,
                    &mut run,
                )
                .await
            {
                // One source that cannot reach its forge or its origin is not a failed pass:
                // the others still run, and this one is where a reader is told why.
                run.flags.push(format!("{namespace}/{name}: {err}"));
            }
        }
        Ok(run)
    }

    /// One source: answer the open proposal, decide, and write what the decision asks for.
    #[allow(clippy::too_many_arguments)]
    async fn run_one(
        &self,
        source: &RawManifest,
        namespace: &str,
        name: &str,
        asked: bool,
        pulls: &[PullRequest],
        repo_dir: &std::path::Path,
        now: u64,
        run: &mut Run,
    ) -> Result<(), RunError> {
        let mut stored = self.states.get(namespace, name).await;
        if let Some(flag) = answer(&mut stored, pulls) {
            run.flags.push(format!("{namespace}/{name}: {flag}"));
            self.states.put(namespace, name, &stored).await;
        }

        let workspace = Scratch::new(&format!("sync-{namespace}-{name}"))?;
        let decision = match self
            .decide(
                source.clone(),
                stored.state.clone(),
                now,
                asked,
                repo_dir,
                workspace.path(),
            )
            .await
        {
            Ok(decision) => decision,
            Err(err) => {
                // An origin that did not answer leaves no run to point at, only a reason. It is
                // recorded so the project page can say `Error` and say why (MF-30).
                stored.last_error = Some(err.to_string());
                self.states.put(namespace, name, &stored).await;
                return Err(err);
            }
        };

        stored.state = decision.state.clone();
        stored.last_error = None;
        let Some(plan) = decision.proposal else {
            run.unchanged += 1;
            self.states.put(namespace, name, &stored).await;
            return Ok(());
        };

        if !plan.rejected.is_empty() {
            // MF-24 is a gate for a sync run too. The revision is deliberately not recorded as
            // observed, so the next run tries the same source again once somebody has fixed
            // what the gates refused.
            for rejection in &plan.rejected {
                run.flags.push(format!("{namespace}/{name}: {rejection}"));
            }
            stored.last_error = Some(plan.rejected.join("; "));
            self.states.put(namespace, name, &stored).await;
            return Ok(());
        }

        let mut files = BTreeMap::new();
        for (path, manifest) in &plan.files {
            files.insert(repo_path(path), yaml(manifest)?);
        }
        let removed: Vec<String> = plan
            .removed
            .iter()
            .map(PathBuf::as_path)
            .map(repo_path)
            .collect();
        let title = format!(
            "sync {namespace}/{name} at {}",
            plan.revision.chars().take(7).collect::<String>()
        );
        let body = serde_json::to_string_pretty(&plan.envelope).unwrap_or_default();

        let pull = match proposal::open(
            &self.gitea,
            &Proposal {
                branch: &plan.name,
                title: &title,
                body: &body,
                files,
                removed,
                auto_merge: plan.auto_merge,
                line: None,
            },
        )
        .await
        {
            Ok(pull) => pull,
            Err(err) => {
                // Nothing is open, so nothing may be recorded as open: a source left pointing
                // at a proposal that was never created would never run again.
                stored.state.open_proposal = None;
                stored.last_error = Some(err.to_string());
                self.states.put(namespace, name, &stored).await;
                return Err(err.into());
            }
        };

        stored.open_revision = Some(plan.revision.clone());
        stored.merge_request = Some(pull.url.clone());
        if plan.auto_merge {
            // Merged by the same call that opened it, so the answer is already in: the source
            // is at that revision and the next run compares against it (MF-29, CC-70).
            stored.state.observed_revision = Some(plan.revision.clone());
            stored.state.open_proposal = None;
            stored.open_revision = None;
        }
        self.states.put(namespace, name, &stored).await;

        let mut envelope = plan.envelope.clone();
        envelope["status"]["mergeRequest"] = Value::String(pull.url.clone());
        tracing::info!(source = %format!("{namespace}/{name}"), url = %pull.url, "sync source proposed a change");
        run.proposed.push(envelope);
        Ok(())
    }

    /// The judgement, on a blocking thread.
    ///
    /// [`jcctl::sync::poll`] and the transport under it are synchronous, which is the right
    /// shape for a decision; what they must not do is sit on a runtime worker while a source
    /// takes its time.
    ///
    /// `asked` picks the entry point: `poll_now` skips the wait and nothing else, which is what
    /// the webhook route and **Sync now** need — a webhook schedule is never due on a timer, so
    /// `poll` would answer both of them with "the next run is not due yet".
    async fn decide(
        &self,
        source: RawManifest,
        state: sync::State,
        now: u64,
        asked: bool,
        repo_dir: &std::path::Path,
        workspace: &std::path::Path,
    ) -> Result<sync::Run, RunError> {
        let remote = Shared(Arc::clone(&self.remote));
        let repo_dir = repo_dir.to_path_buf();
        let workspace = workspace.to_path_buf();
        tokio::task::spawn_blocking(move || {
            if asked {
                sync::poll_now(&source, &state, now, &repo_dir, &workspace, &remote)
            } else {
                sync::poll(&source, &state, now, &repo_dir, &workspace, &remote)
            }
        })
        .await
        .map_err(|err| RunError::Task(err.to_string()))?
        .map_err(|err| RunError::Task(err.to_string()))
    }

    /// The repository's sync sources at one revision.
    ///
    /// Cached by revision, because a tick where nothing is due must not cost a staged copy of
    /// the whole repository. The staged copy is handed back when this run made one, so a pass
    /// that does have work stages once rather than twice.
    async fn sources(
        &self,
        revision: &str,
    ) -> Result<(Vec<RawManifest>, Option<Scratch>), RunError> {
        if let Some(hit) = self.cached.lock().ok().and_then(|cache| {
            cache
                .as_ref()
                .filter(|(at, _)| at.as_str() == revision)
                .cloned()
        }) {
            return Ok((hit.1, None));
        }

        let scratch = stage(&self.gitea, revision).await?;
        let repository = Repository::load(scratch.path())?;
        let sources: Vec<RawManifest> = repository
            .iter()
            .filter(|(id, _)| id.kind == KIND)
            .map(|(_, resource)| resource.manifest.clone())
            .collect();
        if let Ok(mut cache) = self.cached.lock() {
            *cache = Some((revision.to_owned(), sources.clone()));
        }
        Ok((sources, Some(scratch)))
    }

    /// What the project page shows for one source (MF-30).
    pub async fn status(&self, namespace: &str, name: &str) -> Stored {
        self.states.get(namespace, name).await
    }

    /// Switches one source's loop off or on (MF-30, the **Pause** button).
    pub async fn pause(&self, namespace: &str, name: &str, paused: bool) {
        self.states.set_paused(namespace, name, paused).await;
    }

    /// Whether a restart keeps what the loop remembers.
    pub fn is_durable(&self) -> bool {
        self.states.is_durable()
    }
}

/// The loop on its own timer, on the replica that reconciles (CC-03).
///
/// Two replicas syncing would open the same merge request twice, so the leader check is the
/// first thing each tick does — the same flag the reconcile loop maintains.
pub fn spawn_periodic(
    driver: Arc<Driver>,
    syncer: Arc<crate::reconciler::Syncer>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(TICK);
        loop {
            ticker.tick().await;
            if !syncer.is_leader() {
                continue;
            }
            let now = crate::auth::session::now_unix().max(0) as u64;
            match driver.tick(now).await {
                Ok(run) => {
                    for flag in &run.flags {
                        tracing::warn!(flag = %flag, "sync source");
                    }
                }
                Err(err) => tracing::warn!(error = %err, "the sync loop did not run"),
            }
        }
    })
}

/// What a reviewer's answer to the open proposal means, and the flag a reader needs (MF-30).
///
/// Returns `None` when there is nothing to answer or the proposal is still open.
fn answer(stored: &mut Stored, pulls: &[PullRequest]) -> Option<String> {
    let branch = stored.state.open_proposal.clone()?;
    let closed = |stored: &mut Stored| {
        stored.state.open_proposal = None;
        stored.open_revision = None;
        stored.merge_request = None;
    };

    let Some(pull) = pulls.iter().find(|pull| pull.head_branch == branch) else {
        // The branch is gone from the forge and so is the review. Nothing is recorded as
        // observed: the next run proposes the revision again, which is the only way a source
        // whose proposal somebody deleted ever syncs.
        closed(stored);
        return Some(format!(
            "the proposal on `{branch}` is no longer on the forge"
        ));
    };

    if pull.state == "open" {
        stored.merge_request = Some(pull.url.clone());
        return None;
    }

    // Merged or closed, the answer is in and the source moves on. A closed proposal counts as
    // an answer about that revision: re-proposing it every minute would put a queue of
    // identical merge requests in front of the person who just refused one.
    let revision = stored.open_revision.clone();
    let merged = pull.merged;
    closed(stored);
    stored.state.observed_revision = revision;
    (!merged).then(|| format!("the proposal on `{branch}` was closed without merging"))
}

/// The schedule a source declares, or one that is never due when it declares none.
fn schedule_of(source: &RawManifest) -> jc_core::kinds::Schedule {
    source
        .spec
        .get("schedule")
        .and_then(|schedule| serde_json::from_value(schedule.clone()).ok())
        .unwrap_or_default()
}

/// A repository path as the forge takes it.
fn repo_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// One manifest as the document that goes on the branch.
fn yaml(manifest: &RawManifest) -> Result<String, RunError> {
    serde_norway::to_string(manifest)
        .map_err(|err| RunError::Task(format!("a synced manifest did not serialise: {err}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pull(branch: &str, state: &str, merged: bool) -> PullRequest {
        PullRequest {
            number: 7,
            url: format!("https://git.example/org/config/pulls/7#{branch}"),
            state: state.to_owned(),
            title: "sync".to_owned(),
            body: String::new(),
            head_branch: branch.to_owned(),
            head_sha: "sync-head-sha".to_owned(),
            base_branch: "main".to_owned(),
            base_sha: String::new(),
            created_at: String::new(),
            author_name: "joinedcontext reconciler".to_owned(),
            author_email: None,
            mergeable: Some(true),
            merged,
            repository: "config".to_owned(),
            ..Default::default()
        }
    }

    fn waiting() -> Stored {
        Stored {
            state: sync::State {
                observed_revision: Some("older".into()),
                last_run_at: Some(1_000),
                open_proposal: Some("chg-sync-regional-c0ffee".into()),
                paused: false,
            },
            open_revision: Some("c0ffee1234567890".into()),
            merge_request: Some("https://git.example/org/config/pulls/7".into()),
            last_error: None,
        }
    }

    #[test]
    fn a_merged_proposal_is_what_makes_the_revision_observed() {
        let mut stored = waiting();
        let flag = answer(
            &mut stored,
            &[pull("chg-sync-regional-c0ffee", "closed", true)],
        );
        assert_eq!(flag, None);
        assert_eq!(
            stored.state.observed_revision.as_deref(),
            Some("c0ffee1234567890"),
            "the full revision, not the seven characters the branch name carries"
        );
        assert_eq!(stored.state.open_proposal, None);
    }

    #[test]
    fn a_proposal_a_person_closed_is_an_answer_and_not_a_reason_to_ask_again() {
        let mut stored = waiting();
        let flag = answer(
            &mut stored,
            &[pull("chg-sync-regional-c0ffee", "closed", false)],
        );
        assert!(
            flag.is_some_and(|why| why.contains("without merging")),
            "a reader is told"
        );
        assert_eq!(
            stored.state.observed_revision.as_deref(),
            Some("c0ffee1234567890"),
            "a refused revision is not proposed again every minute"
        );
        assert_eq!(stored.state.open_proposal, None);
    }

    #[test]
    fn an_open_proposal_leaves_the_source_where_it_is() {
        let mut stored = waiting();
        assert_eq!(
            answer(
                &mut stored,
                &[pull("chg-sync-regional-c0ffee", "open", false)]
            ),
            None
        );
        assert_eq!(stored.state, waiting().state);
    }

    #[test]
    fn a_proposal_that_vanished_from_the_forge_is_proposed_again() {
        let mut stored = waiting();
        let flag = answer(&mut stored, &[]);
        assert!(flag.is_some_and(|why| why.contains("no longer on the forge")));
        assert_eq!(
            stored.state.observed_revision.as_deref(),
            Some("older"),
            "nothing was reviewed, so nothing is observed"
        );
        assert_eq!(stored.state.open_proposal, None);
    }

    #[test]
    fn a_source_with_nothing_open_has_nothing_to_answer() {
        let mut stored = Stored::default();
        assert_eq!(answer(&mut stored, &[]), None);
        assert_eq!(stored, Stored::default());
    }

    #[test]
    fn a_webhook_source_is_never_due_on_a_tick() {
        let source: RawManifest = serde_json::from_value(serde_json::json!({
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": "SyncSource",
            "metadata": { "name": "regional", "namespace": "bb" },
            "spec": {
                "source": { "bundle": { "url": "https://cdn.example/models.zip" } },
                "schedule": { "webhook": true },
                "mode": "mirror",
                "conflictPolicy": "replace"
            }
        }))
        .expect("a manifest");

        // Which is why the tick never picks one up, and why an asked-for run goes through
        // `poll_now` rather than through a rewritten schedule (T-0475).
        assert!(!sync::due(
            &schedule_of(&source),
            &sync::State::default(),
            10_000_000
        ));
    }

    #[test]
    fn a_paused_source_is_not_due_however_long_it_has_waited() {
        let source: RawManifest = serde_json::from_value(serde_json::json!({
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": "SyncSource",
            "metadata": { "name": "regional", "namespace": "bb" },
            "spec": {
                "source": { "bundle": { "url": "https://cdn.example/models.zip" } },
                "schedule": { "interval": "30m" },
                "mode": "mirror",
                "conflictPolicy": "replace"
            }
        }))
        .expect("a manifest");
        let paused = sync::State {
            paused: true,
            ..sync::State::default()
        };
        assert!(!sync::due(&schedule_of(&source), &paused, 10_000_000));
        assert!(sync::due(
            &schedule_of(&source),
            &sync::State::default(),
            10_000_000
        ));
    }
}
