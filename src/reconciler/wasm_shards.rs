//! Each WASM shard's placement and store key (AP-157, AP-158, ADR-N-044).
//!
//! A shard of `jc-wasm-host` serves the Apps its placement file names and reaches the Apps bucket
//! with a key of its own. On every converge the reconciler writes, in the host's namespace, the
//! ConfigMap `jc-wasm-host-<shard>-placements` (key `placements.json`) for every shard, empty or
//! not, and the Secret `apps-host-<shard>-store` with the shard's derived key.

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde_json::{json, Value};

use crate::apps::apps_db::app_id;
use crate::apps::kube::KubeClient;
use crate::artifact_store;
use crate::resource::JobRun;

/// One published `wasm` App as the placement names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedApp {
    /// The App's project, the host's `tenant`.
    pub project: String,
    pub name: String,
    pub shard: u32,
    /// `sha256:<hex>`, the component the publish recorded in `status.build.component`.
    pub digest: String,
    /// The slug of the App's own Endpoint, the one gateway path its component may call (AP-147);
    /// `None` until the reconciler has committed that Endpoint.
    pub endpoint: Option<String>,
    /// `spec.server.jobs[]` as validation took it, which the host schedules (AP-154).
    pub jobs: Vec<jc_core::kinds::app::AppJob>,
}

impl PlacedApp {
    /// A published `wasm` App with its shard and its component recorded, from its envelope; any
    /// other App, or one still missing either, is not placed.
    pub fn of(envelope: &crate::resource::ResourceEnvelope) -> Option<Self> {
        let spec = &envelope.spec;
        let wasm = spec.get("kind").and_then(Value::as_str) == Some("wasm");
        let published = spec.get("lifecycle").and_then(Value::as_str)
            == Some(jc_core::kinds::AppLifecycle::Published.as_str());
        if envelope.kind != "App" || !wasm || !published {
            return None;
        }
        let status = envelope.status.as_ref()?;
        Some(Self {
            project: envelope.metadata.namespace.clone()?,
            name: envelope.metadata.name.clone(),
            shard: status.shard?,
            digest: status.build.as_ref()?.component.clone()?,
            endpoint: None,
            // A manifest past validation reads; one that does not schedules nothing rather than
            // keep the App off its shard.
            jobs: spec
                .get("server")
                .and_then(|server| {
                    serde_json::from_value::<jc_core::kinds::app::AppServer>(server.clone()).ok()
                })
                .map(|server| server.jobs)
                .unwrap_or_default(),
        })
    }

    /// Every published `wasm` App of `envelopes` with its shard and component recorded, each with
    /// the slug of its own Endpoint: `app-{name}` in its project, written by the App reconciler
    /// (a hand-written Endpoint of that name is not the App's, as `committed_slug` holds).
    pub fn all(envelopes: &[crate::resource::ResourceEnvelope]) -> Vec<Self> {
        let generated = |env: &crate::resource::ResourceEnvelope| {
            env.metadata
                .annotations
                .get(jc_core::annotations::GENERATED_BY)
                .map(String::as_str)
                == Some(crate::apps::reconciler::GENERATOR)
        };
        envelopes
            .iter()
            .filter_map(Self::of)
            .map(|mut app| {
                let name = format!("app-{}", app.name);
                app.endpoint = envelopes
                    .iter()
                    .find(|env| {
                        env.kind == "Endpoint"
                            && env.metadata.name == name
                            && env.metadata.namespace.as_deref() == Some(app.project.as_str())
                            && generated(env)
                    })
                    .and_then(|env| env.spec.get("slug").and_then(Value::as_str))
                    .map(str::to_owned);
                app
            })
            .collect()
    }
}

