//! A build pod per App (AP-130, AP-131, ADR-N-028 §5 amended 2026-09-25, T-2794), started by
//! the Portal itself (the jc build lane).
//!
//! A build is queued when the head of an App's default branch is not the commit its
//! `status.build` names (a push to it), or when a person asks for a Rebuild; the queue is
//! `app_builds` ([`crate::apps::build_records`]). Each pass starts the oldest queued build as one
//! Kubernetes Job in the build namespace. Its pod holds one token of that build alone, valid
//! until the Job's deadline: it fetches that commit's source from the Portal's build listener and
//! puts that build's artifact back, and can do nothing else (`api::build_listener`). The token
//! reaches the pod's init container only, through a Secret the Job owns, and goes with the Job;
//! the Portal keeps only its hash.
//!
//! The pod mounts `build-cache-{app}`, the App's own build cache, which no other pod mounts:
//! whatever an App's code writes there is only ever read by a later build of the same
//! repository. A claim whose App is gone is deleted on the next pass.
//!
//! At most [`MAX_RUNNING`](crate::apps::build_pods::MAX_RUNNING) build Jobs run in the cluster
//! at once, and one per App; the other builds wait in the queue and the next pass after one ends
//! starts the next (T-5815: a seed sync queued five builds, their pods took the node's memory
//! requests to 98 % and a rollout's surge pod stayed Pending).

use std::collections::BTreeSet;

use serde_json::{json, Value};
use sqlx::PgPool;

use crate::apps::build_records::{self, Record, Trigger};
use crate::apps::kube::{KubeClient, KubeError};
use crate::forge::ShakoClient;
use crate::resource::ResourceEnvelope;
use crate::store::{ListOptions, Mirror};

/// The label every build pod, its Job, its Secret and its claim carry, with the App's name.
pub const APP_LABEL: &str = "joinedcontext.com/build-app";

/// What a build pod is to the network policies: a pod of its own, which reaches the Portal's
/// build listener and DNS and nothing else (the deployment's NetworkPolicy selects this name).
pub const POD_NAME: &str = "jc-app-build";

/// The annotation a Job gets once its token Secret is written. A Job without it lost its token
/// half way through a pass, and the Portal learns that from the Job alone: its Role in the
/// build namespace reads no Secret (T-4798).
pub const TOKEN_WRITTEN: &str = "joinedcontext.com/build-token-written";

/// The build's id on its Job, so a pass finds the Job of a running build.
pub const BUILD_ID: &str = "joinedcontext.com/build-id";

/// How long a finished Job, its pod and its token Secret stay for a person to read the logs.
const FINISHED_TTL_SECONDS: u64 = 600;

/// How many build Jobs may run in the cluster at once. One: the node is shared with every
/// platform workload, and a Rust build alone requests 1.5 GiB.
pub const MAX_RUNNING: usize = 1;

/// How long a build's token outlives its Job's deadline, for the last upload and the report.
const TOKEN_GRACE_SECONDS: i64 = 5 * 60;

/// The workflow every application repository carries (AP-100): its `runs-on` names the class.
pub const WORKFLOW_PATH: &str = ".gitea/workflows/build.yml";

/// Where a build pod runs, what it runs, and where it reports back (AP-130).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// The build namespace, where the Portal may write Jobs, Secrets and claims and nothing
    /// else (its Role there).
    pub namespace: String,
    /// The Portal's build listener as a build pod reaches it, in-cluster
    /// (`JC_PORTAL_BUILD_API_URL`).
    pub api: String,
    /// Where the build listener binds (`JC_PORTAL_BUILD_BIND`, default `0.0.0.0:9092`): a port of
    /// its own, so a NetworkPolicy lets a build pod reach it and nothing else of this process.
    pub bind: std::net::SocketAddr,
    /// The `node-22` builder image, by digest.
    pub node_image: String,
    /// The `rust-1.90` builder image, by digest.
    pub rust_image: String,
    /// The size of each App's cache claim, `1Gi` unless the deployment says otherwise.
    pub cache_size: String,
    /// The most a build may write to `/tmp`, `4Gi` unless the deployment says otherwise; the
    /// kubelet evicts a pod that writes more.
    pub tmp_size: String,
    /// Where the uploads of a running build are kept until the Portal publishes them.
    pub work_dir: std::path::PathBuf,
}

/// The two kinds of build, as the workflow names them in `runs-on`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// A static App: Node, the SDK and the template's packages.
    Node,
    /// A fullstack or wasm App: the Rust toolchain besides.
    Rust,
}

