//! `GET /api/v1/projects/{project}/mqttbrokers/{name}/bindings`: which topic of a broker fills
//! which attribute, joined from the manifests the Portal mirrors (API/01 §47, T-5533).
//!
//! Topic filter → DataSource (`mqtt.brokerRef`) → Pipeline (`dataSourceRef`) → Endpoint
//! (`targetEndpoint`) → type and attributes. Each hop is checked against the caller's own rights;
//! a hop they may not read is `{kind, hidden: true}` without its name, and the row ends there.

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use jc_core::kinds::{PipelineOutput, PipelineSpec, Step, Verb};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::api::assistant::ref_name;
use crate::auth::session::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::resource::ResourceEnvelope;
use crate::state::AppState;
use crate::store::{ListOptions, Mirror};

/// The first line of a step the Portal's output mapper wrote: the map, as JSON (outputMapper.ts).
const MAPPER_HEADER: &str = "# jc-mapper ";

/// One hop of a binding: a manifest by name, or `hidden` when the caller may not read it.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Hop {
    /// `DataSource`, `Pipeline` or `Endpoint`.
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The Context Space an Endpoint writes into.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

impl Hop {
    fn named(kind: &'static str, name: &str) -> Self {
        Self {
            kind,
            name: Some(name.to_owned()),
            space: None,
            hidden: false,
        }
    }

    fn hidden(kind: &'static str) -> Self {
        Self {
            kind,
            name: None,
            space: None,
            hidden: true,
        }
    }
}

/// One mapped attribute: `from` the record field (two for a point), absent for an expression or
/// a fixed value.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct AttributeBinding {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    pub to: String,
}

/// One topic filter's way to an attribute; it ends at the first hop the caller may not read.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Binding {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic_filter: Option<String>,
    pub data_source: Hop,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pipeline: Option<Hop>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<Hop>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub entity_type: Option<String>,
    /// The output mapper's map; absent for a step written by hand.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attributes: Option<Vec<AttributeBinding>>,
}

#[derive(Debug, PartialEq, Serialize, ToSchema)]
pub struct Bindings {
    pub broker: String,
    pub bindings: Vec<Binding>,
}

/// Whether the caller may read `kind` in `space` (`None`: anywhere in the project).
pub type May<'a> = &'a dyn Fn(&str, Option<&str>) -> bool;

/// The map of the output mapper's header (outputMapper.ts `MapperState`).
#[derive(Deserialize)]
struct MapperState {
    #[serde(rename = "type")]
    entity_type: String,
    attributes: serde_json::Map<String, Value>,
}

/// Every binding of `broker` in `project` as `may` lets the caller see it.
pub fn bindings(mirror: &Mirror, project: &str, broker: &str, may: May<'_>) -> Vec<Binding> {
    let list = |kind: &str| mirror.list(project, kind, &ListOptions::default()).items;
    let pipelines: Vec<(String, PipelineSpec)> = list("Pipeline")
        .into_iter()
        .filter_map(|p| Some((p.metadata.name, serde_json::from_value(p.spec).ok()?)))
        .collect();
    let join = Join {
        mirror,
        project,
        may,
        pipelines: &pipelines,
    };
    list("DataSource")
        .iter()
        .filter(|ds| ref_name(&ds.spec["mqtt"]["brokerRef"]).as_deref() == Some(broker))
        .flat_map(|ds| join.data_source(ds))
        .collect()
}

struct Join<'a> {
    mirror: &'a Mirror,
    project: &'a str,
    may: May<'a>,
    pipelines: &'a [(String, PipelineSpec)],
}

