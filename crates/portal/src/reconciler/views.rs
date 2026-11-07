//! The broker's views and policy rule documents, written from the repository (T-3640; Antares
//! `docs/analysis/platform-switch-plan.md` sections 2.1, 2.2, 3.3 and 5.1).
//!
//! Every Endpoint becomes the view `{slug}` and every Context Space the view `cs-{segment}`
//! (`PUT /q/views/{id}`); every Context Space's Policies become the rule document of its tenant
//! (`PUT /q/policy/{tenant}`), which the broker's `joinedcontext` engine decides with; every
//! Context Space's data model becomes its tenant's schema (`PUT /q/schemas/{tenant}`, T-3703),
//! the `brokerSchema` Model Tools compiles from it and its imports. A view is
//! the widest an Endpoint ever shows: its projection's classes and slots, else what its Policies
//! name, less its hidden Attributes, with the operations its Policies grant. Who sees how much
//! of that is the engine's.
//!
//! Three rules:
//!
//! - the writer owns the ids an Endpoint slug can be (`^[a-z2-7]{26,63}$`) and the ids starting
//!   with `cs-`; a view written by hand under any other id is never deleted;
//! - an Endpoint the view cannot express is not written and its status says why: a slug over 63
//!   characters, over 256 types, hidden Attributes on a type shown whole, a `viewMappingRef` whose Mapping has no usable IR. It
//!   stays on the gateway path;
//! - the broker's admin token is read from the file its Secret is mounted at, on every run, and
//!   travels only in the `Authorization` header: never in a log, a status or an outcome.

use super::space_rules::{
    hide_quarantined, quality_document, quarantines, retention_document, retention_rows, type_iris,
    RetentionCount,
};
use super::temporal_q::intersect;
use crate::capped::refusal;
use futures_util::StreamExt;
use jc_core::envelope::ORG_NAMESPACE;
use jc_core::kinds::data_model::ModelImport;
use jc_core::kinds::{
    ContextSpaceSpec, DataAgreementSpec, DataModelLifecycle, DataModelRef, DataModelSpec,
    DataOfferSpec, EndpointSpec, MappingSpec, ModelProjectionSpec, Operation, OperationRef,
    PipelineSpec, PolicySpec, PrincipalKind, ProjectedClass, Projection, ProjectionFilter,
    QualityKind, QualityRule, Representation, RetentionRule,
};
use jcctl::loader::Repository;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How many view and rule-document reads one broker pass keeps in flight (T-5804).
const WRITES_AT_ONCE: usize = 8;

/// A broker call that has not answered in this long is a failed run, not a slow one.
const TIMEOUT: Duration = Duration::from_secs(15);

/// A view id is `[a-z0-9-]{1,63}` (Antares `views.rs`).
const MAX_ID: usize = 63;

/// The most names in one list of a view.
const MAX_NAMES: usize = 256;

/// The most rules one view holds.
const MAX_RULES: usize = 64;

/// The rows a view of another model may list (Antares `mapping::READS`, view-mapping.md
/// section 7): the reads the broker inverts and rebuilds. Any other row a Policy grants is left
/// out of a mapped view.
const MAPPED_READS: [&str; 11] = [
    "retrieve_entity",
    "query_entities",
    "retrieve_attribute_value",
    "temporal_retrieve",
    "temporal_query",
    "retrieve_entity_types",
    "retrieve_entity_type_details",
    "retrieve_entity_type_info",
    "retrieve_attributes",
    "retrieve_attribute_details",
    "retrieve_attribute_info",
];

/// What the view of a Context Space's own surface (`/cs/{space}`) is called before its segment.
pub const SPACE_VIEW_PREFIX: &str = "cs-";

/// The condition an Endpoint's or a Context Space's status carries about its view.
pub const CONDITION: &str = "ViewWritten";

/// The CIM 009 clause of a Table 4.20-1 operation, and the rows of Antares'
/// `operation-matrix.yaml` that serve it through a view. Purge, Entity Maps and the source
/// identity have no row a view serves (plan 2.1: they never enter a view).
fn operation(op: Operation) -> (&'static [&'static str], &'static [&'static str]) {
    use Operation as O;
    match op {
        O::CreateEntity => (&["5.6.1"], &["create_entity"]),
        O::UpdateEntity => (&["5.6.2"], &["update_attributes"]),
        O::AppendAttrs => (&["5.6.3"], &["append_attributes"]),
        // The platform's updateAttrs is the attribute edit of either form: the gateway admits
        // `PATCH …/attrs` (5.6.2) and `PATCH …/attrs/{attr}` (5.6.4) under it, every client
        // sends the first, so the view and the engine admit both (T-5090).
        O::UpdateAttrs => (
            &["5.6.4", "5.6.2"],
            &["partial_attribute_update", "update_attributes"],
        ),
        O::DeleteAttrs => (&["5.6.5"], &["delete_attribute"]),
        O::DeleteEntity => (&["5.6.6"], &["delete_entity"]),
        O::CreateBatch => (&["5.6.7"], &["batch_create"]),
        O::UpsertBatch => (&["5.6.8"], &["batch_upsert"]),
        O::UpdateBatch => (&["5.6.9"], &["batch_update"]),
        O::DeleteBatch => (&["5.6.10"], &["batch_delete"]),
        O::UpsertTemporal => (&["5.6.11"], &["temporal_upsert"]),
        O::AppendAttrsTemporal => (&["5.6.12"], &["temporal_add_attributes"]),
        O::DeleteAttrsTemporal => (&["5.6.13"], &["temporal_delete_attribute"]),
        O::UpdateAttrInstanceTemporal => (&["5.6.14"], &["temporal_modify_instance"]),
        O::DeleteAttrInstanceTemporal => (&["5.6.15"], &["temporal_delete_instance"]),
        O::DeleteTemporal => (&["5.6.16"], &["temporal_delete"]),
        O::MergeEntity => (&["5.6.17"], &["merge_entity"]),
        O::ReplaceEntity => (&["5.6.18"], &["replace_entity"]),
        O::ReplaceAttrs => (&["5.6.19"], &["replace_attribute"]),
        O::MergeBatch => (&["5.6.20"], &["batch_merge"]),
        O::PurgeEntity => (&["5.6.21"], &[]),
        // One Attribute of one Entity is a read of that Entity: the broker asks 5.7.1 for both
        // (Antares `OperationId::RetrieveAttributeValue`), and its MCP `get_attribute_value` is
        // offered only where the view names the row (T-4191).
        O::RetrieveEntity => (&["5.7.1"], &["retrieve_entity", "retrieve_attribute_value"]),
        O::QueryEntity => (&["5.7.2"], &["query_entities"]),
        O::QueryBatch => (&["5.7.2"], &["query_entities_post"]),
        O::RetrieveTemporal => (&["5.7.3"], &["temporal_retrieve"]),
        O::QueryTemporal => (&["5.7.4"], &["temporal_query", "temporal_query_post"]),
        O::RetrieveEntityTypes => (&["5.7.5"], &["retrieve_entity_types"]),
        // The broker answers a type's details and the type list at one `/types`, and asks its
        // engine 5.7.5 for both (Antares `OperationId::gate_clause`); 5.7.6 is never asked, so a
        // permit naming it would grant nothing (T-3645). The same for the attributes' 5.7.8.
        O::RetrieveEntityTypeDetails => (&["5.7.5"], &["retrieve_entity_type_details"]),
        O::RetrieveEntityTypeInfo => (&["5.7.7"], &["retrieve_entity_type_info"]),
        O::RetrieveAttrTypes => (&["5.7.8"], &["retrieve_attributes"]),
        O::RetrieveAttrTypeDetails => (&["5.7.8"], &["retrieve_attribute_details"]),
        O::RetrieveAttrTypeInfo => (&["5.7.10"], &["retrieve_attribute_info"]),
        O::CreateSubscription => (&["5.8.1"], &["create_subscription"]),
        O::UpdateSubscription => (&["5.8.2"], &["update_subscription"]),
        O::RetrieveSubscription => (&["5.8.3"], &["retrieve_subscription"]),
        O::QuerySubscription => (&["5.8.4"], &["query_subscriptions"]),
        O::DeleteSubscription => (&["5.8.5"], &["delete_subscription"]),
        O::RetrieveEntityMap => (&["5.14.1"], &[]),
        O::UpdateEntityMap => (&["5.14.2"], &[]),
        O::DeleteEntityMap => (&["5.14.3"], &[]),
        O::CreateEntityMapQueryEntity => (&["5.14.4"], &[]),
        O::CreateEntityMapQueryTemporal => (&["5.14.5"], &[]),
        O::RetrieveContextSourceIdentity => (&["5.15.1"], &[]),
    }
}

/// The rows of a view's `operations` that admit `op` (T-4840).
pub fn rows_of(op: Operation) -> &'static [&'static str] {
    operation(op).1
}

/// The reads of a type's schema (the broker's LinkML surface: the documents, one class, one
/// slot; clauses 5.7.5, 5.7.7 and 5.7.10 as the broker asks them) and the type and Attribute
/// lists (5.7.5, 5.7.8). A Policy that permits anything on a type permits these too, as the
/// gateway's schema surface did before the broker served it (T-3645): whoever may read or
/// write an Entity may read its shape, narrowed by the engine to the types and Attributes the
/// Policy names. The lists come from the view's catalogue, filtered by the caller's grant, so
/// the gateway forwards `/types` and `/attributes` to the view (T-5391).
const SCHEMA_READS: [Operation; 6] = [
    Operation::RetrieveEntityTypes,
    Operation::RetrieveEntityTypeDetails,
    Operation::RetrieveEntityTypeInfo,
    Operation::RetrieveAttrTypes,
    Operation::RetrieveAttrTypeDetails,
    Operation::RetrieveAttrTypeInfo,
];

/// The operations a Policy grants: its own, groups expanded, and the schema reads when it
/// permits any a view serves.
fn permitted(policy: &PolicySpec) -> impl Iterator<Item = Operation> + '_ {
    let own = || policy.operations.iter().flat_map(expand).copied();
    let served = own().any(|op| !operation(op).1.is_empty());
    let schema = (served && !policy.effect.is_prohibition())
        .then_some(SCHEMA_READS)
        .into_iter()
        .flatten();
    own().chain(schema)
}

/// The Table 4.20-1 operations a reference stands for, a Table 4.20-2 group expanded.
fn expand(reference: &OperationRef) -> &[Operation] {
    match reference {
        OperationRef::Single(op) => std::slice::from_ref(op),
        OperationRef::Group(group) => group.operations(),
    }
}

/// One Endpoint as the views are made from it.
#[derive(Debug, Clone)]
pub struct EndpointSource {
    pub project: String,
    pub name: String,
    /// Its space's broker tenant, the rendered URN segment (PF-84).
    pub tenant: String,
    pub spec: EndpointSpec,
}

/// What the views and documents are made of, read from one loaded repository.
#[derive(Debug, Default)]
pub struct Sources {
    /// Every Context Space: `(project, name, tenant)`.
    pub spaces: Vec<(String, String, String)>,
    pub endpoints: Vec<EndpointSource>,
    /// Every Policy by `(project, space)` with its manifest name, each Pipeline's derived ones
    /// included (PL-20): they reach the broker through the same Endpoints.
    pub policies: BTreeMap<(String, String), Vec<(String, PolicySpec)>>,
    /// Every `ModelProjection` by `(project, name)`.
    pub projections: BTreeMap<(String, String), ModelProjectionSpec>,
    /// The classes of each space's own data models by `(project, space)`; a mirrored model is
    /// another organisation's and selects nothing here (DM-49).
    pub classes: BTreeMap<(String, String), BTreeSet<String>>,
    /// Every App by `(project, name)`: its generated Endpoint `app-{name}` evaluates the App's
    /// own grants and the space's prohibitions alone (AP-139).
    pub apps: BTreeSet<(String, String)>,
    /// Every Mapping by `(project, name)`.
    pub mappings: BTreeMap<(String, String), MappingSource>,
    /// Each Context Space's data model by `(project, space)`: the source its `dataModelRef`
    /// names and every platform model that imports, or why they do not read. A space without
    /// `dataModelRef` has none, and its tenant no schema.
    pub models: BTreeMap<(String, String), Result<ModelSource, String>>,
    /// Each Context Space's retention rules by `(project, space)`, empty for keep-forever
    /// (DM-82).
    pub retention: BTreeMap<(String, String), Vec<RetentionRule>>,
    /// Each Context Space's quality rules by `(project, space)` (DM-87).
    pub quality: BTreeMap<(String, String), Vec<QualityRule>>,
    /// The broker each tenant lives on (PF-111, ADR-N-047); `""` for a space whose `brokerRef`
    /// names no broker, which nothing is written for.
    pub brokers: BTreeMap<String, String>,
    /// The Context Spaces with a quality rule that quarantines (DM-90): their permits, but the
    /// owner's and the steward's, exclude a quarantined entity.
    pub quarantines: BTreeSet<(String, String)>,
}

/// One Mapping as a view of another model needs it (G18).
#[derive(Debug, Clone)]
pub struct MappingSource {
    /// The Context Space it belongs to.
    pub space: String,
    /// Its `spec.artifacts.gatewayIr` document, or why there is none to embed.
    pub ir: Result<Value, String>,
}

/// One space's data model as Model Tools compiles it (`POST /generate`, DM-76).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSource {
    /// The DataModel's name, for the messages.
    pub model: String,
    /// Its LinkML source.
    pub source: String,
    /// The source of every platform model it imports, directly or through another import, by
    /// import name (`org.{name}.v{major}`, `project.{name}.v{major}`).
    pub imports: BTreeMap<String, String>,
}

impl ModelSource {
    /// What tells an unchanged model from a changed one: every field, length-prefixed.
    fn digest(&self) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        let mut hash = Sha256::new();
        let mut field = |text: &str| {
            hash.update((text.len() as u64).to_le_bytes());
            hash.update(text.as_bytes());
        };
        field(&self.model);
        field(&self.source);
        for (name, source) in &self.imports {
            field(name);
            field(source);
        }
        hash.finalize().into()
    }
}

impl Sources {
    /// Each Pipeline's `spec.history` written into the retention rules of the spaces its outputs
    /// write (PL-108), once every space and Endpoint is read.
    fn fold_history(&mut self, repo: &Repository) {
        let mut plans: BTreeMap<(String, String), Vec<crate::pipeline_history::Plan>> =
            BTreeMap::new();
        for (id, resource) in repo.iter() {
            if id.kind != "Pipeline" {
                continue;
            }
            let Ok(spec) = serde_json::from_value::<PipelineSpec>(resource.manifest.spec.clone())
            else {
                continue;
            };
            let project = id.namespace.clone().unwrap_or_default();
            for plan in crate::pipeline_history::plans(&id.name, &spec) {
                let spaces: BTreeSet<&str> = spec
                    .outputs()
                    .into_iter()
                    .filter(|o| {
                        o.entity_type
                            .as_deref()
                            .is_none_or(|t| t == plan.entity_type)
                    })
                    .filter_map(|o| {
                        self.endpoints.iter().find(|e| {
                            e.project == project && e.name == o.target_endpoint.local_id()
                        })
                    })
                    .map(|e| e.spec.context_space_ref.name())
                    .collect();
                for space in spaces {
                    plans
                        .entry((project.clone(), space.to_owned()))
                        .or_default()
                        .push(plan.clone());
                }
            }
        }
        for (key, plans) in plans {
            if let Some(rules) = self.retention.get_mut(&key) {
                *rules = crate::pipeline_history::fold(rules, &plans);
            }
        }
    }

    /// Reads the kinds the views are made of. A manifest whose spec does not parse is left
    /// out: the load already reported it.
    pub fn read(repo: &Repository) -> Self {
        let mut sources = Self::default();
        // Data-space agreements and the offers they were made on, compiled once every Policy
        // they narrow is read (DS-10, T-3927).
        let mut offers = BTreeMap::new();
        let mut agreements = Vec::new();
        let declared: Vec<(String, jc_core::kinds::ContextBrokerSpec)> = repo
            .iter()
            .filter(|(id, _)| id.kind == "ContextBroker")
            .filter_map(|(id, resource)| {
                Some((
                    id.name.clone(),
                    serde_json::from_value(resource.manifest.spec.clone()).ok()?,
                ))
            })
            .collect();
        for (id, resource) in repo.iter() {
            let project = id.namespace.clone().unwrap_or_default();
            let spec = &resource.manifest.spec;
            match id.kind.as_str() {
                "ContextSpace" => {
                    let parsed = serde_json::from_value::<ContextSpaceSpec>(spec.clone()).ok();
                    let tenant = repo.space_segment(&project, &id.name);
                    let named = declared.iter().map(|(name, spec)| (name.as_str(), spec));
                    let broker_ref = parsed.as_ref().and_then(|spec| spec.broker_ref.as_deref());
                    let broker = jc_core::kinds::broker_of(broker_ref, named)
                        .map(str::to_owned)
                        .unwrap_or_default();
                    sources.brokers.insert(tenant, broker);
                    let model = parsed.as_ref().and_then(|spec| spec.data_model_ref.clone());
                    if let Some(spec) = parsed {
                        if spec.quality.iter().any(quarantines) {
                            sources
                                .quarantines
                                .insert((project.clone(), id.name.clone()));
                        }
                        sources
                            .retention
                            .insert((project.clone(), id.name.clone()), spec.retention);
                        sources
                            .quality
                            .insert((project.clone(), id.name.clone()), spec.quality);
                    }
                    if let Some(model) = model {
                        sources.models.insert(
                            (project.clone(), id.name.clone()),
                            model_source(repo, &project, model.name()),
                        );
                    }
                    let tenant = repo.space_segment(&project, &id.name);
                    sources.spaces.push((project, id.name.clone(), tenant));
                }
                "Endpoint" => {
                    if let Ok(spec) = serde_json::from_value::<EndpointSpec>(spec.clone()) {
                        let tenant = repo.space_segment(&project, spec.context_space_ref.name());
                        sources.endpoints.push(EndpointSource {
                            project,
                            name: id.name.clone(),
                            tenant,
                            spec,
                        });
                    }
                }
                "Policy" => {
                    // One grant per rule, `{policy}#{n}`, as the gateway reads it (ADR-N-057);
                    // an invalid shape is left out like an unreadable manifest.
                    if let Ok(spec) = serde_json::from_value::<PolicySpec>(spec.clone()) {
                        if let Ok(rules) = spec.expand(&id.name) {
                            let space = spec.context_space_ref.name().to_owned();
                            sources
                                .policies
                                .entry((project, space))
                                .or_default()
                                .extend(rules);
                        }
                    }
                }
                "ModelProjection" => {
                    if let Ok(spec) = serde_json::from_value::<ModelProjectionSpec>(spec.clone()) {
                        sources.projections.insert((project, id.name.clone()), spec);
                    }
                }
                "DataModel" => {
                    let Ok(spec) = serde_json::from_value::<DataModelSpec>(spec.clone()) else {
                        continue;
                    };
                    if spec.lifecycle == DataModelLifecycle::Mirrored {
                        continue;
                    }
                    if let Some(space) = spec.context_space_ref {
                        sources
                            .classes
                            .entry((project, space))
                            .or_default()
                            .extend(spec.classes);
                    }
                }
                "App" => {
                    sources.apps.insert((project, id.name.clone()));
                }
                "DataOffer" => {
                    if let Ok(spec) = serde_json::from_value::<DataOfferSpec>(spec.clone()) {
                        offers.insert((project, id.name.clone()), spec);
                    }
                }
                "DataAgreement" => {
                    if let Ok(spec) = serde_json::from_value::<DataAgreementSpec>(spec.clone()) {
                        agreements.push((project, id.name.clone(), spec));
                    }
                }
                "Mapping" => {
                    let Ok(spec) = serde_json::from_value::<MappingSpec>(spec.clone()) else {
                        continue;
                    };
                    let ir = match &spec.artifacts.gateway_ir {
                        None => Err(format!(
                            "the Mapping {} has no spec.artifacts.gatewayIr",
                            id.name
                        )),
                        Some(relative) => crate::pipeline_validation::beside(
                            repo.root(),
                            &resource.path,
                            relative,
                        )
                        .ok_or_else(|| {
                            format!(
                                "the gatewayIr {relative:?} of the Mapping {} is no JSON file \
                                 in the repository",
                                id.name
                            )
                        }),
                    };
                    sources.mappings.insert(
                        (project, id.name.clone()),
                        MappingSource {
                            space: spec.context_space_ref,
                            ir,
                        },
                    );
                }
                _ => {}
            }
        }
        sources.fold_history(repo);
        for (project, derived) in jcctl::pipeline_identity::derived(repo) {
            for (name, spec) in derived.policies {
                let space = spec.context_space_ref.name().to_owned();
                sources
                    .policies
                    .entry((project.clone(), space))
                    .or_default()
                    .push((name, spec));
            }
        }
        // An accepted agreement is its offer's Policies for the participant's DID, narrowed
        // (DS-10, DS-03): compiled from the Endpoints' bound Policies read above.
        let bound = |project: &str, endpoint: &str| -> Vec<(String, PolicySpec)> {
            sources
                .endpoints
                .iter()
                .find(|e| e.project == project && e.name == endpoint)
                .map(|e| {
                    sources
                        .bound(e)
                        .into_iter()
                        .map(|(name, policy)| (name.to_owned(), policy.clone()))
                        .collect()
                })
                .unwrap_or_default()
        };
        let compiled = super::agreements::compile_all(&agreements, &offers, bound);
        for (space, policies) in compiled {
            sources.policies.entry(space).or_default().extend(policies);
        }
        sources
    }

    fn policies_of(&self, project: &str, space: &str) -> &[(String, PolicySpec)] {
        self.policies
            .get(&(project.to_owned(), space.to_owned()))
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    fn classes_of(&self, project: &str, space: &str) -> BTreeSet<String> {
        self.classes
            .get(&(project.to_owned(), space.to_owned()))
            .cloned()
            .unwrap_or_default()
    }

    /// The Policies an Endpoint evaluates, as the gateway binds them (`store.rs`
    /// `endpoints_with_models`): with `policyRefs` (or `policyRef`) the listed Policies of its
    /// space, else every Policy of its space; an App's Endpoint keeps the App's own grants and the prohibitions.
    fn bound<'s>(&'s self, e: &EndpointSource) -> Vec<(&'s str, &'s PolicySpec)> {
        let space = e.spec.context_space_ref.name();
        let app = self.is_app(e);
        self.policies_of(&e.project, space)
            .iter()
            .filter(|(name, _)| match e.spec.policy_refs() {
                Some(refs) => refs.iter().any(|reference| {
                    reference.space() == Some(e.tenant.as_str())
                        && PolicySpec::policy_of(name) == reference.local_id()
                }),
                None => true,
            })
            .filter(|(_, policy)| {
                !app || policy.effect.is_prohibition() || own_grant(policy, &e.project, &e.name)
            })
            .map(|(name, policy)| (name.as_str(), policy))
            .collect()
    }

    /// Whether `e` is the Endpoint `app-{name}` an App of its project generated (AP-139).
    fn is_app(&self, e: &EndpointSource) -> bool {
        e.name
            .strip_prefix("app-")
            .is_some_and(|app| self.apps.contains(&(e.project.clone(), app.to_owned())))
    }
}

/// The source of the DataModel `name` of `project` and of every platform model it imports, as
/// the checkout holds them (DM-76), resolved the way `jcctl model generate` does: an import at
/// the major it pins and published, an organization model importing organization models only.
fn model_source(repo: &Repository, project: &str, name: &str) -> Result<ModelSource, String> {
    let source = model_text(repo, project, name, None)?;
    let mut imports = BTreeMap::new();
    let mut todo = vec![(project.to_owned(), source.clone())];
    while let Some((importer, text)) = todo.pop() {
        // A source that does not parse is Model Tools' message to give.
        let Ok(document) = serde_norway::from_str::<Value>(&text) else {
            continue;
        };
        for import in ModelImport::all_in(&document).map_err(|err| err.to_string())? {
            let key = import.to_string();
            if imports.contains_key(&key) {
                continue;
            }
            if imports.len() >= crate::api::datamodels::MAX_IMPORTS {
                return Err(format!(
                    "a model imports at most {} platform models, its imports' imports included",
                    crate::api::datamodels::MAX_IMPORTS
                ));
            }
            let home = match (import.organization, importer.as_str()) {
                (true, _) => ORG_NAMESPACE,
                (false, ORG_NAMESPACE) => {
                    return Err(format!(
                        "import '{key}': an organization model imports organization models only (DM-76)"
                    ))
                }
                (false, own) => own,
            };
            let text = model_text(repo, home, &import.name, Some(import.major))
                .map_err(|why| format!("import '{key}': {why}"))?;
            todo.push((home.to_owned(), text.clone()));
            imports.insert(key, text);
        }
    }
    Ok(ModelSource {
        model: name.to_owned(),
        source,
        imports,
    })
}

