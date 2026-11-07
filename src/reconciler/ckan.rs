//! The open-data catalogue, brought to what the repository declares (EP-62…EP-67, T-2405).
//!
//! The publisher itself is `jcctl::publish::ckan`: it builds the dataset from the Endpoint's own
//! DCAT-AP record, the resources from the representations it enables, and the charts from its
//! models; CKAN holds no rows (T-5060). Until now nothing drove it inside the platform, so a dataset appeared when
//! somebody ran `jcctl publish ckan` from a shell and a withdrawn publication stayed in the
//! catalogue until somebody noticed. EP-62 asks the reconciler to do it, which is this wave.
//!
//! Three rules, each from what goes wrong without it:
//!
//! - **The token is resolved per run and never held.** It comes from the instance's `apiTokenRef`
//!   the way the publisher's own command resolves it — the variable the reference names, else the
//!   repository's encrypted secrets under this Portal's age identity — lives in the HTTP client for
//!   the length of one publication, and is in no outcome, no status and no log line (EP-67, CC-06).
//! - **Only what the platform published is ever withdrawn.** A dataset somebody created in
//!   CKAN by hand, or a harvest wrote, is not this reconciler's to delete, so a withdrawal needs
//!   the Endpoint that declared it to have been there a run ago, exactly as the subscription wave
//!   decides its deletions (CC-19), or the dataset to carry jcctl's own `generated_by` mark in an
//!   organization the current Endpoints publish into (T-4461).
//! - **One Endpoint's failure is not the run's.** A catalogue that is down, a record the gateway
//!   will not serve, a reference that does not resolve: each is one outcome with its reason, and
//!   the next Endpoint is still published.
//!
//! - **An Endpoint is published when it is due, never every tick (T-4717).** Republishing every
//!   dataset each sync came to about 1,000 requests a minute through the public edge, the edge
//!   answered 429, and each refusal was retried on the next tick. So an unchanged Endpoint is
//!   published again only after [`REFRESH`], a failed one backs off, and the calls of a run to
//!   the catalogue keep [`CALL_GAP`] apart.
//! - **The catalogue is asked about an organization once a pass, on its Service (T-5314).** Every
//!   publication asked `organization_show` through the public edge: 184 calls in two minutes for
//!   one organization on prod, against the public's own limit. A pass remembers each organization
//!   it saw ([`Organizations`]), and the deployment's own catalogue is called at the in-cluster
//!   address the operator maps its public URL to ([`CkanSync::reaching`]).
//!
//! Everything CKAN is spoken to through `jcctl`'s blocking client, so each publication runs on a
//! blocking thread: dropping a blocking `reqwest` client inside the reconciler's runtime panics.

use crate::store::Mirror;
use jc_core::kinds::ckan::CkanInstanceSpec;
use jc_core::kinds::EndpointSpec;
use jcctl::commands::publish_ckan::{self as publisher, Target};
use jcctl::loader::{RawManifest, RawMetadata, Repository, ResourceId};
use jcctl::publish::ckan::{CkanApi, CkanError, Measurements, Outcome, Settings};
use jcctl::publish::ckan_http::HttpCkan;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What one run did with one Endpoint's publication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Publication {
    /// The dataset is what the Endpoint describes.
    Published {
        /// The dataset name in CKAN.
        dataset: String,
        /// What happened to the dataset itself.
        outcome: Outcome,
        /// The record lists a model without its LinkML artifact, so the dataset links the
        /// schema index for it (EP-68).
        no_linkml: bool,
    },
    /// The Endpoint stopped declaring a publication, or is gone, and the dataset went with it.
    Withdrawn {
        /// The dataset that was removed from the catalogue.
        dataset: String,
    },
    /// Nothing was written, and why. Never carries a credential (EP-67).
    Failed(String),
}

/// One line of a run: which Endpoint of which project, and what happened to its dataset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// The project the Endpoint belongs to.
    pub project: String,
    /// The Endpoint's name.
    pub endpoint: String,
    /// What happened.
    pub publication: Publication,
}

impl Report {
    fn failed(project: &str, endpoint: &str, reason: impl Into<String>) -> Self {
        Self {
            project: project.to_owned(),
            endpoint: endpoint.to_owned(),
            publication: Publication::Failed(reason.into()),
        }
    }
}

