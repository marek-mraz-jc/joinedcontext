//! The assistant keeps an indicator up to date (AG-74, PL-45, PL-51, PF-54): from "keep the
//! average of free bikes updated every 15 minutes in transportation-kpi" to a `Pipeline` that
//! reads the source Endpoint and writes one `KeyPerformanceIndicator` per run into an indicator
//! space, on a period or on every change of a watched value. When that space does not exist yet,
//! its `ContextSpace`, `Endpoint` and the two `Policy` manifests are drafted beside it. Nothing
//! here writes or proposes: the driver keeps the manifests as drafts and opens the form.

use jc_core::kpi::KPI_SPACE_SUFFIX;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agents::kpi::{self, Agg, ComputeKpi};
use crate::agents::share::TOOL_FENCE;
use crate::resource::{is_dns1123, API_VERSION};

/// The shortest period a scheduled indicator takes; anything faster is a change trigger.
const MIN_EVERY_SECONDS: u64 = 60;

/// The model's call: `{"tool": "draft_kpi_pipeline", ...}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftKpiPipeline {
    /// The indicator's name: the `{localId}` of its URN and the pipeline's name.
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The entity type the value is computed over.
    #[serde(rename = "type", default)]
    pub entity_type: String,
    /// The attribute folded; ignored by `count`.
    #[serde(default)]
    pub attribute: String,
    pub agg: Agg,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    /// The Endpoint read; the run's own endpoint when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_endpoint: Option<String>,
    /// The indicator space written; `{project}-kpi` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_space: Option<String>,
    /// A period of at least a minute: `15m`, `1h`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every: Option<String>,
    /// Recompute on every change of a watched value instead of on a period.
    #[serde(default)]
    pub on_change: bool,
    /// The attributes whose change recomputes; the folded attribute when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub watched_attributes: Vec<String>,
}

/// The tool call in a model answer, when the answer is one.
pub fn tool_call(answer: &str) -> Option<Result<DraftKpiPipeline, String>> {
    for fence in TOOL_FENCE.captures_iter(answer) {
        let Ok(value) = serde_json::from_str::<Value>(&fence[1]) else {
            continue;
        };
        if value.get("tool").and_then(Value::as_str) != Some("draft_kpi_pipeline") {
            continue;
        }
        return Some(
            serde_json::from_value::<DraftKpiPipeline>(value)
                .map_err(|err| format!("the draft_kpi_pipeline call does not parse: {err}")),
        );
    }
    None
}

/// When the pipeline runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trigger {
    /// A period, as a Bento duration, of at least a minute.
    Every { period: String, seconds: u64 },
    /// Every change of these attributes.
    OnChange(Vec<String>),
}

impl Trigger {
    /// `every 15m`, `on every change of availableBikeNumber`: the words the chat uses.
    pub fn describe(&self) -> String {
        match self {
            Self::Every { period, .. } => format!("every {period}"),
            Self::OnChange(watched) => format!("on every change of {}", watched.join(", ")),
        }
    }
}

/// `15m`, `1h`, `90s`, `1d` in seconds; anything else is not a period.
fn seconds_of(period: &str) -> Option<u64> {
    let period = period.trim();
    let split = period.find(|c: char| !c.is_ascii_digit())?;
    let (number, unit) = period.split_at(split);
    let number: u64 = number.parse().ok()?;
    match unit {
        "s" => Some(number),
        "m" => Some(number * 60),
        "h" => Some(number * 3600),
        "d" => Some(number * 86_400),
        _ => None,
    }
}

/// The ISO 8601 duration of a period, for `calculationPeriod`.
fn iso_duration(seconds: u64) -> String {
    if seconds.is_multiple_of(3600) {
        format!("PT{}H", seconds / 3600)
    } else if seconds.is_multiple_of(60) {
        format!("PT{}M", seconds / 60)
    } else {
        format!("PT{seconds}S")
    }
}

/// The trigger the call asks for: exactly one of `every` and `onChange`.
pub fn trigger_of(call: &DraftKpiPipeline) -> Result<Trigger, String> {
    let every = call
        .every
        .as_deref()
        .map(str::trim)
        .filter(|e| !e.is_empty());
    match (every, call.on_change) {
        (Some(_), true) => Err("give either every or onChange, not both".to_owned()),
        (None, false) => Err(
            "say when the indicator is recomputed: every (a period such as 15m) or onChange"
                .to_owned(),
        ),
        (Some(period), false) => match seconds_of(period) {
            // A daily indicator asked as `1d` runs as `24h`: the runner's durations stop at hours.
            Some(seconds) if seconds >= MIN_EVERY_SECONDS => Ok(Trigger::Every {
                period: if period.ends_with('d') {
                    format!("{}h", seconds / 3600)
                } else {
                    period.to_owned()
                },
                seconds,
            }),
            Some(_) => Err(format!(
                "every '{period}' is under a minute; recompute on every change instead"
            )),
            None => Err(format!(
                "every '{period}' is not a period such as 15m or 1h"
            )),
        },
        (None, true) => {
            let mut watched: Vec<String> = call
                .watched_attributes
                .iter()
                .map(|a| a.trim().to_owned())
                .filter(|a| !a.is_empty())
                .collect();
            if watched.is_empty() && !call.attribute.trim().is_empty() {
                watched.push(call.attribute.trim().to_owned());
            }
            if watched.is_empty() {
                return Err(
                    "a count on every change names the attributes to watch (watchedAttributes)"
                        .to_owned(),
                );
            }
            Ok(Trigger::OnChange(watched))
        }
    }
}

