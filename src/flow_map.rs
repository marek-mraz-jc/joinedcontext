//! The data flow map of a project (T-4862): every path from an MQTT broker or a DataSource
//! through a pipeline and its Endpoints into a space, and on through derived and KPI pipelines
//! into the next space (PL-31…PL-33, PL-45, PL-52, PL-76).
//!
//! Built from the manifests the Portal mirrors and the runs it logged; nothing is asked of the
//! runner or the broker, so the map answers as fast as the mirror does. Each edge into or out of
//! a pipeline carries the grant that permits it: the Policy that gives the `pipelines`
//! ServiceAccount the operation on the Endpoint's space (PL-19), the same rule the expiry check
//! reads (`pipeline_expiry`), or "missing" with the fix.
//!
//! Only what the caller may read is a node (PF-59). A source Endpoint of another project is that
//! project's: a caller who may not read it gets its name and nothing else, and only because a
//! pipeline here names it.

use std::collections::{BTreeMap, BTreeSet};

use jc_core::kinds::{Operation, OperationRef, PipelineSpec, PolicySpec, PrincipalKind};
use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;

use crate::store::{ListOptions, Mirror};

/// What a node is on the map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, ToSchema)]
pub enum FlowKind {
    MqttBroker,
    DataSource,
    Pipeline,
    Endpoint,
    ContextSpace,
}

impl FlowKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::MqttBroker => "MqttBroker",
            Self::DataSource => "DataSource",
            Self::Pipeline => "Pipeline",
            Self::Endpoint => "Endpoint",
            Self::ContextSpace => "ContextSpace",
        }
    }
}

/// What a pipeline does on the map: reads the outside world, computes from a space, or writes
/// indicators (PL-45). A space ending in `-kpi` is an indicator space (PF-54).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum FlowRole {
    Ingest,
    Derived,
    Kpi,
}

/// A pipeline's latest run, as the Portal logged it (PL-62).
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FlowRun {
    /// The last line the Portal took for it, RFC 3339.
    pub at: String,
    pub sent: u64,
    pub rejected: u64,
    pub failed: u64,
    /// Records sent per minute over the run, when it lasted a minute or more.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub per_minute: Option<f64>,
}

impl FlowRun {
    pub fn of(run: &crate::pipeline_log::Run) -> Self {
        Self {
            at: run.last_at.clone(),
            sent: run.sent,
            rejected: run.rejected,
            failed: run.failed,
            per_minute: per_minute(run.sent, &run.first_at, &run.last_at),
        }
    }
}

fn per_minute(sent: u64, first: &str, last: &str) -> Option<f64> {
    use time::format_description::well_known::Rfc3339;
    let first = time::OffsetDateTime::parse(first, &Rfc3339).ok()?;
    let last = time::OffsetDateTime::parse(last, &Rfc3339).ok()?;
    let minutes = (last - first).as_seconds_f64() / 60.0;
    (minutes >= 1.0).then(|| (sent as f64 / minutes * 10.0).round() / 10.0)
}

/// One resource on the map. Only facts a reader of the manifest already sees: no secret, no
/// credential, no URL path or query (a token can ride in one), only a source's host.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FlowNode {
    /// `{project}/{kind}/{name}`, what the edges name.
    pub id: String,
    pub kind: FlowKind,
    pub name: String,
    pub project: String,
    /// Another project's resource the caller may not read: its name, nothing else.
    pub opaque: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The space an Endpoint reads and writes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<FlowRole>,
    /// A DataSource's connection type (`mqtt`, `http`, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_type: Option<String>,
    /// The hosts a DataSource connects to, without user, path or query.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<String>,
    /// The topic filters an MQTT DataSource subscribes to.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub topics: Vec<String>,
    /// A pipeline's phase as the reconciler last set it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    /// `false` for a paused pipeline.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run: Option<FlowRun>,
    /// An Endpoint's address segment and its audience.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audience: Option<String>,
}

/// How data moves along an edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum FlowRelation {
    /// An MQTT broker a DataSource reads (PL-76).
    Connects,
    /// A DataSource a pipeline reads.
    Feeds,
    /// A space an Endpoint serves.
    Exposes,
    /// An Endpoint a pipeline reads (PL-31).
    Reads,
    /// An Endpoint a pipeline writes (PL-18, PL-32).
    Writes,
    /// A space an Endpoint writes into.
    Stores,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum FlowGrantState {
    Granted,
    /// No Policy gives the pipelines account the operation: the runner is refused.
    Missing,
    /// The Endpoint is another project's, and its owner's grant decides (PL-32).
    External,
}