/// The catalogue wave of one Portal: where the endpoints answer, and what to call an organization
/// it has to create.
#[derive(Debug, Clone)]
pub struct CkanSync {
    settings: Settings,
    /// The age identity that decrypts the repository's secrets, which is how the instance's
    /// `apiTokenRef` is resolved (EP-67). `None` leaves the token to the variable the reference
    /// names, which is how a Job carries it.
    age_key_file: Option<PathBuf>,
    /// Where the measurements of a dataset's data come from: the daily run's freshness and the
    /// gateway's checks (DM-95). `None` publishes without measurements.
    quality: Option<(
        Arc<crate::quality::Store>,
        Arc<crate::quality_checks::Readings>,
    )>,
    /// What the log last said of each `(project, endpoint)`, so an outcome is logged when it
    /// changes and not once a sync (T-4065).
    said: Arc<Mutex<HashMap<(String, String), String>>>,
    /// When each `(project, endpoint)` is next published (T-4717).
    pacing: Arc<Mutex<Pacing>>,
    /// When this Portal may next call the catalogue or read the gateway, shared by every
    /// publication (T-4717).
    last_call: Arc<Mutex<Option<Instant>>>,
    /// The deployment's own catalogue: its public URL, without a trailing slash, and the
    /// in-cluster address its API is called at instead (T-5314). `None` calls every instance
    /// at its `spec.url`.
    api_route: Option<(String, String)>,
}

impl CkanSync {
    /// A wave publishing endpoints that answer on `host`.
    pub fn new(host: impl Into<String>) -> Self {
        Self {
            settings: Settings::new(host),
            age_key_file: None,
            quality: None,
            said: Arc::default(),
            pacing: Arc::default(),
            last_call: Arc::default(),
            api_route: None,
        }
    }

    /// Calls the API of the instance whose `spec.url` is `public` at `api` instead, the
    /// catalogue's Service in this cluster (T-5314). Links in a dataset keep the public URL; only
    /// the calls move. The operator sets both, never a manifest: an instance URL a project
    /// writes cannot send this Portal's calls to an address inside the cluster.
    pub fn reaching(mut self, public: &str, api: &str) -> Self {
        self.api_route = Some((
            public.trim_end_matches('/').to_owned(),
            api.trim_end_matches('/').to_owned(),
        ));
        self
    }

    /// Where the API of `instance` is called: its in-cluster address when it is this
    /// deployment's own catalogue, else its `spec.url`.
    pub fn api_url(&self, instance: &CkanInstanceSpec) -> String {
        match &self.api_route {
            Some((public, api)) if instance.base_url() == public => api.clone(),
            _ => instance.base_url().to_owned(),
        }
    }

    /// Publishes each dataset with the pass rate and the freshness of its space (DM-95).
    pub fn with_quality(
        mut self,
        daily: Arc<crate::quality::Store>,
        checks: Arc<crate::quality_checks::Readings>,
    ) -> Self {
        self.quality = Some((daily, checks));
        self
    }

    /// What was measured of the space an Endpoint serves: the daily run by the space's name, the
    /// gateway's counters by its tenant segment, which is how the gateway labels them.
    pub fn measured(&self, repo: &Repository, target: &Target) -> Measurements {
        let Some((daily, checks)) = &self.quality else {
            return Measurements::default();
        };
        let project = target.id.namespace.as_deref().unwrap_or_default();
        let Some(space) = target
            .manifest
            .spec
            .get("contextSpaceRef")
            .and_then(crate::resource::reference_name)
        else {
            return Measurements::default();
        };
        measurements(
            daily.get(project, space).as_ref(),
            &checks.report(&repo.space_segment(project, space), &[]),
        )
    }

    /// The age identity of this Portal's SOPS backend, so an `apiTokenRef` that lives in the
    /// repository's encrypted secrets resolves (EP-67).
    pub fn with_age_key_file(mut self, path: Option<PathBuf>) -> Self {
        self.age_key_file = path;
        self
    }

    /// The title an organization this run creates is given, from the installation's branding.
    pub fn titled(mut self, title: impl Into<String>) -> Self {
        self.settings = self.settings.clone().titled(title);
        self
    }

    /// Where the installation's documentation lives, so a usage text links its page (EP-108).
    pub fn documented_at(mut self, base: impl Into<String>) -> Self {
        self.settings = self.settings.clone().documented_at(base);
        self
    }