/// One DataModel's LinkML source from the checkout; with `major`, only a published version of
/// that major (DM-22, DM-26). The path stays inside the checkout, a symlink included.
fn model_text(
    repo: &Repository,
    namespace: &str,
    name: &str,
    major: Option<u32>,
) -> Result<String, String> {
    let (_, resource) = repo
        .iter()
        .find(|(id, _)| {
            id.kind == "DataModel" && id.name == name && id.namespace.as_deref() == Some(namespace)
        })
        .ok_or_else(|| format!("{namespace} has no DataModel {name}"))?;
    let spec: DataModelSpec = serde_json::from_value(resource.manifest.spec.clone())
        .map_err(|err| format!("the DataModel {name} does not read: {err}"))?;
    if let Some(major) = major {
        if spec.version.major() != major {
            return Err(format!(
                "'{name}' is at version {}, and the import pins major {major}",
                spec.version
            ));
        }
        if !matches!(
            spec.lifecycle,
            DataModelLifecycle::Published
                | DataModelLifecycle::Deprecated
                | DataModelLifecycle::Mirrored
        ) {
            return Err(format!(
                "'{name}' is {}; only a published model is imported (DM-26)",
                spec.lifecycle
            ));
        }
    }
    let linkml = crate::api::datamodels::confine_linkml_path(&spec.linkml)
        .map_err(|err| format!("the DataModel {name}: {err}"))?;
    let relative = resource.path.parent().unwrap_or(Path::new("")).join(linkml);
    let unread = |err: std::io::Error| format!("{} does not read: {err}", relative.display());
    let root = repo.root().canonicalize().map_err(unread)?;
    let file = root.join(&relative).canonicalize().map_err(unread)?;
    if !file.starts_with(&root) {
        return Err(format!("{} leaves the repository", relative.display()));
    }
    std::fs::read_to_string(&file).map_err(unread)
}

/// Whether `policy` is assigned to a role the Endpoint hands out (AP-96, AP-139).
fn own_grant(policy: &PolicySpec, project: &str, endpoint: &str) -> bool {
    let own = jc_core::kinds::endpoint_role(project, endpoint, None);
    policy.assignee.kind == PrincipalKind::Role
        && policy
            .assignee
            .id
            .strip_prefix(own.as_str())
            .is_some_and(|rest| {
                rest.is_empty()
                    || rest
                        .strip_prefix('/')
                        .is_some_and(|role| !role.is_empty() && !role.contains('/'))
            })
}

/// Whose a view or a document is: the object whose status reports it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Owner {
    /// `Endpoint` or `ContextSpace`.
    pub kind: &'static str,
    pub project: String,
    pub name: String,
}

impl Owner {
    fn endpoint(e: &EndpointSource) -> Self {
        Self {
            kind: "Endpoint",
            project: e.project.clone(),
            name: e.name.clone(),
        }
    }

    fn space(project: &str, name: &str) -> Self {
        Self {
            kind: "ContextSpace",
            project: project.to_owned(),
            name: name.to_owned(),
        }
    }
}

/// What one run wants the broker to hold.
#[derive(Debug, Default)]
pub struct Desired {
    /// The view documents by id.
    pub views: BTreeMap<String, (Owner, Value)>,
    /// The tenant each view reads, by view id: what decides the broker it is written to.
    pub tenants: BTreeMap<String, String>,
    /// The broker each tenant lives on, as [`Sources::brokers`].
    pub brokers: BTreeMap<String, String>,
    /// The rule documents by tenant.
    pub documents: BTreeMap<String, (Owner, Value)>,
    /// Each tenant's data model, which becomes its schema.
    pub schemas: BTreeMap<String, (Owner, ModelSource)>,
    /// Each tenant's retention rules, empty to remove its document (DM-82).
    pub retention: BTreeMap<String, (Owner, Vec<RetentionRule>)>,
    /// Each tenant's `maxAge` freshness rules, which the broker's sweep runs; empty to remove
    /// its document (DM-91).
    pub quality: BTreeMap<String, (Owner, Vec<QualityRule>)>,
    /// The objects the writer refused, and why.
    pub refused: Vec<(Owner, String)>,
}

impl Desired {
    /// What this asks of the broker that `before` did not. Every refusal stays: it is no write,
    /// and an Endpoint a merge touched and the writer still refuses must not read `Live`.
    fn beyond(mut self, before: &Desired) -> Desired {
        self.views
            .retain(|id, entry| before.views.get(id) != Some(&*entry));
        self.documents
            .retain(|tenant, entry| before.documents.get(tenant) != Some(&*entry));
        self.schemas
            .retain(|tenant, entry| before.schemas.get(tenant) != Some(&*entry));
        self.retention
            .retain(|tenant, entry| before.retention.get(tenant) != Some(&*entry));
        self.quality
            .retain(|tenant, entry| before.quality.get(tenant) != Some(&*entry));
        self
    }
}

/// How much of the broker a pass answers for.
#[derive(Clone, Copy, PartialEq)]
enum Pass {
    /// All of it: what the desired state lacks is removed.
    Whole,
    /// Only what it names: what it lacks is simply not its business.
    Merged,
}

/// Every view and rule document the repository declares (plan 2.1, 3.3, 5.1).
pub fn desired(sources: &Sources) -> Desired {
    let mut out = Desired {
        brokers: sources.brokers.clone(),
        ..Desired::default()
    };
    for e in &sources.endpoints {
        let owner = Owner::endpoint(e);
        let made = endpoint_view(sources, e).and_then(|view| {
            let id = e.spec.slug.to_string();
            match out.views.get(&id) {
                Some((other, _)) => Err(format!(
                    "the Endpoint {} of project {} has the same slug",
                    other.name, other.project
                )),
                None => Ok((id, view)),
            }
        });
        match made {
            Ok((id, view)) => {
                out.tenants.insert(id.clone(), e.tenant.clone());
                out.views.insert(id, (owner, view));
            }
            Err(reason) => out.refused.push((owner, reason)),
        }
    }
    for (project, name, tenant) in &sources.spaces {
        let owner = Owner::space(project, name);
        match space_view(sources, project, name, tenant) {
            Ok(view) => {
                let id = format!("{SPACE_VIEW_PREFIX}{tenant}");
                out.tenants.insert(id.clone(), tenant.clone());
                out.views.insert(id, (owner.clone(), view));
            }
            Err(reason) => out.refused.push((owner.clone(), reason)),
        }
        if let Some((other, _)) = out.documents.get(tenant) {
            let reason = format!(
                "the Context Space {} of project {} renders the same tenant",
                other.name, other.project
            );
            out.refused.push((owner, reason));
            continue;
        }
        match sources.models.get(&(project.clone(), name.clone())) {
            Some(Ok(model)) => {
                out.schemas
                    .insert(tenant.clone(), (owner.clone(), model.clone()));
            }
            Some(Err(why)) => out.refused.push((
                owner.clone(),
                format!("its data model is not written to the broker: {why}"),
            )),
            None => {}
        }
        let rules = sources
            .retention
            .get(&(project.clone(), name.clone()))
            .cloned()
            .unwrap_or_default();
        out.retention.insert(tenant.clone(), (owner.clone(), rules));
        let swept: Vec<QualityRule> = sources
            .quality
            .get(&(project.clone(), name.clone()))
            .into_iter()
            .flatten()
            .filter(|rule| rule.kind == QualityKind::Freshness && rule.max_age.is_some())
            .cloned()
            .collect();
        out.quality.insert(tenant.clone(), (owner.clone(), swept));
        out.documents.insert(
            tenant.clone(),
            (owner, with_gateway(rule_document(sources, project, name))),
        );
    }
    out
}

/// The projection an Endpoint narrows by: the one it names, in its own Context Space.
fn projection_of<'a>(sources: &'a Sources, e: &EndpointSource) -> Option<&'a ModelProjectionSpec> {
    let space = e.spec.context_space_ref.name();
    e.spec.projection_ref.as_ref().and_then(|reference| {
        let project = reference.namespace.as_deref().unwrap_or(&e.project);
        sources
            .projections
            .get(&(project.to_owned(), reference.name.clone()))
            .filter(|projection| projection.context_space_ref == space)
    })
}

/// The view of one Endpoint (plan 2.1), or why it has none.
fn endpoint_view(sources: &Sources, e: &EndpointSource) -> Result<Value, String> {
    view_of(sources, e, projection_of(sources, e))
}

/// The view of `e` under `projection` in place of the one its `projectionRef` names: the saved
/// one for the reconciler, a draft for the preview (T-3802).
fn view_of(
    sources: &Sources,
    e: &EndpointSource,
    projection: Option<&ModelProjectionSpec>,
) -> Result<Value, String> {
    surface_fit(sources, e)?;
    let mapping = view_mapping(sources, e)?;
    let id = e.spec.slug.to_string();
    if id.len() > MAX_ID {
        return Err(format!(
            "the slug is {} characters and a view id at most {MAX_ID}",
            id.len()
        ));
    }
    let permits: Vec<&PolicySpec> = sources
        .bound(e)
        .into_iter()
        .map(|(_, policy)| policy)
        .filter(|policy| !policy.effect.is_prohibition())
        .collect();
    if permits.is_empty() {
        return Err("no Policy grants anything through this Endpoint".into());
    }
    let hidden: BTreeSet<String> = e
        .spec
        .projection
        .iter()
        .flat_map(|projection| projection.hidden_attributes.iter().cloned())
        .collect();
    let space = e.spec.context_space_ref.name();

    let mut view = Map::new();
    view.insert("id".into(), json!(id));
    view.insert("tenant".into(), json!(e.tenant));
    let shown = match projection {
        Some(projection) => projected(projection, &permits, &mut view)?,
        None => granted(&permits, &sources.classes_of(&e.project, space))?,
    };
    let model = sources
        .models
        .get(&(e.project.clone(), space.to_owned()))
        .and_then(|model| model.as_ref().ok())
        .map(parsed);
    let mut rules = rules(listed(shown, model.as_deref())?, &hidden)?;
    let mut operations = rows(&permits)?;
    if let Some((ir, target, source)) = &mapping {
        // the broker judges a mapped view in the model it stores, as the gateway's
        // `Constraints::in_source_model` does; Attribute names are the source's already
        for (_, types) in &mut rules {
            for t in types.iter_mut().filter(|t| *t == target) {
                t.clone_from(source);
            }
            types.sort();
            types.dedup();
        }
        if let Some(other) = rules
            .iter()
            .flat_map(|(_, types)| types)
            .find(|t| *t != source)
        {
            return Err(format!(
                "a view of another model selects its source class {source} alone, and the \
                 Policies also name {other}"
            ));
        }
        operations.retain(|row| MAPPED_READS.contains(row));
        // The schema reads every grant brings serve no Entity: a view of another model still
        // needs a read of its own.
        let schema_rows: Vec<&str> = SCHEMA_READS
            .iter()
            .flat_map(|op| operation(*op).1.iter().copied())
            .collect();
        if operations.iter().all(|row| schema_rows.contains(row)) {
            return Err("its Policies grant no read a view of another model serves".into());
        }
        view.insert("mapping".into(), ir.clone());
    }
    let selector: BTreeSet<&String> = rules.iter().flat_map(|(_, types)| types).collect();
    if selector.len() > MAX_NAMES {
        return Err(format!(
            "it selects {} types and a view at most {MAX_NAMES}",
            selector.len()
        ));
    }
    view.insert("selector".into(), json!(selector));
    view.insert(
        "rules".into(),
        Value::Array(
            rules
                .iter()
                .map(|(attributes, types)| json!({"types": types, "attributes": attributes}))
                .collect(),
        ),
    );
    view.insert("operations".into(), json!(operations));
    // the broker serves a calendar only for the outputs listed here (EP-159, T-5208), and the feed
    // outputs as the Endpoint lists them, the broker's `antares-feed` contract (EP-150); a view
    // without a member serves no such format.
    let mut representations = serde_json::Map::new();
    let ical = e.spec.ical_outputs();
    if !ical.is_empty() {
        let outputs = serde_json::to_value(ical).map_err(|err| err.to_string())?;
        representations.insert("ical".into(), outputs);
    }
    let feeds = e.spec.feed_outputs();
    if !feeds.is_empty() {
        representations.insert("feed".into(), json!(feeds));
    }
    if !representations.is_empty() {
        view.insert("representations".into(), Value::Object(representations));
    }
    Ok(Value::Object(view))
}

/// Refuses an Endpoint that lists a surface its space's model cannot serve (EP-125): the
/// Portal's propose check, again for a manifest pushed to the forge past it. A measurement an
/// import declares serves `sta` too. A space without `dataModelRef` gives the reconciler no
/// model to judge by; propose judged it.
fn surface_fit(sources: &Sources, e: &EndpointSource) -> Result<(), String> {
    let space = e.spec.context_space_ref.name();
    let Some(Ok(model)) = sources.models.get(&(e.project.clone(), space.to_owned())) else {
        return Ok(());
    };
    let mut gaps = e.spec.representation_gaps(Some(&model.source))?;
    let measured = |text: &String| {
        Representation::Sta
            .model_gap(text)
            .is_ok_and(|g| g.is_none())
    };
    if model.imports.values().any(measured) {
        let sta = Representation::Sta
            .missing_model_gap()
            .map(|gap| format!("`sta` needs {gap}"));
        gaps.retain(|gap| Some(gap) != sta.as_ref());
    }
    if gaps.is_empty() {
        return Ok(());
    }
    Err(format!(
        "its data model {} cannot serve what it lists: {}; drop the surface from \
         enabledRepresentations or extend the model (EP-125)",
        model.model,
        gaps.join("; ")
    ))
}

/// The classes and slots `projection` shows, its filters written into `view`.
fn projected(
    projection: &ModelProjectionSpec,
    permits: &[&PolicySpec],
    view: &mut Map<String, Value>,
) -> Result<BTreeMap<String, Option<BTreeSet<String>>>, String> {
    // its `temporalQ` is the window of the view's permits, in the rule document
    let filter = projection.filter.clone().unwrap_or_default();
    if let Some(window) = &filter.temporal_q {
        for policy in permits {
            intersect(window, policy.temporal_q.as_deref())
                .map_err(|why| format!("the projection's temporalQ and a Policy's: {why}"))?;
        }
    }
    if let Some(q) = filter.q {
        view.insert("q".into(), json!(q));
    }
    if let Some(scope_q) = filter.scope_q {
        view.insert("scopeQ".into(), json!(scope_q));
    }
    if let Some(geo_q) = filter.geo_q {
        view.insert("geoQ".into(), geo(&geo_q)?);
    }
    Ok(projection
        .classes
        .iter()
        .map(|class| {
            let slots = (!class.slots.is_empty())
                .then(|| class.slots.iter().cloned().collect::<BTreeSet<_>>());
            (class.name.clone(), slots)
        })
        .collect())
}

/// The IR, target class and source class of an Endpoint's `viewMappingRef` (G18), or why it
/// cannot be served as a view.
fn view_mapping(
    sources: &Sources,
    e: &EndpointSource,
) -> Result<Option<(Value, String, String)>, String> {
    let Some(reference) = &e.spec.view_mapping_ref else {
        return Ok(None);
    };
    let project = reference.namespace.as_deref().unwrap_or(&e.project);
    let mapping = sources
        .mappings
        .get(&(project.to_owned(), reference.name.clone()))
        .filter(|m| m.space == e.spec.context_space_ref.name())
        .ok_or_else(|| {
            format!(
                "viewMappingRef names {}, which is no Mapping of this Context Space",
                reference.name
            )
        })?;
    let ir = mapping.ir.clone()?;
    // The broker runs only the IR version the gateway hands on (DM-52, T-5219): a Mapping from
    // another Model Tools is the Endpoint's status here, never a view the broker refuses later.
    context_gateway::view_mapping::ViewMapping::parse(&ir).map_err(|e| {
        format!(
            "the IR of the Mapping {}: {e}; recompile the Mapping with this platform's Model Tools",
            reference.name
        )
    })?;
    let class = |member: &str| {
        ir.get(member)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| format!("the IR of the Mapping {} names no {member}", reference.name))
    };
    let (target, source) = (class("targetClass")?, class("sourceClass")?);
    Ok(Some((ir, target, source)))
}

/// What the Policies name, type by type: `None` is the whole Entity. A permission without
/// `information` grants every type, which the view writes as the space model's classes.
fn granted(
    permits: &[&PolicySpec],
    classes: &BTreeSet<String>,
) -> Result<BTreeMap<String, Option<BTreeSet<String>>>, String> {
    let mut shown: BTreeMap<String, Option<BTreeSet<String>>> = BTreeMap::new();
    let mut add = |entity_type: &str, attributes: Option<BTreeSet<String>>| {
        let entry = shown
            .entry(entity_type.to_owned())
            .or_insert(Some(BTreeSet::new()));
        match (entry.as_mut(), attributes) {
            (Some(held), Some(more)) => held.extend(more),
            _ => *entry = None,
        }
    };
    for policy in permits {
        if policy.information.is_empty() {
            if classes.is_empty() {
                return Err(
                    "a Policy grants every type and the space's data model names no \
                            class to select"
                        .into(),
                );
            }
            for class in classes {
                add(class, None);
            }
        }
        for info in &policy.information {
            let attributes: BTreeSet<String> = info
                .property_names
                .iter()
                .chain(&info.relationship_names)
                .cloned()
                .collect();
            for selector in &info.entities {
                add(
                    &selector.entity_type,
                    (!attributes.is_empty()).then(|| attributes.clone()),
                );
            }
        }
    }
    Ok(shown)
}

/// The space model's documents, its own source and every import, as YAML; one that does not
/// parse is left out, as Model Tools would refuse it.
fn parsed(model: &ModelSource) -> Vec<serde_norway::Value> {
    std::iter::once(&model.source)
        .chain(model.imports.values())
        .map(String::as_str)
        .chain([NGSI_LD_CORE])
        .filter_map(|text| serde_norway::from_str(text).ok())
        .collect()
}

/// The `ngsi-ld-core` import no DataModel holds: its `Entity` gives every class `is_a: Entity`
/// the core `location`, so a view lets a geo write through. Last, so a model's own wins.
const NGSI_LD_CORE: &str = "classes:\n  Entity: {slots: [location]}\n";

/// The Attributes of `class` in the model: its `slots` and inline `attributes` and those of
/// every class it reaches through `is_a` and `mixins`, without the identifier slots (the
/// Entity's `id` and `type` are no Attribute). `None` when the model has no such class.
fn class_slots(model: &[serde_norway::Value], class: &str) -> Option<BTreeSet<String>> {
    let find = |section: &str, name: &str| {
        model
            .iter()
            .find_map(|doc| doc.get(section).and_then(|entries| entries.get(name)))
    };
    let names = |value: Option<&serde_norway::Value>| -> Vec<String> {
        match value {
            Some(serde_norway::Value::Sequence(items)) => items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect(),
            Some(serde_norway::Value::Mapping(entries)) => entries
                .keys()
                .filter_map(|key| key.as_str().map(str::to_owned))
                .collect(),
            Some(serde_norway::Value::String(one)) => vec![one.clone()],
            _ => Vec::new(),
        }
    };
    find("classes", class)?;
    let mut slots = BTreeSet::new();
    let mut seen = BTreeSet::new();
    let mut todo = vec![class.to_owned()];
    while let Some(name) = todo.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        let Some(definition) = find("classes", &name) else {
            continue;
        };
        slots.extend(names(definition.get("slots")));
        slots.extend(names(definition.get("attributes")));
        todo.extend(names(definition.get("is_a")));
        todo.extend(names(definition.get("mixins")));
    }
    slots.retain(|slot| {
        let identifier = find("slots", slot)
            .and_then(|definition| definition.get("identifier"))
            .and_then(serde_norway::Value::as_bool)
            .unwrap_or(false);
        !identifier && slot != "id" && slot != "type"
    });
    Some(slots)
}

/// What each type shows as a list of Attributes: a type shown whole lists its class's slots
/// from the space's data model, since Antares reads an empty list as no Attribute (ADR-0023
/// rule 11). A type shown whole with no class in the model is refused, never written empty.
fn listed(
    shown: BTreeMap<String, Option<BTreeSet<String>>>,
    model: Option<&[serde_norway::Value]>,
) -> Result<BTreeMap<String, BTreeSet<String>>, String> {
    shown
        .into_iter()
        .map(|(entity_type, attributes)| match attributes {
            Some(attributes) => Ok((entity_type, attributes)),
            None => model
                .and_then(|model| class_slots(model, &entity_type))
                .map(|slots| (entity_type.clone(), slots))
                .ok_or_else(|| {
                    format!(
                        "{entity_type} is shown whole and the space's data model has no class \
                         {entity_type}: list the Attributes it shows"
                    )
                }),
        })
        .collect()
}

/// One rule of a view: the Attributes it shows and its types.
type Rule = (Vec<String>, Vec<String>);

/// The rules of a view: the types that show the same Attributes share one. A type whose every
/// Attribute is hidden shows nothing and is left out. A list longer than a rule holds is split
/// over several rules of the same types: none has a condition, so an Entity of those types
/// matches them all and shows their union (ADR-0023 rule 11), the whole list (T-4378: a census
/// table of the open data shows 497 Attributes).
fn rules(
    shown: BTreeMap<String, BTreeSet<String>>,
    hidden: &BTreeSet<String>,
) -> Result<Vec<Rule>, String> {
    let mut by_attributes: BTreeMap<Vec<String>, Vec<String>> = BTreeMap::new();
    for (entity_type, attributes) in shown {
        let attributes: Vec<String> = attributes.difference(hidden).cloned().collect();
        if attributes.is_empty() {
            continue;
        }
        by_attributes
            .entry(attributes)
            .or_default()
            .push(entity_type);
    }
    if by_attributes.is_empty() {
        return Err("it shows no Attribute of any type".into());
    }
    let rules: Vec<Rule> = by_attributes
        .into_iter()
        .flat_map(|(attributes, types)| {
            attributes
                .chunks(MAX_NAMES)
                .map(|part| (part.to_vec(), types.clone()))
                .collect::<Vec<_>>()
        })
        .collect();
    if rules.len() > MAX_RULES {
        return Err(format!(
            "its types need {} rules for their Attribute lists and a view holds at most \
             {MAX_RULES} rules of {MAX_NAMES} Attributes",
            rules.len()
        ));
    }
    Ok(rules)
}

/// The matrix rows the permissions grant, in name order.
fn rows(permits: &[&PolicySpec]) -> Result<Vec<&'static str>, String> {
    let rows: BTreeSet<&'static str> = permits
        .iter()
        .flat_map(|policy| permitted(policy))
        .flat_map(|op| operation(op).1.iter().copied())
        .collect();
    if rows.is_empty() {
        return Err("its Policies grant no operation a view serves".into());
    }
    Ok(rows.into_iter().collect())
}

/// A `geoQ` written as `georel=…;geometry=…;coordinates=…[;geoproperty=…]` as the view's
/// object; `near;maxDistance==…` keeps its distance in `georel`.
fn geo(raw: &str) -> Result<Value, String> {
    let mut parts: BTreeMap<&str, String> = BTreeMap::new();
    for part in raw.split(';').map(str::trim).filter(|p| !p.is_empty()) {
        match part.split_once('=') {
            Some((key @ ("georel" | "geometry" | "coordinates" | "geoproperty"), value)) => {
                parts.insert(key, value.to_owned());
            }
            _ if part.starts_with("maxDistance") || part.starts_with("minDistance") => {
                let georel = parts
                    .get_mut("georel")
                    .ok_or_else(|| format!("geoQ {raw:?}: {part:?} comes before georel"))?;
                georel.push(';');
                georel.push_str(part);
            }
            _ => return Err(format!("geoQ {raw:?}: {part:?} is not a geoquery member")),
        }
    }
    let mut out = Map::new();
    for key in ["georel", "geometry", "coordinates"] {
        let value = parts
            .remove(key)
            .ok_or_else(|| format!("geoQ {raw:?} has no {key}"))?;
        out.insert(key.into(), json!(value));
    }
    if let Some(property) = parts.remove("geoproperty") {
        out.insert("geoproperty".into(), json!(property));
    }
    Ok(Value::Object(out))
}

/// The view of the space surface `/cs/{space}` (plan 5.1): every type and Attribute (the `*`
/// form, ADR-0023 rule 12) with every operation its Policies grant; the engine narrows it per
/// subject. No Endpoint view takes this form.
fn space_view(sources: &Sources, project: &str, name: &str, tenant: &str) -> Result<Value, String> {
    let id = format!("{SPACE_VIEW_PREFIX}{tenant}");
    if id.len() > MAX_ID {
        return Err(format!(
            "the view {id} is {} characters and a view id at most {MAX_ID}",
            id.len()
        ));
    }
    let permits: Vec<&PolicySpec> = sources
        .policies_of(project, name)
        .iter()
        .map(|(_, policy)| policy)
        .filter(|policy| !policy.effect.is_prohibition())
        .collect();
    Ok(json!({
        "id": id,
        "tenant": tenant,
        "selector": ["*"],
        "rules": [{"types": ["*"], "attributes": ["*"]}],
        "operations": rows(&permits)?,
    }))
}

