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
//!   published again only after [`REFRESH`], a failed one backs off, and the calls of a run
//!   through the public edge keep [`CALL_GAP`] apart, gateway reads included. Calls the operator
//!   routed to the catalogue's in-cluster or loopback address take no gap, and a wave publishes
//!   [`PUBLISH_CONCURRENCY`] Endpoints at once after each organization's first ([`wave`], C1).
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
use jcctl::catalogue::{self, Catalogues, Door, Planned};
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

/// One line of a run: which dataset of which Endpoint of which project, and what happened to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// The project the Endpoint belongs to.
    pub project: String,
    /// The Endpoint's name.
    pub endpoint: String,
    /// The CKAN dataset name, one of several when the Endpoint lists `spec.catalog.datasets`
    /// (EP-175); empty for a line about the Endpoint or the project as a whole.
    pub dataset: String,
    /// What happened.
    pub publication: Publication,
}

impl Report {
    fn failed(project: &str, endpoint: &str, reason: impl Into<String>) -> Self {
        Self {
            project: project.to_owned(),
            endpoint: endpoint.to_owned(),
            dataset: String::new(),
            publication: Publication::Failed(reason.into()),
        }
    }

    fn of(target: &Target, publication: Publication) -> Self {
        Self {
            project: target.id.namespace.clone().unwrap_or_default(),
            endpoint: target.id.name.clone(),
            dataset: target.dataset_name().to_owned(),
            publication,
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
    /// What the log last said of each `(project, endpoint, dataset)`, so an outcome is logged
    /// when it changes and not once a sync (T-4065).
    said: Arc<Mutex<HashMap<Owner, String>>>,
    /// When each `(catalogue, dataset)` is next published (T-4717, EP-178), and when an
    /// Endpoint that cannot be published is next said to be (EP-174).
    pacing: Arc<Mutex<Pacing>>,
    /// When this Portal may next call the catalogue or read the gateway, shared by every
    /// publication (T-4717).
    last_call: Arc<Mutex<Option<Instant>>>,
    /// The deployment's own catalogue: its public URL, without a trailing slash, and the
    /// in-cluster address its API is called at instead (T-5314). `None` calls every instance
    /// at its `spec.url`.
    api_route: Option<(String, String)>,
    /// The loopback address the API route's host is reached at instead of through DNS: jc's
    /// own catalogue on a host mount, called on jc's listener under its host name (C1).
    api_reach: Option<std::net::SocketAddr>,
    /// The installation's own catalogue, where a public Endpoint of a project without a
    /// `CkanInstance` is published (EP-174). `None` publishes such an Endpoint nowhere.
    installation: Option<CkanInstanceSpec>,
    /// Each Organization's own catalogue (`organization_host::OrganizationCatalogue`): an
    /// Endpoint of a project it owns publishes there alone, under its name, with its token.
    organizations: Vec<crate::organization_host::OrganizationCatalogue>,
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
            api_reach: None,
            installation: None,
            organizations: Vec::new(),
        }
    }

    /// Publishes every Endpoint of an Organization's projects into that Organization's own
    /// catalogue (jc `[[organizations]]`). The operator configures them, never a manifest.
    pub fn for_organizations(
        mut self,
        catalogues: Vec<crate::organization_host::OrganizationCatalogue>,
    ) -> Self {
        self.organizations = catalogues;
        self
    }

    /// `target` as its Organization's own catalogue takes it: that catalogue's URL, the CKAN
    /// organisation of the Organization's name. A project of an Organization with no catalogue
    /// of its own keeps what its `CkanInstance` says.
    fn routed(&self, repo: &Repository, mut target: Target) -> Target {
        let project = target.id.namespace.clone().unwrap_or_default();
        let Some(catalogue) = organization_in(repo, &project)
            .and_then(|org| self.organizations.iter().find(|c| c.organization == org))
        else {
            return target;
        };
        target.instance.url = catalogue.url.as_str().trim_end_matches('/').to_owned();
        target.instance.organization_default = Some(catalogue.organization.clone());
        target.publication.organization = Some(catalogue.organization.clone());
        target
    }

    /// The Organization catalogue `instance` is, after [`CkanSync::routed`].
    fn organization_catalogue(
        &self,
        instance: &CkanInstanceSpec,
    ) -> Option<&crate::organization_host::OrganizationCatalogue> {
        self.organizations
            .iter()
            .find(|c| c.url.as_str().trim_end_matches('/') == instance.base_url())
    }