/// What permits a pipeline to read or write through an Endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FlowGrant {
    pub state: FlowGrantState,
    pub endpoint: String,
    /// The operation the edge needs, as a Policy names it.
    pub operation: String,
    /// The Policy that grants it, named only to a caller who may read Policy in its space.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy: Option<String>,
    /// What to change when it is missing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FlowEdge {
    pub from: String,
    pub to: String,
    pub relation: FlowRelation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant: Option<FlowGrant>,
}

/// The map of one project, or of the paths that touch one of its spaces.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FlowMap {
    pub project: String,
    pub nodes: Vec<FlowNode>,
    pub edges: Vec<FlowEdge>,
}

impl FlowMap {
    /// The names of the map's own pipelines: the ones whose runs the caller may see.
    pub fn pipelines(&self) -> Vec<String> {
        self.nodes
            .iter()
            .filter(|n| n.kind == FlowKind::Pipeline && n.project == self.project)
            .map(|n| n.name.clone())
            .collect()
    }

    /// Sets each own pipeline's latest run from `runs`, keyed by pipeline name.
    pub fn attach_runs(&mut self, mut runs: BTreeMap<String, FlowRun>) {
        for node in &mut self.nodes {
            if node.kind == FlowKind::Pipeline && node.project == self.project {
                node.last_run = runs.remove(&node.name);
            }
        }
    }
}

/// Whether the caller may read `kind` in `space` of the map's project (`None`: a kind that
/// lives outside a space), and whether they may read anything of another project.
pub struct Reader<'a> {
    pub may: &'a dyn Fn(&str, Option<&str>) -> bool,
    pub may_project: &'a dyn Fn(&str) -> bool,
}

/// The map of `project` as `reader` may see it, only the paths touching `space` when one is
/// given. Runs come after, for the pipelines on the map: [`FlowMap::pipelines`], [`FlowMap::attach_runs`].
pub fn build(mirror: &Mirror, project: &str, reader: &Reader<'_>, space: Option<&str>) -> FlowMap {
    let mut map = Builder::new(mirror, project, reader);
    for pipeline in map.list("Pipeline") {
        map.pipeline(&pipeline, space);
    }
    match space {
        Some(space) => {
            map.space(project, space);
        }
        None => map.islands(),
    }
    map.finish()
}

struct Builder<'a> {
    mirror: &'a Mirror,
    project: &'a str,
    reader: &'a Reader<'a>,
    policies: Vec<(String, PolicySpec)>,
    nodes: BTreeMap<String, FlowNode>,
    edges: BTreeSet<(String, String, FlowRelation)>,
    grants: BTreeMap<(String, String, FlowRelation), FlowGrant>,
}

fn id(project: &str, kind: FlowKind, name: &str) -> String {
    format!("{project}/{}/{name}", kind.as_str())
}

fn bare(project: &str, kind: FlowKind, name: &str) -> FlowNode {
    FlowNode {
        id: id(project, kind, name),
        kind,
        name: name.to_owned(),
        project: project.to_owned(),
        opaque: false,
        title: None,
        space: None,
        role: None,
        source_type: None,
        hosts: Vec::new(),
        topics: Vec::new(),
        phase: None,
        enabled: None,
        last_run: None,
        slug: None,
        audience: None,
    }
}

fn text(value: &Value, pointer: &str) -> Option<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn texts(value: &Value, pointer: &str) -> Vec<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

/// The host and port of a connection URL, never its user, path or query.
fn host(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let host = parsed.host_str()?;
    Some(match parsed.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    })
}

/// The operations an edge needs: the runner writes with `upsertBatch` (`gateway_output`) and a
/// derived source reads with a query.
const WRITE: &[Operation] = &[Operation::UpsertBatch];
const READ: &[Operation] = &[Operation::QueryEntity, Operation::QueryBatch];

fn grants_any(policy: &PolicySpec, wanted: &[Operation]) -> bool {
    policy.operations.iter().any(|op| match op {
        OperationRef::Single(single) => wanted.contains(single),
        OperationRef::Group(group) => group.operations().iter().any(|o| wanted.contains(o)),
    })
}