impl Class {
    /// The class an App's workflow file asks for in its build job's `runs-on`, or `None` when it
    /// is not an App's build workflow (one of the three the Portal writes, AP-100).
    pub fn of_workflow(text: &str) -> Option<Self> {
        text.lines().find_map(|line| {
            match line.trim().strip_prefix("runs-on:")?.trim() {
                "app-build-node" => Some(Self::Node),
                "app-build-rust" => Some(Self::Rust),
                _ => None,
            }
        })
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Node => "node",
            Self::Rust => "rust",
        }
    }

    /// The wall-clock limit of the class, the shared runner's (AP-81).
    fn deadline_seconds(self) -> u64 {
        match self {
            Self::Node => 20 * 60,
            Self::Rust => 30 * 60,
        }
    }

    /// How long a build's token admits its pod: the deadline and a grace for the report.
    pub fn token_lifetime(self) -> chrono::Duration {
        chrono::Duration::seconds(
            i64::try_from(self.deadline_seconds()).unwrap_or(i64::MAX) + TOKEN_GRACE_SECONDS,
        )
    }

    fn resources(self) -> Value {
        match self {
            Self::Node => json!({
                "requests": { "cpu": "250m", "memory": "512Mi" },
                "limits": { "cpu": "2", "memory": "2Gi" }
            }),
            Self::Rust => json!({
                "requests": { "cpu": "250m", "memory": "1536Mi" },
                "limits": { "cpu": "2", "memory": "4Gi" }
            }),
        }
    }
}

/// An App that still builds: one the mirror holds whose lifecycle is not `retired` (AP-21).
fn builds(app: &ResourceEnvelope) -> bool {
    app.spec.get("lifecycle").and_then(Value::as_str) != Some("retired")
}

/// An App built on the forge: it names its own repository (AP-100).
fn on_forge(app: &ResourceEnvelope) -> bool {
    app.spec
        .pointer("/source/git")
        .is_some_and(|git| !git.is_null())
}

/// `build-cache-{app}`: App names are unique in the organization (AP-14a).
pub fn claim_name(app: &str) -> String {
    format!("build-cache-{app}")
}

/// `build-{app}-{build}`, cut to a DNS label: the build's id keeps it unique.
pub fn job_name(app: &str, build: i64) -> String {
    let suffix = format!("-{build}");
    let room = 63 - "build-".len() - suffix.len();
    let app = app.get(..app.len().min(room)).unwrap_or(app);
    format!("build-{}{suffix}", app.trim_end_matches('-'))
}

fn labels(app: &str) -> Value {
    json!({
        "app.kubernetes.io/name": POD_NAME,
        "app.kubernetes.io/component": "build-pod",
        "app.kubernetes.io/part-of": "joinedcontext",
        "app.kubernetes.io/managed-by": "joinedcontext-portal",
        APP_LABEL: app,
    })
}

/// The App's cache claim (AP-131).
pub fn claim(settings: &Settings, app: &str) -> Value {
    json!({
        "apiVersion": "v1",
        "kind": "PersistentVolumeClaim",
        "metadata": {
            "name": claim_name(app),
            "namespace": settings.namespace,
            "labels": labels(app),
        },
        "spec": {
            "accessModes": ["ReadWriteOnce"],
            "resources": { "requests": { "storage": settings.cache_size } },
        }
    })
}

