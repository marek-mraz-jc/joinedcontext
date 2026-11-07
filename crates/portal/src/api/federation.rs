//! The `jc_federation_graph` operation: who reads whose data, as a graph
//! (T-0304, UI-27, EP-71, PF-48; T-4453 removed the route beside it).
//!
//! Managing a registration is not a second write path. A `ContextSourceRegistration` is a
//! manifest like every other one, so it is listed, created and removed through
//! `/api/v1/projects/{project}/csrs`, which means a new federation link arrives as a change
//! proposal a steward approves rather than as a setting somebody flips (CC-03, CC-08).
//!
//! What only this operation can answer is the picture across five kinds at once. It is a
//! projection and holds no state: every node and every edge comes from a manifest in the
//! mirror plus the phase that manifest last reported, so the graph cannot become a second
//! opinion about who talks to whom.
//!
//! Nothing secret is in the answer, by construction rather than by filtering. A registration
//! contributes its name, its mode and whether it authenticates; never a token, never a
//! resolved `secretRef`, and never the address of an external source, which is why an
//! external node is named after the registration that reaches it and not after its URL
//! (UI-27, EP-71).

use crate::resource::reference_name;
use jc_core::Urn;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;
use utoipa::ToSchema;

use crate::resource::{phase_str, ResourceEnvelope};
use crate::state::AppState;
use crate::store::ListOptions;

/// The federation of one project (UI-27).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FederationGraph {
    /// Every object the federation touches, keyed by `kind/name`.
    pub nodes: Vec<Node>,
    /// Directed edges, each naming the manifest it was read from.
    pub edges: Vec<Edge>,
}

/// One object in the graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    /// `kind/name`, stable across runs so a selection survives a refresh.
    pub id: String,
    /// Manifest kind, or `ExternalSource` for a source outside this platform.
    pub kind: String,
    /// Manifest name. For an external source, the name of the registration that reaches it.
    pub name: String,
    /// The object's title as the manifest carries it, one entry per language. Absent when the
    /// manifest has none: the UI falls back to the name rather than the Portal inventing one.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub title: BTreeMap<String, String>,
    /// `ok`, `degraded` or `unknown` (UI-27).
    pub health: NodeHealth,
    /// The lifecycle phase the object last reported, for the card's status chip (UI-25, UI-28).
    /// Absent for an external source and for an object that has reported nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    /// For a `ContextSourceRegistration`, what the card may say about it. Never a credential.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registration: Option<RegistrationCard>,
}

/// What a registration's card shows (UI-27, PF-48).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RegistrationCard {
    /// `inclusive`, `exclusive`, `auxiliary` or `redirect`.
    pub mode: String,
    /// `serviceAccount` or `caller` (PF-48).
    pub identity: String,
    /// The entity types the source is claimed to hold.
    pub types: Vec<String>,
    /// Whether the source is on this platform or outside it.
    pub external: bool,
}

/// How an object last reported (UI-27).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum NodeHealth {
    /// The object reported `Live`.
    Ok,
    /// The object reported `Error`.
    Degraded,
    /// The object has not reported yet, or is still converging.
    Unknown,
}

/// One directed relation between two nodes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Edge {
    /// Node id the edge leaves.
    pub from: String,
    /// Node id the edge enters.
    pub to: String,
    /// `registers`, `serves`, `feeds`, `consumes` or `publishes`.
    pub kind: EdgeKind,
    /// The manifest this edge was read from, as `kind/name`, so a reader can open it.
    pub manifest: String,
}

/// What one edge means (UI-27).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum EdgeKind {
    /// A registration makes a source's data answerable in a space.
    Registers,
    /// An endpoint answers over a space.
    Serves,
    /// A pipeline writes into an endpoint.
    Feeds,
    /// An app reads a space.
    Consumes,
    /// An endpoint is published as a dataset of a CKAN instance (UI-28, EP-62).
    Publishes,
}