    /// Brings every project's catalogue to what the repository declares.
    ///
    /// `repository` is the staged tree of this sync, which is both what `repo` was loaded from and
    /// where a SOPS `secretRef` reads. `previous` is the mirror as it stood before this sync, which
    /// is what says whose dataset may be withdrawn.
    ///
    /// Returns one report per Endpoint this run touched. An Endpoint that declares no publication
    /// and declared none before is not a line: a run over a repository that publishes nothing
    /// reports nothing.
    pub async fn converge(
        &self,
        repo: &Repository,
        previous: &Mirror,
        repository: &Path,
    ) -> Vec<Report> {
        let mut reports = Vec::new();
        let mut wanted: Vec<Target> = Vec::new();
        let known = Organizations::default();
        let mut unread = false;
        for project in projects(repo) {
            match publisher::targets(repo, &project) {
                Ok(targets) => wanted.extend(targets),
                // A project whose publication does not read is one project's problem: the
                // Endpoint names an instance that is not there, or a spec that does not parse.
                Err(err) => {
                    unread = true;
                    reports.push(Report::failed(&project, "", err.to_string()));
                }
            }
        }

        // One catalogue holds one dataset of a name: two Endpoints claiming it would overwrite each
        // other on every pass, across projects too (T-6079). Neither is written.
        let refused = claimed_twice(&wanted);
        let claims: BTreeSet<(String, String)> = wanted.iter().map(claim).collect();

        let published: BTreeSet<(String, String)> = wanted
            .iter()
            .map(|target| {
                (
                    target.id.namespace.clone().unwrap_or_default(),
                    target.id.name.clone(),
                )
            })
            .collect();

        // Per catalogue and organization, the datasets the current Endpoints publish there: what
        // the sweep keeps (T-4461).
        let mut sweeps: BTreeMap<(String, String), (Target, BTreeSet<String>)> = BTreeMap::new();
        for target in &wanted {
            let Some(organization) = target
                .publication
                .organization
                .clone()
                .or_else(|| target.instance.organization_default.clone())
            else {
                continue;
            };
            let key = (target.instance.base_url().to_owned(), organization);
            sweeps
                .entry(key)
                .or_insert_with(|| (target.clone(), BTreeSet::new()))
                .1
                .insert(target.dataset_name().to_owned());
        }

        for target in wanted {
            let key = (
                target.id.namespace.clone().unwrap_or_default(),
                target.id.name.clone(),
            );
            if let Some(reason) = refused.get(&key) {
                reports.push(Report::failed(&key.0, &key.1, reason.clone()));
                continue;
            }
            let print = fingerprint(&target, &self.settings);
            let due = self.pacing.lock().unwrap_or_else(|p| p.into_inner()).due(
                &key,
                print,
                Instant::now(),
            );
            if !due {
                continue;
            }
            let measured = self.measured(repo, &target);
            let report = self.one(&target, false, repository, measured, &known).await;
            self.pacing
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .record(key, print, &report.publication, Instant::now());
            reports.push(report);
        }
        // A dataset a current Endpoint still publishes (a renamed Endpoint keeping its dataset
        // name, or another project's) is not the gone Endpoint's to delete.
        for target in withdrawn(previous, &published)
            .into_iter()
            .filter(|target| !claims.contains(&claim(target)))
        {
            let measured = Measurements::default();
            reports.push(self.one(&target, true, repository, measured, &known).await);
        }
        // A project whose publications did not read keeps its datasets out of `kept`, and a
        // shared organization would lose them: no sweep in such a pass.
        if unread {
            sweeps.clear();
        }
        for ((_, organization), (target, kept)) in sweeps {
            reports.extend(
                self.sweep(&target, organization, kept, repository, &known)
                    .await,
            );
        }
        self.log(&reports);
        reports
    }

    /// One log line per Endpoint whose outcome changed since the last run, and one per write:
    /// the catalogue's state was told only to the activity feed, so a dataset that failed every
    /// sync for hours said so nowhere an operator reads (T-4065). A reason is the publisher's
    /// own sentence, which never carries the token (EP-67).
    /// Returns the lines it logged.
    pub fn log(&self, reports: &[Report]) -> Vec<String> {
        let mut logged = Vec::new();
        let Ok(mut said) = self.said.lock() else {
            return logged;
        };
        for report in reports {
            let (line, wrote) = match &report.publication {
                Publication::Published { outcome, .. } => (
                    outcome_word(*outcome).to_owned(),
                    matches!(outcome, Outcome::Created | Outcome::Updated),
                ),
                Publication::Withdrawn { .. } => ("withdrawn".to_owned(), true),
                Publication::Failed(reason) => (format!("failed: {reason}"), false),
            };
            let key = (report.project.clone(), report.endpoint.clone());
            if !wrote && said.get(&key) == Some(&line) {
                continue;
            }
            let dataset = match &report.publication {
                Publication::Published { dataset, .. } | Publication::Withdrawn { dataset } => {
                    dataset.as_str()
                }
                Publication::Failed(_) => "",
            };
            if matches!(report.publication, Publication::Failed(_)) {
                tracing::warn!(project = %report.project, endpoint = %report.endpoint, outcome = %line, "catalogue");
            } else {
                tracing::info!(project = %report.project, endpoint = %report.endpoint, %dataset, outcome = %line, "catalogue");
            }
            logged.push(format!("{}/{}: {line}", report.project, report.endpoint));
            said.insert(key, line);
        }
        logged
    }

