//! Putting what [`crate::apps::reconciler::render`] compiles onto the cluster (T-0411, AP-13,
//! AP-13a, AP-18, AP-21, AP-27).
//!
//! Rendering is a pure function; this is the half with a side effect. One App manifest becomes
//! four server-side applied objects, a retired one becomes four deletions, and everything else
//! is a documented skip rather than a silent nothing. Nothing is written to the realm: the
//! login front is the edge's one `edge` client, so an app has no OIDC client of its own
//! (AP-27, ADR-N-019).
//!
//! Two properties make a second run cheap and safe. Server-side apply is idempotent by
//! construction, so an unchanged app converges to no change at the API server. And the one
//! endpoint slug is the one the App's committed Endpoint carries (T-2632), or, for an App
//! committed before its grants were, the one read back from the Secret, so a second run does not
//! move the app's endpoint (EP-02).
//!
//! One app's failure never stops another's: the loop reports per app and keeps going, because a
//! single broken manifest should not freeze every other app on the cluster.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use jc_core::kinds::{AppLifecycle, AppSpec, EndpointSlug};
use jcctl::loader::{RawManifest, Repository};
use serde_json::Value;

use std::collections::BTreeSet;

use super::kube::{KubeClient, KubeError};
use super::project_namespace;
use super::reconciler::{generate_slug, render, RenderError, Settings};

/// What no manifest names and no transfer may carry: jc-core refuses both on an `App`, and the
/// Portal's own doors refuse them on every kind (AP-11, AP-13a, T-0822). The artifact an app
/// runs is `status.build`, written back by the build lane.
pub const BUILT_ANNOTATIONS: [&str; 2] = jc_core::kinds::app::BUILT_ANNOTATIONS;

/// The slug of the Endpoint the door committed with this App (T-2632): `app-{name}` in the App's
/// project, written by the reconciler. A hand-written Endpoint of that name is not the App's.
pub fn committed_slug(repository: &Repository, app: &RawManifest) -> Option<EndpointSlug> {
    let endpoint = format!("app-{}", app.metadata.name);
    repository
        .iter()
        .map(|(_, resource)| &resource.manifest)
        .find(|manifest| {
            manifest.kind == "Endpoint"
                && manifest.metadata.name == endpoint
                && manifest.metadata.namespace == app.metadata.namespace
                && manifest
                    .metadata
                    .rest
                    .get("annotations")
                    .and_then(|annotations| annotations.get(jc_core::annotations::GENERATED_BY))
                    .and_then(Value::as_str)
                    == Some(super::reconciler::GENERATOR)
        })
        .and_then(|manifest| manifest.spec["slug"].as_str())
        .and_then(|slug| EndpointSlug::new(slug).ok())
}

/// The four objects an app owns, as the client addresses them.
const OBJECTS: [(&str, &str); 4] = [
    ("apps/v1", "Deployment"),
    ("v1", "Service"),
    ("networking.k8s.io/v1", "NetworkPolicy"),
    ("v1", "Secret"),
];

/// What one App did on one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The four objects now match the manifest.
    Applied,
    /// The app is retired and its objects are gone (AP-21).
    Deleted,
    /// Nothing was attempted, and this is why.
    Skipped(String),
    /// A project's apps namespace is in place with its policy, pull Secret and binding (AP-116).
    NamespaceReady,
    /// A project's last pod-backed App was retired, so its namespace is gone (AP-116).
    NamespaceDeleted,
}

/// Why one app could not be converged.
#[derive(Debug, thiserror::Error)]
pub enum ConvergeError {
    /// A retired wasm App's export or drop failed; what is left stays in place (AP-150).
    #[error("{0}")]
    Wasm(String),
    /// The manifest does not compile into objects.
    #[error("{0}")]
    Render(#[from] RenderError),
    /// The API server refused or could not be reached.
    #[error("{0}")]
    Kube(#[from] KubeError),
    /// The pull Secret the project's namespace needs is not in the Portal's namespace.
    #[error("the pull secret {name} is not in {namespace}, or holds no data, so no node could pull an app image (AP-108)")]
    NoPullSecret {
        /// The Secret's name.
        name: String,
        /// Where it was looked for.
        namespace: String,
    },
    /// The Secret exists and is not the one this reconciler wrote.
    #[error(
        "the secret app-{name}-endpoint exists without the key endpoint-slug, so it is not this reconciler's"
    )]
    ForeignSecret {
        /// The app whose secret it should have been.
        name: String,
    },
}