/// The federation of one project, as the `jc_federation_graph` operation answers it (AG-59, T-0840).
///
/// Drawn for one reader (T-5698): a manifest `effective` does not read is no node, and an edge
/// that starts or ends at one is not drawn, so a reader bound to one context space sees no other
/// space's names. A reference to a manifest nobody stored still draws its dangling edge.
pub fn graph_of(
    state: &AppState,
    project: &str,
    effective: &crate::permissions::Effective,
) -> FederationGraph {
    let project = project.to_owned();
    let opts = ListOptions::default();
    let hidden = std::cell::RefCell::new(BTreeSet::new());
    let of = |kind: &str| {
        let (readable, unreadable): (Vec<_>, Vec<_>) = state
            .mirror
            .list(&project, kind, &opts)
            .items
            .into_iter()
            .partition(|env| effective.may_read_envelope(kind, env));
        hidden
            .borrow_mut()
            .extend(unreadable.iter().map(|env| id(kind, &env.metadata.name)));
        readable
    };

    let mut graph = Builder::default();
    for space in of("ContextSpace") {
        graph.node(space, "ContextSpace", None);
    }

    let mut catalogues = BTreeSet::new();
    for endpoint in of("Endpoint") {
        if let Some(space) = endpoint
            .spec
            .get("contextSpaceRef")
            .and_then(reference_name)
        {
            graph.edge(
                id("Endpoint", &endpoint.metadata.name),
                id("ContextSpace", space),
                EdgeKind::Serves,
                id("Endpoint", &endpoint.metadata.name),
            );
        }
        if let Some(instance) = endpoint
            .spec
            .pointer("/publish/ckan/instanceRef")
            .and_then(reference_name)
        {
            graph.edge(
                id("Endpoint", &endpoint.metadata.name),
                id("CkanInstance", instance),
                EdgeKind::Publishes,
                id("Endpoint", &endpoint.metadata.name),
            );
            catalogues.insert(instance.to_owned());
        }
        graph.node(endpoint, "Endpoint", None);
    }

    // A catalogue is part of the federation only through what is published to it; one nobody
    // publishes to is configuration, not a link, and stays off the picture.
    for instance in of("CkanInstance") {
        if catalogues.contains(&instance.metadata.name) {
            graph.node(instance, "CkanInstance", None);
        }
    }

    for csr in of("ContextSourceRegistration") {
        registration(&mut graph, csr);
    }

    for pipeline in of("Pipeline") {
        if let Some(endpoint) = endpoint_of(&pipeline.spec) {
            graph.edge(
                id("Pipeline", &pipeline.metadata.name),
                id("Endpoint", &endpoint),
                EdgeKind::Feeds,
                id("Pipeline", &pipeline.metadata.name),
            );
        }
        graph.node(pipeline, "Pipeline", None);
    }

    for app in of("App") {
        for need in app
            .spec
            .get("dataNeeds")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            if let Some(space) = need.get("contextSpaceRef").and_then(reference_name) {
                graph.edge(
                    id("App", &app.metadata.name),
                    id("ContextSpace", space),
                    EdgeKind::Consumes,
                    id("App", &app.metadata.name),
                );
            }
        }
        graph.node(app, "App", None);
    }

    let hidden = hidden.into_inner();
    graph
        .edges
        .retain(|edge| !hidden.contains(&edge.from) && !hidden.contains(&edge.to));
    graph.finish()
}

/// One registration: a node for the registration itself, an edge into the space it feeds, and
/// an edge out to what it reaches.
///
/// A registration whose target has left the repository still draws its edge. A dangling
/// reference is exactly what this view exists to make visible, and hiding it would make a
/// broken federation look like no federation.
fn registration(graph: &mut Builder, csr: ResourceEnvelope) {
    let name = csr.metadata.name.clone();
    let node = id("ContextSourceRegistration", &name);

    if let Some(space) = csr.spec.get("contextSpaceRef").and_then(reference_name) {
        graph.edge(
            node.clone(),
            id("ContextSpace", space),
            EdgeKind::Registers,
            node.clone(),
        );
    }

    // The target: an Endpoint on this platform, or a source outside it. The external node is
    // named after the registration, never after the URL — the same rule provenance follows, so
    // what a card shows and what an answer says a datum came from are one string (EP-71).
    let external = csr.spec.get("endpointRef").is_none();
    let target = match csr.spec.get("endpointRef").and_then(reference_name) {
        Some(endpoint) => id("Endpoint", endpoint),
        None => {
            let target = id("ExternalSource", &name);
            graph.external(target.clone(), name.clone());
            target
        }
    };
    graph.edge(node.clone(), target, EdgeKind::Registers, node);

    let card = RegistrationCard {
        mode: text(&csr.spec, "mode", "inclusive"),
        identity: csr
            .spec
            .get("federation")
            .map(|f| text(f, "identity", "serviceAccount"))
            .unwrap_or_else(|| "serviceAccount".into()),
        types: entity_types(&csr.spec),
        external,
    };
    graph.node(csr, "ContextSourceRegistration", Some(card));
}

