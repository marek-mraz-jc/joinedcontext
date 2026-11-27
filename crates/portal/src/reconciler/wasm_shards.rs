//! Each WASM shard's placement and store key (AP-157, AP-158, ADR-N-044).
//!
//! A shard of `jc-wasm-host` serves the Apps its placement file names and reaches the Apps bucket
//! with a key of its own. On every converge the reconciler writes, in the host's namespace, the
//! ConfigMap `jc-wasm-host-<shard>-placements` (key `placements.json`) for every shard, empty or
//! not, and the Secret `apps-host-<shard>-store` with the shard's derived key.
//!
//! Each placed App's objects are under its project's `org/<OrgId>/project/<ProjectId>/apps/<id>/`
//! (ARCH/STO-14); its files from before the key are moved there from `apps/<shard>/<id>/` on the
//! converge (TEN-15, [`crate::apps::files::move_legacy`]), and the placement says `legacy` until the
//! old prefix is empty. The shard's key reaches exactly the prefixes its placement names.

use std::collections::{BTreeMap, BTreeSet};
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
    /// `org/<OrgId>/project/<ProjectId>/apps/<id>/` once its project's key is read; `None` keeps
    /// the App under `apps/<shard>/<id>/`.
    pub prefix: Option<String>,
    /// `apps/<shard>/<id>/` still holds files not yet moved under `prefix` (TEN-15).
    pub legacy: bool,
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
            prefix: None,
            legacy: false,
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
            if let Some(prefix) = &app.prefix {
                placed["prefix"] = json!(prefix);
                placed["legacy"] = json!(app.legacy);
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
/// The object prefixes a shard's key reaches: each App of its `placement`, under its key and, while
/// it is `legacy` or has no key, under `apps/<shard>/<id>/`. Nothing else, not even the whole
/// `apps/<shard>/`: a prefix no placed App holds is no shard's.
pub fn prefixes(shard: u32, placement: &Value) -> Vec<String> {
    let mut prefixes = Vec::new();
    for app in placement["apps"].as_array().into_iter().flatten() {
        let Some(id) = app["id"].as_str() else {
            continue;
        };
        if let Some(prefix) = app["prefix"].as_str() {
            prefixes.push(prefix.to_owned());
        }
        if app["prefix"].is_null() || app["legacy"] == json!(true) {
            prefixes.push(crate::apps::files::legacy_prefix(shard, id));
        }
    }
    prefixes
}

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
    /// The Portal's database, whose project keys name each App's prefix (ARCH/STO-14).
    keys: Option<sqlx::PgPool>,
    /// Apps whose old prefix was found empty: the store is the record of a finished move, and
    /// this only spares the listing on every converge until the Portal restarts.
    moved: Mutex<BTreeSet<String>>,
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
            keys: None,
            moved: Mutex::new(BTreeSet::new()),
        }
    }

    /// Reads each App's project key from `pool`, so its objects are under it.
    pub fn with_keys(mut self, pool: sqlx::PgPool) -> Self {
        self.keys = Some(pool);
        self
    }

    /// Each App with its keyed prefix, and its old files moved there where `store` is
    /// configured; what went wrong comes back, a sentence each. An App whose project key cannot
    /// be read stays where it was, and one whose move stopped keeps reading its old prefix.
    pub async fn keyed(
        &self,
        apps: &[PlacedApp],
        store: Option<&artifact_store::Client>,
    ) -> (Vec<PlacedApp>, Vec<String>) {
        let mut problems = Vec::new();
        let mut keyed = Vec::with_capacity(apps.len());
        for app in apps {
            let mut app = app.clone();
            let key = match &self.keys {
                Some(pool) => crate::project_ids::recorded(pool, &app.project).await,
                None => Ok(None),
            };
            let key = match key {
                Ok(key) => key,
                Err(err) => {
                    problems.push(format!(
                        "App {}/{}: its project key was not read: {err}",
                        app.project, app.name
                    ));
                    None
                }
            };
            if let Some(key) = key {
                let id = app_id(&app.project, &app.name);
                let prefix = crate::apps::files::keyed_prefix(&key, &id);
                let moved = self.moved.lock().is_ok_and(|moved| moved.contains(&id));
                app.legacy = !moved;
                if let (false, Some(store)) = (moved, store) {
                    let legacy = crate::apps::files::legacy_prefix(app.shard, &id);
                    let at = jc_time::TimestampTz::now()
                        .0
                        .strftime("%Y%m%dT%H%M%SZ")
                        .to_string();
                    match crate::apps::files::move_legacy(
                        store,
                        &self.bucket,
                        (&id, &legacy, &prefix),
                        &at,
                    )
                    .await
                    {
                        Ok(done) => {
                            if done.copied + done.superseded > 0 {
                                tracing::info!(app = %id, copied = done.copied, superseded = done.superseded, "an App's files moved under its project key");
                            }
                            if done.done {
                                app.legacy = false;
                                if let Ok(mut moved) = self.moved.lock() {
                                    moved.insert(id);
                                }
                            }
                        }
                        Err(why) => problems.push(format!(
                            "App {}/{}: its files were not moved under its project key: {why}",
                            app.project, app.name
                        )),
                    }
                }
                app.prefix = Some(prefix);
            }
            keyed.push(app);
        }
        (keyed, problems)
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
        let (apps, mut problems) = self.keyed(apps, store).await;
        for shard in 0..self.shards {
            let (placement, clashing) = placement(shard, &apps);
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
            match store
                .ensure_shard(&self.bucket, shard, &prefixes(shard, &placement))
                .await
            {
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
            prefix: None,
            legacy: false,
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

    /// Only a published `wasm` App with a namespace, a shard and a recorded component is placed;
    /// a `server` that does not read schedules nothing and keeps the App on its shard.
    #[test]
    fn an_app_missing_any_part_of_its_placement_is_not_placed() {
        let base = serde_json::to_value(wasm_app("helsinki", "notes")).expect("a value");
        assert!(PlacedApp::of(&envelope(base.clone())).is_some());
        let without = |edit: &dyn Fn(&mut Value)| {
            let mut value = base.clone();
            edit(&mut value);
            PlacedApp::of(&envelope(value))
        };
        assert_eq!(without(&|v| v["kind"] = json!("Endpoint")), None);
        assert_eq!(without(&|v| v["spec"]["kind"] = json!("static")), None);
        assert_eq!(without(&|v| v["spec"]["lifecycle"] = json!("draft")), None);
        assert_eq!(without(&|v| v["status"] = Value::Null), None);
        assert_eq!(
            without(&|v| {
                v["metadata"].as_object_mut().map(|m| m.remove("namespace"));
            }),
            None
        );
        assert_eq!(
            without(&|v| {
                v["status"].as_object_mut().map(|m| m.remove("shard"));
            }),
            None
        );
        assert_eq!(
            without(&|v| {
                v["status"]["build"]
                    .as_object_mut()
                    .map(|m| m.remove("component"));
            }),
            None
        );
        assert_eq!(
            without(&|v| {
                v["status"].as_object_mut().map(|m| m.remove("build"));
            }),
            None
        );
        let odd =
            without(&|v| v["spec"]["server"] = json!({ "jobs": "hourly" })).expect("still placed");
        assert!(odd.jobs.is_empty());
    }

    /// AP-158: a shard's key reaches each placed App's keyed prefix, and its old one only while
    /// files wait there or the App has no key; an entry without an id reaches nothing.
    #[test]
    fn a_shards_key_reaches_exactly_the_prefixes_its_placement_names() {
        let id = |name: &str| app_id("helsinki", name);
        let placed = json!({ "apps": [
            { "id": id("moved"), "prefix": "org/o/project/p/apps/moved/", "legacy": false },
            { "id": id("moving"), "prefix": "org/o/project/p/apps/moving/", "legacy": true },
            { "id": id("unkeyed") },
            { "name": "no-id", "prefix": "org/o/project/p/apps/x/" }
        ]});
        assert_eq!(
            prefixes(3, &placed),
            [
                "org/o/project/p/apps/moved/".to_owned(),
                "org/o/project/p/apps/moving/".to_owned(),
                crate::apps::files::legacy_prefix(3, &id("moving")),
                crate::apps::files::legacy_prefix(3, &id("unkeyed")),
            ]
        );
        assert!(prefixes(0, &json!({})).is_empty());
        let mut keyed = app("helsinki", "moving", 0);
        keyed.prefix = Some("org/o/project/p/apps/moving/".into());
        keyed.legacy = true;
        let (placed, _) = placement(0, &[keyed]);
        assert_eq!(placed["apps"][0]["legacy"], json!(true));
        assert_eq!(
            placed["apps"][0]["prefix"],
            json!("org/o/project/p/apps/moving/")
        );
    }

    fn shards(kube: &str) -> WasmShards {
        WasmShards::new(
            KubeClient::with_token(kube, "t").expect("a client"),
            "apps-host".into(),
            2,
            "jc-apps".into(),
        )
    }

    fn store_on(server: &wiremock::MockServer) -> artifact_store::Client {
        artifact_store::Client::new(artifact_store::Settings {
            endpoint: server.uri(),
            bucket: "jc-artifacts".into(),
            region: "us-east-1".into(),
            root_access_key: "root".into(),
            root_secret_key: "the-root-secret".into(),
        })
        .expect("a store client")
    }

    /// Without the Portal's database no App has a key: each stays under its old prefix. A
    /// database that does not answer says so per App and keeps it there.
    #[tokio::test]
    async fn an_app_whose_project_key_is_not_read_stays_where_it_was() {
        let apps = [app("helsinki", "notes", 0)];
        let (kept, problems) = shards("http://127.0.0.1:9").keyed(&apps, None).await;
        assert_eq!((kept.as_slice(), problems.len()), (apps.as_slice(), 0));

        let closed = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
        let port = closed.local_addr().expect("its address").port();
        drop(closed);
        let pool = sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(std::time::Duration::from_secs(2))
            .connect_lazy(&format!("postgres://u:p@127.0.0.1:{port}/db"))
            .expect("a lazy pool");
        let with_keys = shards("http://127.0.0.1:9").with_keys(pool);
        let (kept, problems) = with_keys.keyed(&apps, None).await;
        assert_eq!(kept, apps);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(
            problems[0].starts_with("App helsinki/notes: its project key was not read: "),
            "{}",
            problems[0]
        );
    }

    /// TEN-15: a keyed App's old prefix that holds nothing finishes its move at once and is not
    /// listed again; one the store will not list keeps the App reading it.
    #[tokio::test]
    async fn a_keyed_app_moves_once_and_a_store_that_fails_keeps_its_old_prefix() {
        use wiremock::matchers::{method, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let url = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
            .ok()
            .filter(|url| !url.trim().is_empty())
            .expect("set JC_PORTAL_TEST_DATABASE_URL to a PostgreSQL database the Portal migrates");
        let db = crate::db::connect(&url).await.expect("connect and migrate");
        let mirror = crate::store::Mirror::new();
        let run = jc_types::ProjectId::mint().path_segment();
        let (project, org) = (format!("t7650-{run}"), format!("t7650org-{run}"));
        crate::project_ids::placed(&db, &mirror, &project, &org).await;
        let key = crate::project_ids::key(&db, &org, &project)
            .await
            .expect("the registry")
            .expect("a key");
        let id = app_id(&project, "notes");
        let legacy = crate::apps::files::legacy_prefix(0, &id);

        let store = MockServer::start().await;
        Mock::given(method("GET"))
            .and(query_param("prefix", legacy.as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                "<ListBucketResult><IsTruncated>false</IsTruncated></ListBucketResult>",
            ))
            .expect(1)
            .mount(&store)
            .await;
        let client = store_on(&store);
        let wasm = shards("http://127.0.0.1:9").with_keys(db.clone());
        let apps = [app(&project, "notes", 0), app(&project, "air", 0)];
        let (keyed, problems) = wasm.keyed(&apps[..1], Some(&client)).await;
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(
            keyed[0].prefix.as_deref(),
            Some(crate::apps::files::keyed_prefix(&key, &id).as_str())
        );
        assert!(!keyed[0].legacy, "nothing is left under the old prefix");
        let (again, _) = wasm.keyed(&apps[..1], Some(&client)).await;
        assert!(!again[0].legacy, "the finished move is remembered");
        store.verify().await;

        // Without a store the move waits: the App reads both prefixes.
        let (waiting, _) = wasm.keyed(&apps[1..], None).await;
        assert!(waiting[0].legacy);
        assert!(waiting[0].prefix.is_some());

        store.reset().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&store)
            .await;
        let (failed, problems) = wasm.keyed(&apps[1..], Some(&client)).await;
        assert!(failed[0].legacy, "the old prefix is still read");
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(
            problems[0].starts_with(&format!(
                "App {project}/air: its files were not moved under its project key: "
            )),
            "{}",
            problems[0]
        );
    }

    /// Poisons `lock`: a holder panics while holding it.
    fn poison<T>(lock: &Mutex<T>) {
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _held = lock.lock().expect("not poisoned yet");
            panic!("T-7924: a holder of the lock panics");
        }));
        assert!(caught.is_err());
        assert!(lock.is_poisoned());
    }

    /// T-7924: with the record of each shard's last answer poisoned, an answer is not kept and
    /// nothing is answered from it: the runs come back empty, and no shard is said to have failed.
    #[tokio::test]
    async fn a_poisoned_record_of_the_last_runs_answers_none() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let hosts = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/jobs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "jobs": [
                { "app": "a1", "job": "hourly", "lastRun": "2026-10-10T00:00:00Z", "outcome": "ok" }
            ]})))
            .expect(2)
            .mount(&hosts)
            .await;
        let mut wasm = shards("http://127.0.0.1:9");
        wasm.http = reqwest::Client::builder()
            .proxy(reqwest::Proxy::http(hosts.uri()).expect("a proxy"))
            .build()
            .expect("a client");
        poison(&wasm.last_runs);
        let (runs, problems) = wasm.runs().await;
        assert!(runs.is_empty(), "{runs:?}");
        assert!(problems.is_empty(), "{problems:?}");
        hosts.verify().await;
    }

    /// AP-154: each shard's `/jobs` on its admin port; a shard that answers an error or does not
    /// answer is said, and keeps the runs it answered before.
    #[tokio::test]
    async fn the_job_runs_of_every_shard_are_read_and_a_silent_shard_keeps_its_last() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let hosts = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/jobs"))
            .and(header(
                "host",
                "jc-wasm-host-0.apps-host.svc.cluster.local:9090",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "jobs": [
                { "app": "a1", "job": "hourly", "lastRun": "2026-10-10T00:00:00Z", "outcome": "ok" }
            ]})))
            .mount(&hosts)
            .await;
        Mock::given(method("GET"))
            .and(path("/jobs"))
            .and(header(
                "host",
                "jc-wasm-host-1.apps-host.svc.cluster.local:9090",
            ))
            .respond_with(ResponseTemplate::new(502))
            .mount(&hosts)
            .await;
        let through = |proxy: &str| {
            reqwest::Client::builder()
                .proxy(reqwest::Proxy::http(proxy).expect("a proxy"))
                .build()
                .expect("a client")
        };
        let mut wasm = shards("http://127.0.0.1:9");
        wasm.http = through(&hosts.uri());
        let (runs, problems) = wasm.runs().await;
        assert_eq!(runs.keys().map(String::as_str).collect::<Vec<_>>(), ["a1"]);
        assert_eq!(runs["a1"][0].outcome, "ok");
        assert_eq!(problems, ["shard 1: /jobs answered HTTP 502"]);

        let closed = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
        let gone = format!("http://{}", closed.local_addr().expect("its address"));
        drop(closed);
        wasm.http = through(&gone);
        let (kept, problems) = wasm.runs().await;
        assert_eq!(kept, runs, "the last answer stands");
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(
            problems[0].starts_with("shard 0: /jobs did not answer: "),
            "{}",
            problems[0]
        );
    }

    /// AP-157, AP-158: every shard gets its placement and, with a store, its key; each refusal is
    /// said once with its shard, and a name two projects share is said rather than served twice.
    #[tokio::test]
    async fn a_converge_writes_each_shards_placement_and_key_and_says_what_failed() {
        use wiremock::matchers::{method, path, path_regex};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let kube = MockServer::start().await;
        Mock::given(method("PATCH"))
            .and(path(
                "/api/v1/namespaces/apps-host/configmaps/jc-wasm-host-0-placements",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(3)
            .mount(&kube)
            .await;
        Mock::given(method("PATCH"))
            .and(path(
                "/api/v1/namespaces/apps-host/configmaps/jc-wasm-host-1-placements",
            ))
            .respond_with(ResponseTemplate::new(403).set_body_json(json!({})))
            .mount(&kube)
            .await;
        Mock::given(method("PATCH"))
            .and(path_regex("/secrets/apps-host-0-store$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(1)
            .mount(&kube)
            .await;
        Mock::given(method("PATCH"))
            .and(path_regex("/secrets/apps-host-1-store$"))
            .respond_with(ResponseTemplate::new(500).set_body_json(json!({})))
            .mount(&kube)
            .await;
        let store = MockServer::start().await;
        Mock::given(method("PUT"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&store)
            .await;
        let wasm = shards(&kube.uri());
        let apps = [
            app("helsinki", "notes", 0),
            app("praha", "notes", 0),
            app("helsinki", "air", 1),
        ];
        let problems = wasm.converge(&apps, Some(&store_on(&store))).await;
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert_eq!(
            problems[0],
            "shard 0: App praha/notes is not served, another project's App of the same name is"
        );
        assert!(
            problems[1].starts_with("shard 1: its placement was not written: "),
            "{}",
            problems[1]
        );
        assert!(
            problems[2].starts_with("shard 1: its store key was not handed over: "),
            "{}",
            problems[2]
        );

        // A store that refuses the key is said per shard; without one nothing asks it.
        store.reset().await;
        Mock::given(method("PUT"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&store)
            .await;
        let refused = wasm.converge(&apps[..1], Some(&store_on(&store))).await;
        assert!(
            refused
                .iter()
                .any(|p| p.starts_with("shard 0: the store refused its key: ")),
            "{refused:?}"
        );
        let quiet = wasm.converge(&apps[..1], None).await;
        assert_eq!(
            quiet.iter().filter(|p| p.contains("store")).count(),
            0,
            "{quiet:?}"
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