/// The rule document of one space's tenant (plan 3.3, the `joinedcontext` engine's README
/// section 2): every Policy, each operation as its clause, and per Endpoint view that binds
/// fewer than all of them (`policyRef`, an App's Endpoint) the ids it is decided by (T-3702).
fn rule_document(sources: &Sources, project: &str, space: &str) -> Value {
    let mut policies: Vec<Value> = sources
        .policies_of(project, space)
        .iter()
        .map(|(name, policy)| policy_document(project, format!("{project}/{name}"), policy, None))
        .collect();
    let mut views = Map::new();
    for e in sources
        .endpoints
        .iter()
        .filter(|e| e.project == project && e.spec.context_space_ref.name() == space)
    {
        let window = projection_of(sources, e)
            .and_then(|projection| projection.filter.as_ref())
            .and_then(|filter| filter.temporal_q.as_deref());
        // a view not listed is decided by every Policy, as it was written
        if e.spec.policy_refs().is_none() && !sources.is_app(e) && window.is_none() {
            continue;
        }
        let slug = e.spec.slug.to_string();
        let mut ids = Vec::new();
        for (name, policy) in sources.bound(e) {
            // The projection's window narrows every permit bound to the view (T-3632, G6): a
            // permit's own window is intersected with it; a prohibition keeps every time.
            let Some(window) = window.filter(|_| !policy.effect.is_prohibition()) else {
                ids.push(format!("{project}/{name}"));
                continue;
            };
            // `endpoint_view` refuses a view whose windows cannot be intersected, so this one
            // is never written; its permit is listed nowhere and grants nothing through it.
            let Ok(narrowed) = intersect(window, policy.temporal_q.as_deref()) else {
                continue;
            };
            let id = format!("{project}/{name}@{slug}");
            policies.push(policy_document(
                project,
                id.clone(),
                policy,
                Some(&narrowed),
            ));
            ids.push(id);
        }
        views.insert(slug, json!(ids));
    }
    if sources
        .quarantines
        .contains(&(project.to_owned(), space.to_owned()))
    {
        policies.iter_mut().for_each(hide_quarantined);
    }
    if views.is_empty() {
        return json!({ "policies": policies });
    }
    json!({ "policies": policies, "views": views })
}

/// One Policy of `project` as the engine reads it (README section 2), under `id`, its `temporalQ`
/// the `window` when one is given (already intersected with the Policy's own).
fn policy_document(project: &str, id: String, policy: &PolicySpec, window: Option<&str>) -> Value {
    let mut clauses: Vec<&str> = Vec::new();
    for op in permitted(policy) {
        for clause in operation(op).0 {
            if !clauses.contains(clause) {
                clauses.push(clause);
            }
        }
    }
    let mut doc = Map::new();
    doc.insert("id".into(), json!(id));
    doc.insert(
        "effect".into(),
        json!(if policy.effect.is_prohibition() {
            "prohibit"
        } else {
            "permit"
        }),
    );
    doc.insert(
        "assignee".into(),
        json!({"kind": policy.assignee.kind.as_str(), "id": assignee_id(project, policy)}),
    );
    doc.insert("operations".into(), json!(clauses));
    if let Some(validity) = &policy.validity {
        doc.insert("validity".into(), json!(validity));
    }
    if !policy.information.is_empty() {
        doc.insert("information".into(), json!(policy.information));
    }
    for (key, value) in [
        ("q", policy.q.as_deref()),
        ("scopeQ", policy.scope_q.as_deref()),
        ("geoQ", policy.geo_q.as_deref()),
        ("temporalQ", window.or(policy.temporal_q.as_deref())),
    ] {
        if let Some(value) = value {
            doc.insert(key.into(), json!(value));
        }
    }
    Value::Object(doc)
}

/// The gateway's own account (context-gateway `GATEWAY_CLIENT`, T-3767): the hops it makes on
/// nobody's behalf (a conditional write's probe, a held delete's unlink, a notification's read,
/// a download's type list).
const GATEWAY_CLIENT: &str = "context-gateway";

/// The clauses those hops use: the unlinking's attribute patch, attribute delete and entity
/// delete, the probes' retrieve and query, a download's type list (T-3790), and the stored
/// subscription's retrieve. An Endpoint's WebSocket fan-out subscribes as its caller (T-4755,
/// ADR-N-049 item 1), so the account creates no subscription and no view lists it (T-5339).
const GATEWAY_CLAUSES: &[&str] = &[
    "5.6.4", "5.6.5", "5.6.6", "5.7.1", "5.7.2", "5.7.5", "5.8.3",
];

/// A rule document plus the one policy that admits the gateway's own hops; `@` keeps its id
/// apart from every `{project}/{name}`.
/// The assignee as the gateway pins the caller: a bare `serviceAccount` is the account of the
/// Policy's own project, `{project}/{name}`, as the gateway sends `x-jc-client`; a same-named
/// account of another project the Endpoint admits never matches it (PL-92, F-SEC1-003, T-5860).
fn assignee_id(project: &str, policy: &PolicySpec) -> String {
    let id = &policy.assignee.id;
    if policy.assignee.kind == PrincipalKind::ServiceAccount && !id.contains('/') {
        format!("{project}/{id}")
    } else {
        id.clone()
    }
}

fn with_gateway(mut document: Value) -> Value {
    if let Some(policies) = document["policies"].as_array_mut() {
        policies.push(json!({
            "id": format!("@{GATEWAY_CLIENT}"),
            "effect": "permit",
            "assignee": {"kind": "serviceAccount", "id": GATEWAY_CLIENT},
            "operations": GATEWAY_CLAUSES,
        }));
    }
    document
}

/// Whether the writer owns a view of this id: an Endpoint slug or a space surface.
fn owned(id: &str) -> bool {
    id.starts_with(SPACE_VIEW_PREFIX)
        || ((26..=MAX_ID).contains(&id.len())
            && id.bytes().all(|b| matches!(b, b'a'..=b'z' | b'2'..=b'7')))
}

/// What happened to one Endpoint's or Context Space's view and document in a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewOutcome {
    /// The broker holds what the repository declares.
    Written,
    /// The writer cannot express it; nothing was sent.
    Refused(String),
    /// The broker refused it or did not answer.
    Failed(String),
}

/// A model as Model Tools compiled it: the digest of its source, its `brokerSchema` and its
/// JSON-LD `context`. The digest, not the source and its imports, is held (T-4353).
type Compiled = ([u8; 32], Value, Option<Value>);

/// One HTTP client, the broker's address and where its admin token is mounted.
pub struct ViewSync {
    http: reqwest::Client,
    /// The broker `main` as this Portal reaches it, without a trailing slash.
    broker: String,
    /// The address of every other `ContextBroker`, `{broker}` replaced by its name
    /// (`JC_PORTAL_BROKER_URL_TEMPLATE`, ADR-N-047).
    template: Option<String>,
    token_file: PathBuf,
    /// The tenants whose rule document this Portal wrote since it started, each with the broker
    /// it went to: only those are deleted, on that broker, when their space goes or moves.
    // ponytail: in memory, so a space deleted while the Portal restarts keeps its document;
    // with no view left to route to it, nothing reads it.
    written_documents: Mutex<BTreeMap<String, String>>,
    /// Model Tools, which compiles a space's data model into its tenant's schema.
    model_tools: Option<String>,
    /// The tenants whose schema this Portal wrote since it started, as `written_documents`.
    written_schemas: Mutex<BTreeMap<String, String>>,
    /// The last schema compiled per tenant and the model it came from: an unchanged model is
    /// not compiled again on every run.
    // ponytail: a Model Tools upgrade reaches an unchanged model when the Portal restarts.
    compiled: Mutex<BTreeMap<String, Compiled>>,
    /// What the last run read, which a preview compiles its draft against (T-3802): the
    /// leader's converge and every follower's sync record it, so any replica previews (T-3820).
    last: Mutex<Option<Arc<Sources>>>,
}

/// What a pipeline's write meets at one Endpoint ([`ViewSync::write_view`], T-4840).
#[derive(Debug, Clone)]
pub struct WriteView {
    pub space: String,
    pub tenant: String,
    /// The Policies bound to the Endpoint, the gateway's input.
    pub policies: Vec<PolicySpec>,
    /// The broker's view of the Endpoint, or why it has none.
    pub view: Result<Value, String>,
}

/// The part of an Endpoint's filter the Portal's editor changes (API/01 §40): the classes,
/// slots and rows of its `ModelProjection`, and its hidden attributes.
#[derive(Debug, Clone, Default)]
pub struct Draft {
    /// `None` names no projection: the Endpoint then serves the classes its Policies grant.
    pub projection: Option<(Vec<ProjectedClass>, Option<ProjectionFilter>)>,
    pub hidden_attributes: Vec<String>,
}

impl ViewSync {
    /// A writer against one broker, its admin token read from `token_file` on every run.
    pub fn new(broker: impl Into<String>, token_file: impl Into<PathBuf>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(TIMEOUT)
                .build()
                .unwrap_or_default(),
            broker: broker.into().trim_end_matches('/').to_owned(),
            template: None,
            token_file: token_file.into(),
            written_documents: Mutex::default(),
            model_tools: None,
            written_schemas: Mutex::default(),
            compiled: Mutex::default(),
            last: Mutex::default(),
        }
    }

    /// Addresses every broker but `main` by `template`, `{broker}` replaced by its name.
    pub fn with_broker_template(mut self, template: Option<String>) -> Self {
        self.template = template;
        self
    }

    /// The address of broker `name`: `main` is the configured broker, any other the template's;
    /// `None` for a broker this Portal cannot reach, whose spaces are never written to `main`.
    fn address(&self, name: &str) -> Option<String> {
        crate::spaces::broker_address(name, Some(&self.broker), self.template.as_deref())
    }

    /// Compiles each space's data model with the Model Tools at `url` and writes it as the
    /// tenant's schema. Without it a space with a `dataModelRef` says its schema is not written.
    pub fn with_model_tools(mut self, url: impl Into<String>) -> Self {
        self.model_tools = Some(url.into().trim_end_matches('/').to_owned());
        self
    }

    /// Brings the broker's views and rule documents to what the repository declares, and says
    /// per Endpoint and Context Space what happened.
    pub async fn converge(&self, repo: &Repository) -> Vec<(Owner, ViewOutcome)> {
        let sources = self.remember(repo);
        self.apply(desired(&sources), Pass::Whole).await
    }

    /// Writes only what `repo` asks of the broker beyond the last read, and removes nothing: a
    /// merged change reaches the broker without a whole run (CC-65, T-6455). What is gone from
    /// `repo` stays for the whole run behind it, except a rule document: it is one object, so a
    /// revoked Policy leaves here. Compiled by the same [`desired`] as a whole run.
    pub async fn converge_merged(&self, repo: &Repository) -> Vec<(Owner, ViewOutcome)> {
        let before = lock(&self.last).clone();
        let sources = self.remember(repo);
        let Some(before) = before else {
            return Vec::new();
        };
        self.apply(desired(&sources).beyond(&desired(&before)), Pass::Merged)
            .await
    }

    /// Reads `repo` as the one a draft compiles against, and writes nothing: a replica that does
    /// not lead the reconciler calls it on every sync, so its preview answers like the leader's
    /// (T-3820, EP-85).
    pub fn remember(&self, repo: &Repository) -> Arc<Sources> {
        let sources = Arc::new(Sources::read(repo));
        *lock(&self.last) = Some(Arc::clone(&sources));
        sources
    }

    /// What a pipeline's write through the Endpoint `name` of `project` meets (T-4840): its space
    /// and tenant, the Policies bound to it with `derived` (the pipeline's own, from its draft) in
    /// place of what the last run derived for `account` (`None` keeps the account's Policies, the
    /// shared `pipelines` account's), and the view those Policies make, or why there is none.
    /// `None` when the last run read no such Endpoint.
    pub fn write_view(
        &self,
        project: &str,
        name: &str,
        account: &str,
        derived: Option<&[(String, PolicySpec)]>,
    ) -> Option<WriteView> {
        let sources = lock(&self.last).clone()?;
        let e = sources
            .endpoints
            .iter()
            .find(|e| e.project == project && e.name == name)?;
        let key = (
            e.project.clone(),
            e.spec.context_space_ref.name().to_owned(),
        );
        let mut policies: Vec<(String, PolicySpec)> = sources
            .policies_of(&key.0, &key.1)
            .iter()
            .filter(|(_, p)| {
                derived.is_none()
                    || !(p.assignee.kind == PrincipalKind::ServiceAccount
                        && p.assignee.id == account)
            })
            .cloned()
            .collect();
        policies.extend(
            derived
                .unwrap_or_default()
                .iter()
                .filter(|(_, p)| p.context_space_ref.name() == key.1)
                .cloned(),
        );
        // Only what this Endpoint's view is made of, so a check copies no other space's model.
        let narrow = Sources {
            endpoints: vec![e.clone()],
            policies: BTreeMap::from([(key.clone(), policies)]),
            projections: sources.projections.clone(),
            classes: sources
                .classes
                .get(&key)
                .map(|c| BTreeMap::from([(key.clone(), c.clone())]))
                .unwrap_or_default(),
            apps: sources.apps.clone(),
            mappings: sources
                .mappings
                .iter()
                .filter(|(_, m)| m.space == key.1)
                .map(|(k, m)| (k.clone(), m.clone()))
                .collect(),
            models: sources
                .models
                .get(&key)
                .map(|m| BTreeMap::from([(key.clone(), m.clone())]))
                .unwrap_or_default(),
            quarantines: sources.quarantines.clone(),
            ..Sources::default()
        };
        let bound = narrow
            .bound(e)
            .into_iter()
            .map(|(_, p)| p.clone())
            .collect();
        Some(WriteView {
            space: key.1,
            tenant: e.tenant.clone(),
            policies: bound,
            view: endpoint_view(&narrow, e),
        })
    }

    /// The view id of the Endpoint `name` of `project` and its view with `draft` in place of
    /// its saved projection and hidden attributes (T-3802, EP-85): its Policies, audience and
    /// view mapping stay as the last run read them. `Ok(None)` when that run read no such
    /// Endpoint; `Err` names the draft's member at fault, or why the Endpoint has no view.
    pub fn draft_view(
        &self,
        project: &str,
        name: &str,
        draft: &Draft,
    ) -> Result<Option<(String, Value)>, String> {
        let Some(sources) = lock(&self.last).clone() else {
            return Ok(None);
        };
        let Some(saved) = sources
            .endpoints
            .iter()
            .find(|e| e.project == project && e.name == name)
        else {
            return Ok(None);
        };
        let hidden = Projection {
            hidden_attributes: draft.hidden_attributes.clone(),
        };
        hidden
            .validate()
            .map_err(|err| format!("hiddenAttributes: {err}"))?;
        let projection = match &draft.projection {
            None => None,
            Some((classes, filter)) => {
                // A window is the rule document's, which the preview does not replace: a
                // draft's would be left out and the page read wider than the draft says.
                if filter.as_ref().is_some_and(|f| f.temporal_q.is_some()) {
                    return Err(
                        "projection.filter.temporalQ: a draft's window is not previewed (G6)"
                            .into(),
                    );
                }
                // Only the classes, their slots and the filter reach the view; the references
                // are the saved projection's, or neutral ones.
                let data_model_ref = projection_of(&sources, saved).map_or_else(
                    || DataModelRef::new("preview", "1"),
                    |saved| saved.data_model_ref.clone(),
                );
                let spec = ModelProjectionSpec {
                    context_space_ref: saved.spec.context_space_ref.name().to_owned(),
                    data_model_ref,
                    classes: classes.clone(),
                    filter: filter.clone(),
                };
                spec.validate().map_err(|err| {
                    format!(
                        "projection: {}",
                        err.to_string().replace("spec.", "projection.")
                    )
                })?;
                Some(spec)
            }
        };
        let mut e = saved.clone();
        e.spec.projection = (!hidden.hidden_attributes.is_empty()).then_some(hidden);
        let view = view_of(&sources, &e, projection.as_ref())?;
        Ok(Some((e.spec.slug.to_string(), view)))
    }

    /// The view id of the Endpoint `name` of `project` and its view as the last run compiled it
    /// (the KPI preview, PL-105). `Ok(None)` when that run read no such Endpoint.
    pub fn saved_view(&self, project: &str, name: &str) -> Result<Option<(String, Value)>, String> {
        let Some(sources) = lock(&self.last).clone() else {
            return Ok(None);
        };
        let Some(saved) = sources
            .endpoints
            .iter()
            .find(|e| e.project == project && e.name == name)
        else {
            return Ok(None);
        };
        Ok(Some((
            saved.spec.slug.to_string(),
            endpoint_view(&sources, saved)?,
        )))
    }

    /// One page of `query` (a Query Entities query string) through the candidate `view`, read
    /// as the subject `headers` name (Antares `POST /q/views/{id}/preview`, T-3813): the
    /// broker's status and body. Nothing is stored.
    pub async fn preview(
        &self,
        id: &str,
        view: &Value,
        query: &str,
        headers: &[(&'static str, String)],
    ) -> Result<(u16, Value), String> {
        let token = match std::fs::read_to_string(&self.token_file) {
            Ok(token) if !token.trim().is_empty() => token.trim().to_owned(),
            _ => return Err("the broker's admin token is not readable".into()),
        };
        let broker = lock(&self.last)
            .as_ref()
            .and_then(|sources| {
                let tenant = &sources
                    .endpoints
                    .iter()
                    .find(|e| e.spec.slug.to_string() == id)?
                    .tenant;
                sources.brokers.get(tenant).cloned()
            })
            .unwrap_or_else(|| jc_core::kinds::IMPLICIT_BROKER.to_owned());
        let base = self
            .address(&broker)
            .ok_or_else(|| format!("the space's broker '{broker}' has no address"))?;
        let mut request = self
            .http
            .post(format!("{base}/q/views/{id}/preview?{query}"))
            .bearer_auth(token)
            .json(view);
        for (name, value) in headers {
            request = request.header(*name, value);
        }
        let answer = request
            .send()
            .await
            .map_err(|err| format!("the broker did not answer: {}", err.without_url()))?;
        let status = answer.status().as_u16();
        let body = crate::capped::json(answer, crate::capped::ANSWER_BYTES).await;
        Ok((status, body.unwrap_or(Value::Null)))
    }

    async fn apply(&self, desired: Desired, pass: Pass) -> Vec<(Owner, ViewOutcome)> {
        // A tenant whose space lost its model, or is gone, keeps no compiled schema (T-4353).
        if pass == Pass::Whole {
            lock(&self.compiled).retain(|tenant, _| desired.schemas.contains_key(tenant));
        }
        let mut outcomes: BTreeMap<Owner, ViewOutcome> = BTreeMap::new();
        for (owner, reason) in &desired.refused {
            outcomes.insert(owner.clone(), ViewOutcome::Refused(reason.clone()));
        }
        let mut note = |owner: &Owner, outcome: ViewOutcome| {
            let held = outcomes
                .entry(owner.clone())
                .or_insert(ViewOutcome::Written);
            if *held == ViewOutcome::Written {
                *held = outcome;
            }
        };
        let token = match std::fs::read_to_string(&self.token_file) {
            Ok(token) if !token.trim().is_empty() => token.trim().to_owned(),
            _ => {
                let reason = format!(
                    "the broker's admin token is not readable at {}",
                    self.token_file.display()
                );
                for (owner, _) in desired.views.values().chain(desired.documents.values()) {
                    note(owner, ViewOutcome::Failed(reason.clone()));
                }
                return outcomes.into_iter().collect();
            }
        };

        // One pass per broker (ADR-N-047): each is told the views, rule documents and schemas of
        // the tenants that live on it, and keeps none of another broker's. A tenant whose broker
        // has no address is written nowhere, never to `main`.
        let broker_of = |tenant: &str| {
            desired
                .brokers
                .get(tenant)
                .cloned()
                .unwrap_or_else(|| jc_core::kinds::IMPLICIT_BROKER.to_owned())
        };
        let mut parts: BTreeMap<String, Desired> = BTreeMap::new();
        parts
            .entry(jc_core::kinds::IMPLICIT_BROKER.to_owned())
            .or_default();
        for (id, entry) in &desired.views {
            let tenant = desired
                .tenants
                .get(id)
                .map(String::as_str)
                .unwrap_or_default();
            parts
                .entry(broker_of(tenant))
                .or_default()
                .views
                .insert(id.clone(), entry.clone());
        }
        for (tenant, entry) in &desired.documents {
            parts
                .entry(broker_of(tenant))
                .or_default()
                .documents
                .insert(tenant.clone(), entry.clone());
        }
        for (tenant, entry) in &desired.schemas {
            parts
                .entry(broker_of(tenant))
                .or_default()
                .schemas
                .insert(tenant.clone(), entry.clone());
        }
        for (tenant, entry) in &desired.retention {
            parts
                .entry(broker_of(tenant))
                .or_default()
                .retention
                .insert(tenant.clone(), entry.clone());
        }
        for (tenant, entry) in &desired.quality {
            parts
                .entry(broker_of(tenant))
                .or_default()
                .quality
                .insert(tenant.clone(), entry.clone());
        }
        let mut passes: BTreeMap<String, Desired> = BTreeMap::new();
        for (broker, part) in parts {
            let Some(base) = self.address(&broker) else {
                let reason = match broker.as_str() {
                    "" => "the space's brokerRef names no ContextBroker, so nothing is written; \
                           fix the space's manifest (PF-111)"
                        .to_owned(),
                    name => format!(
                        "the space's broker '{name}' has no address, so nothing is written; set \
                         JC_PORTAL_BROKER_URL_TEMPLATE (PF-111)"
                    ),
                };
                for (owner, _) in part.views.values().chain(part.documents.values()) {
                    note(owner, ViewOutcome::Failed(reason.clone()));
                }
                for owner in part
                    .schemas
                    .values()
                    .map(|(owner, _)| owner)
                    .chain(part.retention.values().map(|(owner, _)| owner))
                    .chain(part.quality.values().map(|(owner, _)| owner))
                {
                    note(owner, ViewOutcome::Failed(reason.clone()));
                }
                continue;
            };
            passes.insert(base, part);
        }
        // A broker that held a document or schema and now holds no space still gets its pass,
        // so what a deleted space left there is removed.
        if pass == Pass::Whole {
            let held: Vec<String> = lock(&self.written_documents)
                .values()
                .chain(lock(&self.written_schemas).values())
                .cloned()
                .collect();
            for base in held {
                passes.entry(base).or_default();
            }
        }
        for (base, part) in &passes {
            self.apply_on(base, &token, part, pass, &mut note).await;
        }
        outcomes.into_iter().collect()
    }

    /// Brings one broker at `base` to `part`, the share of `all` that lives on it.
    async fn apply_on(
        &self,
        base: &str,
        token: &str,
        part: &Desired,
        pass: Pass,
        note: &mut impl FnMut(&Owner, ViewOutcome),
    ) {
        let listed = match self.get(base, token, "/q/views").await {
            Ok(Some(Value::Object(listed))) => listed,
            other => {
                let reason = match other {
                    Err(reason) => reason,
                    _ => "the broker serves no views (built without the views feature)".into(),
                };
                for (owner, _) in part.views.values().chain(part.documents.values()) {
                    note(owner, ViewOutcome::Failed(reason.clone()));
                }
                return;
            }
        };
        // One read per view, most unchanged, so they go several at once (T-5804): one after
        // the other they held a prod sync run for minutes. The outcomes keep the views' order.
        let mut reads = Vec::with_capacity(part.views.len());
        for (id, (_, view)) in &part.views {
            let path = format!("/q/views/{id}");
            reads.push(async move { self.put_if_changed(base, token, &path, view).await });
        }
        let written: Vec<ViewOutcome> = futures_util::stream::iter(reads)
            .buffered(WRITES_AT_ONCE)
            .collect()
            .await;
        for ((owner, _), outcome) in part.views.values().zip(written) {
            note(owner, outcome);
        }
        for id in listed.keys() {
            if pass == Pass::Whole && owned(id) && !part.views.contains_key(id) {
                match self.delete(base, token, &format!("/q/views/{id}")).await {
                    Ok(()) => tracing::info!(view = %id, "view removed with its Endpoint or space"),
                    Err(reason) => tracing::warn!(view = %id, %reason, "view not removed"),
                }
            }
        }

        for (tenant, (owner, model)) in &part.schemas {
            let outcome = match self.broker_schema(tenant, model).await {
                Ok(schema) => {
                    self.put_if_changed(base, token, &format!("/q/schemas/{tenant}"), &schema)
                        .await
                }
                Err(outcome) => outcome,
            };
            if outcome == ViewOutcome::Written {
                lock(&self.written_schemas).insert(tenant.clone(), base.to_owned());
            }
            note(owner, outcome);
        }
        let gone: Vec<String> = lock(&self.written_schemas)
            .iter()
            .filter(|(tenant, at)| {
                pass == Pass::Whole && at.as_str() == base && !part.schemas.contains_key(*tenant)
            })
            .map(|(tenant, _)| tenant.clone())
            .collect();
        for tenant in gone {
            match self
                .delete(base, token, &format!("/q/schemas/{tenant}"))
                .await
            {
                Ok(()) => {
                    lock(&self.written_schemas).remove(&tenant);
                    tracing::info!(%tenant, "schema removed with its space's data model");
                }
                Err(reason) => tracing::warn!(%tenant, %reason, "schema not removed"),
            }
        }

        for (tenant, (owner, rules)) in &part.retention {
            let path = format!("/q/retention/{tenant}");
            let outcome = if rules.is_empty() {
                match self.delete(base, token, &path).await {
                    Ok(()) => ViewOutcome::Written,
                    Err(reason) => ViewOutcome::Failed(reason),
                }
            } else {
                let context = lock(&self.compiled)
                    .get(tenant)
                    .and_then(|(_, _, context)| context.clone());
                let document = retention_document(rules, context.as_ref());
                self.put_if_changed(base, token, &path, &document).await
            };
            note(owner, outcome);
        }

        for (tenant, (owner, rules)) in &part.quality {
            let path = format!("/q/quality/{tenant}");
            let outcome = if rules.is_empty() {
                match self.delete(base, token, &path).await {
                    Ok(()) => ViewOutcome::Written,
                    Err(reason) => ViewOutcome::Failed(reason),
                }
            } else {
                let context = lock(&self.compiled)
                    .get(tenant)
                    .and_then(|(_, _, context)| context.clone());
                let document = quality_document(rules, context.as_ref());
                self.put_if_changed(base, token, &path, &document).await
            };
            note(owner, outcome);
        }

        // No engine that reads rule documents: `/q/policy` answers 404 and the views stand
        // under whatever engine the broker runs.
        match self.get(base, token, "/q/policy").await {
            Ok(Some(_)) => {}
            Ok(None) => {
                tracing::debug!("the broker runs no engine that reads rule documents");
                return;
            }
            Err(reason) => {
                for (owner, _) in part.documents.values() {
                    note(owner, ViewOutcome::Failed(reason.clone()));
                }
                return;
            }
        }
        let mut reads = Vec::with_capacity(part.documents.len());
        for (tenant, (_, document)) in &part.documents {
            let path = format!("/q/policy/{tenant}");
            reads.push(async move {
                match self.get(base, token, &path).await {
                    Ok(Some(stored)) if stored.get("document") == Some(document) => {
                        ViewOutcome::Written
                    }
                    Ok(_) => self.put(base, token, &path, document).await,
                    Err(reason) => ViewOutcome::Failed(reason),
                }
            });
        }
        let written: Vec<ViewOutcome> = futures_util::stream::iter(reads)
            .buffered(WRITES_AT_ONCE)
            .collect()
            .await;
        for ((tenant, (owner, _)), outcome) in part.documents.iter().zip(written) {
            if outcome == ViewOutcome::Written {
                self.written().insert(tenant.clone(), base.to_owned());
            }
            note(owner, outcome);
        }
        let gone: Vec<String> = self
            .written()
            .iter()
            .filter(|(tenant, at)| {
                pass == Pass::Whole && at.as_str() == base && !part.documents.contains_key(*tenant)
            })
            .map(|(tenant, _)| tenant.clone())
            .collect();
        for tenant in gone {
            match self
                .delete(base, token, &format!("/q/policy/{tenant}"))
                .await
            {
                Ok(()) => {
                    self.written().remove(&tenant);
                    tracing::info!(%tenant, "rule document removed with its space");
                }
                Err(reason) => tracing::warn!(%tenant, %reason, "rule document not removed"),
            }
        }
    }

    fn written(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, String>> {
        lock(&self.written_documents)
    }

    /// The `brokerSchema` Model Tools compiles from a space's data model, or what the space's
    /// condition says instead: a model that does not compile is refused with its messages.
    async fn broker_schema(&self, tenant: &str, model: &ModelSource) -> Result<Value, ViewOutcome> {
        let digest = model.digest();
        if let Some((held, schema, _)) = lock(&self.compiled).get(tenant) {
            if *held == digest {
                return Ok(schema.clone());
            }
        }
        let name = &model.model;
        let Some(base) = &self.model_tools else {
            return Err(ViewOutcome::Failed(format!(
                "no model tools service is configured (JC_PORTAL_MODEL_TOOLS_URL), so the data \
                 model {name} is not compiled into the broker's schema"
            )));
        };
        let mut body = json!({ "source": model.source });
        if !model.imports.is_empty() {
            body["imports"] = json!(model.imports);
        }
        let response = self
            .http
            .post(format!("{base}/generate"))
            .json(&body)
            .send()
            .await
            .map_err(|err| {
                ViewOutcome::Failed(format!(
                    "model tools did not answer for the data model {name}: {err}"
                ))
            })?;
        if !response.status().is_success() {
            let what = format!("model tools refused the data model {name}");
            return Err(ViewOutcome::Failed(refusal(&what, response, None).await));
        }
        let artifacts: crate::tools::model_tools::Artifacts =
            crate::capped::json(response, crate::capped::ANSWER_BYTES)
                .await
                .map_err(|err| {
                    ViewOutcome::Failed(format!(
                        "model tools answered the data model {name} unreadably: {err}"
                    ))
                })?;
        match artifacts.broker_schema {
            Some(schema) => {
                lock(&self.compiled).insert(
                    tenant.to_owned(),
                    (digest, schema.clone(), artifacts.context),
                );
                Ok(schema)
            }
            None if !artifacts.errors.is_empty() => Err(ViewOutcome::Refused(format!(
                "the data model {name} does not compile: {}",
                artifacts.errors.join("; ")
            ))),
            None => Err(ViewOutcome::Failed(format!(
                "model tools rendered no brokerSchema for the data model {name}: its version \
                 predates T-3703"
            ))),
        }
    }

    async fn put_if_changed(
        &self,
        base: &str,
        token: &str,
        path: &str,
        doc: &Value,
    ) -> ViewOutcome {
        match self.get(base, token, path).await {
            Ok(Some(stored)) if stored == *doc => ViewOutcome::Written,
            Ok(_) => self.put(base, token, path, doc).await,
            Err(reason) => ViewOutcome::Failed(reason),
        }
    }

    /// The JSON at `path`, `None` on 404.
    async fn get(&self, base: &str, token: &str, path: &str) -> Result<Option<Value>, String> {
        let response = self
            .http
            .get(format!("{base}{path}"))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|err| format!("the broker did not answer GET {path}: {err}"))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(refusal(&format!("GET {path}"), response, Some(token)).await);
        }
        response
            .json()
            .await
            .map(Some)
            .map_err(|err| format!("GET {path} did not answer JSON: {err}"))
    }

    /// The erasure records the broker of the space whose tenant is `tenant` holds (Antares
    /// `GET /q/erasures/{tenant}`, T-4665): what it applies again when it starts.
    pub(crate) async fn erasure_records(&self, tenant: &str) -> Result<Vec<Value>, String> {
        let token = std::fs::read_to_string(&self.token_file)
            .ok()
            .map(|token| token.trim().to_owned())
            .filter(|token| !token.is_empty())
            .ok_or("the broker's admin token is not readable")?;
        let broker = lock(&self.last)
            .as_ref()
            .and_then(|sources| sources.brokers.get(tenant).cloned())
            .unwrap_or_else(|| jc_core::kinds::IMPLICIT_BROKER.to_owned());
        let base = self
            .address(&broker)
            .ok_or_else(|| format!("the space's broker '{broker}' has no address"))?;
        Ok(self
            .get(&base, &token, &format!("/q/erasures/{tenant}"))
            .await?
            .and_then(|listed| listed.as_array().cloned())
            .unwrap_or_default())
    }

    /// Erases `attrs` (expanded under the space's `@context`; `None`: the whole Entity) of
    /// `entities` from the broker of the space whose tenant is `tenant`: current state, every
    /// version and the dead letters (Antares `POST /q/erasures/{tenant}`, T-4665). The broker
    /// keeps the record and applies it again after a restore (DM-127). Returns its counts.
    pub(crate) async fn erase(
        &self,
        tenant: &str,
        entities: &[String],
        attrs: Option<&[String]>,
        (subject, actor, reason): (&str, &str, &str),
    ) -> Result<Value, String> {
        let token = std::fs::read_to_string(&self.token_file)
            .ok()
            .map(|token| token.trim().to_owned())
            .filter(|token| !token.is_empty())
            .ok_or("the broker's admin token is not readable")?;
        let context = lock(&self.compiled)
            .get(tenant)
            .and_then(|(_, _, context)| context.clone());
        let broker = lock(&self.last)
            .as_ref()
            .and_then(|sources| sources.brokers.get(tenant).cloned())
            .unwrap_or_else(|| jc_core::kinds::IMPLICIT_BROKER.to_owned());
        let base = self
            .address(&broker)
            .ok_or_else(|| format!("the space's broker '{broker}' has no address"))?;
        let attrs: Option<Vec<String>> = attrs.map(|terms| {
            terms
                .iter()
                .flat_map(|term| type_iris(term, context.as_ref()))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect()
        });
        let response = self
            .http
            .post(format!("{base}/q/erasures/{tenant}"))
            .bearer_auth(&token)
            .json(&json!({
                "entities": entities,
                "attrs": attrs,
                "subject": subject,
                "actor": actor,
                "reason": reason,
            }))
            .send()
            .await
            .map_err(|err| format!("the broker did not answer: {}", err.without_url()))?;
        if !response.status().is_success() {
            return Err(refusal("the broker refused the erasure", response, Some(&token)).await);
        }
        let record: Value = response
            .json()
            .await
            .map_err(|err| format!("the broker answered no JSON: {err}"))?;
        Ok(record.get("counts").cloned().unwrap_or(Value::Null))
    }

    /// Per class and kind what `rules` of the space whose broker tenant is `tenant` would delete
    /// now, as the broker counts the document the reconciler would write (DM-85); with `None`,
    /// what this broker's sweeps deleted of the space's `held` rules since it started.
    pub async fn retention_counts(
        &self,
        tenant: &str,
        rules: Option<&[RetentionRule]>,
        held: &[RetentionRule],
    ) -> Result<Vec<RetentionCount>, String> {
        let token = std::fs::read_to_string(&self.token_file)
            .ok()
            .map(|token| token.trim().to_owned())
            .filter(|token| !token.is_empty())
            .ok_or("the broker's admin token is not readable")?;
        let context = lock(&self.compiled)
            .get(tenant)
            .and_then(|(_, _, context)| context.clone());
        let broker = lock(&self.last)
            .as_ref()
            .and_then(|sources| sources.brokers.get(tenant).cloned())
            .unwrap_or_else(|| jc_core::kinds::IMPLICIT_BROKER.to_owned());
        let base = self
            .address(&broker)
            .ok_or_else(|| format!("the space's broker '{broker}' has no address"))?;
        let request = match rules {
            Some(rules) => self
                .http
                .post(format!("{base}/q/retention/{tenant}/preview"))
                .json(&retention_document(rules, context.as_ref())),
            None => self
                .http
                .get(format!("{base}/q/retention/{tenant}/deleted")),
        };
        let response = request
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|err| format!("the broker did not answer: {}", err.without_url()))?;
        if !response.status().is_success() {
            return Err(refusal(
                "the broker refused the retention count",
                response,
                Some(&token),
            )
            .await);
        }
        let answer: Value = response
            .json()
            .await
            .map_err(|err| format!("the broker answered no JSON: {err}"))?;
        Ok(retention_rows(&answer, rules, held, context.as_ref()))
    }

    async fn put(&self, base: &str, token: &str, path: &str, doc: &Value) -> ViewOutcome {
        let sent = self
            .http
            .put(format!("{base}{path}"))
            .bearer_auth(token)
            .json(doc)
            .send()
            .await;
        match sent {
            Ok(response) if response.status().is_success() => ViewOutcome::Written,
            Ok(response) => ViewOutcome::Failed(
                refusal(
                    &format!("the broker refused PUT {path}"),
                    response,
                    Some(token),
                )
                .await,
            ),
            Err(err) => ViewOutcome::Failed(format!("the broker did not answer PUT {path}: {err}")),
        }
    }

    async fn delete(&self, base: &str, token: &str, path: &str) -> Result<(), String> {
        let response = self
            .http
            .delete(format!("{base}{path}"))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        match response.status().is_success() || response.status() == reqwest::StatusCode::NOT_FOUND
        {
            true => Ok(()),
            false => Err(refusal(&format!("DELETE {path}"), response, Some(token)).await),
        }
    }
}