/// Applies the objects of every App in the repository.
pub struct Converger {
    kube: KubeClient,
    settings: Settings,
    /// Where a retired `wasm` App's schema and files are exported and dropped (AP-150).
    wasm: Option<std::sync::Arc<crate::apps::wasm_apps::WasmApps>>,
}

impl Converger {
    /// A converger for one installation's apps namespace.
    pub fn new(kube: KubeClient, settings: Settings) -> Self {
        Self {
            kube,
            settings,
            wasm: None,
        }
    }

    /// With the `wasm` Apps' database and bucket, so a retired one is exported and dropped.
    pub fn with_wasm_apps(
        mut self,
        wasm: Option<std::sync::Arc<crate::apps::wasm_apps::WasmApps>>,
    ) -> Self {
        self.wasm = wasm;
        self
    }

    /// Converges every App in the repository, in the loader's deterministic order.
    ///
    /// Returns one line per app, so the caller logs what happened without deciding what an
    /// outcome means.
    /// `beyond` names the apps the project may not deploy, `(project, name)`: what stands over
    /// its quota is not deployed at all and says so (PF-74).
    pub async fn converge(
        &self,
        repository: &Repository,
        beyond: &std::collections::HashSet<(String, String)>,
    ) -> Vec<(String, ConvergeResult)> {
        let mut report = Vec::new();
        // Each project with a pod-backed App gets its namespace before the Apps go into it; a
        // project whose pod-backed Apps are all retired loses it. Only an App retired in the
        // repository ends a namespace, so a repository read that comes back short never
        // removes one (AP-116).
        // ponytail: a project removed from Git with a live App keeps its namespace; a label
        // sweep against the project list is the upgrade when projects are deleted in practice.
        let (running, retired) = pod_backed_projects(repository);
        let mut ready = BTreeSet::new();
        for project in &running {
            let outcome = self.ensure_namespace(project).await;
            if matches!(outcome, Ok(Outcome::NamespaceReady)) {
                ready.insert(project.clone());
            }
            report.push((format!("namespace {project}"), outcome));
        }
        for project in retired.difference(&running) {
            report.push((
                format!("namespace {project}"),
                self.remove_namespace(project).await,
            ));
        }
        for (id, resource) in repository.iter() {
            if resource.manifest.kind != "App" {
                continue;
            }
            let project = resource
                .manifest
                .metadata
                .namespace
                .clone()
                .unwrap_or_default();
            if beyond.contains(&(project, resource.manifest.metadata.name.clone())) {
                report.push((
                    id.to_string(),
                    Ok(Outcome::Skipped(
                        "the project's app quota is used up, so this one is not deployed \
                         (PF-73, PF-74)"
                            .to_owned(),
                    )),
                ));
                continue;
            }
            let project = resource
                .manifest
                .metadata
                .namespace
                .as_deref()
                .unwrap_or_default();
            if running.contains(project) && !ready.contains(project) {
                report.push((
                    id.to_string(),
                    Ok(Outcome::Skipped(
                        "the project's apps namespace is not ready, so the app waits for it \
                         (AP-116)"
                            .to_owned(),
                    )),
                ));
                continue;
            }
            let committed = committed_slug(repository, &resource.manifest);
            let outcome = self
                .converge_with(&resource.manifest, committed.as_ref())
                .await;
            report.push((id.to_string(), outcome));
        }
        report
    }

    /// Converges one App manifest.
    pub async fn converge_one(&self, manifest: &RawManifest) -> ConvergeResult {
        self.converge_with(manifest, None).await
    }