/// The entity types a registration claims, deduplicated and in a stable order.
fn entity_types(spec: &Value) -> Vec<String> {
    let mut types: Vec<String> = spec
        .get("information")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|info| info.get("entities")?.as_array())
        .flatten()
        .filter_map(|entity| Some(entity.get("type")?.as_str()?.to_owned()))
        .collect();
    types.sort();
    types.dedup();
    types
}

/// The Endpoint a pipeline writes into, read out of its target URN (PL-02).
fn endpoint_of(spec: &Value) -> Option<String> {
    let urn = Urn::from_str(crate::resource::pipeline_targets(spec).first()?).ok()?;
    Some(urn.local_id().to_owned())
}

/// A `Ref` is either a bare name or `{ kind, name }` (MF-09).
fn text(value: &Value, key: &str, fallback: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or(fallback)
        .to_owned()
}

fn id(kind: &str, name: &str) -> String {
    format!("{kind}/{name}")
}

/// Collects nodes and edges, keyed so that two manifests naming the same object produce one
/// node and two runs produce the same order (UI-27).
#[derive(Default)]
struct Builder {
    nodes: BTreeMap<String, Node>,
    edges: Vec<Edge>,
}

impl Builder {
    fn node(&mut self, env: ResourceEnvelope, kind: &str, registration: Option<RegistrationCard>) {
        let id = id(kind, &env.metadata.name);
        self.nodes.insert(
            id.clone(),
            Node {
                id,
                kind: kind.to_owned(),
                name: env.metadata.name,
                // A plain title (UI-50) is the English one until the graph carries one string.
                title: match env.metadata.title {
                    Some(jc_core::Text::Localized(map)) => map
                        .iter()
                        .map(|(locale, text)| (locale.to_owned(), text.to_owned()))
                        .collect(),
                    Some(jc_core::Text::Plain(text)) => BTreeMap::from([("en".to_owned(), text)]),
                    None => BTreeMap::new(),
                },
                health: health(&env.status),
                phase: env.status.as_ref().map(|s| phase_str(s.phase).to_owned()),
                registration,
            },
        );
    }

    /// A source outside this platform, which has no manifest here and so no health of its own.
    fn external(&mut self, id: String, name: String) {
        self.nodes.entry(id.clone()).or_insert(Node {
            id,
            kind: "ExternalSource".into(),
            name,
            title: BTreeMap::new(),
            health: NodeHealth::Unknown,
            phase: None,
            registration: None,
        });
    }

    fn edge(&mut self, from: String, to: String, kind: EdgeKind, manifest: String) {
        self.edges.push(Edge {
            from,
            to,
            kind,
            manifest,
        });
    }

    fn finish(mut self) -> FederationGraph {
        self.edges
            .sort_by(|a, b| (&a.from, &a.to).cmp(&(&b.from, &b.to)));
        FederationGraph {
            nodes: self.nodes.into_values().collect(),
            edges: self.edges,
        }
    }
}

/// The health UI-27 defines, read off the phase the object last reported. Anything still
/// converging is `unknown` rather than `ok`: a graph that showed green for a resource nobody
/// has deployed yet would be worse than one that admits it does not know.
fn health(status: &Option<crate::resource::Status>) -> NodeHealth {
    match status.as_ref().map(|s| phase_str(s.phase)) {
        Some("Live") => NodeHealth::Ok,
        Some("Error") => NodeHealth::Degraded,
        _ => NodeHealth::Unknown,
    }
}