    /// One Endpoint: resolve the token, then publish or withdraw on a blocking thread.
    async fn one(
        &self,
        target: &Target,
        withdraw: bool,
        repository: &Path,
        measured: Measurements,
        known: &Organizations,
    ) -> Report {
        let project = target.id.namespace.clone().unwrap_or_default();
        let endpoint = target.id.name.clone();
        let settings = self.settings.clone();
        let target = target.clone();
        let repository = repository.to_path_buf();
        let age_key_file = self.age_key_file.clone();
        let last_call = self.last_call.clone();
        let api_url = self.api_url(&target.instance);
        let known = known.clone();
        // Resolving the token, opening the socket and reading the gateway are all blocking, and
        // dropping a blocking client inside this runtime panics, so the whole publication of one
        // Endpoint happens on a blocking thread.
        let work = tokio::task::spawn_blocking(move || {
            let source = publisher::TokenSource {
                env: None,
                age_key_file: age_key_file.as_deref(),
            };
            let token = publisher::token(&target.instance, &repository, source)
                .map_err(|err| err.to_string())?;
            let api = HttpCkan::new(&api_url, token).map_err(|err| err.to_string())?;
            let mut api = Paced::new(api, last_call, CALL_GAP).remembering(&api_url, known);
            if withdraw {
                return withdraw_one(&mut api, &target);
            }
            publish_one(&mut api, &target, &settings, &measured)
        })
        .await;

        match work {
            Ok(Ok(publication)) => Report {
                project,
                endpoint,
                publication,
            },
            Ok(Err(reason)) => Report::failed(&project, &endpoint, reason),
            Err(err) => Report::failed(
                &project,
                &endpoint,
                format!("the publication did not finish: {err}"),
            ),
        }
    }

    /// Withdraws the datasets this platform published into `organization` that no current
    /// Endpoint publishes (T-4461): the withdrawal from the previous mirror misses an Endpoint
    /// removed in the same apply that restarted the Portal, because no mirror it holds ever had
    /// it. `target` is any current publication there, for the catalogue and its token.
    async fn sweep(
        &self,
        target: &Target,
        organization: String,
        kept: BTreeSet<String>,
        repository: &Path,
        known: &Organizations,
    ) -> Vec<Report> {
        let project = target.id.namespace.clone().unwrap_or_default();
        let target = target.clone();
        let repository = repository.to_path_buf();
        let age_key_file = self.age_key_file.clone();
        let last_call = self.last_call.clone();
        let api_url = self.api_url(&target.instance);
        let known = known.clone();
        let work = tokio::task::spawn_blocking(move || {
            let source = publisher::TokenSource {
                env: None,
                age_key_file: age_key_file.as_deref(),
            };
            let token = publisher::token(&target.instance, &repository, source)
                .map_err(|err| err.to_string())?;
            let api = HttpCkan::new(&api_url, token).map_err(|err| err.to_string())?;
            let mut api = Paced::new(api, last_call, CALL_GAP).remembering(&api_url, known);
            sweep_with(&mut api, &organization, &kept)
        })
        .await;
        match work {
            Ok(Ok(reports)) => reports
                .into_iter()
                .map(|(dataset, publication)| Report {
                    project: project.clone(),
                    endpoint: dataset,
                    publication,
                })
                .collect(),
            Ok(Err(reason)) => vec![Report::failed(&project, "", reason)],
            Err(err) => vec![Report::failed(
                &project,
                "",
                format!("the catalogue sweep did not finish: {err}"),
            )],
        }
    }
}

/// How long an Endpoint whose publication went through, and has not changed, is left alone:
/// its measurements and its charts are at most this old in the catalogue (T-4717).
pub const REFRESH: Duration = Duration::from_secs(3600);
/// The longest a failing Endpoint waits before it is tried again.
pub const MOST_BACKOFF: Duration = Duration::from_secs(3600);
/// The least time between two calls to the catalogue: at most five a second (T-4717).
pub const CALL_GAP: Duration = Duration::from_millis(200);