/// The indicator space a request names, as the admission check takes it (PF-54): the project's
/// `{project}-kpi` when none is named; `-kpis` read as `-kpi`; the suffix added when missing.
pub fn target_space(project: &str, requested: Option<&str>) -> Result<String, String> {
    let requested = requested.map(str::trim).filter(|s| !s.is_empty());
    let Some(requested) = requested else {
        return Ok(format!("{project}{KPI_SPACE_SUFFIX}"));
    };
    let lower = requested.to_ascii_lowercase();
    let space = if let Some(stem) = lower.strip_suffix("-kpis") {
        format!("{stem}{KPI_SPACE_SUFFIX}")
    } else if lower.ends_with(KPI_SPACE_SUFFIX) {
        lower
    } else {
        format!("{lower}{KPI_SPACE_SUFFIX}")
    };
    if !is_dns1123(&space) {
        return Err(format!("'{requested}' does not make a context space name"));
    }
    Ok(space)
}

/// What the project holds that the plan reads: its endpoints and spaces as manifests, its
/// policies, drafts included.
pub struct World<'a> {
    pub project: &'a str,
    pub org_domain: &'a str,
    pub endpoints: &'a [Value],
    pub spaces: &'a [String],
    pub policies: &'a [Value],
    /// The name of the Endpoint the run reads through, when it has one.
    pub run_endpoint: Option<&'a str>,
    /// A slug for an Endpoint the plan drafts.
    pub new_slug: &'a str,
}

/// One manifest the plan drafts, for the driver to keep and the card to list.
#[derive(Debug, Clone, Serialize)]
pub struct Drafted {
    pub kind: String,
    pub name: String,
    pub plural: String,
    pub manifest: Value,
}

/// The pipeline, what it reads and writes, and the manifests of an indicator space that is new.
#[derive(Debug, Clone)]
pub struct Plan {
    pub pipeline: Value,
    pub trigger: Trigger,
    pub formula: String,
    pub source_endpoint: String,
    pub source_space: String,
    pub target_space: String,
    pub target_endpoint: String,
    pub target_slug: Option<String>,
    /// The indicator space's manifests the project does not have yet, in proposing order.
    pub space_drafts: Vec<Drafted>,
    /// The form of the pipelines page, filled in (UI-45).
    pub prefill: Value,
}

fn space_of(endpoint: &Value) -> Option<String> {
    crate::api::assistant::ref_name(&endpoint["spec"]["contextSpaceRef"])
}

fn name_of(manifest: &Value) -> &str {
    manifest["metadata"]["name"].as_str().unwrap_or_default()
}

/// `transportation-kpi` → `Transportation indicators`.
fn space_title(space: &str) -> String {
    let stem = space.strip_suffix(KPI_SPACE_SUFFIX).unwrap_or(space);
    let words = stem.replace('-', " ");
    let mut chars = words.chars();
    match chars.next() {
        Some(first) => format!("{}{} indicators", first.to_uppercase(), chars.as_str()),
        None => "Indicators".to_owned(),
    }
}