/// The placement of `shard`: its Apps by name, each once. The host routes `/apps/{name}/api` by
/// name and refuses a file that names one twice, so of two projects' Apps of one name on one
/// shard the first by project is placed and the rest are returned, to be said rather than lose
/// the whole shard.
pub fn placement(shard: u32, apps: &[PlacedApp]) -> (Value, Vec<PlacedApp>) {
    let mut chosen: BTreeMap<&str, &PlacedApp> = BTreeMap::new();
    let mut clashing = Vec::new();
    let mut on_shard: Vec<&PlacedApp> = apps.iter().filter(|app| app.shard == shard).collect();
    on_shard.sort_by(|a, b| (&a.name, &a.project).cmp(&(&b.name, &b.project)));
    for app in on_shard {
        if chosen.contains_key(app.name.as_str()) {
            clashing.push(app.clone());
        } else {
            chosen.insert(&app.name, app);
        }
    }
    let placed: Vec<Value> = chosen
        .values()
        .map(|app| {
            let mut placed = json!({
                "name": app.name,
                "id": app_id(&app.project, &app.name),
                "tenant": app.project,
                "digest": app.digest,
            });
            if let Some(slug) = &app.endpoint {
                placed["endpoint"] = json!(slug);
            }
            if !app.jobs.is_empty() {
                placed["jobs"] = json!(app.jobs);
            }
            placed
        })
        .collect();
    (
        json!({ "shard": shard.to_string(), "apps": placed }),
        clashing,
    )
}

/// The ConfigMap a shard's pods mount its placement from.
pub fn config_map(namespace: &str, shard: u32, placement: &Value) -> Value {
    json!({
        "apiVersion": "v1",
        "kind": "ConfigMap",
        "metadata": {
            "name": format!("jc-wasm-host-{shard}-placements"),
            "namespace": namespace,
            "labels": {
                "app.kubernetes.io/managed-by": "joinedcontext-portal",
                "joinedcontext.com/wasm-shard": shard.to_string(),
            },
        },
        "data": { "placements.json": placement.to_string() },
    })
}

/// A shard's `/jobs` on its admin port, which only the Portal reaches; the edge's 8080 never
/// answers it (T-3857).
fn jobs_url(shard: u32, namespace: &str) -> String {
    format!("http://jc-wasm-host-{shard}.{namespace}.svc.cluster.local:9090/jobs")
}

/// The job runs of one shard's `/jobs` answer, by App id (AP-154): `{"jobs": [{app, job, lastRun,
/// outcome, message, durationMs, failuresInARow}]}`. A row that does not read is left out.
pub fn runs_by_app(answer: &Value) -> BTreeMap<String, Vec<JobRun>> {
    let mut runs: BTreeMap<String, Vec<JobRun>> = BTreeMap::new();
    for row in answer["jobs"].as_array().into_iter().flatten() {
        let (Some(app), Some(name), Some(last_run), Some(outcome)) = (
            row["app"].as_str(),
            row["job"].as_str(),
            row["lastRun"].as_str(),
            row["outcome"].as_str(),
        ) else {
            continue;
        };
        runs.entry(app.to_owned()).or_default().push(JobRun {
            name: name.to_owned(),
            last_run: last_run.to_owned(),
            outcome: outcome.to_owned(),
            message: row["message"].as_str().map(str::to_owned),
            duration_ms: row["durationMs"].as_u64().unwrap_or(0),
            failures_in_a_row: row["failuresInARow"]
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(0),
        });
    }
    for list in runs.values_mut() {
        list.sort_by(|a, b| a.name.cmp(&b.name));
    }
    runs
}

/// Writes every shard's placement and key.
pub struct WasmShards {
    kube: KubeClient,
    namespace: String,
    shards: u32,
    bucket: String,
    http: reqwest::Client,
    /// Each shard's last `/jobs` answer, kept for a converge in which it does not answer.
    last_runs: Mutex<BTreeMap<u32, BTreeMap<String, Vec<JobRun>>>>,
}

impl WasmShards {
    pub fn new(kube: KubeClient, namespace: String, shards: u32, bucket: String) -> Self {
        Self {
            kube,
            namespace,
            shards,
            bucket,
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .unwrap_or_default(),
            last_runs: Mutex::new(BTreeMap::new()),
        }
    }

    /// Every placed App's job runs by App id, read from each shard's `GET /jobs` (AP-154). A shard
    /// that does not answer keeps its last answer; what went wrong comes back, a sentence each.
    pub async fn runs(&self) -> (BTreeMap<String, Vec<JobRun>>, Vec<String>) {
        let mut problems = Vec::new();
        for shard in 0..self.shards {
            let url = jobs_url(shard, &self.namespace);
            let answer = match self.http.get(&url).send().await {
                Ok(answer) if answer.status().is_success() => answer.json::<Value>().await.ok(),
                Ok(answer) => {
                    problems.push(format!(
                        "shard {shard}: /jobs answered HTTP {}",
                        answer.status().as_u16()
                    ));
                    None
                }
                Err(err) => {
                    problems.push(format!(
                        "shard {shard}: /jobs did not answer: {}",
                        err.without_url()
                    ));
                    None
                }
            };
            if let Some(answer) = answer {
                if let Ok(mut last) = self.last_runs.lock() {
                    last.insert(shard, runs_by_app(&answer));
                }
            }
        }
        let all = self
            .last_runs
            .lock()
            .map(|last| last.values().flat_map(|runs| runs.clone()).collect())
            .unwrap_or_default();
        (all, problems)
    }