impl Join<'_> {
    fn data_source(&self, ds: &ResourceEnvelope) -> Vec<Binding> {
        if !(self.may)("DataSource", None) {
            return vec![row(None, Hop::hidden("DataSource"))];
        }
        let name = &ds.metadata.name;
        let topics = ds.spec["mqtt"]["topics"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let readers: Vec<_> = self
            .pipelines
            .iter()
            .filter(|(_, spec)| reads(spec, self.project, name))
            .collect();
        let mut rows = Vec::new();
        for topic in topics.iter().filter_map(Value::as_str) {
            let start = row(Some(topic.to_owned()), Hop::named("DataSource", name));
            if readers.is_empty() {
                rows.push(start.clone());
            }
            for (pipeline, spec) in &readers {
                rows.extend(self.pipeline(&start, pipeline, spec));
            }
        }
        rows
    }

    fn pipeline(&self, start: &Binding, name: &str, spec: &PipelineSpec) -> Vec<Binding> {
        if !pipeline_visible(self.mirror, self.project, spec, self.may) {
            return vec![Binding {
                pipeline: Some(Hop::hidden("Pipeline")),
                ..start.clone()
            }];
        }
        let outputs = spec.outputs();
        let at = Binding {
            pipeline: Some(Hop::named("Pipeline", name)),
            ..start.clone()
        };
        if outputs.is_empty() {
            return vec![at];
        }
        let spaces: Vec<Option<String>> = outputs
            .iter()
            .map(|o| space_of(self.mirror, self.project, o))
            .collect();
        let mapper = mapper_of(spec);
        outputs
            .iter()
            .zip(spaces)
            .map(|(output, space)| self.output(&at, output, space, mapper.as_ref()))
            .collect()
    }

    fn output(
        &self,
        at: &Binding,
        output: &PipelineOutput,
        space: Option<String>,
        mapper: Option<&Mapper>,
    ) -> Binding {
        let name = output.target_endpoint.local_id();
        if !space
            .as_deref()
            .is_none_or(|s| (self.may)("Endpoint", Some(s)))
        {
            return Binding {
                endpoint: Some(Hop::hidden("Endpoint")),
                ..at.clone()
            };
        }
        let mut endpoint = Hop::named("Endpoint", name);
        endpoint.space = space;
        let entity_type = output
            .entity_type
            .clone()
            .or_else(|| mapper.map(|m| m.state.entity_type.clone()));
        let attributes = mapper
            .filter(|m| entity_type.as_deref() == Some(m.state.entity_type.as_str()))
            .map(attributes_of);
        Binding {
            endpoint: Some(endpoint),
            entity_type,
            attributes,
            ..at.clone()
        }
    }
}

/// A pipeline is readable as on the flow map (API/01 §50): anywhere, or in a space it writes.
pub fn pipeline_visible(mirror: &Mirror, project: &str, spec: &PipelineSpec, may: May<'_>) -> bool {
    may("Pipeline", None)
        || spec
            .outputs()
            .iter()
            .filter_map(|o| space_of(mirror, project, o))
            .any(|s| may("Pipeline", Some(&s)))
}

/// The space an Endpoint of this project serves; `None` for one the project does not declare.
fn space_of(mirror: &Mirror, project: &str, output: &PipelineOutput) -> Option<String> {
    let endpoint = mirror.get(project, "Endpoint", output.target_endpoint.local_id())?;
    ref_name(&endpoint.spec["contextSpaceRef"])
}

fn row(topic_filter: Option<String>, data_source: Hop) -> Binding {
    Binding {
        topic_filter,
        data_source,
        pipeline: None,
        endpoint: None,
        entity_type: None,
        attributes: None,
    }
}

/// Whether `spec` reads DataSource `name` of its own project.
fn reads(spec: &PipelineSpec, project: &str, name: &str) -> bool {
    spec.sources()
        .iter()
        .filter_map(|s| s.data_source_ref.as_ref())
        .any(|r| r.name() == name && r.namespace().is_none_or(|ns| ns == project))
}

/// The output mapper's map of the first step it wrote, with that step's code.
fn mapper_of(spec: &PipelineSpec) -> Option<Mapper> {
    spec.steps().into_iter().find_map(|step| {
        let Step::Compute(compute) = step else {
            return None;
        };
        let code = compute.bloblang?;
        let state = serde_json::from_str(code.strip_prefix(MAPPER_HEADER)?.lines().next()?).ok()?;
        Some(Mapper { state, code })
    })
}

struct Mapper {
    state: MapperState,
    code: String,
}

/// The mapped attributes the step still writes as the map says: its `root.<name>` line is there
/// and reads the mapped fields. Anything edited by hand past that is the author's code, not shown.
fn attributes_of(mapper: &Mapper) -> Vec<AttributeBinding> {
    let field =
        |source: &Value, key: &str| source.get(key).and_then(Value::as_str).map(str::to_owned);
    let path = |field: &str| match field.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        true => format!("this.{field}"),
        false => format!("this.{}", serde_json::to_string(field).unwrap_or_default()),
    };
    let reads = |fields: &[String]| fields.iter().all(|f| mapper.code.contains(&path(f)));
    let written = |to: &str| {
        let quoted = serde_json::to_string(to).unwrap_or_default();
        mapper.code.lines().any(|l| {
            l.starts_with(&format!("root.{to} = ")) || l.starts_with(&format!("root.{quoted} = "))
        })
    };
    mapper
        .state
        .attributes
        .iter()
        .filter(|(to, _)| written(to))
        .filter_map(|(to, source)| {
            let fields: Vec<String> = match field(source, "field") {
                Some(one) => vec![one],
                None => ["longitude", "latitude"]
                    .iter()
                    .filter_map(|k| field(source, k))
                    .collect(),
            };
            reads(&fields).then(|| AttributeBinding {
                from: (!fields.is_empty()).then(|| fields.join(", ")),
                to: to.clone(),
            })
        })
        .collect()
}