/// Validates the call against the project and renders the manifests.
pub fn plan(call: &DraftKpiPipeline, world: &World) -> Result<Plan, String> {
    let name = call.name.trim();
    if !is_dns1123(name) {
        return Err(format!(
            "'{name}' is not an indicator name: lowercase letters, digits and dashes"
        ));
    }
    let entity_type = call.entity_type.trim();
    if entity_type.is_empty() {
        return Err("name the entity type the indicator is computed over".to_owned());
    }
    let attribute = call.attribute.trim();
    if call.agg != Agg::Count && attribute.is_empty() {
        return Err("name the attribute the indicator folds".to_owned());
    }
    let trigger = trigger_of(call)?;

    let source_name = match call
        .source_endpoint
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(named) => named.to_owned(),
        None => world
            .run_endpoint
            .map(str::to_owned)
            .ok_or("name the endpoint the indicator reads (sourceEndpoint)")?,
    };
    let source = world
        .endpoints
        .iter()
        .find(|e| name_of(e) == source_name)
        .ok_or_else(|| {
            let names: Vec<&str> = world.endpoints.iter().map(name_of).collect();
            format!(
                "the project has no endpoint '{source_name}'; its endpoints are: {}",
                names.join(", ")
            )
        })?;
    let source_space = space_of(source).unwrap_or_else(|| world.project.to_owned());

    let target = target_space(world.project, call.target_space.as_deref())?;
    if target == source_space {
        return Err(format!(
            "'{target}' is the space the indicator reads; an indicator is written into its own space"
        ));
    }
    let space_exists = world.spaces.contains(&target);
    let target_endpoint = world
        .endpoints
        .iter()
        .find(|e| space_of(e).as_deref() == Some(target.as_str()));

    let mut space_drafts = Vec::new();
    if !space_exists {
        space_drafts.push(Drafted {
            kind: "ContextSpace".into(),
            name: target.clone(),
            plural: "contextspaces".into(),
            manifest: json!({
                "apiVersion": API_VERSION,
                "kind": "ContextSpace",
                "metadata": {
                    "name": target,
                    "namespace": world.project,
                    "title": { "en": space_title(&target) },
                    "description": { "en": format!("Key performance indicators computed from {source_space}: one entity per indicator, with its formula and its sources") },
                },
                "spec": { "isSandbox": false, "defaultLocale": "en" },
            }),
        });
    }
    let (endpoint_name, slug) = match target_endpoint {
        Some(endpoint) => (
            name_of(endpoint).to_owned(),
            endpoint["spec"]["slug"].as_str().map(str::to_owned),
        ),
        None => {
            let taken = world.endpoints.iter().any(|e| name_of(e) == target);
            let endpoint_name = if taken {
                format!("{target}-writer")
            } else {
                target.clone()
            };
            space_drafts.push(Drafted {
                kind: "Endpoint".into(),
                name: endpoint_name.clone(),
                plural: "endpoints".into(),
                manifest: json!({
                    "apiVersion": API_VERSION,
                    "kind": "Endpoint",
                    "metadata": {
                        "name": endpoint_name,
                        "namespace": world.project,
                        "title": { "en": space_title(&target) },
                    },
                    "spec": {
                        "contextSpaceRef": target,
                        "slug": world.new_slug,
                        "audience": "public",
                        "enabledRepresentations": ["ngsi-ld", "csv", "json"],
                    },
                }),
            });
            (endpoint_name, Some(world.new_slug.to_owned()))
        }
    };

    // The runner writes the indicators: the grant the project's own indicator space gives it is
    // the one to copy; the demo seed's `pipelines` account otherwise.
    let space_policies: Vec<&Value> = world
        .policies
        .iter()
        .filter(|p| {
            crate::api::assistant::ref_name(&p["spec"]["contextSpaceRef"]).as_deref()
                == Some(target.as_str())
        })
        .collect();
    let writes = |p: &&Value| {
        p["spec"]["operations"]
            .as_array()
            .is_some_and(|ops| ops.iter().any(|op| op == "upsertBatch"))
    };
    if !space_policies.iter().any(writes) {
        let assignee = world
            .policies
            .iter()
            .filter(writes)
            .find(|p| {
                crate::api::assistant::ref_name(&p["spec"]["contextSpaceRef"])
                    .is_some_and(|space| space.ends_with(KPI_SPACE_SUFFIX))
            })
            .map(|p| p["spec"]["assignee"].clone())
            .unwrap_or_else(|| json!({ "kind": "serviceAccount", "id": "pipelines" }));
        space_drafts.push(policy(
            world,
            &target,
            &format!("{target}-pipelines-write"),
            "The KPI pipelines write the indicator space",
            assignee,
            &["upsertBatch", "createBatch", "queryBatch"],
        ));
    }
    if !space_exists {
        space_drafts.push(policy(
            world,
            &target,
            &format!("{target}-read"),
            "Public read: indicators",
            json!({ "kind": "role", "id": "public" }),
            &["retrieveOps"],
        ));
    }

    let formula = kpi::formula(&ComputeKpi {
        name: name.to_owned(),
        title: call.title.clone(),
        entity_type: entity_type.to_owned(),
        attribute: attribute.to_owned(),
        agg: call.agg,
        unit: call.unit.clone(),
        q: call.q.clone(),
        endpoint: None,
    });
    let mapping = bloblang(&Mapping {
        name,
        project: world.project,
        target_space: &target,
        source_space: &source_space,
        source_endpoint: &source_name,
        attribute,
        agg: call.agg,
        unit: call
            .unit
            .as_deref()
            .filter(|u| !u.trim().is_empty())
            .unwrap_or("C62"),
        formula: &formula,
        trigger: &trigger,
    });

    let mut query = json!({ "type": entity_type });
    if !attribute.is_empty() {
        query["attrs"] = json!([attribute]);
    }
    if let Some(q) = call.q.as_deref().map(str::trim).filter(|q| !q.is_empty()) {
        query["q"] = json!(q);
    }
    let mut source_spec = json!({
        "endpointRef": { "kind": "Endpoint", "name": source_name },
        "query": query,
    });
    let mut spec = json!({ "class": "auto" });
    match &trigger {
        Trigger::Every { period, .. } => spec["period"] = json!(period),
        Trigger::OnChange(watched) => {
            source_spec["trigger"] = json!({
                "subscription": { "type": entity_type, "watchedAttributes": watched }
            });
        }
    }
    spec["source"] = source_spec;
    spec["compute"] = json!({ "kind": "bloblang", "bloblang": mapping });
    spec["targetEndpoint"] = json!(format!(
        "urn:ngsi-ld:Endpoint:{}:{target}:{endpoint_name}",
        world.org_domain
    ));
    spec["output"] = json!({ "type": "KeyPerformanceIndicator", "mode": "upsert" });

    let title = call
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .unwrap_or(name);
    let pipeline = json!({
        "apiVersion": API_VERSION,
        "kind": "Pipeline",
        "metadata": {
            "name": name,
            "namespace": world.project,
            "title": { "en": title },
        },
        "spec": spec,
    });

    // The pipelines page's form: refs as names, everything else as the manifest says.
    let mut prefill = pipeline["spec"].clone();
    prefill["name"] = json!(name);
    prefill["title"] = json!(title);
    prefill["source"]["endpointRef"] = json!(source_name);

    Ok(Plan {
        pipeline,
        trigger,
        formula,
        source_endpoint: source_name,
        source_space,
        target_space: target,
        target_endpoint: endpoint_name,
        target_slug: slug,
        space_drafts,
        prefill,
    })
}