    /// The client of `target`'s catalogue: its Organization's (that catalogue's API address,
    /// reach and token file) or the instance's own (`apiTokenRef`), and the settings its
    /// datasets link the Organization's data host with.
    fn client(
        &self,
        target: &Target,
        repository: &Path,
        source: publisher::TokenSource<'_>,
    ) -> Result<(HttpCkan, String, Settings, Duration), String> {
        let mut settings = self.settings.clone();
        let Some(catalogue) = self.organization_catalogue(&target.instance) else {
            let api_url = self.api_url(&target.instance);
            let token = project_token(target, repository, source)?;
            let reach = self
                .api_reach
                .filter(|_| api_url != target.instance.base_url());
            let api = HttpCkan::reaching(&api_url, token, reach).map_err(|err| err.to_string())?;
            let gap = catalogue_gap(&api_url, target.instance.base_url());
            return Ok((api, api_url, settings, gap));
        };
        let api_url = catalogue
            .api
            .as_ref()
            .unwrap_or(&catalogue.url)
            .as_str()
            .trim_end_matches('/')
            .to_owned();
        let token =
            jcctl::secrets::SecretValue::from_file(&catalogue.token_file).ok_or_else(|| {
                format!(
                    "the token file of {}'s catalogue is unreadable or empty",
                    catalogue.organization
                )
            })?;
        let api =
            HttpCkan::reaching(&api_url, token, catalogue.reach).map_err(|err| err.to_string())?;
        if let Some(host) = &catalogue.data_host {
            settings.host = host.clone();
        }
        let gap = if catalogue.reach.is_some() {
            Duration::ZERO
        } else {
            catalogue_gap(&api_url, catalogue.url.as_str().trim_end_matches('/'))
        };
        Ok((api, api_url, settings, gap))
    }