/// PF-59: a caller who may not read the project's brokers learns nothing of this one.
fn readable_broker(
    state: &AppState,
    effective: &crate::permissions::Effective,
    project: &str,
    name: &str,
) -> Result<(), ApiError> {
    if effective.may_read_project()
        && effective.may_read("MqttBroker")
        && state.mirror.get(project, "MqttBroker", name).is_some()
    {
        return Ok(());
    }
    Err(ApiError::NotFound(format!(
        "MQTT broker '{name}' not found in project '{project}'"
    )))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/mqttbrokers/{name}/bindings",
    summary = "Read An MQTT Broker's Topic Bindings",
    description = "Which topic filter of the broker reaches which type and attributes: each DataSource naming the broker, the pipelines of the project that read it and the Endpoints they write through, read from the manifests (API/01 §47). Each hop is checked against the caller's rights; one they may not read is `hidden` without its name and ends the row.",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "MqttBroker name"),
    ),
    responses(
        (status = 200, description = "The bindings; empty when no DataSource names the broker", body = Bindings),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 404, description = "No such broker, or not readable by the caller", body = ProblemDetails),
    )
)]
pub async fn get_bindings(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<Json<Bindings>, ApiError> {
    let effective = crate::permissions::for_request(&state, &user.0.identity, &project);
    readable_broker(&state, &effective, &project, &name)?;
    let may = |kind: &str, space: Option<&str>| effective.may_in(kind, Verb::Read, space);
    let bindings = bindings(&state.mirror, &project, &name, &may);
    Ok(Json(Bindings {
        broker: name,
        bindings,
    }))
}

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/projects/{project}/mqttbrokers/{name}/bindings",
        get(get_bindings),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::{ObjectMeta, API_VERSION};
    use serde_json::json;

    fn manifest(project: &str, kind: &str, name: &str, spec: Value) -> ResourceEnvelope {
        ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: kind.to_owned(),
            metadata: ObjectMeta::new(name, project),
            spec,
            status: None,
        }
    }

    const MAPPER: &str = concat!(
        "# jc-mapper {\"type\":\"WeatherObserved\",\"attributes\":{\"temperature\":{\"field\":\"temp\"},",
        "\"location\":{\"longitude\":\"lon\",\"latitude\":\"lat\"},\"source\":{\"value\":\"sensor\"},\"humidity\":{\"field\":\"hum\"}}}\n",
        "root = {}\n",
        "let a0 = this.temp\n",
        "root.temperature = if $a0 != null { { \"type\": \"Property\", \"value\": $a0 } } else { deleted() }\n",
        "let a1 = if this.lon != null && this.lat != null { [ this.lon, this.lat ] } else { null }\n",
        "root.location = $a1\n",
        "root.source = \"sensor\"\n",
        "root.humidity = this.moisture\n",
    );

    fn pipeline(source: Value) -> Value {
        json!({
            "class": "auto",
            "source": { "dataSourceRef": source },
            "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:helsinki:weather-write",
            "compute": { "kind": "bloblang", "bloblang": MAPPER },
        })
    }

    fn helsinki() -> Mirror {
        let mirror = Mirror::new();
        let mqtt = |broker: &str, topics: Value| json!({ "type": "mqtt", "mqtt": { "brokerRef": { "kind": "MqttBroker", "name": broker }, "topics": topics } });
        for (name, spec) in [
            ("sensors-in", mqtt("sensors", json!(["devices/+/data"]))),
            ("idle-in", mqtt("sensors", json!(["idle/#"]))),
            ("other-in", mqtt("cameras", json!(["cams/#"]))),
        ] {
            mirror.upsert(manifest("helsinki", "DataSource", name, spec));
        }
        mirror.upsert(manifest(
            "helsinki",
            "Pipeline",
            "weather",
            pipeline(json!("sensors-in")),
        ));
        mirror.upsert(manifest(
            "helsinki",
            "Endpoint",
            "weather-write",
            json!({ "contextSpaceRef": { "kind": "ContextSpace", "name": "weather" } }),
        ));
        // Another project's pipeline naming helsinki's source, and one here naming espoo's.
        mirror.upsert(manifest(
            "espoo",
            "Pipeline",
            "copy",
            pipeline(
                json!({ "kind": "DataSource", "name": "sensors-in", "namespace": "helsinki" }),
            ),
        ));
        mirror.upsert(manifest(
            "helsinki",
            "Pipeline",
            "foreign",
            pipeline(json!({ "kind": "DataSource", "name": "idle-in", "namespace": "espoo" })),
        ));
        mirror
    }

    fn of(may: May<'_>) -> Vec<Binding> {
        bindings(&helsinki(), "helsinki", "sensors", may)
    }

    #[test]
    fn a_topic_reaches_its_type_and_the_attributes_the_step_still_writes() {
        let rows = of(&|_, _| true);
        let full = rows
            .iter()
            .find(|b| b.topic_filter.as_deref() == Some("devices/+/data"))
            .expect("bound");
        assert_eq!(full.data_source, Hop::named("DataSource", "sensors-in"));
        assert_eq!(full.pipeline, Some(Hop::named("Pipeline", "weather")));
        let endpoint = full.endpoint.clone().expect("endpoint");
        assert_eq!(
            (endpoint.name.as_deref(), endpoint.space.as_deref()),
            (Some("weather-write"), Some("weather"))
        );
        assert_eq!(full.entity_type.as_deref(), Some("WeatherObserved"));
        // humidity's line reads another field than the map says: edited by hand, left out.
        let attributes = full.attributes.clone().expect("mapper step");
        assert_eq!(
            attributes,
            vec![
                AttributeBinding {
                    from: Some("lon, lat".to_owned()),
                    to: "location".to_owned()
                },
                AttributeBinding {
                    from: None,
                    to: "source".to_owned()
                },
                AttributeBinding {
                    from: Some("temp".to_owned()),
                    to: "temperature".to_owned()
                },
            ]
        );
        assert_eq!(rows.len(), 2, "one per topic: {rows:?}");
    }

    #[test]
    fn a_source_no_pipeline_of_the_project_reads_has_no_pipeline() {
        let rows = of(&|_, _| true);
        let idle = rows
            .iter()
            .find(|b| b.topic_filter.as_deref() == Some("idle/#"))
            .expect("idle");
        assert_eq!(
            (idle.pipeline.as_ref(), idle.endpoint.as_ref()),
            (None, None)
        );
        assert!(!rows
            .iter()
            .any(|b| b.pipeline.as_ref().and_then(|p| p.name.as_deref()) == Some("copy")));
        assert!(!rows
            .iter()
            .any(|b| b.pipeline.as_ref().and_then(|p| p.name.as_deref()) == Some("foreign")));
        assert!(bindings(&helsinki(), "helsinki", "nobody", &|_, _| true).is_empty());
    }

    #[test]
    fn an_unreadable_source_is_hidden_without_its_name_or_topics() {
        let rows = of(&|kind, _| kind != "DataSource");
        assert_eq!(
            rows,
            vec![
                row(None, Hop::hidden("DataSource")),
                row(None, Hop::hidden("DataSource"))
            ]
        );
        let text = serde_json::to_string(&rows).expect("json");
        assert_eq!(
            text,
            r#"[{"dataSource":{"kind":"DataSource","hidden":true}},{"dataSource":{"kind":"DataSource","hidden":true}}]"#
        );
    }

    #[test]
    fn an_unreadable_pipeline_or_endpoint_ends_the_row() {
        let rows = of(&|kind, space| kind != "Pipeline" || space == Some("other"));
        let full = rows
            .iter()
            .find(|b| b.topic_filter.as_deref() == Some("devices/+/data"))
            .expect("bound");
        assert_eq!(
            (full.pipeline.clone(), full.endpoint.clone()),
            (Some(Hop::hidden("Pipeline")), None)
        );
        let pipeline_in_space = of(&|kind, space| {
            kind != "Endpoint" && (kind != "Pipeline" || space == Some("weather"))
        });
        let full = pipeline_in_space
            .iter()
            .find(|b| b.pipeline.is_some())
            .expect("bound");
        assert_eq!(full.pipeline, Some(Hop::named("Pipeline", "weather")));
        assert_eq!(
            (
                full.endpoint.clone(),
                full.entity_type.clone(),
                full.attributes.clone()
            ),
            (Some(Hop::hidden("Endpoint")), None, None)
        );
    }
}