/// The Job that runs one build of one App (AP-130). Offline: the image holds every package
/// and crate the build may use, no step installs anything, and the pod reaches nothing but the
/// build listener (AP-82).
pub fn job(settings: &Settings, build: &Record) -> Value {
    let (class, app) = (build.class(), build.app.as_str());
    let name = job_name(app, build.id);
    let image = match class {
        Class::Node => &settings.node_image,
        Class::Rust => &settings.rust_image,
    };
    let restricted = json!({
        "runAsNonRoot": true,
        "allowPrivilegeEscalation": false,
        "readOnlyRootFilesystem": true,
        "capabilities": { "drop": ["ALL"] },
        "seccompProfile": { "type": "RuntimeDefault" },
    });
    json!({
        "apiVersion": "batch/v1",
        "kind": "Job",
        "metadata": {
            "name": name,
            "namespace": settings.namespace,
            "labels": labels(app),
            "annotations": { BUILD_ID: build.id.to_string() },
        },
        "spec": {
            "backoffLimit": 0,
            "activeDeadlineSeconds": class.deadline_seconds(),
            "ttlSecondsAfterFinished": FINISHED_TTL_SECONDS,
            "template": {
                "metadata": {
                    "labels": labels(app),
                    "annotations": {
                        "linkerd.io/inject": "enabled",
                        // The proxy ends with the build, so the Job can finish.
                        "config.alpha.linkerd.io/proxy-enable-native-sidecar": "true",
                    },
                },
                "spec": {
                    "restartPolicy": "Never",
                    "automountServiceAccountToken": false,
                    "enableServiceLinks": false,
                    "securityContext": {
                        "runAsNonRoot": true,
                        "runAsUser": 1000,
                        "runAsGroup": 1000,
                        "fsGroup": 1000,
                        "seccompProfile": { "type": "RuntimeDefault" },
                    },
                    // Only this container sees the build's token: it copies it into memory, and
                    // the build reads it once and deletes it.
                    "initContainers": [{
                        "name": "build-token",
                        "image": image,
                        "command": ["/bin/sh", "-c", "cp /secret/token /tmp/build-secret/token"],
                        "securityContext": restricted,
                        "resources": {
                            "requests": { "cpu": "10m", "memory": "16Mi" },
                            "limits": { "cpu": "100m", "memory": "64Mi" }
                        },
                        "volumeMounts": [
                            { "name": "token", "mountPath": "/secret", "readOnly": true },
                            { "name": "build-secret", "mountPath": "/tmp/build-secret" }
                        ]
                    }],
                    "containers": [{
                        "name": "build",
                        "image": image,
                        "command": ["/usr/local/bin/jc-build"],
                        "env": [
                            { "name": "JC_BUILD_API", "value": settings.api },
                            { "name": "JC_BUILD_ID", "value": build.id.to_string() },
                            { "name": "JC_APP_COMMIT", "value": build.commit },
                            { "name": "JC_BUILD_TOKEN_FILE", "value": "/tmp/build-secret/token" }
                        ],
                        "securityContext": restricted,
                        "resources": class.resources(),
                        "volumeMounts": [
                            { "name": "build-secret", "mountPath": "/tmp/build-secret" },
                            { "name": "tmp", "mountPath": "/tmp" },
                            { "name": "cache", "mountPath": "/cache" }
                        ]
                    }],
                    "volumes": [
                        { "name": "token", "secret": {
                            "secretName": name,
                            "items": [{ "key": "token", "path": "token" }]
                        } },
                        { "name": "build-secret", "emptyDir": { "medium": "Memory", "sizeLimit": "1Mi" } },
                        { "name": "tmp", "emptyDir": { "sizeLimit": settings.tmp_size } },
                        { "name": "cache", "persistentVolumeClaim": { "claimName": claim_name(app) } }
                    ]
                }
            }
        }
    })
}

/// Whether `job` carries [`TOKEN_WRITTEN`], set only after its token Secret was applied.
fn token_was_written(job: &Value) -> bool {
    job.pointer("/metadata/annotations")
        .and_then(|annotations| annotations.get(TOKEN_WRITTEN))
        .and_then(Value::as_str)
        == Some("true")
}

/// The Secret holding the build's token, owned by its Job so it goes with it.
pub fn token_secret(
    settings: &Settings,
    app: &str,
    build: i64,
    job_uid: &str,
    token: &str,
) -> Value {
    let name = job_name(app, build);
    json!({
        "apiVersion": "v1",
        "kind": "Secret",
        "metadata": {
            "name": name,
            "namespace": settings.namespace,
            "labels": labels(app),
            "ownerReferences": [{
                "apiVersion": "batch/v1",
                "kind": "Job",
                "name": name,
                "uid": job_uid,
                "controller": true,
            }],
        },
        "type": "Opaque",
        "stringData": { "token": token },
    })
}

/// The condition that ended a Job, `Complete` or `Failed` with its reason, or `None` while it
/// runs.
fn ended(job: &Value) -> Option<(String, String)> {
    job.pointer("/status/conditions")
        .and_then(Value::as_array)?
        .iter()
        .find(|condition| {
            matches!(
                condition.get("type").and_then(Value::as_str),
                Some("Complete" | "Failed")
            ) && condition.get("status").and_then(Value::as_str) == Some("True")
        })
        .map(|condition| {
            let text = |key: &str| {
                condition
                    .get(key)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned()
            };
            (text("type"), text("reason"))
        })
}

/// Whether a build Job still holds its pod's requests: not yet complete or failed, and not
/// being deleted.
fn running(job: &Value) -> bool {
    job.pointer("/metadata/deletionTimestamp").is_none() && ended(job).is_none()
}

/// What one pass did, for the log.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Pass {
    /// `build-{app}-{build}` of every Job this pass created.
    pub started: Vec<String>,
    /// Every build this pass ended because its Job ended or its token expired, with why.
    pub ended: Vec<(i64, String)>,
    /// Every claim this pass deleted because its App is gone.
    pub removed: Vec<String>,
}