/// What a target looks like to the publisher, so a changed Endpoint, instance or setting is due
/// at once. In-process only: the hash is never stored, so a differing `Debug` between builds
/// costs one republication, nothing more.
pub fn fingerprint(target: &Target, settings: &Settings) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    format!("{target:?}|{settings:?}").hash(&mut hasher);
    hasher.finish()
}

/// When each Endpoint's publication is next due (T-4717). A publication that went through is due
/// again after [`REFRESH`]; a failed one after 1, 2, 4 … minutes, starting at 5 when the
/// catalogue or the gateway said 429, up to [`MOST_BACKOFF`]; a changed Endpoint at once.
#[derive(Debug, Default)]
pub struct Pacing {
    seen: HashMap<(String, String), Pace>,
}

#[derive(Debug, Clone, Copy)]
struct Pace {
    fingerprint: u64,
    failures: u32,
    next: Instant,
}

impl Pacing {
    /// Whether the Endpoint `key`, looking like `fingerprint`, is published in a run at `now`.
    pub fn due(&self, key: &(String, String), fingerprint: u64, now: Instant) -> bool {
        self.seen
            .get(key)
            .is_none_or(|pace| pace.fingerprint != fingerprint || now >= pace.next)
    }

    /// What the run at `now` did with `key`, which sets when it is next due.
    pub fn record(
        &mut self,
        key: (String, String),
        fingerprint: u64,
        publication: &Publication,
        now: Instant,
    ) {
        let before = self
            .seen
            .get(&key)
            .filter(|pace| pace.fingerprint == fingerprint)
            .map_or(0, |pace| pace.failures);
        let (failures, wait) = match publication {
            Publication::Failed(reason) => {
                let first = if refused_for_load(reason) { 5 * 60 } else { 60 };
                let wait = Duration::from_secs(first << before.min(6)).min(MOST_BACKOFF);
                (before + 1, wait)
            }
            Publication::Published { .. } | Publication::Withdrawn { .. } => (0, REFRESH),
        };
        self.seen.insert(
            key,
            Pace {
                fingerprint,
                failures,
                next: now + wait,
            },
        );
    }
}

/// Whether a failure's reason is a 429: the catalogue's and the gateway's errors carry the status
/// as a number of its own.
fn refused_for_load(reason: &str) -> bool {
    reason
        .split(|c: char| !c.is_ascii_digit())
        .any(|word| word == "429")
}

/// A catalogue client that keeps its calls `gap` apart, the gap shared by every client holding
/// the same `last`, the earliest the next call may go (T-4717). Blocking: it sleeps on the
/// publication's own blocking thread.
pub struct Paced<A> {
    api: A,
    last: Arc<Mutex<Option<Instant>>>,
    gap: Duration,
    /// The catalogue this client calls and the organizations its pass has seen there (T-5314).
    known: Option<(String, Organizations)>,
}

/// The organizations one reconcile pass has seen, by catalogue and slug, as CKAN last answered
/// them (T-5314). A pass over every Endpoint of one organization asks `organization_show` once;
/// the next pass starts empty, so a title changed in CKAN by hand is seen again within one sync.
pub type Organizations = Arc<Mutex<BTreeMap<(String, String), serde_json::Value>>>;

/// The `*_show` action a [`Paced`] client answers from [`Organizations`].
const ORGANIZATION_SHOW: &str = "organization_show";

impl<A> Paced<A> {
    /// `api` paced against every other client holding `last`.
    pub fn new(api: A, last: Arc<Mutex<Option<Instant>>>, gap: Duration) -> Self {
        Self {
            api,
            last,
            gap,
            known: None,
        }
    }

    /// Answers `organization_show` from `known` once `catalogue` has answered it in this pass,
    /// and keeps what an `organization_create` or `organization_patch` answers (T-5314).
    pub fn remembering(mut self, catalogue: &str, known: Organizations) -> Self {
        self.known = Some((catalogue.to_owned(), known));
        self
    }

    fn held(&self, name: &str) -> Option<serde_json::Value> {
        let (catalogue, known) = self.known.as_ref()?;
        let known = known.lock().unwrap_or_else(|p| p.into_inner());
        known.get(&(catalogue.clone(), name.to_owned())).cloned()
    }

