//! A build pod per App (AP-130, AP-131, ADR-N-028 §5 amended 2026-09-25, T-2794).
//!
//! An App's `build` job asks for the label `app-build-{class}`, which no shared runner carries.
//! Each pass lists the organization's queued jobs, and for a job of an App's repository creates
//! one Kubernetes Job in the runner's namespace. Its pod registers once, `--ephemeral`, with a
//! registration token of that repository alone, takes that one job and ends, so it can never
//! take another repository's job. The token reaches the pod's init container only, through a
//! Secret the Job owns, and goes with the Job.
//!
//! The pod mounts `build-cache-{app}`, the App's own build cache, which no other pod mounts:
//! whatever an App's code writes there is only ever read by a later build of the same
//! repository. A claim whose App is gone is deleted on the next pass.
//!
//! At most [`MAX_RUNNING`](crate::apps::build_pods::MAX_RUNNING) build Jobs run in the cluster
//! at once; the other jobs stay queued in the forge and the next pass after one ends starts the
//! next (T-5815: a seed sync queued five builds, their pods took the node's memory requests to
//! 98 % and a rollout's surge pod stayed Pending).

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::apps::kube::{KubeClient, KubeError};
use crate::git::gitea::{GiteaClient, QueuedJob};
use crate::resource::ResourceEnvelope;
use crate::store::{ListOptions, Mirror};

/// The label every build pod, its Job, its Secret and its claim carry, with the App's name.
pub const APP_LABEL: &str = "joinedcontext.com/build-app";

/// What a build pod is to the network policies: a runner, walled in like the shared one (AP-81).
const RUNNER_NAME: &str = "gitea-runner";

/// The annotation a Job gets once its token Secret is written. A Job without it lost its token
/// half way through a pass, and the Portal learns that from the Job alone: its Role in the
/// runner's namespace reads no Secret, so the forge's own credentials there stay out of its
/// reach (T-4798).
pub const TOKEN_WRITTEN: &str = "joinedcontext.com/build-token-written";

/// How long a finished Job, its pod and its token Secret stay for a person to read the logs.
const FINISHED_TTL_SECONDS: u64 = 600;

/// How many build Jobs may run in the cluster at once. One: the node is shared with every
/// platform workload, and a Rust build alone requests 1.5 GiB.
pub const MAX_RUNNING: usize = 1;

/// Where a build pod runs and what it runs (AP-130).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// The runner's namespace, where the Portal may write Jobs, Secrets and claims and nothing
    /// else (its Role there).
    pub namespace: String,
    /// The forge as a runner reaches it, in-cluster.
    pub forge: String,
    /// The `node-22` runner image, by digest.
    pub node_image: String,
    /// The `rust-1.90` runner image, by digest.
    pub rust_image: String,
    /// The size of each App's cache claim, `1Gi` unless the deployment says otherwise.
    pub cache_size: String,
}

/// The two kinds of build, as the workflow names them in `runs-on`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// A static App: Node, the SDK and the template's packages.
    Node,
    /// A fullstack App: the Rust toolchain besides.
    Rust,
}

impl Class {
    /// The class a job asks for, or `None` when it is not an App's build.
    pub fn of(labels: &[String]) -> Option<Self> {
        labels.iter().find_map(|label| match label.as_str() {
            "app-build-node" => Some(Self::Node),
            "app-build-rust" => Some(Self::Rust),
            _ => None,
        })
    }