/// Why a pass stopped.
#[derive(Debug, thiserror::Error)]
pub enum DispatchError {
    /// The forge did not answer.
    #[error("the forge: {0}")]
    Forge(#[from] crate::forge::GitError),
    /// The API server refused or was not reached.
    #[error("the cluster: {0}")]
    Kube(#[from] KubeError),
    /// The build store did not answer.
    #[error("the build store: {0}")]
    Store(#[from] sqlx::Error),
}

/// One pass: running builds whose Job ended or whose token expired are ended, then, while fewer
/// than [`MAX_RUNNING`] build Jobs run, the oldest queued build of an App with none running gets
/// its Job; last, no claim for an App that is gone.
pub async fn dispatch_once(
    kube: &KubeClient,
    db: &PgPool,
    mirror: &Mirror,
    settings: &Settings,
) -> Result<Pass, DispatchError> {
    let mut pass = Pass::default();
    let jobs = kube
        .list(
            "batch/v1",
            "Job",
            &settings.namespace,
            &format!("app.kubernetes.io/component=build-pod,{APP_LABEL}"),
        )
        .await?;
    pass.ended = settle(db, &jobs).await?;
    if jobs.iter().filter(|job| running(job)).count() < MAX_RUNNING {
        if let Some(build) = build_records::next_pending(db).await? {
            let name = job_name(&build.app, build.id);
            start(kube, db, settings, &build).await?;
            pass.started.push(name);
        }
    }
    pass.removed = sweep_claims(kube, mirror, settings).await?;
    Ok(pass)
}

/// Ends every running build whose Job ended without the pod reporting, is gone, or whose token
/// expired; a build being verified is the Portal's own and is left to finish.
async fn settle(db: &PgPool, jobs: &[Value]) -> Result<Vec<(i64, String)>, DispatchError> {
    let now = chrono::Utc::now();
    let mut ended_now = Vec::new();
    for build in build_records::in_state(db, "running").await? {
        let name = job_name(&build.app, build.id);
        let job = jobs
            .iter()
            .find(|job| job.pointer("/metadata/name").and_then(Value::as_str) == Some(&name));
        let why = match job.and_then(ended) {
            Some((kind, reason)) => Some(format!(
                "the build Job {name} ended ({kind}{}) before the build reported",
                if reason.is_empty() { String::new() } else { format!(": {reason}") }
            )),
            None if build.expires_at.is_some_and(|at| at <= now) => {
                Some(format!("the build did not report before its token expired ({name})"))
            }
            // A Job applied in this pass is not listed yet; one gone after its start is.
            None if job.is_none() && build.started_at.is_some_and(|at| now - at > chrono::Duration::minutes(2)) => {
                Some(format!("the build Job {name} is gone"))
            }
            None => None,
        };
        if let Some(why) = why {
            build_records::finish(db, build.id, Err(&why)).await?;
            ended_now.push((build.id, why));
        }
    }
    Ok(ended_now)
}

/// Starts one build: the token is minted and its hash stored first, then the cache claim, the
/// Job, the token Secret the Job owns, and last the annotation that says the Job holds its
/// token. A cluster that refuses any of it puts the build back in the queue.
async fn start(
    kube: &KubeClient,
    db: &PgPool,
    settings: &Settings,
    build: &Record,
) -> Result<(), DispatchError> {
    let name = job_name(&build.app, build.id);
    // A Job of this build whose token never arrived (a pass that stopped half way) would wait
    // for it until its deadline: it goes, and this pass starts the build again.
    if let Some(existing) = kube.get("batch/v1", "Job", &settings.namespace, &name).await? {
        if token_was_written(&existing) {
            return Ok(());
        }
        kube.delete("batch/v1", "Job", &settings.namespace, &name).await?;
    }
    let token = build_records::new_token();
    if !build_records::start(db, build.id, &token, build.class().token_lifetime()).await? {
        return Ok(());
    }
    if let Err(err) = apply_job(kube, settings, build, &token).await {
        build_records::requeue(db, build.id).await?;
        return Err(err.into());
    }
    Ok(())
}

async fn apply_job(
    kube: &KubeClient,
    settings: &Settings,
    build: &Record,
    token: &str,
) -> Result<(), KubeError> {
    let name = job_name(&build.app, build.id);
    kube.apply(&claim(settings, &build.app)).await?;
    let mut spec = job(settings, build);
    kube.apply(&spec).await?;
    let uid = kube
        .get("batch/v1", "Job", &settings.namespace, &name)
        .await?
        .and_then(|job| job.pointer("/metadata/uid")?.as_str().map(str::to_owned))
        .ok_or_else(|| KubeError::Api {
            status: 404,
            path: name.clone(),
            message: "the Job was applied and is not there".into(),
        })?;
    kube.apply(&token_secret(settings, &build.app, build.id, &uid, token))
        .await?;
    spec["metadata"]["annotations"][TOKEN_WRITTEN] = json!("true");
    kube.apply(&spec).await
}

/// The class of a build of `commit` of the App's repository, read from its workflow file at
/// that commit, or `None` when the repository carries no App build workflow there.
pub async fn class_at(repo: &ShakoClient, commit: &str) -> Result<Option<Class>, crate::forge::GitError> {
    Ok(repo
        .get_file(WORKFLOW_PATH, commit)
        .await?
        .and_then(|file| Class::of_workflow(&file.content)))
}

/// One trigger pass: for every App built on the forge that still builds, a build of the head of
/// its default branch is queued when `status.build` names another commit and that commit was
/// never queued. Returns the `(app, commit)` of every build it queued.
pub async fn trigger_once(
    forge: &ShakoClient,
    db: &PgPool,
    mirror: &Mirror,
) -> Result<Vec<(String, String)>, DispatchError> {
    let mut queued = Vec::new();
    let apps: Vec<ResourceEnvelope> = mirror
        .namespaces()
        .iter()
        .flat_map(|namespace| mirror.list(namespace, "App", &ListOptions::default()).items)
        .filter(|app| builds(app) && on_forge(app))
        .collect();
    for app in apps {
        let (Some(project), name) = (app.metadata.namespace.as_deref(), &app.metadata.name) else {
            continue;
        };
        let repo = forge.for_application(crate::agents::repository::name(project, name));
        let head = match repo.default_branch().await {
            Ok(branch) => repo.branch_head(&branch).await,
            Err(err) => Err(err),
        };
        let head = match head {
            Ok(head) => head,
            // A repository not created yet has nothing to build.
            Err(crate::forge::GitError::NotFound) => continue,
            Err(err) => return Err(err.into()),
        };
        let built = app.status.as_ref().and_then(|s| s.build.as_ref()).map(|b| b.commit.as_str());
        if built == Some(head.as_str()) || build_records::known(db, project, name, &head).await? {
            continue;
        }
        let Some(class) = class_at(&repo, &head).await? else {
            tracing::debug!(app = %name, commit = %head, "no App build workflow at the head: nothing to build");
            continue;
        };
        build_records::enqueue(db, project, name, &head, class, Trigger::Push, None).await?;
        queued.push((name.clone(), head));
    }
    Ok(queued)
}

/// Deletes the cache claim of every App that is gone and returns the claims it deleted.
async fn sweep_claims(
    kube: &KubeClient,
    mirror: &Mirror,
    settings: &Settings,
) -> Result<Vec<String>, DispatchError> {
    let mut removed = Vec::new();
    let apps: BTreeSet<String> = mirror
        .namespaces()
        .iter()
        .flat_map(|namespace| mirror.list(namespace, "App", &ListOptions::default()).items)
        .filter(builds)
        .map(|app| app.metadata.name)
        .collect();
    // ponytail: a mirror that holds no App at all is read as "not loaded yet" and sweeps
    // nothing, so a restart never empties every cache; the last App's claim then waits for the
    // organization's next App to go.
    if mirror.is_empty() {
        return Ok(removed);
    }
    for claim in kube
        .list(
            "v1",
            "PersistentVolumeClaim",
            &settings.namespace,
            &format!("app.kubernetes.io/component=build-pod,{APP_LABEL}"),
        )
        .await?
    {
        let Some(app) = claim
            .pointer(&format!(
                "/metadata/labels/{}",
                APP_LABEL.replace('/', "~1")
            ))
            .and_then(Value::as_str)
        else {
            continue;
        };
        if !apps.contains(app) {
            let name = claim_name(app);
            kube.delete("v1", "PersistentVolumeClaim", &settings.namespace, &name)
                .await?;
            removed.push(name);
        }
    }
    Ok(removed)
}

/// How often the queue is read: a build waits at most this long for its pod.
const PASS_EVERY: std::time::Duration = std::time::Duration::from_secs(5);
/// The longest a failing pass waits before it asks again.
const BACKOFF_CEILING: std::time::Duration = std::time::Duration::from_secs(300);

/// What a failing pass does next: wait twice as long each time up to [`BACKOFF_CEILING`], and
/// say why once per reason instead of on every tick (T-2969). A refusal the forge repeats every
/// 5 s is one line in the log, not seventeen thousand a day.
#[derive(Debug, Default)]
struct Backoff {
    failures: u32,
    reason: Option<String>,
}

impl Backoff {
    /// After a failed pass: how long to wait, and whether `reason` is news worth a warning.
    fn failed(&mut self, reason: String) -> (std::time::Duration, bool) {
        self.failures = self.failures.saturating_add(1);
        let news = self.reason.as_deref() != Some(reason.as_str());
        self.reason = Some(reason);
        let wait = PASS_EVERY
            .checked_mul(1u32.checked_shl(self.failures.min(16)).unwrap_or(u32::MAX))
            .unwrap_or(BACKOFF_CEILING)
            .min(BACKOFF_CEILING);
        (wait, news)
    }

    /// After a pass that ran: whether it had been failing, so the recovery is said once too.
    fn recovered(&mut self) -> bool {
        let was_failing = self.failures > 0;
        *self = Self::default();
        was_failing
    }
}

/// How many dispatch passes go by between two trigger passes: the heads are read once a minute,
/// as often as the Portal reads main.
const TRIGGER_EVERY_PASSES: u32 = 12;

/// One tick: every [`TRIGGER_EVERY_PASSES`]th a trigger pass first, then a dispatch pass.
async fn tick(
    kube: &KubeClient,
    forge: &ShakoClient,
    db: &PgPool,
    mirror: &Mirror,
    settings: &Settings,
    count: u32,
) -> Result<Pass, DispatchError> {
    if count % TRIGGER_EVERY_PASSES == 0 && !mirror.is_empty() {
        for (app, commit) in trigger_once(forge, db, mirror).await? {
            tracing::info!(%app, %commit, "a build of a new head of the App is queued");
        }
    }
    dispatch_once(kube, db, mirror, settings).await
}

/// Runs [`trigger_once`] and [`dispatch_once`] on the replica that holds the reconciler's lease.
/// Outside a cluster there is nowhere to start a pod, which is not an error.
pub fn spawn_periodic(
    forge: std::sync::Arc<ShakoClient>,
    db: PgPool,
    syncer: std::sync::Arc<crate::reconciler::Syncer>,
    mirror: std::sync::Arc<Mirror>,
    settings: Settings,
) {
    let kube = match KubeClient::in_cluster() {
        Ok(Some(kube)) => kube,
        Ok(None) => {
            tracing::info!("no cluster: App builds wait in the queue and nobody starts them");
            return;
        }
        Err(err) => {
            tracing::warn!(error = %err, "the ServiceAccount mount is unreadable, so no build pod starts");
            return;
        }
    };
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(PASS_EVERY);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut backoff = Backoff::default();
        let mut resume = tokio::time::Instant::now();
        let mut count: u32 = 0;
        loop {
            ticker.tick().await;
            if !syncer.is_leader() || tokio::time::Instant::now() < resume {
                continue;
            }
            let outcome = tick(&kube, &forge, &db, &mirror, &settings, count).await;
            count = count.wrapping_add(1);
            match outcome {
                Ok(pass) => {
                    if backoff.recovered() {
                        tracing::info!("build pods: the pass runs again");
                    }
                    for job in &pass.started {
                        tracing::info!(%job, "build pod started");
                    }
                    for (build, why) in &pass.ended {
                        tracing::warn!(build, %why, "build failed");
                    }
                    for claim in &pass.removed {
                        tracing::info!(%claim, "build cache of a retired App deleted");
                    }
                }
                Err(err) => {
                    let (wait, news) = backoff.failed(err.to_string());
                    resume = tokio::time::Instant::now() + wait;
                    if news {
                        tracing::warn!(error = %err, retry_in_secs = wait.as_secs(), "build pods: the pass stopped");
                    } else {
                        tracing::debug!(error = %err, retry_in_secs = wait.as_secs(), "build pods: the pass stopped again");
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> Settings {
        Settings {
            namespace: "smartcity".into(),
            api: "http://jc-build.smartcity.svc.cluster.local:9092".into(),
            bind: "0.0.0.0:9092".parse().expect("an address"),
            node_image: "registry.example.org/builder@sha256:aa".into(),
            rust_image: "registry.example.org/builder-rust@sha256:bb".into(),
            cache_size: "1Gi".into(),
            tmp_size: "4Gi".into(),
            work_dir: "/tmp/jc-app-builds".into(),
        }
    }

    fn record(app: &str, id: i64, class: Class) -> Record {
        Record {
            id,
            project: "helsinki".into(),
            app: app.into(),
            commit: "8c56954a1f0e3d2c1b0a99887766554433221100".into(),
            class: class.name().into(),
            trigger: "push".into(),
            requested_by: None,
            state: "pending".into(),
            expires_at: None,
            created_at: chrono::Utc::now(),
            started_at: None,
            finished_at: None,
            digest: None,
            reason: None,
        }
    }

    #[test]
    fn a_failing_pass_backs_off_to_its_ceiling_and_says_each_reason_once() {
        let mut backoff = Backoff::default();
        let refused = "the forge: git api error 403: user should be the owner of the repo";
        let (first, news) = backoff.failed(refused.into());
        assert!(news, "the first refusal is news");
        assert_eq!(first, PASS_EVERY * 2);
        let (second, news) = backoff.failed(refused.into());
        assert!(!news, "the same refusal again is not");
        assert_eq!(second, PASS_EVERY * 4);
        for _ in 0..40 {
            backoff.failed(refused.into());
        }
        assert_eq!(backoff.failed(refused.into()).0, BACKOFF_CEILING);
        // Another reason is news again, at the same wait.
        let (wait, news) = backoff.failed("the cluster: 503".into());
        assert!(news);
        assert_eq!(wait, BACKOFF_CEILING);
        // A pass that runs resets both, and says it recovered once.
        assert!(backoff.recovered());
        assert!(!backoff.recovered());
        assert_eq!(backoff.failed(refused.into()), (PASS_EVERY * 2, true));
    }

    #[test]
    fn the_workflows_build_job_names_the_class_and_nothing_else_does() {
        let workflow = |build: &str| {
            format!(
                "on:\n  push:\n    branches: [main]\njobs:\n  build:\n    runs-on: {build}\n  propose:\n    runs-on: node-22\n"
            )
        };
        assert_eq!(Class::of_workflow(&workflow("app-build-node")), Some(Class::Node));
        assert_eq!(Class::of_workflow(&workflow("app-build-rust")), Some(Class::Rust));
        assert_eq!(
            Class::of_workflow(&workflow("ubuntu-latest")),
            None,
            "a workflow the Portal did not write builds nothing here"
        );
        assert_eq!(Class::of_workflow(""), None);
        // The three templates the Portal writes into an App's first commit.
        for (template, class) in [
            ("template", Class::Node),
            ("template-fullstack", Class::Rust),
            ("template-wasm", Class::Rust),
        ] {
            let path = format!(
                "{}/sdk/{template}/.gitea/workflows/build.yml",
                env!("CARGO_MANIFEST_DIR")
            );
            let text = std::fs::read_to_string(&path).expect("the template's workflow");
            assert_eq!(Class::of_workflow(&text), Some(class), "{template}");
        }
    }

    #[test]
    fn a_job_name_is_a_dns_label_unique_per_build() {
        assert_eq!(job_name("bikes", 42), "build-bikes-42");
        let long = "a".repeat(63);
        let name = job_name(&long, 9_007_199_254_740_991);
        assert!(name.len() <= 63, "{name}");
        assert!(name.ends_with("-9007199254740991"));
        assert!(!job_name("ab-", 1).contains("--"));
    }

    #[test]
    fn the_pod_mounts_its_own_apps_cache_and_nothing_else_of_another() {
        let rendered = job(&settings(), &record("bikes", 7, Class::Rust));
        let volumes = rendered
            .pointer("/spec/template/spec/volumes")
            .and_then(Value::as_array)
            .expect("volumes");
        let claims: Vec<&str> = volumes
            .iter()
            .filter_map(|v| v.pointer("/persistentVolumeClaim/claimName")?.as_str())
            .collect();
        assert_eq!(claims, ["build-cache-bikes"]);
        let other = job(&settings(), &record("events", 8, Class::Rust));
        assert!(!other.to_string().contains("build-cache-bikes"));
    }

    #[test]
    fn the_pod_keeps_the_walls_of_the_shared_runner() {
        let rendered = job(&settings(), &record("bikes", 7, Class::Node));
        let spec = rendered.pointer("/spec/template/spec").expect("pod spec");
        assert_eq!(spec["automountServiceAccountToken"], false);
        assert_eq!(spec["enableServiceLinks"], false);
        assert_eq!(spec["securityContext"]["runAsNonRoot"], true);
        assert_eq!(spec["securityContext"]["runAsUser"], 1000);
        assert_eq!(rendered["spec"]["backoffLimit"], 0);
        assert_eq!(rendered["spec"]["activeDeadlineSeconds"], 1200);
        for container in ["initContainers", "containers"] {
            let context = &spec[container][0]["securityContext"];
            assert_eq!(context["runAsNonRoot"], true);
            assert_eq!(context["allowPrivilegeEscalation"], false);
            assert_eq!(context["readOnlyRootFilesystem"], true);
            assert_eq!(context["capabilities"]["drop"], json!(["ALL"]));
            assert!(context["capabilities"].get("add").is_none());
        }
        assert_eq!(
            spec["containers"][0]["resources"]["limits"]["memory"],
            "2Gi"
        );
        assert_eq!(
            rendered.pointer("/spec/template/metadata/labels/app.kubernetes.io~1name"),
            Some(&json!(POD_NAME)),
            "the build pods' network policy selects it"
        );
        // No hostPath, no runtime socket, no ServiceAccount: the only volumes are these four.
        let names: Vec<&str> = spec["volumes"]
            .as_array()
            .expect("volumes")
            .iter()
            .filter_map(|v| v["name"].as_str())
            .collect();
        assert_eq!(names, ["token", "build-secret", "tmp", "cache"]);
        assert!(!rendered.to_string().contains("hostPath"));
        assert!(spec.get("serviceAccountName").is_none());
    }

    #[test]
    fn tmp_has_a_size_limit_and_the_token_folder_lives_in_memory() {
        let rendered = job(&settings(), &record("bikes", 7, Class::Node));
        let volume = |name: &str| {
            rendered
                .pointer("/spec/template/spec/volumes")
                .and_then(Value::as_array)
                .and_then(|all| all.iter().find(|v| v["name"] == name).cloned())
                .expect("the volume")
        };
        assert_eq!(volume("tmp")["emptyDir"]["sizeLimit"], "4Gi");
        assert_eq!(volume("build-secret")["emptyDir"]["medium"], "Memory");
        assert_eq!(volume("build-secret")["emptyDir"]["sizeLimit"], "1Mi");
    }

    #[test]
    fn only_the_init_container_mounts_the_token_and_the_build_knows_only_its_own_build() {
        let rendered = job(&settings(), &record("bikes", 7, Class::Node));
        let spec = rendered.pointer("/spec/template/spec").expect("pod spec");
        let mounts = |container: &str| spec[container][0]["volumeMounts"].to_string();
        assert!(mounts("initContainers").contains("\"token\""));
        assert!(!mounts("containers").contains("\"token\""));
        let env: Vec<(String, String)> = spec["containers"][0]["env"]
            .as_array()
            .expect("env")
            .iter()
            .map(|e| (e["name"].as_str().unwrap_or_default().to_owned(), e["value"].as_str().unwrap_or_default().to_owned()))
            .collect();
        assert_eq!(
            env,
            [
                ("JC_BUILD_API".to_owned(), settings().api),
                ("JC_BUILD_ID".to_owned(), "7".to_owned()),
                ("JC_APP_COMMIT".to_owned(), "8c56954a1f0e3d2c1b0a99887766554433221100".to_owned()),
                ("JC_BUILD_TOKEN_FILE".to_owned(), "/tmp/build-secret/token".to_owned()),
            ],
            "no forge address, no forge or lane token, no install source"
        );
        assert_eq!(rendered["metadata"]["annotations"][BUILD_ID], "7");
    }

    #[test]
    fn a_job_runs_until_it_completes_fails_or_goes() {
        let with = |status: Value| json!({ "metadata": { "name": "build-a-1" }, "status": status });
        assert!(running(&json!({ "metadata": { "name": "build-a-1" } })));
        assert!(running(&with(json!({ "active": 1 }))));
        assert!(running(&with(
            json!({ "conditions": [{ "type": "Complete", "status": "False" }] })
        )));
        assert!(!running(&with(
            json!({ "conditions": [{ "type": "Complete", "status": "True" }] })
        )));
        let failed = with(json!({ "conditions": [{ "type": "Failed", "status": "True", "reason": "DeadlineExceeded" }] }));
        assert!(!running(&failed));
        assert_eq!(ended(&failed), Some(("Failed".into(), "DeadlineExceeded".into())));
        assert!(!running(&json!({
            "metadata": { "name": "build-a-1", "deletionTimestamp": "2026-10-26T19:06:00Z" }
        })));
    }

    #[test]
    fn the_token_secret_goes_with_its_job() {
        let secret = token_secret(&settings(), "bikes", 7, "0f6c-uid", "tok");
        assert_eq!(secret["metadata"]["name"], "build-bikes-7");
        assert_eq!(secret["metadata"]["ownerReferences"][0]["uid"], "0f6c-uid");
        assert_eq!(secret["metadata"]["ownerReferences"][0]["kind"], "Job");
        assert_eq!(secret["stringData"]["token"], "tok");
    }

    #[test]
    fn the_claim_is_the_apps_own_and_sized_by_the_deployment() {
        let rendered = claim(&settings(), "bikes");
        assert_eq!(rendered["metadata"]["name"], "build-cache-bikes");
        assert_eq!(rendered["metadata"]["labels"][APP_LABEL], "bikes");
        assert_eq!(rendered["spec"]["resources"]["requests"]["storage"], "1Gi");
        assert_eq!(rendered["spec"]["accessModes"][0], "ReadWriteOnce");
    }

    #[test]
    fn a_token_admits_its_pod_until_the_deadline_and_a_grace() {
        assert_eq!(Class::Node.token_lifetime(), chrono::Duration::seconds(1200 + 300));
        assert_eq!(Class::Rust.token_lifetime(), chrono::Duration::seconds(1800 + 300));
    }
}