    /// Keeps `organization` as the pass's answer for `name`, or forgets `name` when the answer
    /// is not an organization, so the next publication asks CKAN again.
    fn remember(&self, name: &str, organization: Option<&serde_json::Value>) {
        let Some((catalogue, known)) = &self.known else {
            return;
        };
        let key = (catalogue.clone(), name.to_owned());
        let mut known = known.lock().unwrap_or_else(|p| p.into_inner());
        match organization.filter(|org| org.get("name").is_some()) {
            Some(organization) => known.insert(key, organization.clone()),
            None => known.remove(&key),
        };
    }

    /// Runs `read`, which makes `calls` requests of its own to the gateway, inside the same pace:
    /// it waits its turn and holds `calls` gaps, so the next call comes no sooner than if each
    /// request had waited. The reads of a publication (the record and its `/ws`, then `file.csv`
    /// and two schemas) went out unpaced and peaked at 25 a second on prod (T-4717).
    pub fn reads<T>(&self, calls: u32, read: impl FnOnce() -> T) -> T {
        if calls > 0 {
            self.wait_for(calls);
        }
        read()
    }

    fn wait(&self) {
        self.wait_for(1);
    }

    /// Waits until the last holder's gap has passed, then holds `calls` gaps from now.
    fn wait_for(&self, calls: u32) {
        let mut last = self.last.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(ready) = *last {
            let now = Instant::now();
            if ready > now {
                std::thread::sleep(ready - now);
            }
        }
        *last = Some(Instant::now() + self.gap * calls);
    }
}

impl<A: CkanApi> CkanApi for Paced<A> {
    fn show(&self, action: &str, name: &str) -> Result<Option<serde_json::Value>, CkanError> {
        if action == ORGANIZATION_SHOW {
            if let Some(held) = self.held(name) {
                return Ok(Some(held));
            }
        }
        self.wait();
        let shown = self.api.show(action, name)?;
        if action == ORGANIZATION_SHOW {
            self.remember(name, shown.as_ref());
        }
        Ok(shown)
    }

    fn action(
        &mut self,
        action: &str,
        payload: &serde_json::Value,
    ) -> Result<serde_json::Value, CkanError> {
        self.wait();
        let answered = self.api.action(action, payload)?;
        // The slug is the name the publisher created the organization under or the id it
        // patched it by; CKAN answers both with the organization as it now stands.
        if matches!(action, "organization_create" | "organization_patch") {
            if let Some(name) = payload
                .get("name")
                .or_else(|| payload.get("id"))
                .and_then(serde_json::Value::as_str)
            {
                self.remember(name, Some(&answered));
            }
        }
        Ok(answered)
    }
}

/// The page `package_search` is read in.
const SEARCH_ROWS: usize = 500;

/// Finds and withdraws, through an open catalogue connection, every dataset in `organization`
/// that carries the platform's own `generated_by` mark and is not in `kept`. A dataset without
/// the mark (a harvest's, one made by hand) is never touched. One outcome per dataset, by name.
pub fn sweep_with(
    api: &mut impl CkanApi,
    organization: &str,
    kept: &BTreeSet<String>,
) -> Result<Vec<(String, Publication)>, String> {
    let mut stray = Vec::new();
    let mut start = 0;
    loop {
        let page = api
            .action(
                "package_search",
                &serde_json::json!({
                    "fq": format!("organization:\"{organization}\""),
                    "include_private": true,
                    "rows": SEARCH_ROWS,
                    "start": start,
                }),
            )
            .map_err(|err| err.to_string())?;
        let results = page["results"].as_array().cloned().unwrap_or_default();
        for dataset in &results {
            let ours = dataset["extras"].as_array().is_some_and(|extras| {
                extras.iter().any(|extra| {
                    extra["key"] == "generated_by"
                        && extra["value"] == jcctl::publish::ckan::GENERATOR
                })
            });
            // The search is by text: only a dataset the catalogue itself files under this
            // organization is this sweep's to withdraw.
            let here = dataset["organization"]["name"].as_str() == Some(organization);
            match dataset["name"].as_str() {
                Some(name) if ours && here && !kept.contains(name) => stray.push(name.to_owned()),
                _ => {}
            }
        }
        start += results.len();
        let total = page["count"].as_u64().unwrap_or(0) as usize;
        if results.is_empty() || start >= total {
            break;
        }
    }
    stray
        .into_iter()
        .map(|name| {
            jcctl::publish::ckan::withdraw(api, &name)
                .map(|_| {
                    let publication = Publication::Withdrawn {
                        dataset: name.clone(),
                    };
                    (name, publication)
                })
                .map_err(|err| err.to_string())
        })
        .collect()
}