    fn name(self) -> &'static str {
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

/// The App a repository belongs to, `(project, app)`: `{project}_{app}` names it (AP-75), and
/// only an App the mirror holds and that is not retired counts, so a repository that merely
/// looks like one gets no pod.
pub fn app_of(mirror: &Mirror, repository: &str) -> Option<(String, String)> {
    let (project, app) = repository.split_once('_')?;
    mirror
        .get(project, "App", app)
        .filter(builds)
        .map(|_| (project.to_owned(), app.to_owned()))
}

/// `build-cache-{app}`: App names are unique in the organization (AP-14a).
pub fn claim_name(app: &str) -> String {
    format!("build-cache-{app}")
}

/// `build-{app}-{job}`, cut to a DNS label: the forge's job id keeps it unique.
pub fn job_name(app: &str, job: u64) -> String {
    let suffix = format!("-{job}");
    let room = 63 - "build-".len() - suffix.len();
    let app = app.get(..app.len().min(room)).unwrap_or(app);
    format!("build-{}{suffix}", app.trim_end_matches('-'))
}

fn labels(app: &str) -> Value {
    json!({
        "app.kubernetes.io/name": RUNNER_NAME,
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

/// The Job that runs one forge job of one App (AP-130).
pub fn job(settings: &Settings, class: Class, app: &str, id: u64) -> Value {
    let name = job_name(app, id);
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
        "metadata": { "name": name, "namespace": settings.namespace, "labels": labels(app) },
        "spec": {
            "backoffLimit": 0,
            "activeDeadlineSeconds": class.deadline_seconds(),
            "ttlSecondsAfterFinished": FINISHED_TTL_SECONDS,
            "template": {
                "metadata": {
                    "labels": labels(app),
                    "annotations": {
                        "linkerd.io/inject": "enabled",
                        // The proxy ends with the runner, so the Job can finish.
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
                    // Only this container sees the repository's registration token: it copies
                    // it into memory, and the runner reads it once and deletes it.
                    "initContainers": [{
                        "name": "registration-token",
                        "image": image,
                        "command": ["/bin/sh", "-c", "cp /secret/token /tmp/runner-secret/token"],
                        "securityContext": restricted,
                        "resources": {
                            "requests": { "cpu": "10m", "memory": "16Mi" },
                            "limits": { "cpu": "100m", "memory": "64Mi" }
                        },
                        "volumeMounts": [
                            { "name": "registration", "mountPath": "/secret", "readOnly": true },
                            { "name": "runner-secret", "mountPath": "/tmp/runner-secret" }
                        ]
                    }],
                    "containers": [{
                        "name": "runner",
                        "image": image,
                        "command": ["/usr/local/bin/runner"],
                        "env": [
                            { "name": "GITEA_INSTANCE", "value": settings.forge },
                            { "name": "GITEA_RUNNER_NAME", "value": name },
                            { "name": "GITEA_RUNNER_ONCE", "value": "1" }
                        ],
                        "securityContext": restricted,
                        "resources": class.resources(),
                        "volumeMounts": [
                            { "name": "config", "mountPath": "/opt/runner", "readOnly": true },
                            { "name": "runner-secret", "mountPath": "/tmp/runner-secret" },
                            { "name": "tmp", "mountPath": "/tmp" },
                            { "name": "cache", "mountPath": "/cache" }
                        ]
                    }],
                    "volumes": [
                        { "name": "registration", "secret": {
                            "secretName": name,
                            "items": [{ "key": "token", "path": "token" }]
                        } },
                        { "name": "runner-secret", "emptyDir": { "medium": "Memory", "sizeLimit": "1Mi" } },
                        { "name": "tmp", "emptyDir": {} },
                        { "name": "config", "configMap": {
                            "name": format!("gitea-runner-build-{}", class.name())
                        } },
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

/// The Secret holding the repository's registration token, owned by its Job so it goes with it.
pub fn token_secret(settings: &Settings, app: &str, id: u64, job_uid: &str, token: &str) -> Value {
    let name = job_name(app, id);
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

/// Whether a build Job still holds its pod's requests: not yet complete or failed, and not
/// being deleted.
fn running(job: &Value) -> bool {
    if job.pointer("/metadata/deletionTimestamp").is_some() {
        return false;
    }
    let ended = job
        .pointer("/status/conditions")
        .and_then(Value::as_array)
        .is_some_and(|conditions| {
            conditions.iter().any(|condition| {
                matches!(
                    condition.get("type").and_then(Value::as_str),
                    Some("Complete" | "Failed")
                ) && condition.get("status").and_then(Value::as_str) == Some("True")
            })
        });
    !ended
}

/// How many build Jobs run in the runner's namespace now.
async fn running_builds(kube: &KubeClient, settings: &Settings) -> Result<usize, KubeError> {
    Ok(kube
        .list(
            "batch/v1",
            "Job",
            &settings.namespace,
            &format!("app.kubernetes.io/component=build-pod,{APP_LABEL}"),
        )
        .await?
        .iter()
        .filter(|job| running(job))
        .count())
}

/// What one pass did, for the log.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Pass {
    /// `build-{app}-{job}` of every Job this pass created.
    pub started: Vec<String>,
    /// Every claim this pass deleted because its App is gone.
    pub removed: Vec<String>,
}

/// Why a pass stopped.
#[derive(Debug, thiserror::Error)]
pub enum DispatchError {
    /// The forge did not list the queue or mint a token.
    #[error("the forge: {0}")]
    Forge(#[from] crate::git::GitError),
    /// The API server refused or was not reached.
    #[error("the cluster: {0}")]
    Kube(#[from] KubeError),
}

/// One pass: a pod for a queued App build that has none while fewer than [`MAX_RUNNING`] build
/// Jobs run, and no claim for an App that is gone. A job of a repository that is not an App's
/// gets no pod and waits in the queue; so does a build while another one runs.
pub async fn dispatch_once(
    kube: &KubeClient,
    forge: &GiteaClient,
    mirror: &Mirror,
    settings: &Settings,
) -> Result<Pass, DispatchError> {
    let mut pass = Pass::default();
    // Read once, when the first build would start, and counted up as this pass starts more.
    let mut busy: Option<usize> = None;
    for QueuedJob {
        id,
        labels,
        repository,
    } in forge.applications().queued_jobs().await?
    {
        let Some(class) = Class::of(&labels) else {
            continue;
        };
        let Some((_, app)) = app_of(mirror, &repository) else {
            tracing::debug!(%repository, job = id, "a queued build of no App: no pod");
            continue;
        };
        let name = job_name(&app, id);
        if let Some(existing) = kube
            .get("batch/v1", "Job", &settings.namespace, &name)
            .await?
        {
            // A Job whose token never arrived (a pass that stopped half way) would wait for it
            // until its deadline; removing it lets the next pass start the job again.
            if !token_was_written(&existing) {
                kube.delete("batch/v1", "Job", &settings.namespace, &name)
                    .await?;
            }
            continue;
        }
        let now = match busy {
            Some(now) => now,
            None => running_builds(kube, settings).await?,
        };
        busy = Some(now);
        if now >= MAX_RUNNING {
            tracing::debug!(job = %name, running = now, "a build waits for the one running");
            continue;
        }
        start(kube, forge, settings, class, &app, id, &repository).await?;
        busy = Some(now.saturating_add(1));
        pass.started.push(name);
    }

    pass.removed = sweep_claims(kube, mirror, settings).await?;
    Ok(pass)
}

/// Starts one build: the runner token first (a forge that mints none leaves nothing behind in
/// the cluster), then the cache claim, the Job, the token Secret the Job owns, and last the
/// annotation that says the Job holds its token.
async fn start(
    kube: &KubeClient,
    forge: &GiteaClient,
    settings: &Settings,
    class: Class,
    app: &str,
    id: u64,
    repository: &str,
) -> Result<(), DispatchError> {
    let name = job_name(app, id);
    let token = forge
        .for_application(repository)
        .repository_registration_token()
        .await?;
    kube.apply(&claim(settings, app)).await?;
    let mut build = job(settings, class, app, id);
    kube.apply(&build).await?;
    let uid = kube
        .get("batch/v1", "Job", &settings.namespace, &name)
        .await?
        .and_then(|job| job.pointer("/metadata/uid")?.as_str().map(str::to_owned))
        .ok_or_else(|| KubeError::Api {
            status: 404,
            path: name.clone(),
            message: "the Job was applied and is not there".into(),
        })?;
    kube.apply(&token_secret(settings, app, id, &uid, &token))
        .await?;
    build["metadata"]["annotations"] = json!({ TOKEN_WRITTEN: "true" });
    kube.apply(&build).await?;
    Ok(())
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

/// Runs [`dispatch_once`] every few seconds on the replica that holds the reconciler's lease.
/// Outside a cluster there is nowhere to start a pod, which is not an error.
pub fn spawn_periodic(
    forge: std::sync::Arc<GiteaClient>,
    syncer: std::sync::Arc<crate::reconciler::Syncer>,
    mirror: std::sync::Arc<Mirror>,
    settings: Settings,
) {
    let kube = match KubeClient::in_cluster() {
        Ok(Some(kube)) => kube,
        Ok(None) => {
            tracing::info!("no cluster: App builds wait for a runner that nobody starts");
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
        loop {
            ticker.tick().await;
            if !syncer.is_leader() || tokio::time::Instant::now() < resume {
                continue;
            }
            match dispatch_once(&kube, &forge, &mirror, &settings).await {
                Ok(pass) => {
                    if backoff.recovered() {
                        tracing::info!("build pods: the pass runs again");
                    }
                    for job in &pass.started {
                        tracing::info!(%job, "build pod started");
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
    use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};

    fn settings() -> Settings {
        Settings {
            namespace: "dev".into(),
            forge: "http://gitea-http.dev.svc.cluster.local:3000".into(),
            node_image: "registry.example.org/builder@sha256:aa".into(),
            rust_image: "registry.example.org/builder-rust@sha256:bb".into(),
            cache_size: "1Gi".into(),
        }
    }

    fn mirror_with(project: &str, app: &str) -> Mirror {
        let mirror = Mirror::new();
        mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: "App".into(),
            metadata: ObjectMeta::new(app, project),
            spec: json!({}),
            status: None,
        });
        mirror
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
    fn only_the_two_build_labels_are_an_apps_build() {
        let of =
            |labels: &[&str]| Class::of(&labels.iter().map(|l| l.to_string()).collect::<Vec<_>>());
        assert_eq!(of(&["app-build-node"]), Some(Class::Node));
        assert_eq!(of(&["app-build-rust"]), Some(Class::Rust));
        assert_eq!(
            of(&["node-22"]),
            None,
            "the propose job stays on the shared runner"
        );
        assert_eq!(of(&[]), None);
    }

    #[test]
    fn a_repository_is_an_apps_only_when_the_mirror_holds_that_app() {
        let mirror = mirror_with("helsinki", "bikes");
        assert_eq!(
            app_of(&mirror, "helsinki_bikes"),
            Some(("helsinki".into(), "bikes".into()))
        );
        assert_eq!(app_of(&mirror, "helsinki_events"), None);
        assert_eq!(app_of(&mirror, "configuration"), None);

        mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: "App".into(),
            metadata: ObjectMeta::new("old", "helsinki"),
            spec: json!({ "lifecycle": "retired" }),
            status: None,
        });
        assert_eq!(
            app_of(&mirror, "helsinki_old"),
            None,
            "a retired App builds no more"
        );
    }

    #[test]
    fn a_job_name_is_a_dns_label_unique_per_forge_job() {
        assert_eq!(job_name("bikes", 42), "build-bikes-42");
        let long = "a".repeat(63);
        let name = job_name(&long, 9_007_199_254_740_991);
        assert!(name.len() <= 63, "{name}");
        assert!(name.ends_with("-9007199254740991"));
        assert!(!job_name("ab-", 1).contains("--"));
    }

    #[test]
    fn the_pod_mounts_its_own_apps_cache_and_nothing_else_of_another() {
        let rendered = job(&settings(), Class::Rust, "bikes", 7);
        let volumes = rendered
            .pointer("/spec/template/spec/volumes")
            .and_then(Value::as_array)
            .expect("volumes");
        let claims: Vec<&str> = volumes
            .iter()
            .filter_map(|v| v.pointer("/persistentVolumeClaim/claimName")?.as_str())
            .collect();
        assert_eq!(claims, ["build-cache-bikes"]);
        let other = job(&settings(), Class::Rust, "events", 8);
        assert!(!other.to_string().contains("build-cache-bikes"));
    }

    #[test]
    fn the_pod_keeps_the_walls_of_the_shared_runner() {
        let rendered = job(&settings(), Class::Node, "bikes", 7);
        let spec = rendered.pointer("/spec/template/spec").expect("pod spec");
        assert_eq!(spec["automountServiceAccountToken"], false);
        assert_eq!(rendered["spec"]["backoffLimit"], 0);
        assert_eq!(rendered["spec"]["activeDeadlineSeconds"], 1200);
        for container in ["initContainers", "containers"] {
            let context = &spec[container][0]["securityContext"];
            assert_eq!(context["allowPrivilegeEscalation"], false);
            assert_eq!(context["readOnlyRootFilesystem"], true);
            assert_eq!(context["capabilities"]["drop"][0], "ALL");
        }
        assert_eq!(
            spec["containers"][0]["resources"]["limits"]["memory"],
            "2Gi"
        );
        assert_eq!(
            rendered.pointer("/spec/template/metadata/labels/app.kubernetes.io~1name"),
            Some(&json!("gitea-runner")),
            "the runner's network policies select it"
        );
        // No hostPath, no runtime socket: the only volumes are these five.
        let names: Vec<&str> = spec["volumes"]
            .as_array()
            .expect("volumes")
            .iter()
            .filter_map(|v| v["name"].as_str())
            .collect();
        assert_eq!(
            names,
            ["registration", "runner-secret", "tmp", "config", "cache"]
        );
        assert!(!rendered.to_string().contains("hostPath"));
    }

    #[test]
    fn only_the_init_container_mounts_the_registration_token() {
        let rendered = job(&settings(), Class::Node, "bikes", 7);
        let spec = rendered.pointer("/spec/template/spec").expect("pod spec");
        let mounts = |container: &str| spec[container][0]["volumeMounts"].to_string();
        assert!(mounts("initContainers").contains("\"registration\""));
        assert!(!mounts("containers").contains("\"registration\""));
        assert_eq!(
            spec["containers"][0]["env"][2]["value"], "1",
            "one job, then the pod ends"
        );
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
        assert!(!running(&with(
            json!({ "conditions": [{ "type": "Failed", "status": "True" }] })
        )));
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
}