/// The Policy giving the pipelines account one of `wanted` in `space`, if any.
fn policy_for<'p>(
    policies: &'p [(String, PolicySpec)],
    space: &str,
    wanted: &[Operation],
) -> Option<&'p str> {
    policies
        .iter()
        .find(|(_, policy)| {
            !policy.effect.is_prohibition()
                && policy.context_space_ref.name() == space
                && policy.assignee.kind == PrincipalKind::ServiceAccount
                && policy.assignee.id == crate::reconciler::streams::PIPELINE_ACCOUNT
                && grants_any(policy, wanted)
        })
        .map(|(name, _)| name.as_str())
}

fn wire(operation: Operation) -> String {
    serde_json::to_value(operation)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// The grant of an edge through `endpoint` into `space` of the map's own project.
/// The Policy is named only to a caller who may read Policy in `space` (`show_policy`).
fn grant(
    policies: &[(String, PolicySpec)],
    endpoint: &str,
    space: &str,
    wanted: &[Operation],
    show_policy: bool,
) -> FlowGrant {
    let operation = wire(wanted[0]);
    match policy_for(policies, space, wanted) {
        Some(policy) => FlowGrant {
            state: FlowGrantState::Granted,
            endpoint: endpoint.to_owned(),
            operation,
            policy: show_policy.then(|| policy.to_owned()),
            fix: None,
        },
        None => FlowGrant {
            state: FlowGrantState::Missing,
            endpoint: endpoint.to_owned(),
            fix: Some(format!(
                "add a Policy on space {space} that grants the ServiceAccount {} {operation}",
                crate::reconciler::streams::PIPELINE_ACCOUNT
            )),
            operation,
            policy: None,
        },
    }
}

/// The role of a pipeline: KPI when it writes an indicator space or type, derived when it reads
/// a space, else it ingests.
fn role(spec: &PipelineSpec, target_spaces: &[String]) -> FlowRole {
    let kpi = target_spaces.iter().any(|s| s.ends_with("-kpi"))
        || spec
            .outputs()
            .iter()
            .any(|o| o.entity_type.as_deref() == Some("KeyPerformanceIndicator"));
    if kpi {
        FlowRole::Kpi
    } else if spec.sources().iter().any(|s| s.endpoint_ref.is_some()) {
        FlowRole::Derived
    } else {
        FlowRole::Ingest
    }
}

impl<'a> Builder<'a> {
    fn new(mirror: &'a Mirror, project: &'a str, reader: &'a Reader<'a>) -> Self {
        let policies = mirror
            .list(project, "Policy", &ListOptions::default())
            .items
            .into_iter()
            .filter_map(|p| {
                serde_json::from_value::<PolicySpec>(p.spec.clone())
                    .ok()
                    .map(|spec| (p.metadata.name, spec))
            })
            // One grant per rule, `{policy}#{n}` (ADR-N-057); an invalid shape grants nothing.
            .flat_map(|(name, spec)| spec.expand(&name).unwrap_or_default())
            .collect();
        Self {
            mirror,
            project,
            reader,
            policies,
            nodes: BTreeMap::new(),
            edges: BTreeSet::new(),
            grants: BTreeMap::new(),
        }
    }

    fn list(&self, kind: &str) -> Vec<crate::resource::ResourceEnvelope> {
        self.mirror
            .list(self.project, kind, &ListOptions::default())
            .items
    }

    fn may(&self, kind: FlowKind, space: Option<&str>) -> bool {
        (self.reader.may)(kind.as_str(), space)
    }

    fn edge(&mut self, from: &str, to: &str, relation: FlowRelation, grant: Option<FlowGrant>) {
        let key = (from.to_owned(), to.to_owned(), relation);
        if let Some(grant) = grant {
            self.grants.insert(key.clone(), grant);
        }
        self.edges.insert(key);
    }

    /// The space an Endpoint of the map's project serves, from the mirror.
    fn space_of(&self, project: &str, endpoint: &str) -> Option<String> {
        self.mirror
            .get(project, "Endpoint", endpoint)
            .and_then(|ep| crate::api::assistant::ref_name(&ep.spec["contextSpaceRef"]))
    }

    /// A readable space of this project, as a node; its id either way.
    fn space(&mut self, project: &str, name: &str) -> Option<String> {
        if !self.may(FlowKind::ContextSpace, Some(name)) {
            return None;
        }
        let envelope = self.mirror.get(project, "ContextSpace", name)?;
        let mut node = bare(project, FlowKind::ContextSpace, name);
        node.title = text(&envelope.spec, "/title");
        node.role = name.ends_with("-kpi").then_some(FlowRole::Kpi);
        let key = node.id.clone();
        self.nodes.entry(key.clone()).or_insert(node);
        Some(key)
    }

    /// An Endpoint of this project with its space, when the caller reads both.
    fn endpoint(&mut self, name: &str) -> Option<(String, String)> {
        let envelope = self.mirror.get(self.project, "Endpoint", name)?;
        let space = crate::api::assistant::ref_name(&envelope.spec["contextSpaceRef"])?;
        if !self.may(FlowKind::Endpoint, Some(&space)) {
            return None;
        }
        let mut node = bare(self.project, FlowKind::Endpoint, name);
        node.title = text(&envelope.spec, "/title");
        node.space = Some(space.clone());
        node.slug = text(&envelope.spec, "/slug");
        node.audience = text(&envelope.spec, "/audience");
        let key = node.id.clone();
        self.nodes.entry(key.clone()).or_insert(node);
        Some((key, space))
    }

    /// An Endpoint of another project a pipeline here reads: whole for a reader of that project,
    /// its name alone for anyone else.
    fn foreign_endpoint(&mut self, project: &str, name: &str) -> String {
        let readable = (self.reader.may_project)(project);
        let mut node = bare(project, FlowKind::Endpoint, name);
        node.opaque = !readable;
        if readable {
            node.space = self.space_of(project, name);
        }
        let key = node.id.clone();
        self.nodes.entry(key.clone()).or_insert(node);
        key
    }

    fn data_source(&mut self, name: &str) -> Option<String> {
        if !self.may(FlowKind::DataSource, None) {
            return None;
        }
        let envelope = self.mirror.get(self.project, "DataSource", name)?;
        let spec = &envelope.spec;
        let mut node = bare(self.project, FlowKind::DataSource, name);
        node.title = text(spec, "/title");
        node.source_type = text(spec, "/type");
        node.topics = texts(spec, "/mqtt/topics");
        let mut urls = texts(spec, "/mqtt/urls");
        urls.extend(
            ["/http/url", "/webSocket/url", "/gtfsRt/url"]
                .iter()
                .filter_map(|p| text(spec, p)),
        );
        node.hosts = urls.iter().filter_map(|u| host(u)).collect();
        let key = node.id.clone();
        self.nodes.entry(key.clone()).or_insert(node);
        if let Some(broker) = text(spec, "/mqtt/brokerRef/name") {
            if let Some(from) = self.broker(&broker) {
                self.edge(&from, &key, FlowRelation::Connects, None);
            }
        }
        Some(key)
    }

    fn broker(&mut self, name: &str) -> Option<String> {
        if !self.may(FlowKind::MqttBroker, None) {
            return None;
        }
        let envelope = self.mirror.get(self.project, "MqttBroker", name)?;
        let mut node = bare(self.project, FlowKind::MqttBroker, name);
        node.title = text(&envelope.spec, "/title");
        let key = node.id.clone();
        self.nodes.entry(key.clone()).or_insert(node);
        Some(key)
    }

    /// The spaces a pipeline reads (through its source Endpoints of this project) and writes.
    fn spaces_touched(&self, spec: &PipelineSpec) -> (Vec<String>, Vec<String>) {
        let read = spec
            .sources()
            .iter()
            .filter_map(|s| s.endpoint_ref.as_ref())
            .filter(|r| r.namespace().is_none_or(|ns| ns == self.project))
            .filter_map(|r| self.space_of(self.project, r.name()))
            .collect();
        let written = spec
            .outputs()
            .iter()
            .filter_map(|o| self.space_of(self.project, o.target_endpoint.local_id()))
            .collect();
        (read, written)
    }

    fn pipeline(&mut self, envelope: &crate::resource::ResourceEnvelope, only: Option<&str>) {
        let Ok(spec) = serde_json::from_value::<PipelineSpec>(envelope.spec.clone()) else {
            return;
        };
        let (read, written) = self.spaces_touched(&spec);
        let readable = self.may(FlowKind::Pipeline, None)
            || written
                .iter()
                .any(|s| self.may(FlowKind::Pipeline, Some(s)));
        let touches = only.is_none_or(|s| read.iter().chain(&written).any(|t| t == s));
        if !readable || !touches {
            return;
        }
        let name = &envelope.metadata.name;
        let mut node = bare(self.project, FlowKind::Pipeline, name);
        node.title = text(&envelope.spec, "/title");
        node.role = Some(role(&spec, &written));
        node.enabled = Some(spec.enabled);
        node.phase = envelope
            .status
            .as_ref()
            .map(|s| crate::resource::phase_str(s.phase).to_owned());
        let key = node.id.clone();
        self.nodes.insert(key.clone(), node);
        self.inputs(&spec, &key);
        self.outputs(&spec, &key);
    }

    fn inputs(&mut self, spec: &PipelineSpec, pipeline: &str) {
        for source in spec.sources() {
            if let Some(ds) = source.data_source_ref.as_ref() {
                if let Some(from) = self.data_source(ds.name()) {
                    self.edge(&from, pipeline, FlowRelation::Feeds, None);
                }
            }
            let Some(reference) = source.endpoint_ref.as_ref() else {
                continue;
            };
            match reference.namespace().filter(|ns| *ns != self.project) {
                Some(other) => {
                    let from = self.foreign_endpoint(other, reference.name());
                    let grant = FlowGrant {
                        state: FlowGrantState::External,
                        endpoint: reference.name().to_owned(),
                        operation: wire(READ[0]),
                        policy: None,
                        fix: None,
                    };
                    self.edge(&from, pipeline, FlowRelation::Reads, Some(grant));
                }
                None => self.own_input(reference.name(), pipeline),
            }
        }
    }

    fn own_input(&mut self, endpoint: &str, pipeline: &str) {
        let Some((from, space)) = self.endpoint(endpoint) else {
            return;
        };
        let show = (self.reader.may)("Policy", Some(&space));
        let grant = grant(&self.policies, endpoint, &space, READ, show);
        self.edge(&from, pipeline, FlowRelation::Reads, Some(grant));
        if let Some(space_id) = self.space(self.project, &space) {
            self.edge(&space_id, &from, FlowRelation::Exposes, None);
        }
    }

    fn outputs(&mut self, spec: &PipelineSpec, pipeline: &str) {
        for output in spec.outputs() {
            let endpoint = output.target_endpoint.local_id();
            let Some((to, space)) = self.endpoint(endpoint) else {
                continue;
            };
            let show = (self.reader.may)("Policy", Some(&space));
            let grant = grant(&self.policies, endpoint, &space, WRITE, show);
            self.edge(pipeline, &to, FlowRelation::Writes, Some(grant));
            if let Some(space_id) = self.space(self.project, &space) {
                self.edge(&to, &space_id, FlowRelation::Stores, None);
            }
        }
    }

    /// Brokers, sources and spaces no pipeline uses yet: where a new path starts.
    fn islands(&mut self) {
        for broker in self.list("MqttBroker") {
            self.broker(&broker.metadata.name);
        }
        for source in self.list("DataSource") {
            self.data_source(&source.metadata.name);
        }
        for space in self.list("ContextSpace") {
            self.space(self.project, &space.metadata.name);
        }
    }

    fn finish(mut self) -> FlowMap {
        let edges = std::mem::take(&mut self.edges)
            .into_iter()
            .map(|key| FlowEdge {
                grant: self.grants.remove(&key),
                from: key.0,
                to: key.1,
                relation: key.2,
            })
            .collect();
        FlowMap {
            project: self.project.to_owned(),
            nodes: self.nodes.into_values().collect(),
            edges,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
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

    fn endpoint(project: &str, name: &str, space: &str) -> ResourceEnvelope {
        manifest(
            project,
            "Endpoint",
            name,
            json!({ "contextSpaceRef": { "kind": "ContextSpace", "name": space }, "slug": name }),
        )
    }

    fn write_grant(name: &str, space: &str) -> ResourceEnvelope {
        manifest(
            "helsinki",
            "Policy",
            name,
            json!({
                "contextSpaceRef": { "kind": "ContextSpace", "name": space },
                "assigner": "did:web:hel.fi",
                "assignee": { "kind": "serviceAccount", "id": "pipelines" },
                "operations": ["upsertBatch", "queryEntity"],
            }),
        )
    }

    fn target(endpoint: &str) -> Value {
        json!(format!("urn:ngsi-ld:Endpoint:hel.fi:helsinki:{endpoint}"))
    }

    /// Helsinki: the HSL broker feeding vehicles, a derived pipeline from vehicles into
    /// stops (no grant there), a KPI pipeline from vehicles into transport-kpi, and a pipeline
    /// reading espoo's shared Endpoint.
    fn helsinki() -> Mirror {
        let mirror = Mirror::new();
        for space in ["vehicles", "stops", "transport-kpi", "empty"] {
            mirror.upsert(manifest("helsinki", "ContextSpace", space, json!({})));
        }
        mirror.upsert(manifest("espoo", "ContextSpace", "roads", json!({})));
        mirror.upsert(endpoint("espoo", "ep-roads", "roads"));
        for (ep, space) in [
            ("ep-vehicles", "vehicles"),
            ("ep-stops", "stops"),
            ("ep-kpi", "transport-kpi"),
        ] {
            mirror.upsert(endpoint("helsinki", ep, space));
        }
        mirror.upsert(write_grant("vehicles-pipelines", "vehicles"));
        mirror.upsert(write_grant("kpi-pipelines", "transport-kpi"));
        mirror.upsert(manifest("helsinki", "MqttBroker", "hsl", json!({})));
        mirror.upsert(manifest(
            "helsinki",
            "DataSource",
            "hfp",
            json!({ "type": "mqtt", "mqtt": {
                "urls": ["tls://reader:hunter2@mqtt.hsl.fi:8883/x?token=abc"],
                "brokerRef": { "kind": "MqttBroker", "name": "hsl" },
                "topics": ["/hfp/v2/journey/ongoing/vp/#"] } }),
        ));
        let mut ingest = manifest(
            "helsinki",
            "Pipeline",
            "hfp-ingest",
            json!({ "class": "resident", "sources": [{ "dataSourceRef": "hfp" }],
                    "outputs": [{ "targetEndpoint": target("ep-vehicles") }] }),
        );
        ingest.status = serde_json::from_value(json!({ "phase": "Live" })).ok();
        mirror.upsert(ingest);
        mirror.upsert(manifest(
            "helsinki",
            "Pipeline",
            "stop-delays",
            json!({ "class": "scheduled", "enabled": false,
                    "sources": [{ "endpointRef": "ep-vehicles", "query": { "type": "Vehicle" } }],
                    "outputs": [{ "targetEndpoint": target("ep-stops") }] }),
        ));
        mirror.upsert(manifest(
            "helsinki",
            "Pipeline",
            "fleet-kpi",
            json!({ "class": "scheduled",
                    "sources": [{ "endpointRef": "ep-vehicles", "query": { "type": "Vehicle" } }],
                    "outputs": [{ "targetEndpoint": target("ep-kpi"), "type": "KeyPerformanceIndicator" }] }),
        ));
        mirror.upsert(manifest(
            "helsinki",
            "Pipeline",
            "roads-in",
            json!({ "class": "scheduled",
                    "sources": [{ "endpointRef": { "kind": "Endpoint", "name": "ep-roads", "namespace": "espoo" },
                                  "query": { "type": "Road" } }],
                    "outputs": [{ "targetEndpoint": target("ep-vehicles") }] }),
        ));
        mirror
    }

    fn everyone(_: &str, _: Option<&str>) -> bool {
        true
    }

    fn map_as(
        may: &dyn Fn(&str, Option<&str>) -> bool,
        may_project: &dyn Fn(&str) -> bool,
        space: Option<&str>,
    ) -> FlowMap {
        let runs = BTreeMap::from([(
            "hfp-ingest".to_owned(),
            FlowRun {
                at: "2026-10-21T04:00:00Z".to_owned(),
                sent: 1200,
                rejected: 3,
                failed: 0,
                per_minute: Some(20.0),
            },
        )]);
        let mut map = build(&helsinki(), "helsinki", &Reader { may, may_project }, space);
        map.attach_runs(runs);
        map
    }

    fn node<'m>(map: &'m FlowMap, id: &str) -> &'m FlowNode {
        map.nodes
            .iter()
            .find(|n| n.id == id)
            .unwrap_or_else(|| panic!("no node {id} in {:?}", map.nodes))
    }

    fn edge<'m>(map: &'m FlowMap, from: &str, to: &str) -> &'m FlowEdge {
        map.edges
            .iter()
            .find(|e| e.from == from && e.to == to)
            .unwrap_or_else(|| panic!("no edge {from} -> {to} in {:?}", map.edges))
    }

    #[test]
    fn the_mqtt_path_runs_broker_source_pipeline_endpoint_space_with_its_grant() {
        let map = map_as(&everyone, &|_| true, None);
        edge(&map, "helsinki/MqttBroker/hsl", "helsinki/DataSource/hfp");
        edge(
            &map,
            "helsinki/DataSource/hfp",
            "helsinki/Pipeline/hfp-ingest",
        );
        let write = edge(
            &map,
            "helsinki/Pipeline/hfp-ingest",
            "helsinki/Endpoint/ep-vehicles",
        );
        let grant = write.grant.as_ref().expect("a write carries its grant");
        assert_eq!(grant.state, FlowGrantState::Granted);
        assert_eq!(grant.policy.as_deref(), Some("vehicles-pipelines"));
        assert_eq!(grant.operation, "upsertBatch");
        edge(
            &map,
            "helsinki/Endpoint/ep-vehicles",
            "helsinki/ContextSpace/vehicles",
        );

        let ingest = node(&map, "helsinki/Pipeline/hfp-ingest");
        assert_eq!(ingest.role, Some(FlowRole::Ingest));
        assert_eq!(ingest.phase.as_deref(), Some("Live"));
        assert_eq!(ingest.last_run.as_ref().map(|r| r.sent), Some(1200));
    }

    #[test]
    fn a_source_shows_its_host_and_topics_never_its_credentials_or_path() {
        let map = map_as(&everyone, &|_| true, None);
        let source = node(&map, "helsinki/DataSource/hfp");
        assert_eq!(source.hosts, vec!["mqtt.hsl.fi:8883".to_owned()]);
        assert_eq!(
            source.topics,
            vec!["/hfp/v2/journey/ongoing/vp/#".to_owned()]
        );
        let body = serde_json::to_string(&map).expect("serializes");
        for secret in ["hunter2", "reader", "token=abc"] {
            assert!(!body.contains(secret), "{secret} leaked: {body}");
        }
    }

    #[test]
    fn a_cross_space_derived_pipeline_without_a_grant_is_red_with_the_fix() {
        let map = map_as(&everyone, &|_| true, None);
        let derived = node(&map, "helsinki/Pipeline/stop-delays");
        assert_eq!(derived.role, Some(FlowRole::Derived));
        assert_eq!(derived.enabled, Some(false), "paused");
        let read = edge(
            &map,
            "helsinki/Endpoint/ep-vehicles",
            "helsinki/Pipeline/stop-delays",
        );
        assert_eq!(
            read.grant.as_ref().map(|g| g.state),
            Some(FlowGrantState::Granted)
        );
        let write = edge(
            &map,
            "helsinki/Pipeline/stop-delays",
            "helsinki/Endpoint/ep-stops",
        );
        let grant = write.grant.as_ref().expect("grant");
        assert_eq!(grant.state, FlowGrantState::Missing);
        assert_eq!(
            grant.fix.as_deref(),
            Some(
                "add a Policy on space stops that grants the ServiceAccount pipelines upsertBatch"
            )
        );
        edge(
            &map,
            "helsinki/ContextSpace/vehicles",
            "helsinki/Endpoint/ep-vehicles",
        );
    }

    #[test]
    fn a_kpi_pipeline_and_its_indicator_space_are_kpi() {
        let map = map_as(&everyone, &|_| true, None);
        assert_eq!(
            node(&map, "helsinki/Pipeline/fleet-kpi").role,
            Some(FlowRole::Kpi)
        );
        assert_eq!(
            node(&map, "helsinki/ContextSpace/transport-kpi").role,
            Some(FlowRole::Kpi)
        );
    }

    #[test]
    fn another_projects_endpoint_is_opaque_unless_the_caller_reads_that_project() {
        let map = map_as(&everyone, &|_| false, None);
        let roads = node(&map, "espoo/Endpoint/ep-roads");
        assert!(roads.opaque);
        assert_eq!(roads.space, None, "nothing but the name");
        let read = edge(
            &map,
            "espoo/Endpoint/ep-roads",
            "helsinki/Pipeline/roads-in",
        );
        assert_eq!(
            read.grant.as_ref().map(|g| g.state),
            Some(FlowGrantState::External)
        );
        assert!(!map.nodes.iter().any(|n| n.id == "espoo/ContextSpace/roads"));

        let map = map_as(&everyone, &|p| p == "espoo", None);
        let roads = node(&map, "espoo/Endpoint/ep-roads");
        assert!(!roads.opaque);
        assert_eq!(roads.space.as_deref(), Some("roads"));
    }

    #[test]
    fn a_steward_of_one_space_sees_only_its_pipelines_and_nodes() {
        // Bound to `stops`: Pipelines, Endpoints and the space there, nothing project-wide.
        let stops = |_: &str, space: Option<&str>| space == Some("stops");
        let map = map_as(&stops, &|_| false, None);
        let ids: BTreeSet<&str> = map.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(
            ids,
            BTreeSet::from([
                "helsinki/Pipeline/stop-delays",
                "helsinki/Endpoint/ep-stops",
                "helsinki/ContextSpace/stops",
            ])
        );
        assert!(map
            .edges
            .iter()
            .all(|e| ids.contains(e.from.as_str()) && ids.contains(e.to.as_str())));
    }

    #[test]
    fn the_policy_is_named_only_to_a_caller_who_reads_policy() {
        let no_policy = |kind: &str, _: Option<&str>| kind != "Policy";
        let map = map_as(&no_policy, &|_| true, None);
        let write = edge(
            &map,
            "helsinki/Pipeline/hfp-ingest",
            "helsinki/Endpoint/ep-vehicles",
        );
        let grant = write.grant.as_ref().expect("a write carries its grant");
        assert_eq!(grant.state, FlowGrantState::Granted);
        assert_eq!(grant.policy, None);
        assert!(map
            .edges
            .iter()
            .filter_map(|e| e.grant.as_ref())
            .all(|g| g.policy.is_none()));
    }

    #[test]
    fn runs_are_asked_and_attached_only_for_pipelines_on_the_map() {
        let stops = |_: &str, space: Option<&str>| space == Some("stops");
        let map = map_as(&stops, &|_| false, None);
        assert_eq!(map.pipelines(), vec!["stop-delays".to_owned()]);
        assert!(map.nodes.iter().all(|n| n.last_run.is_none()));
    }

    #[test]
    fn a_space_filter_keeps_the_paths_through_that_space() {
        let map = map_as(&everyone, &|_| true, Some("stops"));
        let pipelines: BTreeSet<&str> = map
            .nodes
            .iter()
            .filter(|n| n.kind == FlowKind::Pipeline)
            .map(|n| n.name.as_str())
            .collect();
        assert_eq!(pipelines, BTreeSet::from(["stop-delays"]));
        assert!(!map
            .nodes
            .iter()
            .any(|n| n.id == "helsinki/ContextSpace/empty"));

        let map = map_as(&everyone, &|_| true, Some("empty"));
        assert_eq!(map.edges, Vec::new());
        node(&map, "helsinki/ContextSpace/empty");
    }

    #[test]
    fn unused_brokers_sources_and_spaces_are_on_the_whole_map() {
        let map = map_as(&everyone, &|_| true, None);
        node(&map, "helsinki/ContextSpace/empty");
    }

    #[test]
    fn a_rate_needs_a_run_of_a_minute() {
        assert_eq!(
            per_minute(120, "2026-10-21T04:00:00Z", "2026-10-21T04:02:00Z"),
            Some(60.0)
        );
        assert_eq!(
            per_minute(120, "2026-10-21T04:00:00Z", "2026-10-21T04:00:30Z"),
            None
        );
        assert_eq!(per_minute(1, "not a time", "2026-10-21T04:00:30Z"), None);
    }

    /// PL-105: a KPI over several sources is one node with one read edge from each source
    /// Endpoint, each with its grant, and writes into the KPI space.
    #[test]
    fn a_kpi_over_two_sources_reads_from_each_endpoint() {
        let mirror = helsinki();
        mirror.upsert(manifest(
            "helsinki",
            "Pipeline",
            "delay-per-stop",
            json!({ "class": "scheduled", "schedule": "0 * * * *",
                    "sources": [
                        { "name": "vehicles", "endpointRef": "ep-vehicles", "query": { "type": "Vehicle", "attrs": ["delay", "stop"] } },
                        { "name": "stops", "endpointRef": "ep-stops", "query": { "type": "TransitStop", "attrs": ["stop"] } }
                    ],
                    "steps": [{ "kind": "kpi", "kpi": { "name": "delay-per-stop",
                        "formula": "sum(vehicles.delay) / count(stops)", "unitCode": "C62",
                        "join": { "by": "entity", "keys": { "vehicles": "stop", "stops": "id" } } } }],
                    "outputs": [{ "targetEndpoint": target("ep-kpi") }] }),
        ));
        let reader = Reader {
            may: &everyone,
            may_project: &|_| true,
        };
        let map = build(&mirror, "helsinki", &reader, None);
        let kpi = "helsinki/Pipeline/delay-per-stop";
        assert_eq!(node(&map, kpi).role, Some(FlowRole::Kpi));
        for from in [
            "helsinki/Endpoint/ep-vehicles",
            "helsinki/Endpoint/ep-stops",
        ] {
            assert_eq!(edge(&map, from, kpi).relation, FlowRelation::Reads);
        }
        edge(&map, kpi, "helsinki/Endpoint/ep-kpi");
    }
}