    /// Converges one App manifest on the slug its committed Endpoint carries, when it has one:
    /// the gateway routes that slug, so the pod must call it (T-2632).
    pub async fn converge_with(
        &self,
        manifest: &RawManifest,
        committed: Option<&EndpointSlug>,
    ) -> ConvergeResult {
        let name = manifest.metadata.name.clone();
        let project = manifest
            .metadata
            .namespace
            .clone()
            .ok_or(ConvergeError::Render(RenderError::NoProject))?;
        let spec: AppSpec = match serde_json::from_value(manifest.spec.clone()) {
            Ok(spec) => spec,
            Err(err) => {
                return Err(ConvergeError::Render(RenderError::Spec(
                    jc_core::Error::Parse(err.to_string()),
                )))
            }
        };

        // A retired app is the one lifecycle that acts without rendering: there is nothing to
        // compile, only four objects to remove (AP-21).
        if spec.lifecycle == AppLifecycle::Retired {
            // A wasm App's schema and files: exported, and dropped only after (AP-150). A failed
            // export leaves everything in place, and the next sync tries again.
            if spec.class == jc_core::kinds::AppClass::Wasm {
                if let Some(wasm) = &self.wasm {
                    let at = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
                    wasm.retire(&project, &name, &at)
                        .await
                        .map_err(ConvergeError::Wasm)?;
                }
            }
            // A namespace that cannot be named held nothing of it.
            if let Ok(namespace) = self.settings.apps_namespace(&project) {
                self.delete_objects(&namespace, &name).await?;
            }
            return Ok(Outcome::Deleted);
        }
        if !matches!(
            spec.lifecycle,
            AppLifecycle::Preview | AppLifecycle::Published
        ) {
            return Ok(Outcome::Skipped(format!(
                "an app in state {} renders no pod (AP-18)",
                spec.lifecycle.as_str()
            )));
        }

        // A static app is served by the Portal's own static host, so it has no objects at all
        // and its absence here is the design, not a gap (AP-14); a wasm App's interface too, its
        // server running on the shared WASM host (AP-148).
        if matches!(
            spec.class,
            jc_core::kinds::AppClass::Ui | jc_core::kinds::AppClass::Wasm
        ) {
            return Ok(Outcome::Skipped(
                "a static app is served by the Portal, not by a pod (AP-14)".to_owned(),
            ));
        }
        // The digest the build lane wrote back in the commit that published the artifact, and
        // the only place one is read from: an annotation naming an image is refused at every
        // door now, so a manifest that still carries one deploys nothing (AP-13a, AP-72).
        let Some(digest) = built_digest(manifest) else {
            return Ok(Outcome::Skipped(
                "no status.build yet, so the build lane has not published an artifact (AP-13a)"
                    .to_owned(),
            ));
        };
        // The reference is composed from this installation's registry and the digest the
        // Portal checked on push, never read from the manifest (AP-107, AP-108).
        let Some(image) = self.settings.image_of(&name, &digest) else {
            return Ok(Outcome::Skipped(
                "no registry is configured (JC_PORTAL_APPS_REGISTRY), so no image reference can \
                 be composed (AP-108)"
                    .to_owned(),
            ));
        };

        let namespace = self.settings.apps_namespace(&project)?;
        let slug = match committed {
            Some(slug) => slug.clone(),
            None => self.slug_of(&namespace, &name).await?,
        };
        let rendered = render(manifest, Some(&image), &slug, &self.settings)?;
        let Some(workload) = rendered.workload else {
            return Ok(Outcome::Skipped(
                "a static app is served by the Portal, not by a pod (AP-14)".to_owned(),
            ));
        };

        // The Secret first: it is the slug's home between runs, and a run that wrote the pod
        // and then failed before the Secret would mint a second slug next time and move the
        // endpoint under the pod it just deployed (EP-02).
        for object in [
            &workload.secret,
            &workload.network_policy,
            &workload.service,
            &workload.deployment,
        ] {
            self.kube.apply(object).await?;
        }
        Ok(Outcome::Applied)
    }

    /// The project's apps namespace with its policy, pull Secret and binding (AP-116). Applying
    /// is idempotent, so a namespace in place costs one no-op per object.
    async fn ensure_namespace(&self, project: &str) -> ConvergeResult {
        let namespace = self.settings.apps_namespace(project)?;
        let (Some(release), Some(service_account)) = (
            self.settings.release.as_deref(),
            self.settings.service_account.as_deref(),
        ) else {
            return Ok(Outcome::Skipped(
                "no ServiceAccount is configured (JC_PORTAL_SERVICE_ACCOUNT), so the Portal cannot \
                 be bound in the project's apps namespace (AP-116)"
                    .to_owned(),
            ));
        };
        let objects = project_namespace::objects(
            project,
            &namespace,
            release,
            service_account,
            &self.settings,
        );
        for object in &objects {
            self.kube.apply(object).await?;
        }
        if let Some(pull) = self.settings.pull_secret.as_deref() {
            let copy = self
                .kube
                .get("v1", "Secret", &self.settings.namespace, pull)
                .await?
                .and_then(|source| project_namespace::pull_secret_copy(&source, pull, &namespace))
                .ok_or_else(|| ConvergeError::NoPullSecret {
                    name: pull.to_owned(),
                    namespace: self.settings.namespace.clone(),
                })?;
            self.kube.apply(&copy).await?;
        }
        Ok(Outcome::NamespaceReady)
    }