/// The gateway requests of `publisher::record`: the record, `/ws` and `/ngsi-ld/v1/types` for
/// its example type.
pub const RECORD_READS: u32 = 3;
/// The gateway requests of `publisher::models`: `model.schema.json`, `model.linkml.yaml`.
pub const MODELS_READS: u32 = 2;

/// Publishes one Endpoint through an open catalogue connection.
///
/// The record and the models its charts are drawn from are read from the gateway as an ordinary
/// consumer, which is what keeps a dataset to what the Endpoint's policy set already allows
/// (EP-66). Both reads keep the catalogue's pace, since they pass the same edge (T-4717).
/// Blocking: both reads and every catalogue call open a socket.
pub fn publish_one<A: CkanApi>(
    api: &mut Paced<A>,
    target: &Target,
    settings: &Settings,
    measured: &Measurements,
) -> Result<Publication, String> {
    let record = api
        .reads(RECORD_READS, || publisher::record(target, settings))
        .map_err(|err| err.to_string())?;
    let record = jcctl::publish::ckan::measured(&record, measured);
    match api.reads(MODELS_READS, || publisher::models(target, settings)) {
        Ok(models) => publish_with(api, target, &record, Some(&models), settings),
        Err(err) => publish_without_models(api, target, &record, &err.to_string(), settings),
    }
}

/// The dataset brought to its record when the models did not read, and a failure that says so:
/// a failed read must not skip the dataset, or helsinki-transport keeps the resources of 05:28
/// while its Endpoint gained `/ws` (T-4065). The charts come back with the next good read.
pub fn publish_without_models(
    api: &mut impl CkanApi,
    target: &Target,
    record: &serde_json::Value,
    unread: &str,
    settings: &Settings,
) -> Result<Publication, String> {
    let outcome = match publish_with(api, target, record, None, settings)? {
        Publication::Published { outcome, .. } => outcome_word(outcome),
        _ => "not published",
    };
    Err(format!(
        "the dataset is {outcome}, its charts were not refreshed: {unread}"
    ))
}

/// The word a log line and a failure say for what happened to a dataset.
fn outcome_word(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Created => "created",
        Outcome::Updated => "updated",
        Outcome::Unchanged => "unchanged",
        Outcome::NotPublished => "not published",
        Outcome::Withdrawn => "withdrawn",
    }
}

/// The catalogue half of a publication: the dataset and its views, from a record already read.
///
/// Split from [`publish_one`] so what a run writes into the catalogue can be asserted without a
/// gateway to read from and without a catalogue to write to.
pub fn publish_with(
    api: &mut impl CkanApi,
    target: &Target,
    record: &serde_json::Value,
    models: Option<&publisher::Models>,
    settings: &Settings,
) -> Result<Publication, String> {
    let line = publisher::publish_one(api, target, record, models, settings)
        .map_err(|err| err.to_string())?;
    Ok(Publication::Published {
        dataset: line.dataset,
        outcome: line.outcome,
        no_linkml: line.no_linkml,
    })
}

/// The pass rate over seven days of every rule of the space, to a hundredth, and the age of
/// its newest data when the daily run read it (DM-95). Both are steady between two readings, so
/// an unchanged dataset is not written again every sync (CC-18).
pub fn measurements(
    daily: Option<&crate::quality::SpaceQuality>,
    checks: &crate::quality_checks::SpaceChecks,
) -> Measurements {
    let (pass, all) = checks.rules.iter().fold((0, 0), |(pass, all), rule| {
        let week = &rule.week;
        (
            pass + week.pass,
            all + week.pass + week.reject + week.flag + week.quarantine,
        )
    });
    let freshness_seconds = daily.and_then(|report| {
        let newest = report.types.iter().filter_map(|kind| kind.newest).max()?;
        Some((report.observed_at - newest).num_seconds().max(0))
    });
    Measurements {
        pass_rate: (all > 0).then(|| (pass as f64 / all as f64 * 100.0).round() / 100.0),
        freshness_seconds,
    }
}

/// Withdraws one Endpoint's dataset (EP-62, CC-19).
pub fn withdraw_one(api: &mut impl CkanApi, target: &Target) -> Result<Publication, String> {
    let line = publisher::withdraw_one(api, target).map_err(|err| err.to_string())?;
    Ok(Publication::Withdrawn {
        dataset: line.dataset,
    })
}

/// The projects of a repository, as the loader files them.
fn projects(repo: &Repository) -> BTreeSet<String> {
    repo.iter()
        .filter_map(|(id, _)| id.namespace.clone())
        .filter(|namespace| namespace != crate::api::blueprints::ORG_NAMESPACE)
        .collect()
}