/// A lock a panicked holder left poisoned still holds a whole set.
fn lock<T>(held: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    held.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Each Endpoint's and Context Space's view condition, replacing the one a run before set
/// (T-3640); the phase stays the owner's own.
pub(super) fn record(fresh_mirror: &crate::store::Mirror, outcomes: Vec<(Owner, ViewOutcome)>) {
    for (owner, outcome) in outcomes {
        let Some(mut envelope) = fresh_mirror.get(&owner.project, owner.kind, &owner.name) else {
            continue;
        };
        if let Some(status) = envelope.status.as_mut() {
            let condition = match &outcome {
                ViewOutcome::Written => super::streams::make_condition(
                    CONDITION,
                    "True",
                    "Written",
                    "the broker holds this view and rule document",
                ),
                ViewOutcome::Refused(reason) => {
                    super::streams::make_condition(CONDITION, "False", "WriterRefused", reason)
                }
                ViewOutcome::Failed(reason) => {
                    super::streams::make_condition(CONDITION, "False", "BrokerRefused", reason)
                }
            };
            status.conditions.retain(|held| held.r#type != CONDITION);
            status.conditions.push(condition);
        }
        fresh_mirror.upsert(envelope);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const SLUG: &str = "k4y7pq2mzt6vhx3nbwrs5cjdaf";

    fn endpoint(spec: Value) -> EndpointSource {
        endpoint_named("air-public", spec)
    }

    fn endpoint_named(name: &str, extra: Value) -> EndpointSource {
        let mut spec = json!({
            "contextSpaceRef": "air", "slug": SLUG, "audience": "public",
            "enabledRepresentations": ["ngsi-ld"]
        });
        if let (Some(spec), Some(extra)) = (spec.as_object_mut(), extra.as_object()) {
            spec.extend(extra.clone());
        }
        EndpointSource {
            project: "city".into(),
            name: name.into(),
            tenant: "city-air".into(),
            spec: serde_json::from_value(spec).expect("an Endpoint spec"),
        }
    }

    fn policy(extra: Value) -> PolicySpec {
        let mut spec = json!({
            "contextSpaceRef": "air", "assigner": "did:web:city.example",
            "assignee": {"kind": "role", "id": "public"},
            "operations": ["retrieveOps"]
        });
        if let (Some(spec), Some(extra)) = (spec.as_object_mut(), extra.as_object()) {
            spec.extend(extra.clone());
        }
        serde_json::from_value(spec).expect("a Policy spec")
    }

    fn sources(policies: Vec<(&str, PolicySpec)>) -> Sources {
        let mut sources = Sources::default();
        sources
            .spaces
            .push(("city".into(), "air".into(), "city-air".into()));
        sources.policies.insert(
            ("city".into(), "air".into()),
            policies
                .into_iter()
                .map(|(name, spec)| (name.to_owned(), spec))
                .collect(),
        );
        sources
    }

    /// T-4840: a pipeline's write meets the Policies bound to the Endpoint with its draft's own
    /// in place of what the last run derived for its account, and the view they make.
    #[test]
    fn a_pipelines_write_view_holds_its_drafts_policies_in_place_of_the_last_runs() {
        let writes = |entity_type: &str| {
            policy(json!({
                "assignee": {"kind": "serviceAccount", "id": "pl-buses"},
                "operations": ["upsertBatch", "createBatch", "queryBatch"],
                "information": [{"entities": [{"type": entity_type}]}]
            }))
        };
        let mut s = fleet(sources(vec![("pl-buses-w-1", writes("Car"))]));
        s.endpoints.push(endpoint(json!({})));
        let sync = ViewSync::new("http://broker.invalid", "/nonexistent");
        assert!(sync
            .write_view("city", "air-public", "pl-buses", None)
            .is_none());
        *lock(&sync.last) = Some(Arc::new(s));
        assert!(sync
            .write_view("city", "nowhere", "pl-buses", None)
            .is_none());

        let draft = vec![("pl-buses-w-1".to_owned(), writes("Bus"))];
        let write = sync
            .write_view("city", "air-public", "pl-buses", Some(&draft))
            .expect("the Endpoint");
        assert_eq!(
            (write.space.as_str(), write.tenant.as_str()),
            ("air", "city-air")
        );
        assert_eq!(
            write.policies,
            vec![writes("Bus")],
            "the last run's Car grant is gone"
        );
        let view = write.view.expect("a view");
        let bus = json!({"id": "urn:ngsi-ld:Bus:7", "type": "Bus",
            "speed": {"type": "Property", "value": 41, "expiresAt": "2026-10-21T00:00:00Z"},
            "expiresAt": "2026-10-21T00:00:00Z"});
        assert!(crate::record_check::judge_view(&view, "air-public", "Bus", &bus).is_empty());
        let mut stray = bus.clone();
        stray["pm25"] = json!({"type": "Property", "value": 3});
        let findings = crate::record_check::judge_view(&view, "air-public", "Bus", &stray);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].field, "pm25");
        assert!(findings[0]
            .reason
            .starts_with("not writable through air-public"));
        let car = json!({"id": "urn:ngsi-ld:Car:1", "type": "Car", "speed": {"type": "Property", "value": 1}});
        assert_eq!(
            crate::record_check::judge_view(&view, "air-public", "Car", &car)[0].field,
            "type",
            "the draft writes no Car any more"
        );

        // The shared account keeps the Policies it has.
        let kept = sync
            .write_view("city", "air-public", "pl-buses", None)
            .expect("the Endpoint");
        assert_eq!(kept.policies, vec![writes("Car")]);
    }

    /// `s` with `fleet_model()` as its space's data model.
    fn fleet(mut s: Sources) -> Sources {
        s.models
            .insert(("city".into(), "air".into()), Ok(fleet_model()));
        s
    }

    /// The space's model: Car and Bus inherit `speed` from Vehicle, Car adds `ownedBy` past
    /// its identifier slot, Stop takes `location` from a mixin, and an import holds Vehicle.
    fn fleet_model() -> ModelSource {
        ModelSource {
            model: "fleet".into(),
            source: "id: https://city.example/fleet\nname: fleet\nimports: [org.vehicles.v1]\n\
                     classes:\n  Car: {is_a: Vehicle, slots: [id, ownedBy]}\n  Bus: {is_a: Vehicle}\n\
                     \x20 Stop: {slots: [name], mixins: [Located]}\n  Located: {mixin: true, slots: [location]}\n\
                     slots:\n  id: {identifier: true}\n  ownedBy: {}\n  name: {}\n  location: {}\n"
                .into(),
            imports: BTreeMap::from([(
                "org.vehicles.v1".to_owned(),
                "id: https://city.example/vehicles\nname: vehicles\n\
                 classes:\n  Vehicle: {attributes: {speed: {range: float}}, is_a: Vehicle}\n"
                    .to_owned(),
            )]),
        }
    }

    fn cars() -> PolicySpec {
        policy(
            json!({"information": [{"entities": [{"type": "Car"}], "propertyNames": ["speed"]}]}),
        )
    }

    fn view_of(sources: &Sources, e: &EndpointSource) -> Result<Value, String> {
        endpoint_view(sources, e)
    }

    /// Plan 2.1 `id`, `tenant`, `selector` and `rules` without a projection: the slug, the
    /// space's segment, the Policies' types with their Attributes; `operations` the rows of the
    /// group they grant.
    #[test]
    fn an_endpoint_view_is_its_slug_tenant_and_policies() {
        let s = sources(vec![("cars", cars())]);
        let view = view_of(&s, &endpoint(json!({}))).expect("a view");
        assert_eq!(
            view,
            json!({"id": SLUG, "tenant": "city-air", "selector": ["Car"],
                   "rules": [{"types": ["Car"], "attributes": ["speed"]}],
                   "operations": ["query_entities", "retrieve_attribute_details", "retrieve_attribute_info", "retrieve_attribute_value", "retrieve_attributes", "retrieve_entity", "retrieve_entity_type_details", "retrieve_entity_type_info", "retrieve_entity_types"]})
        );
    }

    /// T-5299, EP-125: an Endpoint pushed past the Portal that lists `sta` on a model with no
    /// measurement gets no view, and its condition names the gap; a measurement in the model or
    /// in an import serves it, and a space without a model is propose's to judge.
    #[test]
    fn an_endpoint_listing_sta_its_model_cannot_serve_is_refused() {
        let sta = || endpoint(json!({"enabledRepresentations": ["ngsi-ld", "sta"]}));
        let mut s = fleet(sources(vec![("cars", cars())]));
        s.endpoints.push(sta());
        let refused = desired(&s);
        assert!(!refused.views.contains_key(SLUG));
        let (owner, reason) = &refused.refused[0];
        assert_eq!(owner.name, "air-public");
        assert!(
            reason.contains("fleet")
                && reason
                    .contains("`sta` needs a type with a numeric attribute that declares its unit"),
            "{reason}"
        );

        let unit = "\n    attributes: {pm10: {range: float, unit: {ucum_code: ug/m3}}}\n";
        let mut imported = fleet(sources(vec![("cars", cars())]));
        if let Some(Ok(model)) = imported.models.get_mut(&("city".into(), "air".into())) {
            for text in model.imports.values_mut() {
                text.push_str("  Air:");
                text.push_str(unit);
            }
        }
        assert!(
            view_of(&imported, &sta()).is_ok(),
            "an import's measurement serves sta"
        );

        let mut own = fleet(sources(vec![("cars", cars())]));
        if let Some(Ok(model)) = own.models.get_mut(&("city".into(), "air".into())) {
            model.source = model
                .source
                .replace("slots:\n  id:", &format!("  Air:{unit}slots:\n  id:"));
        }
        assert!(
            view_of(&own, &sta()).is_ok(),
            "the model's own measurement serves sta"
        );

        let unmodelled = sources(vec![("cars", cars())]);
        assert!(view_of(&unmodelled, &sta()).is_ok());
        assert!(
            view_of(&s, &endpoint(json!({}))).is_ok(),
            "ngsi-ld needs no model"
        );
    }

    /// Plan 2.1 `id`: a slug longer than a view id is refused with the reason, not cut.
    #[test]
    fn a_slug_over_63_characters_is_refused() {
        let long = "a".repeat(64);
        let s = sources(vec![("cars", cars())]);
        let reason = view_of(&s, &endpoint(json!({"slug": long}))).expect_err("refused");
        assert!(reason.contains("64 characters"), "{reason}");
        assert!(owned(SLUG) && owned("cs-city-air") && owned(&"a".repeat(63)));
        assert!(!owned("my-view") && !owned(&long) && !owned("k4y7pq2mzt6vhx3nbwrs5cjda"));
    }

    /// Plan 2.1 `selector`, `rules`: a projection's classes and slots, less the hidden
    /// Attributes; a class whose every slot is hidden is left out; the filter is copied.
    #[test]
    fn a_projection_gives_classes_slots_and_filter() {
        let mut s = sources(vec![("cars", cars())]);
        s.projections.insert(
            ("city".into(), "cars-only".into()),
            serde_json::from_value(json!({
                "contextSpaceRef": "air", "dataModelRef": {"name": "air", "version": "1.0.0"},
                "classes": [{"name": "Car", "slots": ["speed", "owner"]},
                            {"name": "Bus", "slots": ["speed", "owner"]},
                            {"name": "Note", "slots": ["owner"]}],
                "filter": {"q": "speed>0", "scopeQ": "/city/#",
                           "geoQ": "georel=near;maxDistance==2000;geometry=Point;coordinates=[1,2]"}
            }))
            .expect("a projection"),
        );
        let e = endpoint(json!({
            "projectionRef": {"kind": "ModelProjection", "name": "cars-only"},
            "projection": {"hiddenAttributes": ["owner"]}
        }));
        let view = view_of(&s, &e).expect("a view");
        assert_eq!(view["selector"], json!(["Bus", "Car"]));
        assert_eq!(
            view["rules"],
            json!([{"types": ["Bus", "Car"], "attributes": ["speed"]}])
        );
        assert_eq!(view["q"], json!("speed>0"));
        assert_eq!(view["scopeQ"], json!("/city/#"));
        assert_eq!(
            view["geoQ"],
            json!({"georel": "near;maxDistance==2000", "geometry": "Point", "coordinates": "[1,2]"})
        );
    }

    /// `sources` with `policies` and an Endpoint narrowed by a projection whose `temporalQ` is
    /// `window`, beside one not narrowed in time.
    fn windowed(policies: Vec<(&str, PolicySpec)>, window: &str) -> (Sources, EndpointSource) {
        let mut s = sources(policies);
        s.projections.insert(
            ("city".into(), "recent".into()),
            serde_json::from_value(json!({
                "contextSpaceRef": "air", "dataModelRef": {"name": "air", "version": "1.0.0"},
                "classes": [{"name": "Car", "slots": ["speed"]}],
                "filter": {"temporalQ": window}
            }))
            .expect("a projection"),
        );
        let e = endpoint(json!({"projectionRef": {"kind": "ModelProjection", "name": "recent"}}));
        s.endpoints = vec![
            e.clone(),
            endpoint_named("plain", json!({"slug": "b".repeat(26)})),
        ];
        (s, e)
    }

    fn temporal_q(document: &Value, id: &str) -> Value {
        document["policies"]
            .as_array()
            .and_then(|all| all.iter().find(|p| p["id"] == id))
            .map_or(Value::Null, |p| {
                p.get("temporalQ").cloned().unwrap_or(Value::Null)
            })
    }

    /// T-3812 (G6, T-3632): a projection's `temporalQ` narrows each permit bound to the view, a
    /// permit's own window intersected with it (sent back: a wider own window served its whole
    /// width); a prohibition keeps its id and every time, and the view is written.
    #[test]
    fn a_projection_temporal_q_is_the_window_of_the_views_permits() {
        let own = |q: &str| {
            policy(json!({"temporalQ": q, "information": [{"entities": [{"type": "Car"}]}]}))
        };
        let no_trucks = policy(json!({"effect": "prohibition",
                                      "information": [{"entities": [{"type": "Truck"}]}]}));
        let (s, e) = windowed(
            vec![
                ("cars", cars()),
                ("month", own("timerel=after;timeAt=P-30D")),
                ("day", own("timerel=after;timeAt=PT-24H")),
                ("no-trucks", no_trucks),
            ],
            "timerel=after;timeAt=P-7D",
        );
        assert_eq!(
            view_of(&s, &e).err(),
            None,
            "the Endpoint is served by its view"
        );

        let document = &desired(&s).documents["city-air"].1;
        let copy = |name: &str| format!("city/{name}@{SLUG}");
        assert_eq!(
            document["views"],
            json!({ SLUG: [copy("cars"), copy("month"), copy("day"), "city/no-trucks"] }),
            "a view without a window and without a policyRef stays unlisted"
        );
        assert_eq!(
            temporal_q(document, &copy("cars")),
            "timerel=after;timeAt=P-7D"
        );
        assert_eq!(
            temporal_q(document, &copy("month")),
            "timerel=after;timeAt=P-7D",
            "a wider own window is cut to the projection's"
        );
        assert_eq!(
            temporal_q(document, &copy("day")),
            "timerel=after;timeAt=PT-24H"
        );
        assert_eq!(
            temporal_q(document, "city/cars"),
            Value::Null,
            "the Policy is unchanged"
        );
        assert_eq!(
            temporal_q(document, "city/month"),
            "timerel=after;timeAt=P-30D"
        );
        assert_eq!(temporal_q(document, "city/no-trucks"), Value::Null);
    }

    /// T-3812: two windows whose ends cannot be ordered, or a window that does not parse, refuse
    /// the view, and the rule document grants nothing through it.
    #[test]
    fn windows_that_cannot_be_intersected_refuse_the_view() {
        let fixed = policy(
            json!({"temporalQ": "timerel=after;timeAt=2026-01-01T00:00:00Z",
                                  "information": [{"entities": [{"type": "Car"}]}]}),
        );
        let (s, e) = windowed(vec![("fixed", fixed)], "timerel=after;timeAt=P-7D");
        let why = view_of(&s, &e).expect_err("refused");
        assert!(why.contains("cannot be compared"), "{why}");
        assert_eq!(
            desired(&s).documents["city-air"].1["views"],
            json!({ SLUG: [] })
        );

        let (s, e) = windowed(vec![("cars", cars())], "timerel=after;timeAt=P-1M");
        assert!(view_of(&s, &e).is_err(), "months have no fixed length");
    }

    /// A writer holding `s` as its last run, which a draft compiles against (T-3802).
    fn having(s: Sources, broker: &str, token_file: &Path) -> ViewSync {
        let sync = ViewSync::new(broker, token_file);
        *lock(&sync.last) = Some(Arc::new(s));
        sync
    }

    fn cars_projection(s: &mut Sources, endpoint: EndpointSource) {
        s.projections.insert(
            ("city".into(), "cars-only".into()),
            serde_json::from_value(json!({
                "contextSpaceRef": "air", "dataModelRef": {"name": "air", "version": "1.0.0"},
                "classes": [{"name": "Car", "slots": ["speed"]}],
                "filter": {"q": "speed>0"}
            }))
            .expect("a projection"),
        );
        s.endpoints.push(endpoint);
    }

    /// EP-85: a draft compiles to the view the same filter saved compiles to, so the preview
    /// narrows as the saved Endpoint will; the Policies stay the saved ones.
    #[test]
    fn a_draft_is_the_view_of_the_same_filter_saved() {
        let mut s = sources(vec![("cars", cars())]);
        let saved = endpoint(json!({
            "projectionRef": {"kind": "ModelProjection", "name": "cars-only"},
            "projection": {"hiddenAttributes": ["owner"]}
        }));
        cars_projection(&mut s, endpoint(json!({})));
        let expected = view_of(&s, &saved).expect("the saved view");
        let sync = having(s, "http://broker.invalid", Path::new("/nonexistent"));
        let draft = Draft {
            projection: Some((
                vec![ProjectedClass {
                    name: "Car".into(),
                    slots: vec!["speed".into()],
                }],
                Some(ProjectionFilter {
                    q: Some("speed>0".into()),
                    ..ProjectionFilter::default()
                }),
            )),
            hidden_attributes: vec!["owner".into()],
        };
        let (id, view) = sync
            .draft_view("city", "air-public", &draft)
            .expect("a draft view")
            .expect("the Endpoint was read");
        assert_eq!((id.as_str(), &view), (SLUG, &expected));
        assert_eq!(view["q"], json!("speed>0"));

        // No projection in the draft: what the Policies grant, as an Endpoint without one.
        let (_, granted) = sync
            .draft_view("city", "air-public", &Draft::default())
            .expect("a draft view")
            .expect("the Endpoint was read");
        assert_eq!(
            granted["rules"],
            json!([{"types": ["Car"], "attributes": ["speed"]}])
        );
    }

    /// EP-85: a hidden attribute in the draft is out of the view's rules; a class whose every
    /// granted slot it hides is out of the view.
    #[test]
    fn a_drafts_hidden_attribute_is_absent_from_its_view() {
        let both = policy(json!({"information": [{"entities": [{"type": "Car"}],
                                                   "propertyNames": ["speed", "owner"]}]}));
        let mut s = sources(vec![("cars", both)]);
        cars_projection(&mut s, endpoint(json!({})));
        let sync = having(s, "http://broker.invalid", Path::new("/nonexistent"));
        let hiding = |hidden: &str| Draft {
            projection: None,
            hidden_attributes: vec![hidden.into()],
        };
        let (_, view) = sync
            .draft_view("city", "air-public", &hiding("speed"))
            .expect("a draft view")
            .expect("the Endpoint was read");
        assert_eq!(
            view["rules"],
            json!([{"types": ["Car"], "attributes": ["owner"]}])
        );
        let (_, view) = sync
            .draft_view("city", "air-public", &Draft::default())
            .expect("a draft view")
            .expect("the Endpoint was read");
        assert_eq!(
            view["rules"],
            json!([{"types": ["Car"], "attributes": ["owner", "speed"]}])
        );
    }

    /// The members of a draft are checked as the manifest's are, and the reason names the
    /// member; an Endpoint the last run did not read, or no run yet, has no draft view.
    #[test]
    fn a_draft_is_checked_and_an_unread_endpoint_has_none() {
        let mut s = sources(vec![("cars", cars())]);
        cars_projection(&mut s, endpoint(json!({})));
        let sync = having(s, "http://broker.invalid", Path::new("/nonexistent"));
        let hidden = Draft {
            projection: None,
            hidden_attributes: vec!["id".into()],
        };
        let reason = sync
            .draft_view("city", "air-public", &hidden)
            .expect_err("refused");
        assert!(reason.starts_with("hiddenAttributes: "), "{reason}");
        let empty = Draft {
            projection: Some((Vec::new(), None)),
            hidden_attributes: Vec::new(),
        };
        let reason = sync
            .draft_view("city", "air-public", &empty)
            .expect_err("refused");
        assert!(reason.starts_with("projection: "), "{reason}");
        let temporal = Draft {
            projection: Some((
                vec![ProjectedClass {
                    name: "Car".into(),
                    slots: vec!["speed".into()],
                }],
                Some(ProjectionFilter {
                    temporal_q: Some("timerel=after;timeAt=2026-01-01T00:00:00Z".into()),
                    ..ProjectionFilter::default()
                }),
            )),
            hidden_attributes: Vec::new(),
        };
        let reason = sync
            .draft_view("city", "air-public", &temporal)
            .expect_err("refused");
        // the window would go to the rule document, which a preview does not replace
        assert!(
            reason.starts_with("projection.filter.temporalQ"),
            "{reason}"
        );
        assert_eq!(
            sync.draft_view("city", "elsewhere", &Draft::default()),
            Ok(None)
        );
        let unread = ViewSync::new("http://broker.invalid", "/nonexistent");
        assert_eq!(
            unread.draft_view("city", "air-public", &Draft::default()),
            Ok(None)
        );
    }

    /// T-6455: an Endpoint a merge touched and the writer still refuses stays refused, or the
    /// merge would show it `Live` until the whole run.
    #[test]
    fn a_refusal_a_merge_did_not_change_still_counts() {
        let refused = Desired {
            refused: vec![(Owner::space("city", "air"), "no such type".into())],
            ..Desired::default()
        };
        let before = Desired {
            refused: refused.refused.clone(),
            ..Desired::default()
        };
        assert_eq!(refused.beyond(&before).refused.len(), 1);
    }

    /// T-3813: the page is asked with the admin token, the candidate view as the body and the
    /// subject headers as given; no admin token, no question.
    #[tokio::test]
    async fn a_preview_asks_the_broker_with_the_view_and_the_subject() {
        let broker = MockServer::start().await;
        let view = json!({"id": SLUG, "tenant": "city-air"});
        Mock::given(method("POST"))
            .and(path(format!("/q/views/{SLUG}/preview")))
            .and(wiremock::matchers::query_param("type", "Car"))
            .and(header("authorization", format!("Bearer {TOKEN}")))
            .and(header("x-jc-subject", "jana%40hel.fi"))
            .and(body_json(&view))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"count": 1, "entities": []})),
            )
            .expect(1)
            .mount(&broker)
            .await;
        let sync = ViewSync::new(broker.uri(), token_file());
        let headers = [("x-jc-subject", "jana%40hel.fi".to_owned())];
        assert_eq!(
            sync.preview(SLUG, &view, "type=Car&limit=50&offset=0", &headers)
                .await,
            Ok((200, json!({"count": 1, "entities": []})))
        );
        let tokenless = ViewSync::new(broker.uri(), "/nonexistent/token");
        assert!(tokenless
            .preview(SLUG, &view, "type=Car", &headers)
            .await
            .is_err());
    }

    fn vehicles_ir() -> Value {
        json!({"version": 2, "sourceClass": "Car", "targetClass": "Vehicle",
               "slots": [{"target": "velocity", "source": "speed", "kind": "rename"}]})
    }

    fn with_mapping(policies: Vec<(&str, PolicySpec)>, ir: Result<Value, String>) -> Sources {
        let mut s = fleet(sources(policies));
        s.mappings.insert(
            ("city".into(), "to-vehicle".into()),
            MappingSource {
                space: "air".into(),
                ir,
            },
        );
        s
    }

    fn mapped_endpoint() -> EndpointSource {
        endpoint(json!({"viewMappingRef": {"kind": "Mapping", "name": "to-vehicle"}}))
    }

    /// Plan 2.1 `mapping` (G18): the Mapping's IR as the view's `mapping`, the target class
    /// swapped for the source class in `selector` and every rule, Attribute names kept, and
    /// only the reads a mapped view serves: a write the Policies grant does not reach it.
    #[test]
    fn an_endpoint_with_a_mapping_gets_the_view_with_its_ir_and_source_types() {
        let vehicles = policy(json!({
            "operations": ["retrieveOps", "createEntity", "updateAttrs", "queryTemporal", "queryBatch"],
            "information": [{"entities": [{"type": "Vehicle"}], "propertyNames": ["speed"]}]
        }));
        let s = with_mapping(vec![("vehicles", vehicles)], Ok(vehicles_ir()));
        let view = view_of(&s, &mapped_endpoint()).expect("a view");
        assert_eq!(
            view,
            json!({"id": SLUG, "tenant": "city-air", "selector": ["Car"],
                   "rules": [{"types": ["Car"], "attributes": ["speed"]}],
                   "operations": ["query_entities", "retrieve_attribute_details", "retrieve_attribute_info", "retrieve_attribute_value", "retrieve_attributes", "retrieve_entity", "retrieve_entity_type_details", "retrieve_entity_type_info", "retrieve_entity_types", "temporal_query"],
                   "mapping": vehicles_ir()})
        );
        for op in view["operations"].as_array().into_iter().flatten() {
            assert!(
                MAPPED_READS.contains(&op.as_str().unwrap_or_default()),
                "{op}"
            );
        }
    }

    /// A Mapping the view cannot carry is the Endpoint's status, never a view.
    #[test]
    fn a_mapping_without_an_ir_is_a_status_not_a_view() {
        let vehicles = || policy(json!({"information": [{"entities": [{"type": "Vehicle"}]}]}));
        let cases = [
            (
                with_mapping(
                    vec![("v", vehicles())],
                    Err("the Mapping to-vehicle has no spec.artifacts.gatewayIr".into()),
                ),
                mapped_endpoint(),
                "gatewayIr",
            ),
            (
                with_mapping(vec![("v", vehicles())], Ok(json!({"version": 2}))),
                mapped_endpoint(),
                "names no targetClass",
            ),
            (
                with_mapping(
                    vec![("v", vehicles())],
                    Ok(
                        json!({"version": 3, "sourceClass": "Car", "targetClass": "Vehicle", "slots": []}),
                    ),
                ),
                mapped_endpoint(),
                "version 3, and this gateway serves version 2",
            ),
            (
                with_mapping(vec![("v", vehicles())], Ok(json!({"sourceClass": "Car"}))),
                mapped_endpoint(),
                "not an object with a version",
            ),
            (
                with_mapping(vec![("v", vehicles())], Ok(vehicles_ir())),
                endpoint(json!({"viewMappingRef": {"kind": "Mapping", "name": "absent"}})),
                "no Mapping of this Context Space",
            ),
            (
                with_mapping(
                    vec![
                        ("v", vehicles()),
                        (
                            "buses",
                            policy(json!({"information": [{"entities": [{"type": "Bus"}]}]})),
                        ),
                    ],
                    Ok(vehicles_ir()),
                ),
                mapped_endpoint(),
                "Policies also name Bus",
            ),
            (
                with_mapping(
                    vec![(
                        "w",
                        policy(json!({"operations": ["createEntity"],
                                      "information": [{"entities": [{"type": "Vehicle"}]}]})),
                    )],
                    Ok(vehicles_ir()),
                ),
                mapped_endpoint(),
                "no read",
            ),
        ];
        for (s, e, reason) in cases {
            let got = view_of(&s, &e).expect_err("refused");
            assert!(got.contains(reason), "{reason}: {got}");
        }
        let mut elsewhere = with_mapping(vec![("v", vehicles())], Ok(vehicles_ir()));
        for m in elsewhere.mappings.values_mut() {
            m.space = "water".into();
        }
        assert!(view_of(&elsewhere, &mapped_endpoint())
            .expect_err("refused")
            .contains("no Mapping of this Context Space"));
        let mut unservable = with_mapping(vec![("v", vehicles())], Err("no IR".into()));
        unservable.endpoints.push(mapped_endpoint());
        let desired = desired(&unservable);
        assert!(!desired.views.contains_key(SLUG));
        assert!(desired
            .refused
            .iter()
            .any(|(owner, reason)| owner.name == "air-public" && reason == "no IR"));
    }

    /// Plan 2.1 `rules`: types the Policies grant with the same Attributes share a rule; a
    /// type granted whole by one Policy is whole; a Policy without information is the model's
    /// classes; hidden Attributes on a whole type are refused rather than shown.
    #[test]
    fn policies_union_into_rules() {
        let s = fleet(sources(vec![
            ("cars", cars()),
            (
                "buses",
                policy(json!({"information": [{"entities": [{"type": "Bus"}],
                                               "propertyNames": ["speed"]},
                                              {"entities": [{"type": "Car"}],
                                               "relationshipNames": ["ownedBy"]}]})),
            ),
            (
                "stops",
                policy(json!({"information": [{"entities": [{"type": "Stop"}]}]})),
            ),
        ]));
        let view = view_of(&s, &endpoint(json!({}))).expect("a view");
        assert_eq!(
            view["rules"],
            json!([{"types": ["Stop"], "attributes": ["location", "name"]},
                   {"types": ["Car"], "attributes": ["ownedBy", "speed"]},
                   {"types": ["Bus"], "attributes": ["speed"]}])
        );
    }

    /// T-4378: a list longer than a rule holds is split over rules of the same types, whose
    /// union is the whole list less the hidden Attributes (ADR-0023 rule 11); 256 is one rule,
    /// and only more rules than a view holds are refused.
    #[test]
    fn a_long_attribute_list_is_split_over_rules_of_the_same_types() {
        let names = |n: usize| -> BTreeSet<String> { (0..n).map(|i| format!("a{i:05}")).collect() };
        let shown = |n: usize| -> BTreeMap<String, BTreeSet<String>> {
            [
                ("Census".to_owned(), names(n)),
                ("Table".to_owned(), names(n)),
            ]
            .into()
        };
        let hidden: BTreeSet<String> = ["a00007".to_owned()].into();

        let split = rules(shown(498), &hidden).expect("rules");
        assert_eq!(split.len(), 2);
        assert!(split
            .iter()
            .all(|(attributes, types)| attributes.len() <= MAX_NAMES
                && types == &["Census".to_owned(), "Table".to_owned()]));
        let union: BTreeSet<String> = split.iter().flat_map(|(a, _)| a.iter().cloned()).collect();
        assert_eq!(union, names(498).difference(&hidden).cloned().collect());
        assert_eq!(
            split[0].0.len() + split[1].0.len(),
            497,
            "no Attribute twice"
        );

        assert_eq!(
            rules(shown(MAX_NAMES), &BTreeSet::new())
                .expect("rules")
                .len(),
            1
        );
        assert!(rules(shown(MAX_NAMES * MAX_RULES), &BTreeSet::new()).is_ok());
        let reason =
            rules(shown(MAX_NAMES * MAX_RULES + 1), &BTreeSet::new()).expect_err("refused");
        assert!(
            reason.contains("65 rules") && reason.contains("at most 64"),
            "{reason}"
        );
    }

    /// T-3788: a type shown whole lists its class's slots (Antares reads `[]` as no
    /// Attribute), less the hidden ones; a whole type the model has no class for is refused,
    /// and so is every whole type when the space's model does not read.
    #[test]
    fn a_type_shown_whole_lists_its_classes_slots() {
        let whole = |types: &[&str]| {
            let entities: Vec<Value> = types.iter().map(|t| json!({"type": t})).collect();
            policy(json!({"information": [{"entities": entities}]}))
        };
        let s = fleet(sources(vec![("all", whole(&["Car", "Stop"]))]));
        let view = view_of(&s, &endpoint(json!({}))).expect("a view");
        assert_eq!(
            view["rules"],
            json!([{"types": ["Stop"], "attributes": ["location", "name"]},
                   {"types": ["Car"], "attributes": ["ownedBy", "speed"]}])
        );
        let hidden = endpoint(json!({"projection": {"hiddenAttributes": ["name", "ownedBy"]}}));
        assert_eq!(
            view_of(&s, &hidden).expect("a view")["rules"],
            json!([{"types": ["Stop"], "attributes": ["location"]},
                   {"types": ["Car"], "attributes": ["speed"]}])
        );
        let tram = fleet(sources(vec![("trams", whole(&["Car", "Tram"]))]));
        let reason = view_of(&tram, &endpoint(json!({}))).expect_err("refused");
        assert!(
            reason.contains("Tram is shown whole") && reason.contains("no class Tram"),
            "{reason}"
        );
        for model in [None, Some(Err("does not read".to_owned()))] {
            let mut s = fleet(sources(vec![("all", whole(&["Car"]))]));
            s.models.remove(&("city".into(), "air".into()));
            s.models
                .extend(model.map(|m| (("city".into(), "air".into()), m)));
            let reason = view_of(&s, &endpoint(json!({}))).expect_err("refused");
            assert!(reason.contains("Car is shown whole"), "{reason}");
        }
        let projection = {
            let mut s = fleet(sources(vec![("cars", cars())]));
            s.projections.insert(
                ("city".into(), "all".into()),
                serde_json::from_value(json!({
                    "contextSpaceRef": "air", "dataModelRef": {"name": "air", "version": "1.0.0"},
                    "classes": [{"name": "Bus"}]
                }))
                .expect("a projection"),
            );
            s
        };
        let bus = endpoint(json!({"projectionRef": {"kind": "ModelProjection", "name": "all"}}));
        assert_eq!(
            view_of(&projection, &bus).expect("a view")["rules"],
            json!([{"types": ["Bus"], "attributes": ["speed"]}]),
            "a projection class without slots is the class whole"
        );
    }

    #[test]
    fn a_classless_or_cyclic_model_reads_as_it_is() {
        let model = parsed(&fleet_model());
        assert_eq!(
            class_slots(&model, "Vehicle"),
            Some(["speed".to_owned()].into())
        );
        assert_eq!(
            class_slots(&model, "Located"),
            Some(["location".to_owned()].into())
        );
        assert_eq!(class_slots(&model, "Tram"), None);
        let core = parsed(&ModelSource {
            model: "hsl".into(),
            source: "imports: [ngsi-ld-core]\nclasses:\n  Bus: {is_a: Entity, slots: [speed]}\n"
                .into(),
            imports: BTreeMap::new(),
        });
        assert_eq!(
            class_slots(&core, "Bus"),
            Some(["location".to_owned(), "speed".to_owned()].into()),
            "the core Entity's location reaches every class built on it"
        );
        let odd = parsed(&ModelSource {
            model: "odd".into(),
            source: "classes:\n  A: {is_a: B, mixins: Gone, slots: [x]}\n  B: {is_a: A, attributes: [y]}\n".into(),
            imports: BTreeMap::from([("broken".to_owned(), "classes: [".to_owned())]),
        });
        assert_eq!(
            class_slots(&odd, "A"),
            Some(["x".to_owned(), "y".to_owned()].into()),
            "a cycle ends, a missing mixin is skipped, a list of attributes reads"
        );
    }

    /// A permission without `information` grants the space model's classes, each whole.
    #[test]
    fn a_policy_without_information_grants_the_models_classes() {
        let mut every = fleet(sources(vec![("all", policy(json!({})))]));
        assert!(
            view_of(&every, &endpoint(json!({}))).is_err(),
            "no class to select"
        );
        every.classes.insert(
            ("city".into(), "air".into()),
            ["Car".to_owned(), "Bus".to_owned()].into(),
        );
        let view = view_of(&every, &endpoint(json!({}))).expect("a view");
        assert_eq!(view["selector"], json!(["Bus", "Car"]));
    }

    /// Plan 2.1 `selector`: over 256 types is refused, not truncated.
    #[test]
    fn over_256_types_is_refused() {
        let entities: Vec<Value> = (0..257).map(|n| json!({"type": format!("T{n}")})).collect();
        let s = sources(vec![(
            "many",
            policy(json!({"information": [{"entities": entities, "propertyNames": ["a"]}]})),
        )]);
        let reason = view_of(&s, &endpoint(json!({}))).expect_err("refused");
        assert!(reason.contains("257 types"), "{reason}");
    }

    /// Plan 2.1 `operations`: groups expanded, each name to the rows of its clause; purge and
    /// Entity Maps never enter a view; a prohibition grants no operation; a Policy that grants
    /// only what no view serves leaves nothing to write.
    #[test]
    fn operations_map_to_matrix_rows() {
        let s = fleet(sources(vec![
            (
                "write",
                policy(
                    json!({"operations": ["queryBatch", "updateAttrs", "purgeEntity",
                                             "retrieveEntityMap", "queryTemporal"],
                              "information": [{"entities": [{"type": "Car"}]}]}),
                ),
            ),
            (
                "deny",
                policy(
                    json!({"effect": "prohibition", "operations": ["deleteEntity"],
                              "information": [{"entities": [{"type": "Car"}]}]}),
                ),
            ),
        ]));
        let view = view_of(&s, &endpoint(json!({}))).expect("a view");
        assert_eq!(
            view["operations"],
            json!([
                "partial_attribute_update",
                "query_entities_post",
                "retrieve_attribute_details",
                "retrieve_attribute_info",
                "retrieve_attributes",
                "retrieve_entity_type_details",
                "retrieve_entity_type_info",
                "retrieve_entity_types",
                "temporal_query",
                "temporal_query_post",
                "update_attributes"
            ])
        );
        let only_maps = fleet(sources(vec![(
            "maps",
            policy(json!({"operations": ["retrieveEntityMap"],
                          "information": [{"entities": [{"type": "Car"}]}]})),
        )]));
        assert!(view_of(&only_maps, &endpoint(json!({})))
            .expect_err("refused")
            .contains("no operation"));
        let none = fleet(sources(vec![]));
        assert!(view_of(&none, &endpoint(json!({})))
            .expect_err("refused")
            .contains("no Policy"));
    }

    /// T-3645: a grant of anything a view serves brings the schema reads of the broker's LinkML
    /// surface, a writer's too (the gateway's schema surface answered every grant); a
    /// prohibition takes none of them back, and the rule document says the same.
    #[test]
    fn every_grant_brings_the_schema_reads_and_no_prohibition_takes_them() {
        let s = sources(vec![
            (
                "writer",
                policy(json!({"operations": ["createEntity"],
                              "information": [{"entities": [{"type": "Car"}], "propertyNames": ["speed"]}]})),
            ),
            (
                "no-delete",
                policy(
                    json!({"effect": "prohibition", "operations": ["deleteEntity"],
                              "information": [{"entities": [{"type": "Car"}], "propertyNames": ["speed"]}]}),
                ),
            ),
        ]);
        let view = view_of(&s, &endpoint(json!({}))).expect("a view");
        assert_eq!(
            view["operations"],
            json!([
                "create_entity",
                "retrieve_attribute_details",
                "retrieve_attribute_info",
                "retrieve_attributes",
                "retrieve_entity_type_details",
                "retrieve_entity_type_info",
                "retrieve_entity_types"
            ])
        );
        let desired = desired(&s);
        let policies = &desired.documents["city-air"].1["policies"];
        let clauses = |id: &str| {
            policies
                .as_array()
                .into_iter()
                .flatten()
                .find(|p| p["id"] == id)
                .map(|p| p["operations"].clone())
        };
        assert_eq!(
            clauses("city/writer"),
            Some(json!(["5.6.1", "5.7.5", "5.7.7", "5.7.8", "5.7.10"]))
        );
        assert_eq!(clauses("city/no-delete"), Some(json!(["5.6.6"])));
    }

    /// T-5090: the gateway admits `PATCH …/attrs` under updateAttrs, and the broker asks 5.6.2
    /// for it: a grant of updateAttrs that opened only 5.6.4 answered every grid edit with the
    /// view's 404. A prohibition of updateAttrs takes both forms back.
    #[test]
    fn update_attrs_admits_both_attribute_edits() {
        let s = sources(vec![
            (
                "editor",
                policy(json!({"operations": ["updateAttrs"],
                              "information": [{"entities": [{"type": "Car"}], "propertyNames": ["speed"]}]})),
            ),
            (
                "no-edit",
                policy(
                    json!({"effect": "prohibition", "operations": ["updateAttrs"],
                              "information": [{"entities": [{"type": "Car"}], "propertyNames": ["speed"]}]}),
                ),
            ),
        ]);
        let view = view_of(&s, &endpoint(json!({}))).expect("a view");
        let rows = view["operations"].as_array().expect("rows");
        for row in ["update_attributes", "partial_attribute_update"] {
            assert!(rows.contains(&json!(row)), "{row} in {rows:?}");
        }
        let desired = desired(&s);
        let policies = &desired.documents["city-air"].1["policies"];
        let clauses = |id: &str| {
            policies
                .as_array()
                .into_iter()
                .flatten()
                .find(|p| p["id"] == id)
                .map(|p| p["operations"].clone())
        };
        assert_eq!(
            clauses("city/editor"),
            Some(json!([
                "5.6.4", "5.6.2", "5.7.5", "5.7.7", "5.7.8", "5.7.10"
            ]))
        );
        assert_eq!(clauses("city/no-edit"), Some(json!(["5.6.4", "5.6.2"])));
    }

    /// Every Table 4.20-1 name has the clause the broker asks its engine: Query Entities has two
    /// names, and a type's or an attribute's details share the list's clause (one `/types`, one
    /// `/attributes`, T-3645).
    #[test]
    fn every_operation_has_a_clause() {
        let mut clauses: BTreeMap<&str, Vec<Operation>> = BTreeMap::new();
        for op in Operation::ALL {
            clauses.entry(operation(*op).0[0]).or_default().push(*op);
        }
        assert_eq!(clauses.len(), Operation::ALL.len() - 3);
        assert_eq!(
            clauses["5.7.2"],
            [Operation::QueryEntity, Operation::QueryBatch]
        );
        assert_eq!(
            clauses["5.7.5"],
            [
                Operation::RetrieveEntityTypes,
                Operation::RetrieveEntityTypeDetails
            ]
        );
        assert_eq!(
            clauses["5.7.8"],
            [
                Operation::RetrieveAttrTypes,
                Operation::RetrieveAttrTypeDetails
            ]
        );
    }

    /// Plan 2.1 `policyRef`: the one Policy it names, of this space; another space's binds
    /// nothing. An App's Endpoint keeps its own grants and the prohibitions (AP-139).
    #[test]
    fn policy_ref_and_app_endpoints_bind_as_the_gateway_does() {
        let own = jc_core::kinds::endpoint_role("city", "app-map", None);
        let s = {
            let mut s = fleet(sources(vec![
                ("cars", cars()),
                (
                    "buses",
                    policy(json!({"information": [{"entities": [{"type": "Bus"}]}]})),
                ),
                (
                    "mine",
                    policy(json!({"assignee": {"kind": "role", "id": own},
                                  "information": [{"entities": [{"type": "Stop"}]}]})),
                ),
            ]));
            s.apps.insert(("city".into(), "map".into()));
            s
        };
        let bound =
            endpoint(json!({"policyRef": "urn:ngsi-ld:Policy:city.example:city-air:buses"}));
        assert_eq!(
            view_of(&s, &bound).expect("a view")["selector"],
            json!(["Bus"])
        );
        let elsewhere =
            endpoint(json!({"policyRef": "urn:ngsi-ld:Policy:city.example:other:buses"}));
        assert!(view_of(&s, &elsewhere).is_err());
        let app = endpoint_named("app-map", json!({}));
        assert_eq!(
            view_of(&s, &app).expect("a view")["selector"],
            json!(["Stop"])
        );
    }

    /// ADR-N-057 decision 2 (T-5037): `policyRefs` binds the listed Policies of the space, as
    /// the gateway does; an entry naming another space adds nothing.
    #[test]
    fn policy_refs_bind_the_listed_policies_as_the_gateway_does() {
        let s = fleet(sources(vec![
            ("cars", cars()),
            (
                "buses",
                policy(json!({"information": [{"entities": [{"type": "Bus"}]}]})),
            ),
            (
                "stops",
                policy(json!({"information": [{"entities": [{"type": "Stop"}]}]})),
            ),
        ]));
        let listed = endpoint(json!({"policyRefs": [
            "urn:ngsi-ld:Policy:city.example:city-air:stops",
            "urn:ngsi-ld:Policy:city.example:other:cars",
            "urn:ngsi-ld:Policy:city.example:city-air:buses",
        ]}));
        assert_eq!(
            view_of(&s, &listed).expect("a view")["selector"],
            json!(["Bus", "Stop"])
        );
    }

    /// T-3702: the rule document lists, per Endpoint view that binds fewer than all the
    /// space's Policies, the ids it is decided by: `policyRef` its one Policy (another space's
    /// binds none), an App's Endpoint its own grants and the prohibitions. An Endpoint binding
    /// every Policy is not listed, so the engine decides it with all of them.
    #[test]
    fn the_rule_document_lists_the_policies_each_bound_view_is_decided_by() {
        let own = jc_core::kinds::endpoint_role("city", "app-map", None);
        let mut s = sources(vec![
            ("cars", cars()),
            (
                "buses",
                policy(json!({"information": [{"entities": [{"type": "Bus"}]}]})),
            ),
            (
                "mine",
                policy(json!({"assignee": {"kind": "role", "id": own},
                              "information": [{"entities": [{"type": "Stop"}]}]})),
            ),
            (
                "no-trucks",
                policy(json!({"effect": "prohibition",
                              "information": [{"entities": [{"type": "Truck"}]}]})),
            ),
        ]);
        s.apps.insert(("city".into(), "map".into()));
        let slug = |c: char| c.to_string().repeat(26);
        let with_slug = |name: &str, c: char, mut extra: Value| {
            if let Some(o) = extra.as_object_mut() {
                o.insert("slug".into(), json!(slug(c)));
            }
            endpoint_named(name, extra)
        };
        s.endpoints = vec![
            with_slug("air-public", 'a', json!({})),
            with_slug(
                "buses-only",
                'b',
                json!({"policyRef": "urn:ngsi-ld:Policy:city.example:city-air:buses"}),
            ),
            with_slug(
                "elsewhere",
                'c',
                json!({"policyRef": "urn:ngsi-ld:Policy:city.example:other:buses"}),
            ),
            with_slug("app-map", 'd', json!({})),
        ];
        let document = &desired(&s).documents["city-air"].1;
        assert_eq!(
            document["views"],
            json!({
                slug('b'): ["city/buses"],
                slug('c'): [],
                slug('d'): ["city/mine", "city/no-trucks"],
            })
        );
    }

    /// T-5036, ADR-N-057: a `rules[]` Policy is one engine policy per rule, `{policy}#{n}`,
    /// permit or prohibit as the rule says, and a `policyRef` naming the Policy binds every rule.
    #[test]
    fn a_rules_policy_is_one_engine_policy_per_rule() {
        let kiosks: PolicySpec = serde_json::from_value(json!({
            "contextSpaceRef": "air", "assigner": "did:web:city.example",
            "rules": [
                {"assignee": {"kind": "role", "id": "public"}, "operations": ["retrieveOps"]},
                {"effect": "prohibition", "assignee": {"kind": "role", "id": "public"},
                 "operations": ["queryEntity"], "information": [{"entities": [{"type": "Truck"}]}]}
            ]
        }))
        .expect("a rules Policy");
        let rules = kiosks.expand("kiosks").expect("a valid shape");
        let mut named: Vec<(&str, PolicySpec)> = rules
            .iter()
            .map(|(name, rule)| (name.as_str(), rule.clone()))
            .collect();
        named.push(("cars", cars()));
        let mut s = sources(named);
        s.endpoints = vec![endpoint_named(
            "kiosks",
            json!({"slug": "b".repeat(26),
                   "policyRef": "urn:ngsi-ld:Policy:city.example:city-air:kiosks"}),
        )];
        let document = &desired(&s).documents["city-air"].1;
        assert_eq!(
            document["views"],
            json!({ "b".repeat(26): ["city/kiosks#1", "city/kiosks#2"] })
        );
        let effect = |id: &str| {
            document["policies"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|p| p["id"] == id)
                .map(|p| p["effect"].clone())
        };
        assert_eq!(effect("city/kiosks#1"), Some(json!("permit")));
        assert_eq!(effect("city/kiosks#2"), Some(json!("prohibit")));
    }

    /// T-5339 (ADR-N-049 item 1): the fan-out subscribes as its caller since T-4755, so no view
    /// lists the gateway, a WebSocket Endpoint's included, and its account creates no subscription.
    #[test]
    fn no_view_lists_the_gateway_and_it_creates_no_subscription() {
        let mut s = sources(vec![
            ("cars", cars()),
            (
                "buses",
                policy(json!({"information": [{"entities": [{"type": "Bus"}]}]})),
            ),
        ]);
        let bound = |name: &str, c: char, representations: Value| {
            endpoint_named(
                name,
                json!({"slug": c.to_string().repeat(26),
                       "policyRef": "urn:ngsi-ld:Policy:city.example:city-air:buses",
                       "enabledRepresentations": representations}),
            )
        };
        s.endpoints = vec![
            bound("live", 'a', json!(["ngsi-ld", "websocket"])),
            bound("rest", 'b', json!(["ngsi-ld"])),
        ];
        let document = &desired(&s).documents["city-air"].1;
        assert_eq!(
            document["views"],
            json!({
                "a".repeat(26): ["city/buses"],
                "b".repeat(26): ["city/buses"],
            })
        );
        let gateway = document["policies"]
            .as_array()
            .and_then(|all| all.iter().find(|p| p["id"] == "@context-gateway"))
            .expect("the gateway's permit");
        assert_eq!(
            gateway["operations"],
            json!(["5.6.4", "5.6.5", "5.6.6", "5.7.1", "5.7.2", "5.7.5", "5.8.3"])
        );
    }

    /// T-5208 (EP-159): the broker serves a calendar only for the outputs the view lists, so the
    /// view carries the Endpoint's `representations.ical` as written, and nothing when the
    /// Endpoint lists none or does not serve `ical`.
    #[test]
    fn the_view_carries_the_ical_outputs_the_endpoint_lists() {
        let s = sources(vec![("cars", cars())]);
        let outputs = json!([{"name": "vsetky", "title": {"sk": "Všetky"}, "lang": "sk",
            "sources": [{"type": "Event", "preset": "sdm-event",
                         "mapping": {"LAST-MODIFIED": "dateModified"}}],
            "filter": {"q": "eventCategory==\"sport\""}}]);
        let with = |served: Value, ical: Option<&Value>| {
            let mut spec = json!({"policyRef": "urn:ngsi-ld:Policy:city.example:city-air:cars",
                                  "enabledRepresentations": served});
            if let Some(ical) = ical {
                spec["representations"] = json!({ "ical": ical });
            }
            endpoint_view(&s, &endpoint(spec)).unwrap_or_else(|why| panic!("a view: {why}"))
        };
        let view = with(json!(["ngsi-ld", "ical"]), Some(&outputs));
        assert_eq!(view["representations"], json!({ "ical": outputs }));
        assert!(with(json!(["ngsi-ld", "ical"]), None)
            .get("representations")
            .is_none());
        let legacy = with(
            json!(["ngsi-ld", "ical"]),
            Some(&json!({"preset": "schema-org"})),
        );
        assert_eq!(
            legacy["representations"],
            json!({"ical": [{"name": "events", "sources": [{"type": "Event", "preset": "schema-org"}]}]}),
            "the deprecated single object is the output `events`"
        );
    }

    /// T-3788: the space view is the `*` form, every type and Attribute; the engine narrows
    /// it per subject. Its operations are the space's grants.
    #[test]
    fn the_space_view_is_the_star_form() {
        let s = sources(vec![("cars", cars())]);
        let view = space_view(&s, "city", "air", "city-air").expect("a view");
        assert_eq!(
            view,
            json!({"id": "cs-city-air", "tenant": "city-air", "selector": ["*"],
                   "rules": [{"types": ["*"], "attributes": ["*"]}],
                   "operations": ["query_entities", "retrieve_attribute_details", "retrieve_attribute_info", "retrieve_attribute_value", "retrieve_attributes", "retrieve_entity", "retrieve_entity_type_details", "retrieve_entity_type_info", "retrieve_entity_types"]})
        );
        let long = "x".repeat(61);
        assert!(space_view(&s, "city", "air", &long).is_err());
        assert!(
            space_view(&sources(Vec::new()), "city", "air", "city-air").is_err(),
            "no grant, no operation"
        );
    }

    /// T-3767: a space with no Policy still admits the gateway's own hops, and only on the
    /// clauses they use; nothing else in the document names the gateway.
    #[test]
    fn every_rule_document_admits_the_gateway_on_its_own_hops_alone() {
        let desired = desired(&sources(Vec::new()));
        assert_eq!(
            desired.documents["city-air"].1,
            json!({"policies": [
                {"id": "@context-gateway", "effect": "permit",
                 "assignee": {"kind": "serviceAccount", "id": "context-gateway"},
                 "operations": ["5.6.4", "5.6.5", "5.6.6", "5.7.1", "5.7.2", "5.7.5", "5.8.3"]}
            ]})
        );
    }

    /// Plan 3.3: one document per tenant, every Policy with its clauses, effect, assignee,
    /// validity, information and conditions, in the engine's format.
    #[test]
    fn the_rule_document_carries_every_policy() {
        let s = sources(vec![
            ("cars", cars()),
            (
                "no-trucks",
                policy(
                    json!({"effect": "prohibition", "operations": ["updateOps", "updateAttrs"],
                              "assignee": {"kind": "group", "id": "drivers"},
                              "information": [{"entities": [{"type": "Truck",
                                                             "idPattern": "^urn:ngsi-ld:Truck:.*$"}]}],
                              "q": "weight>3", "scopeQ": "/city/#", "temporalQ": "timerel=after;timeAt=P-1D",
                              "validity": {"from": "2026-01-01T00:00:00Z"}}),
                ),
            ),
        ]);
        let desired = desired(&s);
        let (owner, document) = &desired.documents["city-air"];
        assert_eq!(owner, &Owner::space("city", "air"));
        assert_eq!(
            document,
            &json!({"policies": [
                {"id": "city/cars", "effect": "permit", "assignee": {"kind": "role", "id": "public"},
                 "operations": ["5.7.1", "5.7.2", "5.7.5", "5.7.7", "5.7.8", "5.7.10"],
                 "information": [{"entities": [{"type": "Car"}], "propertyNames": ["speed"]}]},
                {"id": "city/no-trucks", "effect": "prohibit", "assignee": {"kind": "group", "id": "drivers"},
                 "operations": ["5.6.2", "5.6.4", "5.6.18", "5.6.19"],
                 "validity": {"from": "2026-01-01T00:00:00Z"},
                 "information": [{"entities": [{"type": "Truck", "idPattern": "^urn:ngsi-ld:Truck:.*$"}]}],
                 "q": "weight>3", "scopeQ": "/city/#", "temporalQ": "timerel=after;timeAt=P-1D"},
                {"id": "@context-gateway", "effect": "permit",
                 "assignee": {"kind": "serviceAccount", "id": "context-gateway"},
                 "operations": ["5.6.4", "5.6.5", "5.6.6", "5.7.1", "5.7.2", "5.7.5", "5.8.3"]}
            ]})
        );
        assert!(desired.views.contains_key(SLUG) || desired.views.contains_key("cs-city-air"));
    }

    #[test]
    fn a_geo_query_without_its_members_is_refused() {
        for bad in [
            "georel=within;geometry=Point",
            "maxDistance==1;georel=near;geometry=Point;coordinates=[1,2]",
            "georel=within;geometry=Point;coordinates=[1,2];shape=x",
        ] {
            assert!(geo(bad).is_err(), "{bad}");
        }
    }

    /// T-3820: a replica that never converged (it does not lead) has no draft view until it
    /// remembers the repository its sync loaded; then it previews as the leader does, and writes
    /// nothing to the broker doing so.
    #[test]
    fn a_follower_that_remembers_the_repository_previews_a_draft() {
        let dir = std::env::temp_dir().join(format!(
            "portal-views-follower-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        ));
        let write = |rel: &str, body: &str| {
            let path = dir.join(rel);
            std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
            std::fs::write(path, body).expect("write");
        };
        let head = |kind: &str, name: &str| {
            format!("apiVersion: joinedcontext.com/v1alpha1\nkind: {kind}\nmetadata:\n  name: {name}\n  namespace: city\nspec:\n")
        };
        write(
            "projects/city/spaces/air/space.yaml",
            &format!("{}  isSandbox: false\n", head("ContextSpace", "air")),
        );
        write(
            "projects/city/spaces/air/endpoints/air-public.yaml",
            &format!("{}  contextSpaceRef: air\n  slug: {SLUG}\n  audience: public\n  enabledRepresentations: [ngsi-ld]\n", head("Endpoint", "air-public")),
        );
        write(
            "projects/city/spaces/air/policies/cars.yaml",
            &format!("{}  contextSpaceRef: air\n  assigner: did:web:city.example\n  assignee: {{ kind: role, id: public }}\n  operations: [retrieveOps]\n  information:\n    - entities: [{{ type: Car }}]\n      propertyNames: [speed]\n", head("Policy", "cars")),
        );
        let repo = Repository::load(&dir).expect("the repository loads");
        let _ = std::fs::remove_dir_all(&dir);
        // An unreachable broker and no token: remembering must not need either.
        let follower = ViewSync::new("http://127.0.0.1:1", dir.join("no-token"));
        let draft = Draft {
            projection: None,
            hidden_attributes: vec!["internalNote".into()],
        };
        assert_eq!(follower.draft_view("city", "air-public", &draft), Ok(None));

        follower.remember(&repo);
        let (id, view) = follower
            .draft_view("city", "air-public", &draft)
            .expect("the draft compiles")
            .expect("the remembered repository holds the Endpoint");
        assert_eq!(id, SLUG);
        assert_eq!(view["id"], json!(SLUG));
        assert_eq!(
            follower.draft_view("city", "nosuch", &draft),
            Ok(None),
            "an Endpoint the repository does not hold has no draft"
        );
        assert!(follower.written_documents.lock().expect("lock").is_empty());
    }

    /// The tenant is the space's rendered segment and the derived Policies are read, from a
    /// repository loaded the way the reconciler loads it.
    #[test]
    fn sources_read_a_loaded_repository() {
        let dir = std::env::temp_dir().join(format!(
            "portal-views-read-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        ));
        let write = |rel: &str, body: &str| {
            let path = dir.join(rel);
            std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
            std::fs::write(path, body).expect("write");
        };
        let head = |kind: &str, name: &str| {
            format!("apiVersion: joinedcontext.com/v1alpha1\nkind: {kind}\nmetadata:\n  name: {name}\n  namespace: city\nspec:\n")
        };
        write(
            "projects/city/spaces/air/space.yaml",
            &format!(
                "{}  isSandbox: false\n  urnSegment: luft\n",
                head("ContextSpace", "air")
            ),
        );
        write(
            "projects/city/spaces/air/endpoints/air-public.yaml",
            &format!("{}  contextSpaceRef: air\n  slug: {SLUG}\n  audience: public\n  enabledRepresentations: [ngsi-ld]\n", head("Endpoint", "air-public")),
        );
        write(
            "projects/city/spaces/air/policies/cars.yaml",
            &format!("{}  contextSpaceRef: air\n  assigner: did:web:city.example\n  assignee: {{ kind: role, id: public }}\n  operations: [retrieveOps]\n  information:\n    - entities: [{{ type: Car }}]\n      propertyNames: [speed]\n", head("Policy", "cars")),
        );
        let mapping = |name: &str, artifacts: &str| {
            format!("{}  contextSpaceRef: air\n  source: {{ kind: DataModel, name: cars, version: \"1\" }}\n  target: {{ kind: DataModel, name: vehicles, version: \"1\" }}\n  transformation: {{ id: https://city.example/m }}\n  tests: [{{ input: in.json, expect: out.json }}]\n{artifacts}", head("Mapping", name))
        };
        write(
            "projects/city/spaces/air/mappings/to-vehicle.yaml",
            &mapping(
                "to-vehicle",
                "  artifacts: { gatewayIr: ir/to-vehicle.json }\n",
            ),
        );
        write(
            "projects/city/spaces/air/mappings/ir/to-vehicle.json",
            &vehicles_ir().to_string(),
        );
        write(
            "projects/city/spaces/air/mappings/bare.yaml",
            &mapping("bare", ""),
        );
        write(
            "projects/city/spaces/air/mappings/escapes.yaml",
            &mapping("escapes", "  artifacts: { gatewayIr: ir/missing.json }\n"),
        );
        let repo = Repository::load(&dir).expect("the repository loads");
        let read = Sources::read(&repo);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(read.spaces, [("city".into(), "air".into(), "luft".into())]);
        assert_eq!(read.endpoints.len(), 1);
        assert_eq!(read.endpoints[0].tenant, "luft");
        let desired = desired(&read);
        assert_eq!(desired.views[SLUG].1["tenant"], json!("luft"));
        assert!(desired.views.contains_key("cs-luft"));
        assert_eq!(
            desired.documents["luft"].1["policies"][0]["id"],
            json!("city/cars")
        );
        let ir = |name: &str| {
            read.mappings[&("city".to_owned(), name.to_owned())]
                .ir
                .clone()
        };
        assert_eq!(ir("to-vehicle"), Ok(vehicles_ir()));
        assert!(ir("bare").is_err_and(|e| e.contains("has no spec.artifacts.gatewayIr")));
        assert!(ir("escapes").is_err_and(|e| e.contains("no JSON file")));
        assert_eq!(
            read.mappings[&("city".to_owned(), "bare".to_owned())].space,
            "air"
        );
    }

    #[test]
    fn a_pipelines_history_plan_reaches_its_spaces_retention_document() {
        let dir = std::env::temp_dir().join(format!(
            "portal-views-history-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        ));
        let write = |rel: &str, body: &str| {
            let path = dir.join(rel);
            std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
            std::fs::write(path, body).expect("write");
        };
        write(
            "projects/city/spaces/air/space.yaml",
            "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata: { name: air, namespace: city }\nspec:\n  isSandbox: false\n  urnSegment: luft\n  retention:\n    - { entityType: Vehicle, history: P30D, attributes: { delay: { record: every } } }\n",
        );
        write(
            "projects/city/spaces/air/endpoints/air-write.yaml",
            &format!("apiVersion: joinedcontext.com/v1alpha1\nkind: Endpoint\nmetadata: {{ name: air-write, namespace: city }}\nspec:\n  contextSpaceRef: air\n  slug: {SLUG}\n  audience: public\n  enabledRepresentations: [ngsi-ld]\n"),
        );
        write(
            "projects/city/pipelines/hfp.yaml",
            "apiVersion: joinedcontext.com/v1alpha2\nkind: Pipeline\nmetadata: { name: hfp, namespace: city }\nspec:\n  class: resident\n  sources:\n    - dataSourceRef: { kind: DataSource, name: feed }\n  outputs:\n    - targetEndpoint: urn:ngsi-ld:Endpoint:air-write\n  history:\n    - type: Vehicle\n      attributes:\n        - { names: [location], keep: changes, heartbeat: PT15M, observedAt: observedOn }\n        - { names: [speed], keep: latest }\n        - { names: [delay], keep: changes, heartbeat: PT1H }\n",
        );
        let repo = Repository::load(&dir).expect("the repository loads");
        let read = Sources::read(&repo);
        let _ = std::fs::remove_dir_all(&dir);
        let rules = &read.retention[&("city".to_owned(), "air".to_owned())];
        let attributes = &rules[0].attributes;
        assert_eq!(attributes["location"].record.as_deref(), Some("onChange"));
        assert_eq!(attributes["location"].heartbeat.as_deref(), Some("PT15M"));
        assert!(
            !attributes.contains_key("speed"),
            "latest writes no recording"
        );
        assert_eq!(
            attributes["delay"].record.as_deref(),
            Some("every"),
            "the space's own rule wins"
        );
        let document = retention_document(rules, None);
        let location = document["rules"][0]["attributes"]
            .as_object()
            .and_then(|a| a.iter().find(|(iri, _)| iri.ends_with("/location")))
            .map(|(_, recording)| recording["record"].clone());
        assert_eq!(location, Some(json!("onChange")), "{document}");
    }

    const TOKEN: &str = "an-admin-token-of-at-least-32-bytes!";

    fn token_file() -> PathBuf {
        let file = std::env::temp_dir().join(format!(
            "portal-views-token-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        ));
        std::fs::write(&file, format!("{TOKEN}\n")).expect("write the token");
        file
    }

    async fn broker() -> MockServer {
        MockServer::start().await
    }

    fn json_at(server_path: &str, status: u16, body: Value) -> Mock {
        Mock::given(method("GET"))
            .and(path(server_path))
            .respond_with(ResponseTemplate::new(status).set_body_json(body))
    }

    /// The writer against the broker's `/q`: a new view is written, an unchanged one is not
    /// sent again, a changed one is replaced, an owned one that is gone is deleted, a view
    /// written by hand is left alone, a refused Endpoint is reported without a call, a broker
    /// 400 lands in the outcome with its detail, the rule document is written, and every call
    /// carries the token, which no outcome repeats.
    #[tokio::test]
    async fn the_writer_converges_the_brokers_views_and_documents() {
        let server = broker().await;
        let gone = "zzzzzzzzzzzzzzzzzzzzzzzzzz";
        let s = {
            let mut s = sources(vec![("cars", cars())]);
            s.endpoints.push(endpoint(json!({})));
            s.endpoints.push(endpoint_named(
                "mapped",
                json!({"slug": "m3mqavzt5xkc2nhw7brj4gd6sy",
                       "viewMappingRef": {"kind": "Mapping", "name": "m"}}),
            ));
            s.endpoints.push(endpoint_named(
                "refused-by-broker",
                json!({"slug": "p3mqavzt5xkc2nhw7brj4gd6sy"}),
            ));
            s
        };
        let desired = desired(&s);
        let space_view = desired.views["cs-city-air"].1.clone();
        let document = desired.documents["city-air"].1.clone();

        json_at(
            "/q/views",
            200,
            json!({"cs-city-air": {"state": "loaded"}, gone: {"state": "loaded"},
                   "my-view": {"state": "loaded"}}),
        )
        .mount(&server)
        .await;
        json_at(&format!("/q/views/{SLUG}"), 404, json!({}))
            .mount(&server)
            .await;
        json_at("/q/views/cs-city-air", 200, space_view)
            .mount(&server)
            .await;
        json_at(
            "/q/views/p3mqavzt5xkc2nhw7brj4gd6sy",
            200,
            json!({"id": "old"}),
        )
        .mount(&server)
        .await;
        json_at("/q/policy", 200, json!({"loaded": [], "refused": []}))
            .mount(&server)
            .await;
        json_at("/q/policy/city-air", 404, json!({}))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path(format!("/q/views/{SLUG}")))
            .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
            .respond_with(ResponseTemplate::new(201))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/q/views/cs-city-air"))
            .respond_with(ResponseTemplate::new(204))
            .expect(0)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/q/views/p3mqavzt5xkc2nhw7brj4gd6sy"))
            .respond_with(ResponseTemplate::new(400).set_body_json(
                json!({"type": "https://uri.etsi.org/ngsi-ld/errors/BadRequestData",
                       "detail": "rules[0].types: \"Car\" is wrong"}),
            ))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path(format!("/q/views/{gone}")))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/q/views/my-view"))
            .respond_with(ResponseTemplate::new(204))
            .expect(0)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/q/policy/city-air"))
            .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
            .and(body_json(&document))
            .respond_with(ResponseTemplate::new(201))
            .expect(1)
            .mount(&server)
            .await;

        let file = token_file();
        let sync = ViewSync::new(server.uri(), &file);
        let outcomes: BTreeMap<String, ViewOutcome> = sync
            .apply(desired, Pass::Whole)
            .await
            .into_iter()
            .map(|(owner, outcome)| (owner.name, outcome))
            .collect();
        let _ = std::fs::remove_file(&file);

        assert_eq!(outcomes["air-public"], ViewOutcome::Written);
        assert_eq!(outcomes["air"], ViewOutcome::Written);
        assert!(
            matches!(&outcomes["mapped"], ViewOutcome::Refused(r) if r.contains("viewMappingRef")),
            "{outcomes:?}"
        );
        assert!(
            matches!(&outcomes["refused-by-broker"],
                     ViewOutcome::Failed(r) if r.contains("400") && r.contains("is wrong")),
            "{outcomes:?}"
        );
        assert!(!format!("{outcomes:?}").contains(TOKEN));
        assert!(sync.written().contains_key("city-air"));
    }

    /// T-5804: a broker pass over views the broker already holds sends no write and reads them
    /// several at once, so a slow broker costs a run one read's wait per batch, never per view.
    #[tokio::test]
    async fn unchanged_views_are_read_several_at_once_and_never_written() {
        let server = broker().await;
        let view = json!({"id": "v", "rules": []});
        let mut desired = Desired::default();
        for n in 0..24 {
            let owner = Owner {
                kind: "Endpoint",
                project: "city".into(),
                name: format!("e{n}"),
            };
            desired
                .views
                .insert(format!("v{n:02}"), (owner, view.clone()));
        }
        json_at("/q/views", 200, json!({})).mount(&server).await;
        json_at("/q/policy", 404, json!({})).mount(&server).await;
        Mock::given(method("GET"))
            .and(wiremock::matchers::path_regex("^/q/views/v[0-9]+$"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(&view)
                    .set_delay(Duration::from_millis(400)),
            )
            .expect(24)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .respond_with(ResponseTemplate::new(204))
            .expect(0)
            .mount(&server)
            .await;

        let file = token_file();
        let sync = ViewSync::new(server.uri(), &file);
        let started = std::time::Instant::now();
        let outcomes = sync.apply(desired, Pass::Whole).await;
        let elapsed = started.elapsed();
        let _ = std::fs::remove_file(&file);

        assert_eq!(outcomes.len(), 24);
        assert!(
            outcomes.iter().all(|(_, o)| *o == ViewOutcome::Written),
            "{outcomes:?}"
        );
        // One after the other the 24 reads take 9.6 s; WRITES_AT_ONCE of them at once, 1.2 s.
        assert!(elapsed < Duration::from_millis(4800), "{elapsed:?}");
    }

    /// PF-111: a space on broker `canary` is written to the canary's address and nothing of it
    /// reaches `main`; a space whose brokerRef names no broker is refused and written nowhere.
    #[tokio::test]
    async fn each_space_is_written_on_its_own_broker_and_never_on_main() {
        let main = MockServer::start().await;
        let canary = MockServer::start().await;
        // `main` is listed, so what it held is cleaned up, and is told nothing.
        Mock::given(method("GET"))
            .and(path("/q/views"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&main)
            .await;
        Mock::given(method("PUT"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&main)
            .await;
        Mock::given(method("DELETE"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&main)
            .await;
        for listing in ["/q/views", "/q/policy"] {
            Mock::given(method("GET"))
                .and(path(listing))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
                .mount(&canary)
                .await;
        }
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({})))
            .mount(&canary)
            .await;
        Mock::given(method("PUT"))
            .and(path("/q/policy/city-air"))
            .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
            .respond_with(ResponseTemplate::new(201))
            .expect(1)
            .mount(&canary)
            .await;
        Mock::given(method("PUT"))
            .respond_with(ResponseTemplate::new(201))
            .mount(&canary)
            .await;

        let canary_host = canary.uri().replace("127.0.0.1", "{broker}");
        let mut on_canary = sources(vec![("cars", cars())]);
        on_canary
            .brokers
            .insert("city-air".into(), "127.0.0.1".into());
        let file = token_file();
        let sync = ViewSync::new(main.uri(), &file).with_broker_template(Some(canary_host));
        let written = sync.apply(desired(&on_canary), Pass::Whole).await;

        let mut nowhere = sources(vec![("cars", cars())]);
        nowhere.brokers.insert("city-air".into(), String::new());
        let refused = ViewSync::new(main.uri(), &file)
            .apply(desired(&nowhere), Pass::Whole)
            .await;
        let _ = std::fs::remove_file(&file);

        assert!(
            written
                .iter()
                .all(|(_, outcome)| *outcome == ViewOutcome::Written),
            "{written:?}"
        );
        assert_eq!(sync.written().get("city-air"), Some(&canary.uri()));
        assert!(
            refused.iter().all(
                |(_, outcome)| matches!(outcome, ViewOutcome::Failed(r) if r.contains("brokerRef"))
            ),
            "{refused:?}"
        );
        assert!(!refused.is_empty());
    }

    /// A rule document whose space is gone is deleted; one the broker already holds is not
    /// sent again; a broker without a document-reading engine gets views only.
    #[tokio::test]
    async fn rule_documents_follow_their_spaces() {
        let server = broker().await;
        let s = sources(vec![("cars", cars())]);
        let document = desired(&s).documents["city-air"].1.clone();
        json_at("/q/views", 200, json!({})).mount(&server).await;
        json_at(
            "/q/views/cs-city-air",
            200,
            desired(&s).views["cs-city-air"].1.clone(),
        )
        .mount(&server)
        .await;
        json_at("/q/policy", 200, json!({})).mount(&server).await;
        json_at(
            "/q/policy/city-air",
            200,
            json!({"tenant": "city-air", "revision": 3, "document": document}),
        )
        .mount(&server)
        .await;
        Mock::given(method("PUT"))
            .respond_with(ResponseTemplate::new(204))
            .expect(0)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/q/policy/old-space"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        let file = token_file();
        let sync = ViewSync::new(server.uri(), &file);
        sync.written().insert("old-space".into(), server.uri());
        let outcomes = sync.apply(desired(&s), Pass::Whole).await;
        assert_eq!(
            outcomes,
            [(Owner::space("city", "air"), ViewOutcome::Written)]
        );
        assert!(!sync.written().contains_key("old-space"));

        let no_engine = broker().await;
        json_at("/q/views", 200, json!({})).mount(&no_engine).await;
        json_at("/q/views/cs-city-air", 404, json!({}))
            .mount(&no_engine)
            .await;
        Mock::given(method("PUT"))
            .and(path("/q/views/cs-city-air"))
            .respond_with(ResponseTemplate::new(201))
            .expect(1)
            .mount(&no_engine)
            .await;
        Mock::given(method("PUT"))
            .and(path("/q/policy/city-air"))
            .respond_with(ResponseTemplate::new(201))
            .expect(0)
            .mount(&no_engine)
            .await;
        let outcomes = ViewSync::new(no_engine.uri(), &file)
            .apply(desired(&s), Pass::Whole)
            .await;
        let _ = std::fs::remove_file(&file);
        assert_eq!(
            outcomes,
            [(Owner::space("city", "air"), ViewOutcome::Written)]
        );
    }

    /// Without a readable token nothing is sent and every object says why, naming the file and
    /// never a token.
    #[tokio::test]
    async fn without_the_token_nothing_is_sent() {
        let server = broker().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        let sync = ViewSync::new(server.uri(), "/nonexistent/broker-admin-token");
        let outcomes = sync
            .apply(desired(&sources(vec![("cars", cars())])), Pass::Whole)
            .await;
        assert!(
            matches!(&outcomes[..], [(_, ViewOutcome::Failed(r))] if r.contains("/nonexistent/broker-admin-token")),
            "{outcomes:?}"
        );
    }

    /// Two Endpoints with one slug, or two spaces rendering one tenant: the second is refused
    /// and the first keeps its view and document.
    #[test]
    fn a_second_owner_of_a_slug_or_tenant_is_refused() {
        let mut s = sources(vec![("cars", cars())]);
        s.endpoints.push(endpoint(json!({})));
        s.endpoints.push(endpoint_named("copy", json!({})));
        s.spaces
            .push(("other".into(), "air".into(), "city-air".into()));
        let d = desired(&s);
        assert_eq!(d.views[SLUG].0.name, "air-public");
        assert_eq!(d.documents["city-air"].0.project, "city");
        let refused: Vec<(&str, &str)> = d
            .refused
            .iter()
            .map(|(owner, reason)| (owner.name.as_str(), reason.as_str()))
            .collect();
        assert!(
            refused
                .iter()
                .any(|(n, r)| *n == "copy" && r.contains("same slug")),
            "{refused:?}"
        );
        assert!(
            refused
                .iter()
                .any(|(n, r)| *n == "air" && r.contains("same tenant")),
            "{refused:?}"
        );
    }

    /// An empty repository wants nothing: every owned view goes, a hand-made one stays, and
    /// there is no outcome to report.
    #[tokio::test]
    async fn an_empty_repository_removes_every_owned_view() {
        let server = broker().await;
        json_at(
            "/q/views",
            200,
            json!({SLUG: {}, "cs-city-air": {}, "my-view": {}}),
        )
        .mount(&server)
        .await;
        json_at("/q/policy", 404, json!({})).mount(&server).await;
        for (id, times) in [(SLUG, 1), ("cs-city-air", 1), ("my-view", 0)] {
            Mock::given(method("DELETE"))
                .and(path(format!("/q/views/{id}")))
                .respond_with(ResponseTemplate::new(204))
                .expect(times)
                .mount(&server)
                .await;
        }
        let file = token_file();
        let outcomes = ViewSync::new(server.uri(), &file)
            .apply(desired(&Sources::default()), Pass::Whole)
            .await;
        let _ = std::fs::remove_file(&file);
        assert!(outcomes.is_empty(), "{outcomes:?}");
    }

    /// A broker that does not answer, or one without the views feature, fails every object
    /// with the reason and writes nothing.
    #[tokio::test]
    async fn a_broker_that_cannot_take_views_fails_every_object() {
        let file = token_file();
        let s = sources(vec![("cars", cars())]);
        let down = ViewSync::new("http://127.0.0.1:9", &file)
            .apply(desired(&s), Pass::Whole)
            .await;
        assert!(
            matches!(&down[..], [(_, ViewOutcome::Failed(r))] if r.contains("did not answer")),
            "{down:?}"
        );
        let vanilla = broker().await;
        json_at("/q/views", 404, json!({})).mount(&vanilla).await;
        Mock::given(method("PUT"))
            .respond_with(ResponseTemplate::new(201))
            .expect(0)
            .mount(&vanilla)
            .await;
        let outcomes = ViewSync::new(vanilla.uri(), &file)
            .apply(desired(&s), Pass::Whole)
            .await;
        let _ = std::fs::remove_file(&file);
        assert!(
            matches!(&outcomes[..], [(_, ViewOutcome::Failed(r))] if r.contains("views feature")),
            "{outcomes:?}"
        );
    }

    fn air_model() -> ModelSource {
        ModelSource {
            model: "air-quality".into(),
            source: "id: https://city.example/air\nname: air\nimports: [linkml:types, org.stations.v1]\n".into(),
            imports: BTreeMap::from([(
                "org.stations.v1".to_owned(),
                "id: https://city.example/stations\nname: stations\n".to_owned(),
            )]),
        }
    }

    fn with_model(mut s: Sources) -> Sources {
        s.models
            .insert(("city".into(), "air".into()), Ok(air_model()));
        s
    }

    /// T-3788 through the writer: a type a Policy grants whole reaches the broker as its
    /// class's slots, never as `"attributes": []`.
    #[tokio::test]
    async fn the_writer_puts_a_whole_type_as_its_classes_slots() {
        let mut s = fleet(sources(vec![(
            "stops",
            policy(json!({"information": [{"entities": [{"type": "Stop"}]}]})),
        )]));
        s.endpoints.push(endpoint(json!({})));
        let tools = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/generate"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"brokerSchema": {"classes": {}}, "errors": []})),
            )
            .mount(&tools)
            .await;
        let server = broker_with_the_space_view(&s).await;
        json_at("/q/schemas/city-air", 404, json!({}))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/q/schemas/city-air"))
            .respond_with(ResponseTemplate::new(201))
            .mount(&server)
            .await;
        json_at(&format!("/q/views/{SLUG}"), 404, json!({}))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path(format!("/q/views/{SLUG}")))
            .and(body_json(json!({
                "id": SLUG, "tenant": "city-air", "selector": ["Stop"],
                "rules": [{"types": ["Stop"], "attributes": ["location", "name"]}],
                "operations": ["query_entities", "retrieve_attribute_details", "retrieve_attribute_info", "retrieve_attribute_value", "retrieve_attributes", "retrieve_entity", "retrieve_entity_type_details", "retrieve_entity_type_info", "retrieve_entity_types"]
            })))
            .respond_with(ResponseTemplate::new(201))
            .expect(1)
            .mount(&server)
            .await;

        let file = token_file();
        let sync = ViewSync::new(server.uri(), &file).with_model_tools(format!("{}/", tools.uri()));
        let outcomes = sync.apply(desired(&s), Pass::Whole).await;
        let _ = std::fs::remove_file(&file);
        let endpoint = outcomes
            .iter()
            .find(|(owner, _)| owner.kind == "Endpoint")
            .map(|(_, outcome)| outcome.clone());
        assert_eq!(endpoint, Some(ViewOutcome::Written), "{outcomes:?}");
    }

    /// The `brokerSchema` Model Tools answers for `air_model()`, as `POST /generate` takes it.
    async fn model_tools(answer: Value, calls: u64) -> MockServer {
        let tools = MockServer::start().await;
        let model = air_model();
        Mock::given(method("POST"))
            .and(path("/generate"))
            .and(body_json(
                json!({"source": model.source, "imports": model.imports}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(answer))
            .expect(calls)
            .mount(&tools)
            .await;
        tools
    }

    /// A broker that holds the space's view and runs no rule-document engine, so a run is
    /// views and schemas only.
    async fn broker_with_the_space_view(s: &Sources) -> MockServer {
        let server = broker().await;
        json_at("/q/views", 200, json!({})).mount(&server).await;
        json_at(
            "/q/views/cs-city-air",
            200,
            desired(s).views["cs-city-air"].1.clone(),
        )
        .mount(&server)
        .await;
        json_at("/q/policy", 404, json!({})).mount(&server).await;
        server
    }

    /// The space's model and its import are read from the checkout, an import at its pinned
    /// major and published; a draft import, another major and a source outside the checkout
    /// are the space's reason, not a read.
    #[test]
    fn a_spaces_model_and_its_imports_are_read_from_the_checkout() {
        let dir = std::env::temp_dir().join(format!(
            "portal-views-model-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        ));
        let write = |rel: &str, body: &str| {
            let path = dir.join(rel);
            std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
            std::fs::write(path, body).expect("write");
        };
        let manifest = |namespace: &str, space: &str, name: &str| {
            jc_core::registry::by_kind("DataModel")
                .expect("DataModel is a kind")
                .repo_path(namespace, space, name)
        };
        let model = |namespace: &str, name: &str, extra: &str| {
            format!("apiVersion: joinedcontext.com/v1alpha1\nkind: DataModel\nmetadata:\n  name: {name}\n  namespace: {namespace}\nspec:\n  linkml: ./{name}.linkml.yaml\n{extra}")
        };
        write(
            "projects/city/spaces/air/space.yaml",
            "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: air\n  namespace: city\nspec:\n  isSandbox: false\n  dataModelRef: air-quality\n",
        );
        let air = manifest("city", "air", "air-quality");
        write(
            &air,
            &model(
                "city",
                "air-quality",
                "  contextSpaceRef: air\n  version: 1.0.0\n  lifecycle: draft\n",
            ),
        );
        let air_source =
            "id: https://city.example/air\nname: air\nimports: [linkml:types, org.stations.v1]\n";
        let folder = air
            .rsplit_once('/')
            .map(|(f, _)| f.to_owned())
            .unwrap_or_default();
        write(&format!("{folder}/air-quality.linkml.yaml"), air_source);
        let stations = manifest(ORG_NAMESPACE, "", "stations");
        let org_folder = stations
            .rsplit_once('/')
            .map(|(f, _)| f.to_owned())
            .unwrap_or_default();
        let stations_source = "id: https://city.example/stations\nname: stations\n";
        write(
            &format!("{org_folder}/stations.linkml.yaml"),
            stations_source,
        );
        let read = |stations_spec: &str| {
            write(&stations, &model(ORG_NAMESPACE, "stations", stations_spec));
            let repo = Repository::load(&dir).expect("the repository loads");
            Sources::read(&repo).models[&("city".to_owned(), "air".to_owned())].clone()
        };

        let draft = read("  version: 1.2.0\n  lifecycle: draft\n");
        let next_major = read("  version: 2.0.0\n  lifecycle: published\n");
        let published = read("  version: 1.2.0\n  lifecycle: published\n");
        // The loader refuses a symlink out of the checkout; one that appears after the load is
        // refused when the source is read.
        let repo = Repository::load(&dir).expect("the repository loads");
        let outside = dir.with_extension("outside.linkml.yaml");
        std::fs::write(&outside, "id: https://elsewhere\n").expect("write");
        let _ = std::fs::remove_file(dir.join(format!("{org_folder}/stations.linkml.yaml")));
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            &outside,
            dir.join(format!("{org_folder}/stations.linkml.yaml")),
        )
        .expect("symlink");
        let escaped = Sources::read(&repo).models[&("city".to_owned(), "air".to_owned())].clone();
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&outside);

        assert_eq!(
            published,
            Ok(ModelSource {
                model: "air-quality".into(),
                source: air_source.into(),
                imports: BTreeMap::from([("org.stations.v1".into(), stations_source.into())]),
            })
        );
        assert!(
            matches!(&draft, Err(r) if r.contains("org.stations.v1") && r.contains("only a published model")),
            "{draft:?}"
        );
        assert!(
            matches!(&next_major, Err(r) if r.contains("pins major 1")),
            "{next_major:?}"
        );
        #[cfg(unix)]
        assert!(
            matches!(&escaped, Err(r) if r.contains("leaves the repository")),
            "{escaped:?}"
        );
        let refused = desired(&Sources {
            models: BTreeMap::from([(("city".into(), "air".into()), draft)]),
            ..sources(vec![("cars", cars())])
        })
        .refused;
        assert!(
            matches!(&refused[..], [(owner, r)] if owner.name == "air" && r.starts_with("its data model is not written to the broker: import 'org.stations.v1'")),
            "{refused:?}"
        );
    }

    /// A model's digest changes with its name, its source or any import, and with nothing else.
    #[test]
    fn a_models_digest_tells_every_change() {
        let model = air_model();
        assert_eq!(model.digest(), air_model().digest());
        let mut renamed = air_model();
        renamed.model.push('x');
        let mut edited = air_model();
        edited.source.push(' ');
        let mut imported = air_model();
        imported.imports.insert("org.x.v1".into(), String::new());
        let mut moved = air_model();
        // The same bytes split differently between name and source are another model.
        moved.source = format!("{}{}", moved.model.pop().unwrap_or('a'), moved.source);
        for other in [renamed, edited, imported, moved] {
            assert_ne!(model.digest(), other.digest(), "{other:?}");
        }
    }

    /// The space's model is compiled once and written as its tenant's schema; an unchanged one
    /// is neither compiled nor sent again; a space that loses its model loses the schema.
    #[tokio::test]
    async fn a_spaces_schema_is_written_once_and_removed_with_its_model() {
        let s = with_model(sources(vec![("cars", cars())]));
        let schema: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/broker-schema-senzor.json"
        ))
        .expect("the fixture is JSON");
        let tools = model_tools(json!({"brokerSchema": schema, "errors": []}), 1).await;
        let server = broker_with_the_space_view(&s).await;
        Mock::given(method("GET"))
            .and(path("/q/schemas/city-air"))
            .respond_with(ResponseTemplate::new(404))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        json_at("/q/schemas/city-air", 200, schema.clone())
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/q/schemas/city-air"))
            .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
            .and(body_json(&schema))
            .respond_with(ResponseTemplate::new(201))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/q/schemas/city-air"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        let file = token_file();
        let sync = ViewSync::new(server.uri(), &file).with_model_tools(format!("{}/", tools.uri()));
        let first = sync.apply(desired(&s), Pass::Whole).await;
        let second = sync.apply(desired(&s), Pass::Whole).await;
        let held = lock(&sync.written_schemas).clone();
        let compiled = lock(&sync.compiled).keys().cloned().collect::<Vec<_>>();
        let third = sync
            .apply(desired(&sources(vec![("cars", cars())])), Pass::Whole)
            .await;
        let _ = std::fs::remove_file(&file);
        // The compiled schema leaves with the model, after one run (T-4353).
        assert_eq!(compiled, ["city-air"]);
        assert!(lock(&sync.compiled).is_empty());

        for outcomes in [&first, &second, &third] {
            assert!(
                matches!(&outcomes[..], [(owner, ViewOutcome::Written)] if owner.name == "air"),
                "{outcomes:?}"
            );
        }
        assert_eq!(held.keys().collect::<Vec<_>>(), ["city-air"]);
        assert!(lock(&sync.written_schemas).is_empty());
    }

    /// DM-82: a space's retention rules reach the broker under the core context's IRI and the
    /// model's; a space whose rules are gone has its document removed, which keeps forever.
    #[tokio::test]
    async fn a_spaces_retention_is_written_and_removed_with_its_rules() {
        let mut s = with_model(sources(vec![("cars", cars())]));
        s.retention.insert(
            ("city".into(), "air".into()),
            vec![RetentionRule {
                entity_type: "Vehicle".into(),
                current: Some("PT5M".into()),
                history: None,
                reason: None,
                ..Default::default()
            }],
        );
        let context = json!({"@context": {"sdm": "https://smartdatamodels.org/",
                                          "Vehicle": {"@id": "sdm:Vehicle"}}});
        let tools = model_tools(
            json!({"brokerSchema": {"classes": {}}, "context": context, "errors": []}),
            1,
        )
        .await;
        let server = broker_with_the_space_view(&s).await;
        Mock::given(method("PUT"))
            .and(path("/q/schemas/city-air"))
            .respond_with(ResponseTemplate::new(201))
            .mount(&server)
            .await;
        let document = json!({"rules": [
            {"type": "https://uri.etsi.org/ngsi-ld/default-context/Vehicle", "current": "PT5M"},
            {"type": "https://smartdatamodels.org/Vehicle", "current": "PT5M"}]});
        Mock::given(method("PUT"))
            .and(path("/q/retention/city-air"))
            .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
            .and(body_json(&document))
            .respond_with(ResponseTemplate::new(201))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/q/retention/city-air"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        let file = token_file();
        let sync = ViewSync::new(server.uri(), &file).with_model_tools(tools.uri());
        let first = sync.apply(desired(&s), Pass::Whole).await;
        s.retention
            .insert(("city".into(), "air".into()), Vec::new());
        let second = sync.apply(desired(&s), Pass::Whole).await;
        let _ = std::fs::remove_file(&file);
        for outcomes in [&first, &second] {
            assert!(
                matches!(&outcomes[..], [(owner, ViewOutcome::Written)] if owner.name == "air"),
                "{outcomes:?}"
            );
        }
    }

    /// DM-85: the broker counts the document the reconciler would write, and its IRIs come back
    /// as the classes the rules name, both IRIs of one class summed; a type no rule names keeps
    /// its last segment. A broker refusal is the caller's error, with its detail.
    #[tokio::test]
    async fn retention_counts_fold_the_brokers_iris_back_into_classes() {
        let server = broker().await;
        let core = "https://uri.etsi.org/ngsi-ld/default-context/Bus";
        let rules = [RetentionRule {
            entity_type: "Bus".into(),
            current: None,
            history: Some("PT8H".into()),
            reason: None,
            ..Default::default()
        }];
        Mock::given(method("POST"))
            .and(path("/q/retention/city-air/preview"))
            .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
            .and(body_json(
                json!({"rules": [{"type": core, "history": "PT8H"}, {"type": "https://e.org/Bus", "history": "PT8H"}]}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"counts": [
                {"type": core, "kind": "history", "count": 3}]})))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/q/retention/city-air/deleted"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"counts": [
                {"type": core, "kind": "history", "count": 2},
                {"type": "https://e.org/Bus", "kind": "history", "count": 5},
                {"type": "https://e.org/v#Stop", "kind": "entity", "count": 1}]})))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/q/retention/refused/preview"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({"detail": "type twice"})))
            .mount(&server)
            .await;
        // A broker that repeats the request's credential in its refusal (T-4772): the admin
        // token never reaches a status or an API error.
        Mock::given(method("POST"))
            .and(path("/q/retention/echoed/preview"))
            .respond_with(ResponseTemplate::new(401).set_body_json(
                json!({"detail": format!("Authorization: Bearer {TOKEN} is not valid here")}),
            ))
            .mount(&server)
            .await;
        let file = token_file();
        let sync = ViewSync::new(server.uri(), &file);
        lock(&sync.compiled).insert(
            "city-air".into(),
            (
                air_model().digest(),
                json!({}),
                Some(json!({"@context": {"Bus": "https://e.org/Bus"}})),
            ),
        );
        let count = |entity_type: &str, kind: &str, count: u64| RetentionCount {
            entity_type: entity_type.into(),
            kind: kind.into(),
            count,
        };
        let preview = sync.retention_counts("city-air", Some(&rules), &[]).await;
        let deleted = sync.retention_counts("city-air", None, &rules).await;
        let refused = sync.retention_counts("refused", Some(&rules), &[]).await;
        let echoed = sync.retention_counts("echoed", Some(&rules), &[]).await;
        let _ = std::fs::remove_file(&file);
        let unreadable = ViewSync::new(server.uri(), "/nonexistent/token")
            .retention_counts("city-air", None, &rules)
            .await;
        assert_eq!(preview, Ok(vec![count("Bus", "history", 3)]));
        assert_eq!(
            deleted,
            Ok(vec![count("Bus", "history", 7), count("Stop", "entity", 1)])
        );
        assert!(
            refused.as_ref().is_err_and(|e| e.contains("type twice")),
            "{refused:?}"
        );
        assert!(unreadable.is_err());
        assert!(
            echoed
                .as_ref()
                .is_err_and(|e| !e.contains(TOKEN) && e.contains("[redacted]")),
            "{echoed:?}"
        );
    }

    /// DM-91: a space's `maxAge` rules reach the broker's sweep under the core context's IRIs
    /// and the model's, each name with its type and attribute; a range rule and a `maxAhead` one
    /// stay on the write path; a space whose rules are gone has its document removed.
    #[tokio::test]
    async fn a_spaces_max_age_rules_are_written_and_removed_with_them() {
        let mut s = with_model(sources(vec![("cars", cars())]));
        let rule = |value: Value| -> QualityRule {
            serde_json::from_value(value).expect("a quality rule")
        };
        s.quality.insert(
            ("city".into(), "air".into()),
            vec![
                rule(json!({"name": "stale-position", "entityType": "Vehicle",
                            "kind": "freshness", "attribute": "location", "maxAge": "PT10M"})),
                rule(
                    json!({"name": "ahead", "entityType": "Vehicle", "kind": "freshness",
                            "attribute": "location", "maxAhead": "PT1M"}),
                ),
                rule(
                    json!({"name": "speed", "entityType": "Vehicle", "kind": "range",
                            "attribute": "speed", "max": 200}),
                ),
            ],
        );
        let context = json!({"@context": {"sdm": "https://smartdatamodels.org/",
                                          "Vehicle": {"@id": "sdm:Vehicle"}}});
        let tools = model_tools(
            json!({"brokerSchema": {"classes": {}}, "context": context, "errors": []}),
            1,
        )
        .await;
        let server = broker_with_the_space_view(&s).await;
        Mock::given(method("PUT"))
            .and(path("/q/schemas/city-air"))
            .respond_with(ResponseTemplate::new(201))
            .mount(&server)
            .await;
        let core = "https://uri.etsi.org/ngsi-ld/default-context/";
        let document = json!({"rules": [
            {"name": "stale-position", "type": format!("{core}Vehicle"),
             "attribute": format!("{core}location"), "maxAge": "PT10M"},
            {"name": "stale-position", "type": "https://smartdatamodels.org/Vehicle",
             "attribute": format!("{core}location"), "maxAge": "PT10M"}]});
        Mock::given(method("PUT"))
            .and(path("/q/quality/city-air"))
            .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
            .and(body_json(&document))
            .respond_with(ResponseTemplate::new(201))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/q/quality/city-air"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/q/retention/city-air"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let file = token_file();
        let sync = ViewSync::new(server.uri(), &file).with_model_tools(tools.uri());
        let first = sync.apply(desired(&s), Pass::Whole).await;
        s.quality.insert(("city".into(), "air".into()), Vec::new());
        let second = sync.apply(desired(&s), Pass::Whole).await;
        let _ = std::fs::remove_file(&file);
        for outcomes in [&first, &second] {
            assert!(
                outcomes
                    .iter()
                    .all(|(owner, outcome)| owner.name == "air" && *outcome == ViewOutcome::Written),
                "{outcomes:?}"
            );
        }
    }

    /// What keeps a schema out of the broker is the space's condition: a model that does not
    /// compile (nothing sent), a broker with no schema bound (its 503 and detail), no Model
    /// Tools configured.
    #[tokio::test]
    async fn a_schema_that_is_not_written_says_why_on_the_space() {
        let s = with_model(sources(vec![("cars", cars())]));
        let file = token_file();

        let broken = model_tools(
            json!({"errors": ["slot 'pm10': range 'Foo' is unknown"]}),
            1,
        )
        .await;
        let server = broker_with_the_space_view(&s).await;
        Mock::given(method("PUT"))
            .and(path("/q/schemas/city-air"))
            .respond_with(ResponseTemplate::new(201))
            .expect(0)
            .mount(&server)
            .await;
        let refused = ViewSync::new(server.uri(), &file)
            .with_model_tools(broken.uri())
            .apply(desired(&s), Pass::Whole)
            .await;

        let tools = model_tools(
            json!({"brokerSchema": {"id": "https://city.example/air", "name": "air"}}),
            1,
        )
        .await;
        let unbound = broker_with_the_space_view(&s).await;
        json_at("/q/schemas/city-air", 404, json!({}))
            .mount(&unbound)
            .await;
        Mock::given(method("PUT"))
            .and(path("/q/schemas/city-air"))
            .respond_with(ResponseTemplate::new(503).set_body_json(json!({
                "type": "https://uri.etsi.org/ngsi-ld/errors/OperationNotSupported",
                "detail": "ANTARES_SCHEMA_MAX_BYTES is unset"
            })))
            .expect(1)
            .mount(&unbound)
            .await;
        let failed = ViewSync::new(unbound.uri(), &file)
            .with_model_tools(tools.uri())
            .apply(desired(&s), Pass::Whole)
            .await;

        let unconfigured = ViewSync::new(broker_with_the_space_view(&s).await.uri(), &file)
            .apply(desired(&s), Pass::Whole)
            .await;
        let _ = std::fs::remove_file(&file);

        assert!(
            matches!(&refused[..], [(_, ViewOutcome::Refused(r))]
                if r.contains("air-quality does not compile") && r.contains("range 'Foo'")),
            "{refused:?}"
        );
        assert!(
            matches!(&failed[..], [(_, ViewOutcome::Failed(r))]
                if r.contains("PUT /q/schemas/city-air") && r.contains("503") && r.contains("ANTARES_SCHEMA_MAX_BYTES")),
            "{failed:?}"
        );
        assert!(
            matches!(&unconfigured[..], [(_, ViewOutcome::Failed(r))] if r.contains("JC_PORTAL_MODEL_TOOLS_URL")),
            "{unconfigured:?}"
        );
    }

    /// The broker this test writes to: the `-jc` image ci-full starts (views, linkml,
    /// `ANTARES_POLICY=joinedcontext`, `ANTARES_SCHEMA_MAX_BYTES`) and the file holding its
    /// admin token. ci-full's `test-all`, the job that starts it, without them is a failure,
    /// never a pass; elsewhere the test returns, other ci-full jobs (the README's commands) too.
    fn jc_broker() -> Option<(String, PathBuf)> {
        let url = std::env::var("JC_PORTAL_TEST_BROKER_URL").ok();
        let token = std::env::var("JC_PORTAL_TEST_BROKER_TOKEN_FILE").ok();
        match (url, token) {
            (Some(url), Some(token)) if !url.trim().is_empty() && !token.trim().is_empty() => {
                Some((
                    url.trim().trim_end_matches('/').to_owned(),
                    PathBuf::from(token),
                ))
            }
            _ => {
                assert!(
                    !(std::env::var("GITHUB_WORKFLOW").as_deref() == Ok("ci-full")
                        && std::env::var("GITHUB_JOB").as_deref() == Ok("test-all")),
                    "ci-full test-all starts the -jc broker and must set \
                     JC_PORTAL_TEST_BROKER_URL and JC_PORTAL_TEST_BROKER_TOKEN_FILE"
                );
                eprintln!(
                    "JC_PORTAL_TEST_BROKER_URL is not set: the -jc broker test runs in ci-full"
                );
                None
            }
        }
    }

    /// What [`the_writer_converges_the_jc_broker`] creates: the cars Policy, the Endpoint and a
    /// mapped Endpoint no view serves.
    fn jc_created() -> Sources {
        let mut s = with_model(sources(vec![("cars", cars())]));
        s.endpoints.push(endpoint(json!({})));
        s.endpoints.push(endpoint_named(
            "mapped",
            json!({"slug": "m3mqavzt5xkc2nhw7brj4gd6sy",
                   "viewMappingRef": {"kind": "Mapping", "name": "m"}}),
        ));
        s
    }

    /// What it changes to: a second Policy over `Bike`, a type the model has no class for, so
    /// it names its Attribute (T-3788 refuses a whole type without a class).
    fn jc_changed() -> Sources {
        let mut s = with_model(sources(vec![
            ("cars", cars()),
            (
                "bikes",
                policy(
                    json!({"information": [{"entities": [{"type": "Bike"}], "propertyNames": ["wheels"]}]}),
                ),
            ),
        ]));
        s.endpoints.push(endpoint(json!({})));
        s
    }

    /// The `-jc` broker test runs in ci-full alone (T-3824): its sources are checked here on
    /// every run, so a refusal they meet is red before that lane.
    #[test]
    fn the_jc_broker_tests_sources_write_the_endpoints_view() {
        for (step, s) in [("create", jc_created()), ("change", jc_changed())] {
            let wanted = desired(&s);
            assert!(
                wanted.views.contains_key(SLUG),
                "{step}: {:?}",
                wanted.refused
            );
            assert!(
                wanted
                    .refused
                    .iter()
                    .all(|(owner, _)| owner.name == "mapped"),
                "{step}: {:?}",
                wanted.refused
            );
        }
        assert_ne!(
            desired(&jc_changed()).views[SLUG].1,
            desired(&jc_created()).views[SLUG].1,
            "the change widens the view"
        );
    }

    /// The writer against the real `-jc` broker: create, change and delete; a view written by
    /// hand is left alone; a refused Endpoint is reported and never sent; the rule document
    /// and the schema are what the broker then holds. The schema is what Model Tools' `gen_broker_schema.py`
    /// renders from the platform's `senzor.linkml.yaml` fixture.
    #[tokio::test]
    async fn the_writer_converges_the_jc_broker() {
        let Some((broker, token_file)) = jc_broker() else {
            return;
        };
        let token = std::fs::read_to_string(&token_file).expect("the token file reads");
        let token = token.trim();
        let http = reqwest::Client::new();
        let get = |path: &str| {
            let request = http.get(format!("{broker}{path}")).bearer_auth(token);
            async move {
                let response = request.send().await.expect("the broker answers");
                let status = response.status().as_u16();
                (
                    status,
                    response.json::<Value>().await.unwrap_or(Value::Null),
                )
            }
        };
        let hand_made = json!({"id": "my-view", "tenant": "city-air", "selector": ["Car"],
                               "rules": [{"types": ["Car"], "attributes": []}],
                               "operations": ["retrieve_entity"]});
        let put = http
            .put(format!("{broker}/q/views/my-view"))
            .bearer_auth(token)
            .json(&hand_made)
            .send()
            .await
            .expect("the broker answers");
        assert!(
            put.status().is_success(),
            "the hand-made view: {}",
            put.status()
        );

        let schema: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/broker-schema-senzor.json"
        ))
        .expect("the fixture is JSON");
        let tools = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/generate"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"brokerSchema": schema, "errors": []})),
            )
            .mount(&tools)
            .await;
        let sync = ViewSync::new(broker.clone(), &token_file).with_model_tools(tools.uri());
        let s = jc_created();

        // Create.
        let made = desired(&s);
        let outcomes: BTreeMap<String, ViewOutcome> = sync
            .apply(desired(&s), Pass::Whole)
            .await
            .into_iter()
            .map(|(owner, outcome)| (owner.name, outcome))
            .collect();
        assert_eq!(outcomes["air-public"], ViewOutcome::Written, "{outcomes:?}");
        assert_eq!(outcomes["air"], ViewOutcome::Written, "{outcomes:?}");
        assert!(
            matches!(&outcomes["mapped"], ViewOutcome::Refused(r) if r.contains("viewMappingRef"))
        );
        let (_, listed) = get("/q/views").await;
        for id in [SLUG, "cs-city-air", "my-view"] {
            assert!(listed.get(id).is_some(), "{id} is not listed: {listed}");
        }
        assert!(
            listed.get("m3mqavzt5xkc2nhw7brj4gd6sy").is_none(),
            "{listed}"
        );
        assert_eq!(
            get(&format!("/q/views/{SLUG}")).await,
            (200, made.views[SLUG].1.clone())
        );
        let (_, document) = get("/q/policy/city-air").await;
        assert_eq!(document["document"], made.documents["city-air"].1);
        assert_eq!(get("/q/schemas/city-air").await, (200, schema.clone()));
        let (_, health) = get("/q/health").await;
        assert!(
            health["schemas"]["city-air"]["bytes"]
                .as_u64()
                .is_some_and(|b| b > 0),
            "{health}"
        );

        // Change: a second Policy widens the Endpoint's view and the rule document.
        let changed = jc_changed();
        let wanted = desired(&changed);
        assert_ne!(wanted.views[SLUG].1, made.views[SLUG].1);
        let outcomes = sync.apply(desired(&changed), Pass::Whole).await;
        assert!(
            outcomes.iter().all(|(_, o)| *o == ViewOutcome::Written),
            "{outcomes:?}"
        );
        assert_eq!(
            get(&format!("/q/views/{SLUG}")).await,
            (200, wanted.views[SLUG].1.clone())
        );
        let (_, document) = get("/q/policy/city-air").await;
        assert_eq!(document["document"], wanted.documents["city-air"].1);

        // Delete: nothing declared, so every owned view, the document and the schema go.
        let outcomes = sync.apply(Desired::default(), Pass::Whole).await;
        assert!(outcomes.is_empty(), "{outcomes:?}");
        let (_, listed) = get("/q/views").await;
        assert!(
            listed.get(SLUG).is_none() && listed.get("cs-city-air").is_none(),
            "{listed}"
        );
        assert!(
            listed.get("my-view").is_some(),
            "the hand-made view was removed: {listed}"
        );
        assert_eq!(get("/q/policy/city-air").await.0, 404);
        assert_eq!(get("/q/schemas/city-air").await.0, 404);
    }

    /// F-SEC1-003 (T-5860, PL-92): the engine compares `x-jc-client`, which the gateway sends
    /// as `{project}/{name}`, with the assignee as written, so a bare serviceAccount assignee is
    /// rendered as its Policy's own project's account; a qualified one and other kinds stay.
    #[test]
    fn a_bare_service_account_assignee_is_rendered_as_its_own_projects_account() {
        let to = |kind: &str, id: &str| policy(json!({"assignee": {"kind": kind, "id": id}}));
        let s = sources(vec![
            ("own", to("serviceAccount", "pl-buses")),
            ("shared", to("serviceAccount", "fleet/pl-buses")),
            ("readers", to("role", "reader")),
        ]);
        let doc = rule_document(&s, "city", "air");
        let assignee = |id: &str| {
            doc["policies"]
                .as_array()
                .and_then(|all| all.iter().find(|p| p["id"] == id))
                .map(|p| p["assignee"]["id"].clone())
        };
        assert_eq!(assignee("city/own"), Some(json!("city/pl-buses")), "{doc}");
        assert_eq!(assignee("city/shared"), Some(json!("fleet/pl-buses")));
        assert_eq!(assignee("city/readers"), Some(json!("reader")));
    }

    /// DM-90: a space that can quarantine narrows every permit but the owner's and the
    /// steward's to entities without `quarantinedAt`, its own `q` kept; a prohibition and a
    /// space without such a rule are left as they were.
    #[test]
    fn a_quarantining_space_hides_quarantined_entities_from_all_but_owner_and_steward() {
        let steward = policy(json!({"assignee": {"kind": "role", "id": "steward"}}));
        let narrowed = policy(json!({"q": "speed>3|speed<1"}));
        let deny = policy(json!({"effect": "prohibition"}));
        let mut s = sources(vec![
            ("public", policy(json!({}))),
            ("narrowed", narrowed),
            ("stewards", steward),
            ("deny", deny),
        ]);
        let before = rule_document(&s, "city", "air");
        assert!(
            !before.to_string().contains("quarantinedAt"),
            "no rule, no narrowing"
        );
        s.quarantines.insert(("city".into(), "air".into()));
        let doc = rule_document(&s, "city", "air");
        let q = |id: &str| {
            doc["policies"]
                .as_array()
                .and_then(|all| all.iter().find(|p| p["id"] == id))
                .map(|p| p["q"].clone())
                .expect(id)
        };
        assert_eq!(q("city/public"), json!("!quarantinedAt"));
        assert_eq!(
            q("city/narrowed"),
            json!("!quarantinedAt;(speed>3|speed<1)")
        );
        assert_eq!(q("city/stewards"), Value::Null, "the steward acts on it");
        assert_eq!(q("city/deny"), Value::Null, "a prohibition never narrows");
        let gateway = with_gateway(doc.clone());
        assert!(gateway["policies"].as_array().is_some_and(|all| all
            .iter()
            .any(|p| p["id"] == "@context-gateway" && p.get("q").is_none())));
    }
}