    /// Removes a project's apps namespace and everything in it; one already gone succeeds.
    async fn remove_namespace(&self, project: &str) -> ConvergeResult {
        let namespace = self.settings.apps_namespace(project)?;
        self.kube.delete("v1", "Namespace", "", &namespace).await?;
        Ok(Outcome::NamespaceDeleted)
    }

    /// The endpoint slug this app already has, or a fresh one the first time it is deployed
    /// (EP-02). Read in the project's namespace only: the Portal's own namespace holds platform
    /// credentials, and its Role names the one Secret there it reads (T-4798).
    async fn slug_of(&self, namespace: &str, name: &str) -> Result<EndpointSlug, ConvergeError> {
        let secret_name = format!("app-{name}-endpoint");
        let Some(secret) = self
            .kube
            .get("v1", "Secret", namespace, &secret_name)
            .await?
        else {
            return Ok(generate_slug());
        };
        let slug = secret_value(&secret, "endpoint-slug").ok_or(ConvergeError::ForeignSecret {
            name: name.to_owned(),
        })?;
        EndpointSlug::new(&slug).map_err(|err| ConvergeError::Render(RenderError::Slug(err)))
    }

    /// Removes the four objects of one app from one namespace; removing what is not there
    /// succeeds (CC-18).
    async fn delete_objects(&self, namespace: &str, name: &str) -> Result<(), ConvergeError> {
        for (api_version, kind) in OBJECTS {
            let object_name = match kind {
                "Secret" => format!("app-{name}-endpoint"),
                _ => format!("app-{name}"),
            };
            self.kube
                .delete(api_version, kind, namespace, &object_name)
                .await?;
        }
        Ok(())
    }
}

/// The projects with a pod-backed App that runs, and those with one that is retired (AP-116).
fn pod_backed_projects(repository: &Repository) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut running = BTreeSet::new();
    let mut retired = BTreeSet::new();
    for (_, resource) in repository.iter() {
        let manifest = &resource.manifest;
        let (Some(project), Ok(spec)) = (
            manifest.metadata.namespace.as_deref(),
            serde_json::from_value::<AppSpec>(manifest.spec.clone()),
        ) else {
            continue;
        };
        if manifest.kind != "App"
            || matches!(
                spec.class,
                jc_core::kinds::AppClass::Ui | jc_core::kinds::AppClass::Wasm
            )
        {
            continue;
        }
        match spec.lifecycle {
            AppLifecycle::Preview | AppLifecycle::Published => running.insert(project.to_owned()),
            AppLifecycle::Retired => retired.insert(project.to_owned()),
            _ => false,
        };
    }
    (running, retired)
}

/// What one app's convergence produced.
pub type ConvergeResult = Result<Outcome, ConvergeError>;

/// One annotation of a manifest, as a `String` because `RawMetadata` keeps the rest untyped.
/// The artifact the build lane published for this app, `status.build.digest` (AP-13a).
fn built_digest(manifest: &RawManifest) -> Option<String> {
    manifest
        .status
        .as_ref()?
        .get("build")?
        .get("digest")?
        .as_str()
        .map(str::to_owned)
}

/// One value of a Secret as the API server returns it: base64 in `data`, plain in `stringData`.
///
/// `stringData` is write-only in Kubernetes and never comes back, but a test double and a
/// hand-written fixture both use it, and accepting either costs one branch.
fn secret_value(secret: &Value, key: &str) -> Option<String> {
    if let Some(plain) = secret.get("stringData").and_then(|data| data.get(key)) {
        return plain.as_str().map(str::to_owned);
    }
    let encoded = secret.get("data")?.get(key)?.as_str()?;
    let bytes = STANDARD.decode(encoded).ok()?;
    String::from_utf8(bytes).ok()
}