    /// Publishes a public Endpoint of a project that declares no `CkanInstance` into the
    /// installation's own catalogue (EP-174). The operator configures it, never a manifest.
    pub fn with_installation_catalogue(mut self, instance: CkanInstanceSpec) -> Self {
        self.installation = Some(instance);
        self
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

    /// Reaches the API route's host at `addr` (a loopback listener) instead of through DNS.
    pub fn reached_at(mut self, addr: Option<std::net::SocketAddr>) -> Self {
        self.api_reach = addr;
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
        let mut unread: BTreeSet<String> = BTreeSet::new();
        // Endpoints that would be datasets and cannot be, this run: each keeps what it had in
        // the catalogue until its author fixes it, and is said to be unpublished (EP-174).
        let mut held: BTreeSet<(String, String)> = BTreeSet::new();
        for project in projects(repo) {
            match publisher::targets_with(repo, &project, self.installation.as_ref()) {
                Ok((targets, unplanned)) => {
                    wanted.extend(targets.into_iter().map(|t| self.routed(repo, t)));
                    for line in unplanned {
                        held.insert((line.project.clone(), line.endpoint.clone()));
                        if let Some(report) = self.unplanned(line) {
                            reports.push(report);
                        }
                    }
                }
                // A project whose publication does not read is one project's problem: the
                // Endpoint names an instance that is not there, or a spec that does not parse.
                Err(err) => {
                    unread.insert(project.clone());
                    reports.push(Report::failed(&project, "", err.to_string()));
                }
            }
        }

        // One catalogue holds one dataset of a name: two Endpoints claiming it would overwrite each
        // other on every pass, across projects too (T-6079). Neither is written.
        let refused = claimed_twice(&wanted);
        let claims: BTreeSet<(String, String)> = wanted.iter().map(claim).collect();

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

        // Each public dashboard goes beside the Endpoints it reads, kept from the sweep by its
        // package name (T-6790); one a dataset name already claims is refused.
        let dashboards = crate::reconciler::ckan_dashboards::targets(repo, &wanted);
        let mut due_dashboards = Vec::new();
        for dashboard in dashboards {
            let package = dashboard.package_name();
            if claims.iter().any(|(_, dataset)| dataset == &package) {
                reports.push(Report::failed(
                    &dashboard.project,
                    &dashboard.name,
                    format!("the dashboard's catalogue name {package} is a dataset's already: rename the dashboard"),
                ));
                continue;
            }
            if let Some(organization) = dashboard.organization() {
                let key = (
                    dashboard.anchor.instance.base_url().to_owned(),
                    organization,
                );
                if let Some((_, kept)) = sweeps.get_mut(&key) {
                    kept.insert(package.clone());
                }
            }
            let key = (
                format!("dashboard:{}", dashboard.project),
                dashboard.name.clone(),
            );
            let print = dashboard_fingerprint(&dashboard, &self.settings);
            let due = self.pacing.lock().unwrap_or_else(|p| p.into_inner()).due(
                &key,
                print,
                Instant::now(),
            );
            if due {
                due_dashboards.push((key, print, dashboard));
            }
        }

        let mut due_targets = Vec::new();
        for target in wanted {
            if let Some(reason) = refused.get(&owner(&target)) {
                reports.push(Report::of(&target, Publication::Failed(reason.clone())));
                continue;
            }
            let key = claim(&target);
            let print = fingerprint(&target, &self.settings);
            let due = self.pacing.lock().unwrap_or_else(|p| p.into_inner()).due(
                &key,
                print,
                Instant::now(),
            );
            if due {
                due_targets.push(target);
            }
        }
        let published = wave(due_targets, PUBLISH_CONCURRENCY, |target| {
            let known = &known;
            async move {
                let measured = self.measured(repo, &target);
                let report = self.one(&target, false, repository, measured, known).await;
                let print = fingerprint(&target, &self.settings);
                self.pacing
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .record(claim(&target), print, &report.publication, Instant::now());
                report
            }
        })
        .await;
        reports.extend(published);
        for (key, print, dashboard) in due_dashboards {
            let report = self.dashboard(dashboard, repository, &known).await;
            self.pacing
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .record(key, print, &report.publication, Instant::now());
            reports.push(report);
        }
        // Each dataset the previous run published and this one does not, by its catalogue and
        // name (EP-178): an entry removed from an Endpoint's list goes alone. A dataset a
        // current Endpoint still publishes (a renamed Endpoint keeping its dataset name, or
        // another project's) is not the gone one's to delete; nor is the dataset of a project
        // that did not read or of an Endpoint held back this run.
        for target in withdrawn(previous, &claims, self.installation.as_ref())
            .into_iter()
            .map(|target| self.routed(repo, target))
            .filter(|target| {
                let project = target.id.namespace.clone().unwrap_or_default();
                !unread.contains(&project) && !held.contains(&(project, target.id.name.clone()))
            })
        {
            let measured = Measurements::default();
            reports.push(self.one(&target, true, repository, measured, &known).await);
        }
        // A project whose publications did not read keeps its datasets out of `kept`, and a
        // shared organization would lose them: no sweep in such a pass. An Endpoint held back
        // keeps its datasets out of `kept` the same way.
        if !unread.is_empty() || !held.is_empty() {
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

    /// The line of an Endpoint that would be a dataset and cannot be (EP-174), when it is due:
    /// paced as a failed publication is, so the activity feed hears it again only after a
    /// back-off or when the reason changes.
    fn unplanned(&self, line: catalogue::Unplanned) -> Option<Report> {
        let key = (format!("{}/{}", line.project, line.endpoint), String::new());
        let print = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            line.reason.hash(&mut hasher);
            hasher.finish()
        };
        let mut pacing = self.pacing.lock().unwrap_or_else(|p| p.into_inner());
        if !pacing.due(&key, print, Instant::now()) {
            return None;
        }
        let report = Report::failed(&line.project, &line.endpoint, line.reason);
        pacing.record(key, print, &report.publication, Instant::now());
        Some(report)
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
            let key = (
                report.project.clone(),
                report.endpoint.clone(),
                report.dataset.clone(),
            );
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
        let line = |publication| Report::of(target, publication);
        let target = target.clone();
        let repository = repository.to_path_buf();
        let age_key_file = self.age_key_file.clone();
        let last_call = self.last_call.clone();
        let known = known.clone();
        let this = self.clone();
        // Resolving the token, opening the socket and reading the gateway are all blocking, and
        // dropping a blocking client inside this runtime panics, so the whole publication of one
        // Endpoint happens on a blocking thread.
        let work = tokio::task::spawn_blocking(move || {
            let source = publisher::TokenSource {
                env: None,
                age_key_file: age_key_file.as_deref(),
            };
            let (api, api_url, settings, gap) = this.client(&target, &repository, source)?;
            let mut api = Paced::new(api, last_call, gap)
                .with_read_gap(read_gap(&settings.host))
                .remembering(&api_url, known);
            if withdraw {
                return withdraw_one(&mut api, &target);
            }
            publish_one(&mut api, &target, &settings, &measured)
        })
        .await;

        match work {
            Ok(Ok(publication)) => line(publication),
            Ok(Err(reason)) => line(Publication::Failed(reason)),
            Err(err) => line(Publication::Failed(format!(
                "the publication did not finish: {err}"
            ))),
        }
    }

    /// Publishes one public dashboard through its anchor Endpoint's catalogue client (T-6790).
    async fn dashboard(
        &self,
        dashboard: crate::reconciler::ckan_dashboards::DashboardTarget,
        repository: &Path,
        known: &Organizations,
    ) -> Report {
        use crate::reconciler::ckan_dashboards as dashboards;
        let (project, name) = (dashboard.project.clone(), dashboard.name.clone());
        let dataset = dashboard.package_name();
        let repository = repository.to_path_buf();
        let age_key_file = self.age_key_file.clone();
        let last_call = self.last_call.clone();
        let known = known.clone();
        let this = self.clone();
        let work = tokio::task::spawn_blocking(move || {
            let source = publisher::TokenSource {
                env: None,
                age_key_file: age_key_file.as_deref(),
            };
            let (api, api_url, settings, gap) =
                this.client(&dashboard.anchor, &repository, source)?;
            let mut api = Paced::new(api, last_call, gap).remembering(&api_url, known);
            let organization = dashboard
                .organization()
                .ok_or("the dashboard's catalogue names no organization")?;
            let desired = dashboards::desired(&dashboard, &organization, &settings)?;
            dashboards::publish(&mut api, &desired)
        })
        .await;
        let publication = match work {
            Ok(Ok(outcome)) => Publication::Published {
                dataset: dataset.clone(),
                outcome,
                no_linkml: false,
            },
            Ok(Err(reason)) => Publication::Failed(reason),
            Err(err) => Publication::Failed(format!("the publication did not finish: {err}")),
        };
        Report {
            project,
            endpoint: name,
            dataset,
            publication,
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
        let known = known.clone();
        let this = self.clone();
        let work = tokio::task::spawn_blocking(move || {
            let source = publisher::TokenSource {
                env: None,
                age_key_file: age_key_file.as_deref(),
            };
            let (api, api_url, _, gap) = this.client(&target, &repository, source)?;
            let mut api = Paced::new(api, last_call, gap).remembering(&api_url, known);
            sweep_with(&mut api, &organization, &kept)
        })
        .await;
        match work {
            Ok(Ok(reports)) => reports
                .into_iter()
                .map(|(dataset, publication)| Report {
                    project: project.clone(),
                    endpoint: dataset.clone(),
                    dataset,
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

/// How many Endpoints one wave publishes at once: a fixed number per deployment, never per
/// organization (C1: 609 datasets one at a time took about 50 minutes).
pub const PUBLISH_CONCURRENCY: usize = 8;

/// The gap between catalogue calls made at `api_url` for the catalogue whose public URL is
/// `public`: [`CALL_GAP`] through the public edge (T-4717), none on an in-cluster or loopback
/// address the operator routed the calls to, which no edge limits (C1).
pub fn catalogue_gap(api_url: &str, public: &str) -> Duration {
    if api_url.trim_end_matches('/') == public.trim_end_matches('/') && !is_loopback(api_url) {
        CALL_GAP
    } else {
        Duration::ZERO
    }
}

/// The gap between gateway reads at the data host `host` (a host or an origin): [`CALL_GAP`]
/// unless it is a loopback address.
pub fn read_gap(host: &str) -> Duration {
    if is_loopback(host) {
        Duration::ZERO
    } else {
        CALL_GAP
    }
}

/// Whether `address` (a host, `host:port` or a URL) names this machine.
fn is_loopback(address: &str) -> bool {
    let rest = address.split_once("://").map_or(address, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(v6),
        None => authority.rsplit_once(':').map_or(authority, |(h, _)| h),
    };
    let host = host.to_ascii_lowercase();
    host == "localhost"
        || host.ends_with(".localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// The CKAN organisation a target publishes into, by its catalogue: two targets with one key
/// share one `organization_show`/`organization_create`.
fn organization_key(target: &Target) -> (String, Option<String>) {
    (
        target.instance.base_url().to_owned(),
        target
            .publication
            .organization
            .clone()
            .or_else(|| target.instance.organization_default.clone()),
    )
}

/// Runs `publish` over `targets` and answers each one's result in their order: the first target
/// of each organization one at a time, so each organization is created once and the pass's
/// [`Organizations`] holds it before anyone else asks (two first datasets would race
/// `organization_create`), then every other target at most `concurrency` at once. One target's
/// failure is its own result.
pub async fn wave<T, F, Fut>(targets: Vec<Target>, concurrency: usize, publish: F) -> Vec<T>
where
    F: Fn(Target) -> Fut,
    Fut: std::future::Future<Output = T>,
{
    use futures_util::StreamExt;
    let mut seen = BTreeSet::new();
    let (first, rest): (Vec<_>, Vec<_>) = targets
        .into_iter()
        .enumerate()
        .partition(|(_, t)| seen.insert(organization_key(t)));
    let mut out: Vec<(usize, T)> = Vec::new();
    for (i, target) in first {
        out.push((i, publish(target).await));
    }
    let publish = &publish;
    let mut done: Vec<(usize, T)> = futures_util::stream::iter(rest)
        .map(|(i, target)| async move { (i, publish(target).await) })
        .buffer_unordered(concurrency.max(1))
        .collect()
        .await;
    out.append(&mut done);
    out.sort_by_key(|(i, _)| *i);
    out.into_iter().map(|(_, r)| r).collect()
}

/// What a target looks like to the publisher, so a changed Endpoint, instance or setting is due
/// at once. In-process only: the hash is never stored, so a differing `Debug` between builds
/// costs one republication, nothing more.
pub fn fingerprint(target: &Target, settings: &Settings) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    format!("{target:?}|{settings:?}").hash(&mut hasher);
    hasher.finish()
}

/// What a public dashboard looks like to the publisher: its spec and its anchor's fingerprint,
/// so an edited dashboard or a moved catalogue is due at once (T-6790).
pub fn dashboard_fingerprint(
    dashboard: &crate::reconciler::ckan_dashboards::DashboardTarget,
    settings: &Settings,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    format!("{:?}|{:?}", dashboard.spec, dashboard.endpoints).hash(&mut hasher);
    fingerprint(&dashboard.anchor, settings).hash(&mut hasher);
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
    /// Between two catalogue calls; zero on the in-cluster or loopback route, which no edge
    /// limits, so those calls never take the shared gate (C1).
    gap: Duration,
    /// Between two gateway reads, which pass the public edge unless the data host is loopback.
    read_gap: Duration,
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
            read_gap: gap,
            known: None,
        }
    }

    /// Gateway reads paced `read_gap` apart instead of the catalogue's gap.
    #[must_use]
    pub fn with_read_gap(mut self, read_gap: Duration) -> Self {
        self.read_gap = read_gap;
        self
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
            self.wait_for(calls, self.read_gap);
        }
        read()
    }

    fn wait(&self) {
        self.wait_for(1, self.gap);
    }

    /// Waits until the last holder's gap has passed, then holds `calls` gaps from now. A zero
    /// gap takes no turn: it would move a public caller's reservation earlier.
    fn wait_for(&self, calls: u32, gap: Duration) {
        if gap.is_zero() {
            return;
        }
        let mut last = self.last.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(ready) = *last {
            let now = Instant::now();
            if ready > now {
                std::thread::sleep(ready - now);
            }
        }
        *last = Some(Instant::now() + gap * calls);
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

/// The instance's API token, resolved against the Endpoint's own project's secrets
/// (`projects/{project}/secrets`, CC-06) and never the whole staged tree, so one project's
/// `apiTokenRef` cannot name another project's secret (T-6227). The instance lives in that
/// project too: the publisher refuses an `instanceRef` into another one.
fn project_token(
    target: &Target,
    repository: &Path,
    source: publisher::TokenSource<'_>,
) -> Result<jcctl::secrets::SecretValue, String> {
    let project = target.id.namespace.as_deref().unwrap_or_default();
    let folder = crate::pipeline_secrets::secrets_folder(
        repository,
        project,
        std::slice::from_ref(&target.instance.api_token_ref),
    )
    .map_err(|err| err.to_string())?;
    publisher::token(&target.instance, &folder, source).map_err(|err| err.to_string())
}

/// The Organization that owns `project` in `repo`, as `permissions::organization_of` reads it
/// in the mirror: its Project's `spec.organizationRef`, else the installation's (the only
/// Organization, or the only one that sets `spec.installation`).
fn organization_in(repo: &Repository, project: &str) -> Option<String> {
    let org_level = |id: &ResourceId| {
        id.namespace
            .as_deref()
            .is_none_or(|n| n == crate::api::blueprints::ORG_NAMESPACE)
    };
    let owner = repo
        .iter()
        .find(|(id, _)| org_level(id) && id.kind == "Project" && id.name == project)
        .and_then(|(_, r)| {
            r.manifest
                .spec
                .get("organizationRef")
                .and_then(crate::resource::reference_name)
                .map(str::to_owned)
        });
    if owner.is_some() {
        return owner;
    }
    let organizations: Vec<(&str, bool)> = repo
        .iter()
        .filter(|(id, _)| org_level(id) && id.kind == "Organization")
        .map(|(id, r)| {
            (
                id.name.as_str(),
                r.manifest.spec.get("installation") == Some(&serde_json::Value::Bool(true)),
            )
        })
        .collect();
    match organizations.as_slice() {
        [(only, _)] => Some((*only).to_owned()),
        _ => {
            let mut claims = organizations
                .iter()
                .filter(|(_, installation)| *installation);
            match (claims.next(), claims.next()) {
                (Some((one, _)), None) => Some((*one).to_owned()),
                _ => None,
            }
        }
    }
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
pub fn claim(target: &Target) -> (String, String) {
    (
        target.instance.base_url().to_ascii_lowercase(),
        target.dataset_name().to_owned(),
    )
}

/// Who publishes a target: `(project, endpoint, dataset)`, the key [`claimed_twice`] refuses by.
pub type Owner = (String, String, String);

/// The [`Owner`] of a target.
pub fn owner(target: &Target) -> Owner {
    (
        target.id.namespace.clone().unwrap_or_default(),
        target.id.name.clone(),
        target.dataset_name().to_owned(),
    )
}

/// Every dataset whose catalogue and name another Endpoint's dataset also claims, keyed by
/// [`owner`], with the reason it is not published: the others it collides with and the fix.
/// CKAN dataset names are one namespace per catalogue, so publishing either would overwrite the
/// other's dataset (T-6079, EP-177); none of them is written until one takes a name of its own.
/// Another dataset of the same Endpoint is not refused with it.
pub fn claimed_twice(wanted: &[Target]) -> HashMap<Owner, String> {
    let mut claimants: BTreeMap<(String, String), Vec<&Target>> = BTreeMap::new();
    for target in wanted {
        claimants.entry(claim(target)).or_default().push(target);
    }
    let mut refused = HashMap::new();
    for ((catalogue, dataset), targets) in claimants {
        for target in &targets {
            let others: BTreeSet<String> = targets
                .iter()
                .filter(|other| owner(other) != owner(target))
                .map(|other| {
                    format!(
                        "{}/{}",
                        other.id.namespace.as_deref().unwrap_or_default(),
                        other.id.name
                    )
                })
                .collect();
            if others.is_empty() {
                continue;
            }
            let field = if target.slice.is_some() {
                "the spec.catalog.datasets entry"
            } else {
                "spec.publish.ckan.name"
            };
            let project = target.id.namespace.as_deref().unwrap_or_default();
            let reason = format!(
                "not published: the CKAN dataset `{dataset}` on {catalogue} is also what Endpoint \
                 {} publishes, and one catalogue holds one dataset of a name, so neither is \
                 written; give {field} a name of its own, such as `{project}-{dataset}`",
                others.into_iter().collect::<Vec<_>>().join(", "),
            );
            refused.insert(owner(target), reason);
        }
    }
    refused
}

/// The datasets the previous run published and this one does not (CC-19, EP-178): an
/// Endpoint whose publication was withdrawn or that is gone altogether, an Endpoint that stopped
/// being public, and one entry removed from an Endpoint's `spec.catalog.datasets`.
///
/// `kept` is what this run publishes, as `(catalogue, dataset)` ([`claim`]); `installation` the
/// installation's catalogue the previous run published into, as this one does.
///
/// Read from the previous mirror rather than from CKAN, so a dataset this reconciler never
/// published is never deleted by it. A withdrawal that happens while the Portal is down is not
/// seen here; [`sweep_with`] finds it in the catalogue by the platform's own `generated_by` mark.
pub fn withdrawn(
    previous: &Mirror,
    kept: &BTreeSet<(String, String)>,
    installation: Option<&CkanInstanceSpec>,
) -> Vec<Target> {
    let mut catalogues = Catalogues {
        installation: installation.cloned(),
        ..Catalogues::default()
    };
    let mut endpoints = Vec::new();
    for envelope in
        previous.matching(|env| matches!(env.kind.as_str(), "Endpoint" | "CkanInstance" | "App"))
    {
        let project = envelope.metadata.namespace.clone().unwrap_or_default();
        let name = envelope.metadata.name.clone();
        match envelope.kind.as_str() {
            "CkanInstance" => {
                if let Ok(spec) = serde_json::from_value(envelope.spec.clone()) {
                    catalogues
                        .instances
                        .entry(project)
                        .or_default()
                        .insert(name, spec);
                }
            }
            "App" => {
                catalogues.apps.insert((project, name));
            }
            _ => {
                if let Ok(spec) = serde_json::from_value::<EndpointSpec>(envelope.spec.clone()) {
                    endpoints.push((project, envelope, spec));
                }
            }
        }
    }
    let (planned, unplanned) = catalogue::datasets(
        endpoints.iter().map(|(project, envelope, spec)| Door {
            project,
            name: &envelope.metadata.name,
            spec,
        }),
        &catalogues,
    );
    for line in unplanned {
        // The catalogue manifest went with the Endpoint. Nothing can be reached without the
        // instance's URL and token reference, and inventing either would be worse than leaving
        // the dataset to the sweep.
        tracing::warn!(
            project = %line.project,
            endpoint = %line.endpoint,
            reason = %line.reason,
            "a dataset the previous run published cannot be reached to withdraw; it stays until \
             the catalogue is declared again"
        );
    }
    planned
        .into_iter()
        .filter(|dataset| !kept.contains(&dataset.key()))
        .filter_map(|dataset| {
            let (_, envelope, spec) = endpoints.iter().find(|(project, envelope, _)| {
                *project == dataset.project && envelope.metadata.name == dataset.endpoint
            })?;
            Some(gone(envelope, spec, dataset))
        })
        .collect()
}

/// The target that withdraws `dataset` of the Endpoint `envelope` held.
fn gone(
    envelope: &crate::resource::ResourceEnvelope,
    spec: &EndpointSpec,
    dataset: Planned,
) -> Target {
    let name = envelope.metadata.name.clone();
    Target {
        id: ResourceId {
            group: "joinedcontext.com".to_owned(),
            kind: "Endpoint".to_owned(),
            namespace: Some(dataset.project),
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
        publication: dataset.publication,
        slice: dataset.slice,
        instance_name: dataset.instance_name,
        instance: dataset.instance,
        // A withdrawal deletes by name: no title is written and no organization created.
        language: None,
        instance_title: None,
        // A withdrawal draws no chart.
        history: Default::default(),
        granted: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn target(project: &str) -> Target {
        let publication = json!({ "instanceRef": { "kind": "CkanInstance", "name": "hel-fi" } });
        Target {
            id: ResourceId {
                group: "joinedcontext.com".to_owned(),
                kind: "Endpoint".to_owned(),
                namespace: Some(project.to_owned()),
                name: "bikes".to_owned(),
            },
            manifest: RawManifest {
                api_version: "joinedcontext.com/v1alpha1".to_owned(),
                kind: "Endpoint".to_owned(),
                metadata: RawMetadata {
                    name: "bikes".to_owned(),
                    namespace: Some(project.to_owned()),
                    rest: serde_json::Map::new(),
                },
                spec: json!({}),
                status: None,
            },
            slug: "scsd2eehkx42n53z2zyd6vshfh7s7irf".to_owned(),
            publication: serde_json::from_value(publication).expect("a publication"),
            instance_name: "hel-fi".to_owned(),
            instance: serde_json::from_value(json!({
                "url": "https://data.example.fi",
                "organizationDefault": "hel-fi",
                "apiTokenRef": { "name": "ckan-api-token", "key": "token" },
            }))
            .expect("an instance"),
            language: None,
            instance_title: None,
            history: Default::default(),
            granted: Vec::new(),
            slice: None,
        }
    }

    /// T-6227 for the catalogue token: the publisher read every SOPS file of the staged tree, so
    /// an `apiTokenRef` of one project resolved a secret of the same name in another.
    #[test]
    fn an_api_token_resolves_only_from_its_own_projects_secrets() {
        use age::secrecy::ExposeSecret as _;
        let identity = age::x25519::Identity::generate();
        let tree = std::env::temp_dir().join(format!("t6227-ckan-{}", std::process::id()));
        let folder = tree.join("projects/helsinki/secrets");
        std::fs::create_dir_all(&folder).expect("folder");
        let text = crate::secret_files::encrypt(
            "ckan-api-token",
            &BTreeMap::from([(
                "token".to_owned(),
                zeroize::Zeroizing::new("helsinki-ckan-token".to_owned()),
            )]),
            &[identity.to_public().to_string()],
            "2026-11-08T00:00:00Z",
        )
        .expect("encrypted");
        std::fs::write(folder.join("ckan-api-token.enc.yaml"), text).expect("file");
        let key = tree.join("age.key");
        std::fs::write(&key, identity.to_string().expose_secret()).expect("key");
        let source = publisher::TokenSource {
            env: None,
            age_key_file: Some(&key),
        };

        let own = project_token(&target("helsinki"), &tree, source).expect("its own");
        assert_eq!(own.expose(), "helsinki-ckan-token");
        let other = project_token(&target("oulu"), &tree, source).expect_err("another");
        assert!(other.contains("ckan-api-token"), "{other}");
        assert!(!other.contains("helsinki-ckan-token"), "{other}");
        assert!(project_token(&target("../helsinki"), &tree, source).is_err());
        std::fs::remove_dir_all(&tree).ok();
    }

    fn manifest(kind: &str, name: &str, spec: serde_json::Value) -> RawManifest {
        RawManifest {
            api_version: "joinedcontext.com/v1alpha1".to_owned(),
            kind: kind.to_owned(),
            metadata: RawMetadata {
                name: name.to_owned(),
                namespace: None,
                rest: serde_json::Map::new(),
            },
            spec,
            status: None,
        }
    }

    /// jc-multiorg-portal: an Endpoint of each Organization's project publishes into that
    /// Organization's own catalogue, under its name, with its own token, whatever catalogue its
    /// `CkanInstance` names; the other Organization's catalogue is never the target.
    #[test]
    fn each_organizations_endpoint_publishes_into_its_own_catalogue_alone() {
        let repo = Repository::from_manifests(
            vec![
                manifest("Organization", "hel", json!({ "installation": true })),
                manifest("Organization", "ba", json!({})),
                manifest("Project", "helsinki", json!({ "organizationRef": "hel" })),
                manifest(
                    "Project",
                    "bratislava",
                    json!({ "organizationRef": { "kind": "Organization", "name": "ba" } }),
                ),
                manifest("Project", "unowned", json!({})),
            ],
            None,
        )
        .expect("a repository");
        let dir = std::env::temp_dir().join(format!("jc-multiorg-ckan-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let catalogue = |org: &str| {
            let token_file = dir.join(format!("{org}.token"));
            std::fs::write(&token_file, format!("{org}-token\n")).expect("token");
            crate::organization_host::OrganizationCatalogue {
                organization: org.to_owned(),
                url: format!("http://opendata.{org}.localhost:42301")
                    .parse()
                    .unwrap(),
                api: Some(format!("http://opendata.{org}.localhost").parse().unwrap()),
                reach: Some("127.0.0.1:42301".parse().unwrap()),
                data_host: Some(format!("http://api.{org}.localhost:42301")),
                token_file,
            }
        };
        let sync = CkanSync::new("portal.example")
            .for_organizations(vec![catalogue("hel"), catalogue("ba")]);
        let source = publisher::TokenSource {
            env: None,
            age_key_file: None,
        };
        for (project, org) in [
            ("helsinki", "hel"),
            ("bratislava", "ba"),
            ("unowned", "hel"),
        ] {
            // The instance names hel-fi on data.example.fi: the Organization's catalogue wins.
            let routed = sync.routed(&repo, target(project));
            let own = format!("http://opendata.{org}.localhost:42301");
            assert_eq!(routed.instance.url, own, "{project}");
            assert_eq!(routed.instance.organization_default.as_deref(), Some(org));
            assert_eq!(routed.publication.organization.as_deref(), Some(org));
            let (_, api_url, settings, _) = sync
                .client(&routed, &dir, source)
                .unwrap_or_else(|e| panic!("{project}: {e}"));
            assert_eq!(api_url, format!("http://opendata.{org}.localhost"));
            assert_eq!(settings.host, format!("http://api.{org}.localhost:42301"));
            let other = if org == "hel" { "ba" } else { "hel" };
            assert!(!routed.instance.url.contains(other), "{project}");
        }
        // Without its token file the run says so, and never falls back to another's.
        std::fs::remove_file(dir.join("ba.token")).expect("rm");
        let routed = sync.routed(&repo, target("bratislava"));
        let Err(err) = sync.client(&routed, &dir, source) else {
            panic!("bratislava published without its token file");
        };
        assert!(err.contains("ba's catalogue"), "{err}");
        assert!(!err.contains("hel-token"), "{err}");
        // A Portal without Organization catalogues keeps the instance's.
        let plain = CkanSync::new("portal.example").routed(&repo, target("bratislava"));
        assert_eq!(plain.instance.url, "https://data.example.fi");
        std::fs::remove_dir_all(&dir).ok();
    }
}