    /// One converge: each shard's placement of `apps`, and, where `store` is configured, its
    /// key. Returns what went wrong, one sentence each; a shard that failed keeps what it had.
    pub async fn converge(
        &self,
        apps: &[PlacedApp],
        store: Option<&artifact_store::Client>,
    ) -> Vec<String> {
        let mut problems = Vec::new();
        for shard in 0..self.shards {
            let (placement, clashing) = placement(shard, apps);
            for app in clashing {
                problems.push(format!(
                    "shard {shard}: App {}/{} is not served, another project's App of the same name is",
                    app.project, app.name
                ));
            }
            if let Err(err) = self
                .kube
                .apply_placement(&config_map(&self.namespace, shard, &placement))
                .await
            {
                problems.push(format!(
                    "shard {shard}: its placement was not written: {err}"
                ));
            }
            let Some(store) = store else {
                continue;
            };
            match store.ensure_shard(&self.bucket, shard).await {
                Ok(key) => {
                    let secret = artifact_store::shard_secret(&self.namespace, shard, &key);
                    if let Err(err) = self.kube.apply(&secret).await {
                        problems.push(format!(
                            "shard {shard}: its store key was not handed over: {err}"
                        ));
                    }
                }
                Err(err) => {
                    problems.push(format!("shard {shard}: the store refused its key: {err}"))
                }
            }
        }
        problems
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_records_are_read_on_the_admin_port() {
        assert_eq!(
            jobs_url(1, "wasm-host"),
            "http://jc-wasm-host-1.wasm-host.svc.cluster.local:9090/jobs"
        );
    }

    fn app(project: &str, name: &str, shard: u32) -> PlacedApp {
        PlacedApp {
            project: project.to_owned(),
            name: name.to_owned(),
            shard,
            digest: format!("sha256:{}", "a".repeat(64)),
            endpoint: None,
            jobs: Vec::new(),
        }
    }

    /// AP-154 (T-3372): the host schedules what the placement carries, so an App's jobs reach
    /// it as `{name, schedule, export}`; an App without jobs carries no key.
    /// AP-154: a shard's `/jobs` rows become `status.jobs[]` per App, sorted by job; a row
    /// missing a member is left out rather than guessed.
    #[test]
    fn a_shards_job_records_read_as_status_jobs_by_app() {
        let runs = runs_by_app(&json!({"jobs": [
            {"app": "a1", "job": "nightly", "lastRun": "2026-10-10T02:00:00Z", "outcome": "failed",
             "message": "403 from the Endpoint", "durationMs": 120, "failuresInARow": 3},
            {"app": "a1", "job": "hourly", "lastRun": "2026-10-10T14:00:00Z", "outcome": "succeeded",
             "message": null, "durationMs": 40, "failuresInARow": 0},
            {"app": "a2", "job": "x", "outcome": "succeeded"}
        ]}));
        assert_eq!(runs.len(), 1, "the row without lastRun is left out");
        let a1 = &runs["a1"];
        assert_eq!(a1[0].name, "hourly");
        assert_eq!(a1[0].message, None);
        assert_eq!(a1[1].outcome, "failed");
        assert_eq!(a1[1].message.as_deref(), Some("403 from the Endpoint"));
        assert_eq!(a1[1].failures_in_a_row, 3);
        assert!(runs_by_app(&json!({})).is_empty());
    }

    #[test]
    fn a_placed_app_carries_its_jobs_to_the_host() {
        let mut envelope = wasm_app("helsinki", "kpi");
        envelope.spec["server"] = json!({"jobs": [
            {"name": "hourly", "schedule": "0 * * * *", "export": "compute-kpis"}
        ]});
        let placed = PlacedApp::of(&envelope).expect("placed");
        let (zero, _) = placement(0, &[placed, app("helsinki", "notes", 0)]);
        assert_eq!(
            zero["apps"][0]["jobs"],
            json!([{"name": "hourly", "schedule": "0 * * * *", "export": "compute-kpis"}])
        );
        assert!(zero["apps"][1].get("jobs").is_none());
    }

    #[test]
    fn a_shard_names_its_own_apps_only_by_name_and_id() {
        let apps = [
            app("helsinki", "notes", 0),
            app("praha", "odpad", 1),
            app("helsinki", "air", 0),
        ];
        let (zero, clashing) = placement(0, &apps);
        assert!(clashing.is_empty());
        assert_eq!(zero["shard"], "0");
        let names: Vec<&str> = zero["apps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["air", "notes"]);
        assert_eq!(zero["apps"][1]["id"], app_id("helsinki", "notes"));
        assert_eq!(zero["apps"][1]["tenant"], "helsinki");
        let (two, _) = placement(2, &apps);
        assert_eq!(two["apps"], json!([]));
    }

    #[test]
    fn two_projects_apps_of_one_name_on_one_shard_place_the_first_and_say_the_other() {
        let (zero, clashing) =
            placement(0, &[app("praha", "notes", 0), app("helsinki", "notes", 0)]);
        assert_eq!(zero["apps"].as_array().unwrap().len(), 1);
        assert_eq!(zero["apps"][0]["tenant"], "helsinki");
        assert_eq!(clashing, [app("praha", "notes", 0)]);
    }

    fn envelope(value: Value) -> crate::resource::ResourceEnvelope {
        serde_json::from_value(value).expect("envelope")
    }

    fn wasm_app(project: &str, name: &str) -> crate::resource::ResourceEnvelope {
        envelope(json!({
            "apiVersion": "joinedcontext.com/v1alpha1", "kind": "App",
            "metadata": {"name": name, "namespace": project},
            "spec": {"kind": "wasm", "lifecycle": "published"},
            "status": {"shard": 0, "build": {
                "digest": format!("sha256:{}", "b".repeat(64)), "commit": "c0ffee",
                "sdkVersion": "1.0.0", "builtAt": "2026-10-10T00:00:00Z",
                "component": format!("sha256:{}", "a".repeat(64))}}
        }))
    }

    fn endpoint(
        project: &str,
        name: &str,
        slug: &str,
        generated: bool,
    ) -> crate::resource::ResourceEnvelope {
        let annotations = if generated {
            json!({ jc_core::annotations::GENERATED_BY: crate::apps::reconciler::GENERATOR })
        } else {
            json!({})
        };
        envelope(json!({
            "apiVersion": "joinedcontext.com/v1alpha1", "kind": "Endpoint",
            "metadata": {"name": name, "namespace": project, "annotations": annotations},
            "spec": {"slug": slug}
        }))
    }

    #[test]
    fn a_placed_app_names_its_own_endpoint_and_no_other() {
        let apps = PlacedApp::all(&[
            wasm_app("helsinki", "notes"),
            endpoint("helsinki", "app-notes", "ab12", true),
            // Another project's Endpoint of the same name, and a hand-written one, are not its.
            endpoint("praha", "app-notes", "zz99", true),
            wasm_app("helsinki", "air"),
            endpoint("helsinki", "app-air", "cd34", false),
        ]);
        let slug = |name: &str| {
            apps.iter()
                .find(|a| a.name == name)
                .and_then(|a| a.endpoint.clone())
        };
        assert_eq!(slug("notes").as_deref(), Some("ab12"));
        assert_eq!(slug("air"), None);
        let (zero, _) = placement(0, &apps);
        assert_eq!(zero["apps"][1]["endpoint"], "ab12");
        assert!(
            zero["apps"][0].get("endpoint").is_none(),
            "no slug, no key: the host refuses its calls"
        );
    }

    #[test]
    fn the_config_map_carries_the_placement_under_its_key() {
        let (placed, _) = placement(1, &[app("helsinki", "notes", 1)]);
        let map = config_map("apps-host", 1, &placed);
        assert_eq!(map["metadata"]["name"], "jc-wasm-host-1-placements");
        assert_eq!(map["metadata"]["namespace"], "apps-host");
        let text = map["data"]["placements.json"].as_str().unwrap();
        assert_eq!(serde_json::from_str::<Value>(text).unwrap(), placed);
    }
}