fn policy(
    world: &World,
    space: &str,
    name: &str,
    title: &str,
    assignee: Value,
    operations: &[&str],
) -> Drafted {
    Drafted {
        kind: "Policy".into(),
        name: name.to_owned(),
        plural: "policies".into(),
        manifest: json!({
            "apiVersion": API_VERSION,
            "kind": "Policy",
            "metadata": {
                "name": name,
                "namespace": world.project,
                "title": { "en": title },
            },
            "spec": {
                "contextSpaceRef": { "kind": "ContextSpace", "name": space },
                "assigner": "did:web:{orgDomain}",
                "assignee": assignee,
                "operations": operations,
                "information": [{ "entities": [{ "type": "KeyPerformanceIndicator" }] }],
            },
        }),
    }
}

/// What the mapping is written from.
pub struct Mapping<'a> {
    pub name: &'a str,
    pub project: &'a str,
    pub target_space: &'a str,
    pub source_space: &'a str,
    pub source_endpoint: &'a str,
    pub attribute: &'a str,
    pub agg: Agg,
    pub unit: &'a str,
    pub formula: &'a str,
    pub trigger: &'a Trigger,
}

/// The Bloblang that folds one page of the source into one indicator (Architecture/08, KPI
/// pipelines): the page is one message, an empty one writes `0`, the timestamps are whole
/// seconds because the broker refuses a fraction, and the provenance names the endpoint read
/// and the pipeline that computed it.
pub fn bloblang(m: &Mapping) -> String {
    let quote = |text: &str| Value::String(text.to_owned()).to_string();
    let values = format!(
        "let values = $rows.map_each(s -> s.get({})).filter(v -> v.type() == \"number\")",
        quote(&format!("{}.value", m.attribute))
    );
    let value = match m.agg {
        Agg::Count => "$rows.length()".to_owned(),
        Agg::Sum => "if $values.length() == 0 { 0 } else { $values.sum() }".to_owned(),
        Agg::Avg => {
            "if $values.length() == 0 { 0 } else { $values.sum() / $values.length() }".to_owned()
        }
        Agg::Min => "if $values.length() == 0 { 0 } else { $values.fold($values.index(0), item -> if item.value < item.tally { item.value } else { item.tally }) }".to_owned(),
        Agg::Max => "if $values.length() == 0 { 0 } else { $values.fold($values.index(0), item -> if item.value > item.tally { item.value } else { item.tally }) }".to_owned(),
    };
    let start = match m.trigger {
        Trigger::Every { seconds, .. } => {
            format!("$now.ts_sub_iso8601(\"{}\")", iso_duration(*seconds))
        }
        Trigger::OnChange(_) => "$now".to_owned(),
    };
    let mut lines = vec![
        "let domain = env(\"JC_ORG_DOMAIN\")".to_owned(),
        // The space the indicator is written into, rendered per stream (PL-57).
        "let space = env(\"JC_SPACE\")".to_owned(),
        "let rows = if this.type() == \"array\" { this } else { [this] }".to_owned(),
    ];
    if m.agg != Agg::Count {
        lines.push(values);
    }
    lines.extend([
        "let now = now().ts_format(\"2006-01-02T15:04:05Z\")".to_owned(),
        format!(
            "root.id = \"urn:ngsi-ld:KeyPerformanceIndicator:%v:%v:{}\".format($domain, $space)",
            m.name
        ),
        "root.type = \"KeyPerformanceIndicator\"".to_owned(),
        format!("root.name = {{ \"type\": \"Property\", \"value\": {} }}", quote(m.name)),
        format!(
            "root.calculationFormula = {{ \"type\": \"Property\", \"value\": {} }}",
            quote(m.formula)
        ),
        format!(
            "root.currentValue = {{ \"type\": \"Property\", \"value\": {value}, \"unitCode\": {}, \"observedAt\": $now }}",
            quote(m.unit)
        ),
        format!(
            "root.calculationPeriod = {{ \"type\": \"Property\", \"value\": {{ \"start\": {start}, \"end\": $now }} }}"
        ),
        "root.updatedAt = { \"type\": \"Property\", \"value\": { \"@type\": \"DateTime\", \"@value\": $now } }".to_owned(),
        format!(
            "root.derivedFrom = {{ \"type\": \"Property\", \"value\": \"urn:ngsi-ld:Endpoint:%v:{}:{}\".format($domain) }}",
            m.source_space, m.source_endpoint
        ),
        format!(
            "root.computedBy = {{ \"type\": \"Property\", \"value\": \"urn:ngsi-ld:Pipeline:%v:{}:{}\".format($domain) }}",
            m.project, m.name
        ),
    ]);
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint(name: &str, space: &str, slug: &str) -> Value {
        json!({
            "apiVersion": API_VERSION,
            "kind": "Endpoint",
            "metadata": { "name": name, "namespace": "helsinki" },
            "spec": { "contextSpaceRef": space, "slug": slug },
        })
    }

    fn call(json: Value) -> DraftKpiPipeline {
        serde_json::from_value(json).expect("a call")
    }

    fn world<'a>(endpoints: &'a [Value], spaces: &'a [String], policies: &'a [Value]) -> World<'a> {
        World {
            project: "helsinki",
            org_domain: "hel.fi",
            endpoints,
            spaces,
            policies,
            run_endpoint: Some("helsinki-all"),
            new_slug: "newslugnewslugnewslugnewsl",
        }
    }

    #[test]
    fn the_call_is_read_out_of_its_fence() {
        let answer = "I will keep it updated.\n\n```json\n{\"tool\":\"draft_kpi_pipeline\",\"name\":\"bikes-available-avg\",\"type\":\"BikeHireDockingStation\",\"attribute\":\"availableBikeNumber\",\"agg\":\"avg\",\"every\":\"15m\"}\n```";
        let parsed = tool_call(answer).expect("a call").expect("parses");
        assert_eq!(parsed.agg, Agg::Avg);
        assert_eq!(parsed.every.as_deref(), Some("15m"));
        assert!(tool_call(
            "```json\n{\"tool\":\"compute_kpi\",\"name\":\"x\",\"agg\":\"avg\"}\n```"
        )
        .is_none());
        assert!(
            tool_call("```json\n{\"tool\":\"draft_kpi_pipeline\",\"agg\":\"mean\"}\n```")
                .expect("a call")
                .is_err()
        );
    }

    #[test]
    fn a_trigger_is_a_period_of_a_minute_or_more_or_a_change_and_never_both() {
        let base = json!({ "name": "x", "type": "T", "attribute": "a", "agg": "avg" });
        let with = |extra: Value| {
            let mut value = base.clone();
            value
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            trigger_of(&call(value))
        };
        assert_eq!(
            with(json!({ "every": "15m" })),
            Ok(Trigger::Every {
                period: "15m".into(),
                seconds: 900
            })
        );
        // T-6532: a daily indicator runs once a day, as the runner writes it.
        assert_eq!(
            with(json!({ "every": "1d" })),
            Ok(Trigger::Every {
                period: "24h".into(),
                seconds: 86_400
            })
        );
        assert!(with(json!({ "every": "30s" }))
            .unwrap_err()
            .contains("under a minute"));
        assert!(with(json!({ "every": "often" }))
            .unwrap_err()
            .contains("not a period"));
        assert!(with(json!({})).is_err());
        assert!(with(json!({ "every": "1h", "onChange": true })).is_err());
        assert_eq!(
            with(json!({ "onChange": true })),
            Ok(Trigger::OnChange(vec!["a".into()]))
        );
        assert_eq!(
            with(json!({ "onChange": true, "watchedAttributes": ["b", " "] })),
            Ok(Trigger::OnChange(vec!["b".into()]))
        );
    }

    #[test]
    fn the_target_space_always_ends_with_kpi() {
        assert_eq!(target_space("helsinki", None).unwrap(), "helsinki-kpi");
        assert_eq!(
            target_space("helsinki", Some("transportation-kpi")).unwrap(),
            "transportation-kpi"
        );
        assert_eq!(
            target_space("helsinki", Some("transportation-kpis")).unwrap(),
            "transportation-kpi"
        );
        assert_eq!(
            target_space("helsinki", Some("Transportation")).unwrap(),
            "transportation-kpi"
        );
        assert!(target_space("helsinki", Some("no spaces here")).is_err());
    }

    #[test]
    fn an_existing_indicator_space_gets_a_scheduled_pipeline_and_no_other_draft() {
        let endpoints = [
            endpoint("helsinki-all", "helsinki", "allslug"),
            endpoint("helsinki-kpi", "helsinki-kpi", "kpislug"),
        ];
        let spaces = ["helsinki".to_owned(), "helsinki-kpi".to_owned()];
        let policies = [json!({
            "kind": "Policy",
            "metadata": { "name": "kpi-pipelines-write" },
            "spec": { "contextSpaceRef": { "kind": "ContextSpace", "name": "helsinki-kpi" }, "assignee": { "kind": "serviceAccount", "id": "pipelines" }, "operations": ["upsertBatch"] }
        })];
        let plan = plan(
            &call(json!({ "name": "bikes-available-avg", "title": "Average available bikes", "type": "BikeHireDockingStation", "attribute": "availableBikeNumber", "agg": "avg", "every": "15m" })),
            &world(&endpoints, &spaces, &policies),
        )
        .expect("a plan");

        assert!(plan.space_drafts.is_empty(), "{:?}", plan.space_drafts);
        assert_eq!(plan.target_space, "helsinki-kpi");
        assert_eq!(plan.target_slug.as_deref(), Some("kpislug"));
        let spec = &plan.pipeline["spec"];
        assert_eq!(spec["class"], "auto");
        assert_eq!(spec["period"], "15m");
        assert!(spec["source"].get("trigger").is_none());
        assert_eq!(
            spec["source"]["endpointRef"],
            json!({ "kind": "Endpoint", "name": "helsinki-all" })
        );
        assert_eq!(
            spec["source"]["query"],
            json!({ "type": "BikeHireDockingStation", "attrs": ["availableBikeNumber"] })
        );
        assert_eq!(
            spec["targetEndpoint"],
            "urn:ngsi-ld:Endpoint:hel.fi:helsinki-kpi:helsinki-kpi"
        );
        assert_eq!(
            spec["output"],
            json!({ "type": "KeyPerformanceIndicator", "mode": "upsert" })
        );
        let mapping = spec["compute"]["bloblang"].as_str().expect("mapping");
        assert!(
            mapping.contains("let space = env(\"JC_SPACE\")"),
            "{mapping}"
        );
        assert!(mapping.contains(
            "KeyPerformanceIndicator:%v:%v:bikes-available-avg\".format($domain, $space)"
        ));
        assert!(mapping.contains("ts_sub_iso8601(\"PT15M\")"));
        assert!(mapping.contains("urn:ngsi-ld:Endpoint:%v:helsinki:helsinki-all"));
        assert!(mapping.contains("urn:ngsi-ld:Pipeline:%v:helsinki:bikes-available-avg"));
        // The pipelines page's form takes the refs as names.
        assert_eq!(plan.prefill["source"]["endpointRef"], "helsinki-all");
        assert_eq!(plan.prefill["name"], "bikes-available-avg");
        assert_eq!(plan.trigger.describe(), "every 15m");
    }

    #[test]
    fn a_new_indicator_space_is_drafted_with_its_endpoint_and_both_policies_on_change() {
        let endpoints = [endpoint("helsinki-all", "helsinki", "allslug")];
        let spaces = ["helsinki".to_owned()];
        let policies = [json!({
            "kind": "Policy",
            "spec": { "contextSpaceRef": "helsinki-kpi", "assignee": { "kind": "serviceAccount", "id": "runner" }, "operations": ["upsertBatch", "createBatch"] }
        })];
        let plan = plan(
            &call(json!({ "name": "free-bikes", "type": "BikeHireDockingStation", "attribute": "availableBikeNumber", "agg": "sum", "targetSpace": "transportation-kpis", "onChange": true, "sourceEndpoint": "helsinki-all" })),
            &world(&endpoints, &spaces, &policies),
        )
        .expect("a plan");

        let drafted: Vec<(&str, &str)> = plan
            .space_drafts
            .iter()
            .map(|d| (d.kind.as_str(), d.name.as_str()))
            .collect();
        assert_eq!(
            drafted,
            [
                ("ContextSpace", "transportation-kpi"),
                ("Endpoint", "transportation-kpi"),
                ("Policy", "transportation-kpi-pipelines-write"),
                ("Policy", "transportation-kpi-read"),
            ]
        );
        let endpoint = &plan.space_drafts[1].manifest["spec"];
        assert_eq!(endpoint["slug"], "newslugnewslugnewslugnewsl");
        assert_eq!(endpoint["contextSpaceRef"], "transportation-kpi");
        // Nobody asked for a limit, so the draft sets none (T-2775, EP-20).
        assert!(endpoint.get("rateLimits").is_none(), "{endpoint}");
        // The write grant copies the assignee of the project's own indicator space.
        assert_eq!(
            plan.space_drafts[2].manifest["spec"]["assignee"],
            json!({ "kind": "serviceAccount", "id": "runner" })
        );
        assert_eq!(
            plan.space_drafts[3].manifest["spec"]["assignee"],
            json!({ "kind": "role", "id": "public" })
        );
        let spec = &plan.pipeline["spec"];
        assert!(spec.get("period").is_none());
        assert_eq!(
            spec["source"]["trigger"],
            json!({ "subscription": { "type": "BikeHireDockingStation", "watchedAttributes": ["availableBikeNumber"] } })
        );
        assert_eq!(
            spec["targetEndpoint"],
            "urn:ngsi-ld:Endpoint:hel.fi:transportation-kpi:transportation-kpi"
        );
        let mapping = spec["compute"]["bloblang"].as_str().expect("mapping");
        assert!(
            mapping.contains("KeyPerformanceIndicator:%v:%v:free-bikes\".format($domain, $space)")
        );
        assert!(mapping.contains("\"start\": $now, \"end\": $now"));
        assert!(mapping.contains("$values.sum() }"));
        assert_eq!(
            plan.trigger.describe(),
            "on every change of availableBikeNumber"
        );
    }

    #[test]
    fn what_the_project_does_not_have_is_named_back() {
        let endpoints = [endpoint("helsinki-all", "helsinki", "allslug")];
        let spaces = ["helsinki".to_owned()];
        let base =
            json!({ "name": "x", "type": "T", "attribute": "a", "agg": "avg", "every": "15m" });
        let with = |extra: Value| {
            let mut value = base.clone();
            value
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            plan(&call(value), &world(&endpoints, &spaces, &[]))
                .map(|_| ())
                .unwrap_err()
        };
        assert!(with(json!({ "sourceEndpoint": "helsinki-parking" })).contains("helsinki-all"));
        assert!(with(json!({ "name": "Not A Name" })).contains("not an indicator name"));
        assert!(with(json!({ "attribute": "" })).contains("attribute"));
        let mut no_run = world(&endpoints, &spaces, &[]);
        no_run.run_endpoint = None;
        assert!(plan(&call(base.clone()), &no_run)
            .unwrap_err()
            .contains("sourceEndpoint"));
    }

    #[test]
    fn every_aggregate_writes_its_fold_and_count_reads_no_attribute() {
        let trigger = Trigger::Every {
            period: "1h".into(),
            seconds: 3600,
        };
        let mapping = |agg| {
            bloblang(&Mapping {
                name: "n",
                project: "helsinki",
                target_space: "helsinki-kpi",
                source_space: "helsinki",
                source_endpoint: "helsinki-all",
                attribute: "pm10",
                agg,
                unit: "GQ",
                formula: "f",
                trigger: &trigger,
            })
        };
        assert!(mapping(Agg::Count).contains("\"value\": $rows.length()"));
        assert!(!mapping(Agg::Count).contains("let values"));
        assert!(mapping(Agg::Avg).contains("$values.sum() / $values.length()"));
        assert!(mapping(Agg::Min).contains("item.value < item.tally"));
        assert!(mapping(Agg::Max).contains("item.value > item.tally"));
        assert!(mapping(Agg::Avg).contains("s.get(\"pm10.value\")"));
        assert!(mapping(Agg::Avg).contains("\"unitCode\": \"GQ\""));
        assert!(mapping(Agg::Avg).contains("PT1H"));
        // Provenance is a Property: a view refuses a Relationship to an Endpoint (T-4231).
        assert!(mapping(Agg::Avg).contains(
            "root.derivedFrom = { \"type\": \"Property\", \"value\": \"urn:ngsi-ld:Endpoint:"
        ));
        assert!(mapping(Agg::Avg).contains(
            "root.computedBy = { \"type\": \"Property\", \"value\": \"urn:ngsi-ld:Pipeline:"
        ));
    }

    fn write_policy(name: &str, space: &str, assignee: Value) -> Value {
        json!({
            "kind": "Policy",
            "metadata": { "name": name },
            "spec": {
                "contextSpaceRef": { "kind": "ContextSpace", "name": space },
                "assignee": assignee,
                "operations": ["upsertBatch"],
            }
        })
    }

    fn base_call() -> Value {
        json!({ "name": "bikes-avg", "type": "BikeHireDockingStation", "attribute": "availableBikeNumber", "agg": "avg", "every": "15m" })
    }

    fn drafted<'a>(plan: &'a Plan, kind: &str) -> Vec<&'a Drafted> {
        plan.space_drafts
            .iter()
            .filter(|d| d.kind == kind)
            .collect()
    }

    /// AG-74, PF-54, T-2511: an indicator is never written into the space it reads, whichever way
    /// the target is spelled.
    #[test]
    fn target_equal_to_source_space_is_refused() {
        let endpoints = [endpoint("bikes-kpi-feed", "bikes-kpi", "feedslug")];
        let spaces = ["bikes-kpi".to_owned()];
        for target in ["bikes-kpi", "bikes", "Bikes-KPIS"] {
            let mut value = base_call();
            value["sourceEndpoint"] = json!("bikes-kpi-feed");
            value["targetSpace"] = json!(target);
            let refused = plan(&call(value), &world(&endpoints, &spaces, &[]))
                .map(|_| ())
                .unwrap_err();
            assert!(
                refused.contains("is the space the indicator reads"),
                "{target}: {refused}"
            );
        }
    }

    /// PF-50, T-2511: with no write grant on any indicator space, the one drafted names the
    /// platform's pipelines account; nobody the model named.
    #[test]
    fn no_existing_write_policy_on_the_target_falls_back_to_the_pipelines_service_account() {
        let endpoints = [endpoint("helsinki-all", "helsinki", "allslug")];
        let spaces = ["helsinki".to_owned()];
        let policies = [write_policy(
            "helsinki-edit",
            "helsinki",
            json!({ "kind": "user", "id": "jana" }),
        )];
        let plan =
            plan(&call(base_call()), &world(&endpoints, &spaces, &policies)).expect("a plan");
        let writes = drafted(&plan, "Policy")
            .into_iter()
            .find(|d| d.name.ends_with("-pipelines-write"))
            .expect("a write grant");
        assert_eq!(
            writes.manifest["spec"]["assignee"],
            json!({ "kind": "serviceAccount", "id": "pipelines" })
        );
        assert_eq!(writes.manifest["spec"]["assigner"], "did:web:{orgDomain}");
    }

    /// PF-50, T-2511: the write grant of an existing indicator space is what a new one copies, so
    /// the account that writes indicators today writes these too; a write grant on a space that is
    /// not an indicator space is never copied.
    #[test]
    fn an_existing_kpi_space_write_grant_is_copied_as_the_new_spaces_assignee() {
        let endpoints = [endpoint("helsinki-all", "helsinki", "allslug")];
        let spaces = ["helsinki".to_owned(), "mobility-kpi".to_owned()];
        let runner = json!({ "kind": "serviceAccount", "id": "kpi-runner" });
        let policies = [
            write_policy(
                "data-edit",
                "helsinki",
                json!({ "kind": "user", "id": "jana" }),
            ),
            write_policy("mobility-kpi-write", "mobility-kpi", runner.clone()),
        ];
        let plan =
            plan(&call(base_call()), &world(&endpoints, &spaces, &policies)).expect("a plan");
        assert_eq!(plan.target_space, "helsinki-kpi");
        let writes = drafted(&plan, "Policy")
            .into_iter()
            .find(|d| d.name == "helsinki-kpi-pipelines-write")
            .expect("a write grant");
        assert_eq!(writes.manifest["spec"]["assignee"], runner);
    }

    /// PL-45, T-2511: the model's strings reach the mapping only as JSON string literals, and the
    /// indicator's name, which is spliced into a format string, is refused unless it is DNS-1123.
    #[test]
    fn bloblang_name_interpolation_never_breaks_out_of_the_format_string() {
        let endpoints = [endpoint("helsinki-all", "helsinki", "allslug")];
        let spaces = ["helsinki".to_owned()];
        for name in ["x\".format(env(\"SECRET\"))", "a b", "x\\", "x:y", ""] {
            let mut value = base_call();
            value["name"] = json!(name);
            assert!(
                plan(&call(value), &world(&endpoints, &spaces, &[])).is_err(),
                "{name:?} was accepted"
            );
        }
        let mut value = base_call();
        value["attribute"] = json!("pm10\") + env(\"SECRET\") + (\"");
        value["unit"] = json!("GQ\" }\nroot = deleted()\n#");
        let plan = plan(&call(value), &world(&endpoints, &spaces, &[])).expect("a plan");
        let mapping = plan.pipeline["spec"]["compute"]["bloblang"]
            .as_str()
            .expect("mapping");
        assert!(
            mapping.contains(r#"s.get("pm10\") + env(\"SECRET\") + (\".value")"#),
            "{mapping}"
        );
        assert!(!mapping.contains("\nroot = deleted()"), "{mapping}");
        assert!(!mapping.contains("env(\"SECRET\")\n"), "{mapping}");
    }

    /// AG-74, T-2511: the named cases of `what_the_project_does_not_have_is_named_back`, one each.
    #[test]
    fn a_name_an_endpoint_or_a_run_endpoint_that_is_missing_is_named_back() {
        let endpoints = [
            endpoint("helsinki-all", "helsinki", "allslug"),
            endpoint("helsinki-parking", "parking", "parkslug"),
        ];
        let spaces = ["helsinki".to_owned()];
        // a_name_that_is_not_dns1123_is_refused
        let mut value = base_call();
        value["name"] = json!("Bikes_Avg");
        assert!(plan(&call(value), &world(&endpoints, &spaces, &[]))
            .map(|_| ())
            .unwrap_err()
            .contains("not an indicator name"));
        // unknown_source_endpoint_names_back_the_projects_endpoints
        let mut value = base_call();
        value["sourceEndpoint"] = json!("nowhere");
        let refused = plan(&call(value), &world(&endpoints, &spaces, &[]))
            .map(|_| ())
            .unwrap_err();
        assert!(
            refused.contains("'nowhere'") && refused.contains("helsinki-all, helsinki-parking"),
            "{refused}"
        );
        // no_run_endpoint_and_no_named_source_is_refused
        let mut alone = world(&endpoints, &spaces, &[]);
        alone.run_endpoint = None;
        assert!(plan(&call(base_call()), &alone)
            .map(|_| ())
            .unwrap_err()
            .contains("sourceEndpoint"));
    }

    /// AG-74, T-2511: an Endpoint that already carries the indicator space's name in another space
    /// is left alone, and the drafted one takes `-writer`.
    #[test]
    fn an_endpoint_name_already_taken_by_the_target_gets_a_writer_suffix() {
        let endpoints = [
            endpoint("helsinki-all", "helsinki", "allslug"),
            endpoint("helsinki-kpi", "helsinki", "oddslug"),
        ];
        let spaces = ["helsinki".to_owned()];
        let plan = plan(&call(base_call()), &world(&endpoints, &spaces, &[])).expect("a plan");
        assert_eq!(plan.target_endpoint, "helsinki-kpi-writer");
        let endpoint = drafted(&plan, "Endpoint");
        assert_eq!(endpoint.len(), 1);
        assert_eq!(endpoint[0].name, "helsinki-kpi-writer");
        assert_eq!(
            endpoint[0].manifest["spec"]["contextSpaceRef"],
            "helsinki-kpi"
        );
    }

    /// AG-74, T-2511: `count` needs no attribute and asks for none; every other fold needs one.
    #[test]
    fn count_agg_needs_no_attribute_but_every_other_agg_does() {
        let endpoints = [endpoint("helsinki-all", "helsinki", "allslug")];
        let spaces = ["helsinki".to_owned()];
        let mut counted = base_call();
        counted["agg"] = json!("count");
        counted["attribute"] = json!("");
        let plan_of_count = plan(&call(counted), &world(&endpoints, &spaces, &[])).expect("count");
        assert_eq!(
            plan_of_count.pipeline["spec"]["source"]["query"],
            json!({ "type": "BikeHireDockingStation" })
        );
        for agg in ["sum", "avg", "min", "max"] {
            let mut value = base_call();
            value["agg"] = json!(agg);
            value["attribute"] = json!("  ");
            assert!(
                plan(&call(value), &world(&endpoints, &spaces, &[])).is_err(),
                "{agg} took no attribute"
            );
        }
    }

    /// PL-51, T-2511: a change trigger subscribes to the watched attributes and carries no period.
    #[test]
    fn on_change_trigger_writes_a_subscription_not_a_period() {
        let endpoints = [endpoint("helsinki-all", "helsinki", "allslug")];
        let spaces = ["helsinki".to_owned()];
        let mut value = base_call();
        value.as_object_mut().expect("object").remove("every");
        value["onChange"] = json!(true);
        let plan = plan(&call(value), &world(&endpoints, &spaces, &[])).expect("a plan");
        let spec = &plan.pipeline["spec"];
        assert!(spec.get("period").is_none(), "{spec}");
        assert_eq!(
            spec["source"]["trigger"]["subscription"],
            json!({ "type": "BikeHireDockingStation", "watchedAttributes": ["availableBikeNumber"] })
        );
    }

    /// AG-74, T-2511: an indicator space the project holds is not drafted again, nor its read
    /// grant; only what it lacks (its endpoint, its write grant) is.
    #[test]
    fn an_existing_target_space_drafts_no_contextspace_or_read_policy() {
        let endpoints = [endpoint("helsinki-all", "helsinki", "allslug")];
        let spaces = ["helsinki".to_owned(), "helsinki-kpi".to_owned()];
        let plan = plan(&call(base_call()), &world(&endpoints, &spaces, &[])).expect("a plan");
        assert!(
            drafted(&plan, "ContextSpace").is_empty(),
            "{:?}",
            plan.space_drafts
        );
        let names: Vec<&str> = plan.space_drafts.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["helsinki-kpi", "helsinki-kpi-pipelines-write"],
            "{names:?}"
        );
        assert_eq!(drafted(&plan, "Endpoint").len(), 1);
    }
}