/// The catalogue and dataset name a publication writes: the instance's URL, so two
/// `CkanInstance`s of two projects pointing at one CKAN are one catalogue, and the dataset name.
fn claim(target: &Target) -> (String, String) {
    (
        target.instance.base_url().to_ascii_lowercase(),
        target.dataset_name().to_owned(),
    )
}

/// Every Endpoint that shares its catalogue and dataset name with another, keyed by (project,
/// name), with the reason it is not published: the others it collides with and the fix. CKAN
/// dataset names are one namespace per catalogue, so publishing either would overwrite the
/// other's dataset (T-6079); none of them is written until one takes a name of its own.
pub fn claimed_twice(wanted: &[Target]) -> HashMap<(String, String), String> {
    let mut claimants: BTreeMap<(String, String), Vec<(String, String)>> = BTreeMap::new();
    for target in wanted {
        claimants.entry(claim(target)).or_default().push((
            target.id.namespace.clone().unwrap_or_default(),
            target.id.name.clone(),
        ));
    }
    let mut refused = HashMap::new();
    for ((catalogue, dataset), endpoints) in claimants {
        if endpoints.len() < 2 {
            continue;
        }
        for endpoint in &endpoints {
            let others: Vec<String> = endpoints
                .iter()
                .filter(|other| *other != endpoint)
                .map(|(project, name)| format!("{project}/{name}"))
                .collect();
            let reason = format!(
                "not published: the CKAN dataset `{dataset}` on {catalogue} is also what Endpoint \
                 {} publishes, and one catalogue holds one dataset of a name, so neither is \
                 written; set spec.publish.ckan.name to a name of its own, such as `{}-{dataset}`",
                others.join(", "),
                endpoint.0,
            );
            refused.insert(endpoint.clone(), reason);
        }
    }
    refused
}

/// The publications the previous run had and this one does not: an Endpoint whose `publish.ckan`
/// block was withdrawn, and an Endpoint that is gone altogether (CC-19).
///
/// `published` is what this run publishes, as `(project, endpoint)`.
///
/// Read from the previous mirror rather than from CKAN, so a dataset this reconciler never
/// published is never deleted by it. A withdrawal that happens while the Portal is down is not
/// seen here; [`sweep_with`] finds it in the catalogue by the platform's own `generated_by` mark.
pub fn withdrawn(previous: &Mirror, published: &BTreeSet<(String, String)>) -> Vec<Target> {
    let mut gone = Vec::new();
    for envelope in previous.matching(|env| env.kind == "Endpoint") {
        let project = envelope.metadata.namespace.clone().unwrap_or_default();
        let name = envelope.metadata.name.clone();
        if published.contains(&(project.clone(), name.clone())) {
            continue;
        }
        let Ok(spec) = serde_json::from_value::<EndpointSpec>(envelope.spec.clone()) else {
            continue;
        };
        let Some(publication) = spec.publish.as_ref().and_then(|p| p.ckan.clone()) else {
            continue;
        };
        let instance_name = publication.instance_ref.name().to_owned();
        let Some(instance) = previous
            .get(&project, "CkanInstance", &instance_name)
            .and_then(|env| serde_json::from_value::<CkanInstanceSpec>(env.spec).ok())
        else {
            // The catalogue manifest went with the Endpoint. Nothing can be reached without the
            // instance's URL and token reference, and inventing either would be worse than
            // leaving the dataset to the sweep.
            tracing::warn!(
                project = %project,
                endpoint = %name,
                instance = %instance_name,
                "the endpoint and its catalogue were both removed; the dataset stays until the \
                 catalogue is declared again"
            );
            continue;
        };
        gone.push(Target {
            id: ResourceId {
                group: "joinedcontext.com".to_owned(),
                kind: "Endpoint".to_owned(),
                namespace: Some(project),
                name: name.clone(),
            },
            manifest: RawManifest {
                api_version: envelope.api_version.clone(),
                kind: envelope.kind.clone(),
                metadata: RawMetadata {
                    name,
                    namespace: envelope.metadata.namespace.clone(),
                    rest: serde_json::Map::new(),
                },
                spec: envelope.spec.clone(),
                status: None,
            },
            slug: spec.slug.as_str().to_owned(),
            publication,
            instance_name,
            instance,
            // A withdrawal deletes by name: no title is written and no organization created.
            language: None,
            instance_title: None,
            // A withdrawal draws no chart.
            history: Default::default(),
            granted: Vec::new(),
        });
    }
    gone
}
