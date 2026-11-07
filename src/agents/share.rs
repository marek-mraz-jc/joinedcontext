//! The assistant drafts a publication (EP-72, API/01 §19): from "share the bike stations with
//! the transport team, hide the maintenance notes" to a rendered `Endpoint` manifest with a slug
//! the Portal mints, the draft `Policy` manifests for the audience, and the lane the Change
//! would take. Nothing here writes: the person submits the form the rendering prefills.

use argon2::password_hash::rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::LazyLock;

use crate::change::{self, Lane, Operation};
use crate::resource::{is_dns1123, API_VERSION};
use jc_core::kinds::ical_mapping::LEGACY_OUTPUT;
use jc_core::kinds::{Preset as IcalPreset, Representation, PRESET_TYPES};

/// The representations a share may name: every jc-core `Representation` but `feed`, which an
/// Endpoint serves from its outputs and never lists (EP-150). The retired files are not among
/// them (T-4600).
pub fn known_representations() -> Vec<&'static str> {
    Representation::ALL
        .iter()
        .filter(|r| **r != Representation::Feed)
        .map(Representation::as_str)
        .collect()
}
const AUDIENCES: [&str; 3] = ["project-list", "organization", "public"];
/// Requests a minute a caller may set, for a share and for an edit alike: one field, one bound.
pub const MAX_REQUESTS_PER_MINUTE: u32 = 100_000;
const SLUG_ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

/// What the tool takes: the model's or a caller's reading of the request. Unknown fields are
/// ignored and absent ones default, because a model answer is not a form.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProposeEndpoint {
    #[serde(default)]
    pub context_space: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub audience: Option<String>,
    #[serde(default)]
    pub allowed_projects: Vec<String>,
    #[serde(default)]
    pub representations: Vec<String>,
    #[serde(default)]
    pub hidden_attributes: Vec<String>,
    #[serde(default)]
    pub entity_types: Vec<String>,
    #[serde(default)]
    pub rate_limits: Option<RateLimits>,
    /// The access preset of an endpoint Build an app proposes inline (AP-132); absent for a share.
    #[serde(default)]
    pub access: Option<Preset>,
    /// The properties a public form asks for: its Policy's `propertyNames` (T-3172).
    #[serde(default)]
    pub write_attributes: Vec<String>,
    /// The relationships a public form asks for: its Policy's `relationshipNames` (T-3172).
    #[serde(default)]
    pub write_relationships: Vec<String>,
    /// A public form's creates per UTC day: its Endpoint's `spec.creates.perDay` (EP-97).
    #[serde(default)]
    pub creates_per_day: Option<u32>,
    /// The sites that may frame a public form: its Endpoint's `spec.creates.embedOrigins`
    /// (EP-101).
    #[serde(default)]
    pub embed_origins: Vec<String>,
}

/// The creates a public form takes a day when its request names no count (API/01 §19).
pub const DEFAULT_CREATES_PER_DAY: u32 = 200;

/// What an app may do through a new endpoint (AP-132). A read reaches the person's own grants on
/// the space and adds none; a write preset grants its operations, which takes the red lane.
/// `Create` is a public form's (T-3103, API/01 §33): anyone may create one entity of one type and
/// do nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Preset {
    Read,
    Update,
    Full,
    Create,
}

impl Preset {
    /// The operations the preset names, reads first, as AP-132 lists them.
    pub fn operations(self) -> &'static [&'static str] {
        const FULL: [&str; 8] = [
            "queryEntity",
            "retrieveEntity",
            "queryTemporal",
            "retrieveTemporal",
            "updateAttrs",
            "appendAttrs",
            "createEntity",
            "deleteEntity",
        ];
        match self {
            Preset::Read => &FULL[..4],
            Preset::Update => &FULL[..6],
            Preset::Full => &FULL,
            Preset::Create => &["createEntity"],
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimits {
    pub requests_per_minute: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub burst: Option<u32>,
}

/// The rendering: the manifests, the lane, and the form values the endpoint page takes.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Proposal {
    pub lane: Lane,
    pub slug: String,
    pub endpoint: Value,
    pub policies: Vec<Value>,
    /// Placeholder `Group` manifests for the consumer projects that have none yet (T-1042).
    ///
    /// A Policy naming a group the repository does not declare fails validation, so the share
    /// would be refused for a reason the person cannot act on. The group is drafted empty: who
    /// belongs to it is the consumer's own administrators' to say, on their Access page (PF-62).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<Value>,
    pub prefill: Value,
}

/// A fresh slug: 26 base32 characters, 130 bits, minted here and nowhere else (EP-02).
pub fn slug() -> String {
    let mut bytes = [0u8; 26];
    OsRng.fill_bytes(&mut bytes);
    // 256 is not a multiple of 32: masking to five bits keeps every character equally likely.
    bytes
        .iter()
        .map(|b| SLUG_ALPHABET[(b & 31) as usize] as char)
        .collect()
}

fn is_identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == ':')
}

/// The list without its repeats, in the order the person wrote it. A project or a
/// representation dictated twice is one assignee and one representation, not two manifests
/// with one name (EP-72).
fn without_repeats(items: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut kept = items;
    kept.retain(|item| seen.insert(item.clone()));
    kept
}

/// What a value is, for a sentence about a manifest the repository holds.
/// `spec.representations.ical` for an Endpoint that lists `ical`: one calendar over the types
/// a preset maps, since a calendar is served only for a listed output (EP-159). No type named
/// means `Event`, as an App's calendar; another type writes its own mapping in the form.
fn calendar(entity_types: &[String]) -> Result<Value, String> {
    let mapped = PRESET_TYPES
        .iter()
        .filter(|t| entity_types.iter().any(|named| named == *t));
    let mut types: Vec<&str> = mapped.copied().collect();
    if entity_types.is_empty() {
        types.push("Event");
    }
    if types.is_empty() {
        return Err(format!(
            "`ical` has a preset for {} and not for {}: draft the endpoint without ical and map \
             its calendar in the endpoint form",
            PRESET_TYPES.join(", "),
            entity_types.join(", ")
        ));
    }
    let sources: Vec<Value> = types
        .into_iter()
        .map(|t| json!({ "type": t, "preset": IcalPreset::SdmEvent.as_str() }))
        .collect();
    Ok(json!([{ "name": LEGACY_OUTPUT, "sources": sources }]))
}

fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "empty",
        Value::Bool(_) => "true or false",
        Value::Number(_) => "a number",
        Value::String(_) => "text",
        Value::Array(_) => "a list",
        Value::Object(_) => "an object",
    }
}

/// The shapes `edit` writes into, checked once before the first write.
///
/// The mirror holds whatever YAML the project's repository committed and `ResourceEnvelope::spec`
/// is a bare `Value`, so `spec: oops` reaches this module intact; `serde_json` panics when a
/// write indexes a value that is neither an object nor absent. Anyone who may commit to the
/// repository could otherwise crash the assistant's request (PF-57).
fn readable_for_edit(endpoint: &Value, name: &str) -> Result<(), String> {
    let fix = "fix the manifest in the project's repository, then edit it from the chat again";
    let Some(spec) = endpoint.get("spec") else {
        return Err(format!(
            "the manifest of endpoint '{name}' has no spec: {fix}"
        ));
    };
    let Some(spec) = spec.as_object() else {
        return Err(format!(
            "the spec of endpoint '{name}' is {}, not an object: {fix}",
            json_kind(spec)
        ));
    };
    for field in ["projection", "rateLimits"] {
        if let Some(value) = spec.get(field) {
            if !value.is_object() {
                return Err(format!(
                    "spec.{field} of endpoint '{name}' is {}, not an object: {fix}",
                    json_kind(value)
                ));
            }
        }
    }
    Ok(())
}

/// Refuses an `entityTypes` entry the space's data model has no class for, naming the class and
/// the classes there are (T-6530). A space whose model lists no classes yet is not judged here.
pub fn check_types(params: &ProposeEndpoint, classes: &[String]) -> Result<(), String> {
    if classes.is_empty() {
        return Ok(());
    }
    match params
        .entity_types
        .iter()
        .find(|t| !classes.iter().any(|c| c == t.trim()))
    {
        Some(missing) => Err(format!(
            "entityTypes entry '{missing}' is no class of the data model of space '{}'; its \
             classes are {}",
            params.context_space.trim(),
            classes.join(", ")
        )),
        None => Ok(()),
    }
}

/// The manifests for `params` in `project`, or the first reason they cannot be rendered.
pub fn render(
    project: &str,
    params: &ProposeEndpoint,
    known_groups: &[String],
) -> Result<Proposal, String> {
    let name = params.name.trim();
    let space = params.context_space.trim();
    if !is_dns1123(name) {
        return Err(format!("name '{name}' is not a DNS-1123 label"));
    }
    if !is_dns1123(space) {
        return Err(format!("contextSpace '{space}' is not a DNS-1123 label"));
    }
    let audience = params
        .audience
        .as_deref()
        .map(str::trim)
        .unwrap_or("project-list");
    // An app's own endpoint: this project reads it, and nobody else (AP-132).
    let own_project = [project.to_owned()];
    let allowed = if params.access.is_some() && params.allowed_projects.is_empty() {
        &own_project[..]
    } else {
        &params.allowed_projects[..]
    };
    if !AUDIENCES.contains(&audience) {
        return Err(format!(
            "audience '{audience}' is not one of {}",
            AUDIENCES.join(", ")
        ));
    }
    let projects = without_repeats(
        allowed
            .iter()
            .map(|p| p.trim().to_owned())
            .filter(|p| !p.is_empty())
            .collect(),
    );
    if let Some(bad) = projects.iter().find(|p| !is_dns1123(p)) {
        return Err(format!(
            "allowedProjects entry '{bad}' is not a DNS-1123 label"
        ));
    }
    if audience == "project-list" && projects.is_empty() {
        return Err(
            "audience project-list needs at least one project in allowedProjects".to_owned(),
        );
    }
    if params.access == Some(Preset::Create) {
        // A public form: anyone creates one entity of one type, so the endpoint is public and
        // names the type it takes (T-3103).
        if audience != "public" || params.entity_types.len() != 1 {
            return Err(
                "a public form (access create) is a public endpoint over exactly one entity type: \
                 audience public and one entry in entityTypes"
                    .to_owned(),
            );
        }
        // An anonymous caller sets the form's fields and nothing else of the type (T-3172).
        if params.write_attributes.is_empty() && params.write_relationships.is_empty() {
            return Err(
                "a public form names the fields it asks for in writeAttributes or \
                 writeRelationships, the only attributes an anonymous caller may set"
                    .to_owned(),
            );
        }
        if let Some(per_day) = params.creates_per_day {
            if per_day == 0 || per_day > jc_core::kinds::endpoint::MAX_CREATES_PER_DAY {
                return Err(format!(
                    "createsPerDay {per_day} is outside 1..={}",
                    jc_core::kinds::endpoint::MAX_CREATES_PER_DAY
                ));
            }
        }
        // The Endpoint's own rule, so the request and the manifest cannot disagree (EP-101).
        jc_core::kinds::Creates {
            mint_ids: true,
            per_day: None,
            embed_origins: params.embed_origins.clone(),
        }
        .validate()
        .map_err(|e| format!("embedOrigins: {e}"))?;
    } else if !params.write_attributes.is_empty()
        || !params.write_relationships.is_empty()
        || params.creates_per_day.is_some()
        || !params.embed_origins.is_empty()
    {
        return Err(
            "writeAttributes, writeRelationships, createsPerDay and embedOrigins are for a \
             public form (access create)"
                .to_owned(),
        );
    }
    if let Some(preset) = params.access.filter(|preset| *preset != Preset::Create) {
        if audience != "project-list" || projects != own_project {
            return Err(format!(
                "an access preset is for an endpoint this project's apps read: audience must be \
                 project-list with allowedProjects ['{project}'] alone"
            ));
        }
        if preset != Preset::Read && params.entity_types.is_empty() {
            return Err(
                "a write preset needs entityTypes: the types the app may change".to_owned(),
            );
        }
    }
    let representations: Vec<String> = if params.representations.is_empty() {
        vec!["ngsi-ld".to_owned(), "csv".to_owned()]
    } else {
        without_repeats(
            params
                .representations
                .iter()
                // A person asking for GeoJSON means what `ogc-features` answers (T-4597).
                .map(|r| match r.trim() {
                    "geojson" => Representation::OgcFeatures.as_str().to_owned(),
                    name => name.to_owned(),
                })
                .collect(),
        )
    };
    let known = known_representations();
    if let Some(bad) = representations
        .iter()
        .find(|r| !known.contains(&r.as_str()))
    {
        return Err(format!(
            "representation '{bad}' is not one of {}",
            known.join(", ")
        ));
    }
    if let Some(bad) = params.hidden_attributes.iter().find(|a| !is_identifier(a)) {
        return Err(format!(
            "hiddenAttributes entry '{bad}' is not an attribute name"
        ));
    }
    if let Some(bad) = params
        .write_attributes
        .iter()
        .chain(&params.write_relationships)
        .find(|a| !is_identifier(a))
    {
        return Err(format!("form field '{bad}' is not an attribute name"));
    }
    if let Some(bad) = params.entity_types.iter().find(|t| !is_identifier(t)) {
        return Err(format!("entityTypes entry '{bad}' is not a type name"));
    }
    if let Some(limits) = &params.rate_limits {
        let per_minute = limits.requests_per_minute;
        if per_minute == 0 || per_minute > MAX_REQUESTS_PER_MINUTE {
            return Err(format!(
                "rateLimits.requestsPerMinute {per_minute} is outside 1..={MAX_REQUESTS_PER_MINUTE}"
            ));
        }
    }

    let ical = Representation::Ical.as_str();
    let calendar = representations
        .iter()
        .any(|r| r == ical)
        .then(|| calendar(&params.entity_types))
        .transpose()?;

    let slug = slug();
    let mut spec = json!({
        "contextSpaceRef": space,
        "slug": slug,
        "audience": audience,
        "enabledRepresentations": representations,
    });
    if audience == "project-list" {
        spec["allowedProjects"] = json!(projects);
    }
    if let Some(calendar) = calendar {
        spec["representations"] = json!({ "ical": calendar });
    }
    if !params.hidden_attributes.is_empty() {
        spec["projection"] = json!({ "hiddenAttributes": params.hidden_attributes });
    }
    if let Some(limits) = &params.rate_limits {
        spec["rateLimits"] = serde_json::to_value(limits).unwrap_or(Value::Null);
    }
    // A public form's caller chooses no id and has a daily count (EP-97, T-3177).
    if params.access == Some(Preset::Create) {
        spec["creates"] = json!({
            "mintIds": true,
            "perDay": params.creates_per_day.unwrap_or(DEFAULT_CREATES_PER_DAY),
        });
        if !params.embed_origins.is_empty() {
            spec["creates"]["embedOrigins"] = json!(params.embed_origins);
        }
    }
    let mut metadata = json!({
        "name": name,
        "namespace": project,
        "labels": { "joinedcontext.com/space": space },
    });
    if let Some(title) = params
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        metadata["title"] = json!({ "en": title });
    }
    let endpoint = json!({
        "apiVersion": API_VERSION,
        "kind": "Endpoint",
        "metadata": metadata,
        "spec": spec,
    });
    let lane = change::classify("Endpoint", Operation::Create, &endpoint["spec"]);

    // One draft Policy per assignee: the audience decides who, the request decides what.
    let assignees: Vec<(String, Value)> = match audience {
        "public" => vec![(
            "public".to_owned(),
            json!({ "kind": "role", "id": "public" }),
        )],
        // The organization's domain names no group (T-6464). A person of the project's root
        // group carries no `jc_org` (T-6408), so that group gets its own grant.
        "organization" => vec![
            ("organization".to_owned(), json!({ "kind": "organization" })),
            (
                "members".to_owned(),
                json!({ "kind": "group", "id": project }),
            ),
        ],
        _ => projects
            .iter()
            .map(|p| (p.clone(), json!({ "kind": "group", "id": p })))
            .collect(),
    };
    // Two labels do not make a third: `name` and a project may each be 63 characters, and the
    // Policy is named after both. Refused here with both halves and their lengths, rather than
    // by manifest validation with a name the person never typed (T-1042, EP-72).
    if let Some((suffix, _)) = assignees
        .iter()
        .find(|(suffix, _)| !is_dns1123(&format!("{name}-{suffix}")))
    {
        return Err(format!(
            "the Policy for '{suffix}' would be named '{name}-{suffix}', {total} characters, and \
             a name is at most 63. The endpoint name is {own} characters and '{suffix}' is \
             {theirs}: shorten the endpoint name and share again.",
            total = name.len() + 1 + suffix.len(),
            own = name.len(),
            theirs = suffix.len(),
        ));
    }
    let entities: Vec<Value> = params
        .entity_types
        .iter()
        .map(|t| json!({ "type": t }))
        .collect();
    let mut information = json!({ "entities": entities });
    if params.access == Some(Preset::Create) {
        let properties = without_repeats(params.write_attributes.clone());
        let relationships = without_repeats(params.write_relationships.clone());
        if !properties.is_empty() {
            information["propertyNames"] = json!(properties);
        }
        if !relationships.is_empty() {
            information["relationshipNames"] = json!(relationships);
        }
    }
    // A read preset grants nothing: the person reads through the endpoint what they already
    // read in the space. A write preset grants exactly its operations (AP-132).
    let operations = match params.access {
        None => json!(["retrieveOps"]),
        Some(Preset::Read) => Value::Null,
        Some(preset) => json!(preset.operations()),
    };
    let policies: Vec<Value> = if operations.is_null() {
        Vec::new()
    } else {
        assignees
            .into_iter()
            .map(|(suffix, assignee)| {
                json!({
                    "apiVersion": API_VERSION,
                    "kind": "Policy",
                    "metadata": { "name": format!("{name}-{suffix}"), "namespace": project },
                    "spec": {
                        "contextSpaceRef": { "kind": "ContextSpace", "name": space },
                        // Rendered by the loader, so a copied organization is its own assigner (CC-82).
                        "assigner": "did:web:{orgDomain}",
                        "assignee": assignee,
                        "operations": operations,
                        "information": [information],
                    },
                })
            })
            .collect()
    };

    // A Policy that names a group the repository does not declare does not validate, and the
    // share is refused for something the person cannot fix from the chat. The missing groups
    // are drafted empty beside it (T-1042, PF-62).
    let groups: Vec<Value> = if audience == "project-list" && !policies.is_empty() {
        projects
            .iter()
            .filter(|p| !known_groups.iter().any(|known| known == *p))
            .map(|p| {
                json!({
                    "apiVersion": API_VERSION,
                    "kind": "Group",
                    "metadata": { "name": p, "namespace": project },
                    "spec": { "members": [] },
                })
            })
            .collect()
    } else {
        Vec::new()
    };

    let listed: Vec<String> = if audience == "project-list" {
        projects.clone()
    } else {
        Vec::new()
    };
    let mut prefill = json!({
        "name": name,
        "contextSpaceRef": space,
        "slug": slug,
        "audience": audience,
        "allowedProjects": listed,
        "enabledRepresentations": representations,
        "hiddenAttributes": params.hidden_attributes,
        "entityTypes": params.entity_types,
    });
    if let Some(title) = endpoint["metadata"].get("title") {
        // Flattened, the same way `form_values` does it below (T-1221): a manifest carries the
        // title as a language map and the form's `title` is one string. Handing the map over
        // crashed the whole Portal to its error boundary — `s?.trim is not a function`, on
        // camera during the share take — because the page reads the draft's title as text.
        prefill["title"] = json!(crate::resource::title_text(title).unwrap_or_default());
    }
    if let Some(limits) = &params.rate_limits {
        prefill["rateLimits"] = serde_json::to_value(limits).unwrap_or(Value::Null);
    }

    // Proposed together, the Change takes the lane of its highest manifest: a Policy is red.
    let lane = match (params.access, policies.first()) {
        (Some(_), Some(policy)) => change::classify("Policy", Operation::Create, &policy["spec"]),
        _ => lane,
    };

    Ok(Proposal {
        lane,
        slug,
        endpoint,
        policies,
        groups,
        prefill,
    })
}

pub(crate) static TOOL_FENCE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?s)```(?:json)?\s*(\{.*?\})\s*```").expect("a literal pattern")
});

/// The tool call in a model answer, when the answer is one: a fenced JSON object whose `tool`
/// is `propose_endpoint`. Anything else is an ordinary answer.
pub fn tool_call(answer: &str) -> Option<Result<ProposeEndpoint, String>> {
    for fence in TOOL_FENCE.captures_iter(answer) {
        let Ok(value) = serde_json::from_str::<Value>(&fence[1]) else {
            continue;
        };
        if value.get("tool").and_then(Value::as_str) != Some("propose_endpoint") {
            continue;
        }
        return Some(
            serde_json::from_value::<ProposeEndpoint>(value)
                .map_err(|err| format!("the propose_endpoint call does not parse: {err}")),
        );
    }
    None
}

/// The prose of a tool answer: what stands before the fence, for the chat.
pub fn prose_of(answer: &str) -> String {
    TOOL_FENCE.replace_all(answer, "").trim().to_owned()
}

/// What `edit_endpoint` takes: the name of an endpoint that exists and only the fields that
/// change. Absent fields keep what the manifest says; unknown fields are ignored, as for a share.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditEndpoint {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub audience: Option<String>,
    #[serde(default)]
    pub allowed_projects: Option<Vec<String>>,
    #[serde(default)]
    pub add_representations: Vec<String>,
    #[serde(default)]
    pub remove_representations: Vec<String>,
    /// The whole list after the change; an empty list shows every attribute again.
    #[serde(default)]
    pub hidden_attributes: Option<Vec<String>>,
    #[serde(default)]
    pub requests_per_minute: Option<u32>,
}

/// One field the edit changes, as the chat and the tool step show it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FieldChange {
    pub field: String,
    pub before: Value,
    pub after: Value,
}

/// An edited endpoint: the manifest with the change and what changed.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Edit {
    pub endpoint: Value,
    pub changes: Vec<FieldChange>,
}

/// `params` applied to the endpoint of that name among `endpoints` (the project's manifests), or
/// the first reason it cannot be. Only the fields the call names change: the slug, the space,
/// the name and the labels stay what they were, so the Change is an edit of that endpoint.
/// `served` names the types an endpoint serves: its manifest lists none, the space's model does.
pub fn edit(
    endpoints: &[Value],
    params: &EditEndpoint,
    served: impl Fn(&Value) -> Vec<String>,
) -> Result<Edit, String> {
    let name = params.name.trim();
    let Some(found) = endpoints
        .iter()
        .find(|endpoint| endpoint["metadata"]["name"].as_str() == Some(name))
    else {
        let mut names: Vec<&str> = endpoints
            .iter()
            .filter_map(|endpoint| endpoint["metadata"]["name"].as_str())
            .collect();
        names.sort_unstable();
        return Err(if names.is_empty() {
            format!("there is no endpoint '{name}': this project has no endpoints")
        } else {
            format!(
                "there is no endpoint '{name}' in this project; its endpoints are {}",
                names.join(", ")
            )
        });
    };
    readable_for_edit(found, name)?;
    let served = served(found);
    let mut endpoint = found.clone();
    if let Some(object) = endpoint.as_object_mut() {
        object.remove("status");
    }
    let mut changes = Vec::new();
    let mut record = |field: &str, before: Value, after: Value| {
        if before != after {
            changes.push(FieldChange {
                field: field.to_owned(),
                before,
                after,
            });
        }
    };

    if let Some(title) = params
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        let before = endpoint["metadata"]
            .get("title")
            .map(|t| json!(crate::resource::title_text(t).unwrap_or_default()))
            .unwrap_or(Value::Null);
        endpoint["metadata"]["title"] = json!(title);
        record("title", before, json!(title));
    }

    let spec = &mut endpoint["spec"];
    let current_audience = spec["audience"]
        .as_str()
        .unwrap_or("project-list")
        .to_owned();
    let audience = match params.audience.as_deref().map(str::trim) {
        Some(audience) if !AUDIENCES.contains(&audience) => {
            return Err(format!(
                "audience '{audience}' is not one of {}",
                AUDIENCES.join(", ")
            ));
        }
        Some(audience) => audience.to_owned(),
        None => current_audience.clone(),
    };
    let current_projects = spec
        .get("allowedProjects")
        .cloned()
        .unwrap_or_else(|| json!([]));
    if let Some(projects) = &params.allowed_projects {
        if audience != "project-list" {
            return Err(format!(
                "'{name}' is {audience}: allowedProjects only narrows a project-list audience; \
                 set audience to project-list with the projects"
            ));
        }
        if let Some(bad) = projects.iter().find(|p| !is_dns1123(p.trim())) {
            return Err(format!(
                "allowedProjects entry '{bad}' is not a DNS-1123 label"
            ));
        }
    }
    let projects = without_repeats(match &params.allowed_projects {
        Some(projects) => projects.iter().map(|p| p.trim().to_owned()).collect(),
        None => serde_json::from_value(current_projects.clone()).unwrap_or_default(),
    });
    if audience == "project-list" {
        if projects.is_empty() {
            return Err(
                "audience project-list needs at least one project in allowedProjects".to_owned(),
            );
        }
        spec["allowedProjects"] = json!(projects);
    } else if let Some(object) = spec.as_object_mut() {
        object.remove("allowedProjects");
    }
    spec["audience"] = json!(audience);
    record("audience", json!(current_audience), json!(audience));
    record(
        "allowedProjects",
        current_projects,
        spec.get("allowedProjects")
            .cloned()
            .unwrap_or_else(|| json!([])),
    );

    let before: Vec<String> =
        serde_json::from_value(spec["enabledRepresentations"].clone()).unwrap_or_default();
    let known = known_representations();
    if let Some(bad) = params
        .add_representations
        .iter()
        .chain(&params.remove_representations)
        .find(|r| !known.contains(&r.trim()))
    {
        return Err(format!(
            "representation '{bad}' is not one of {}",
            known.join(", ")
        ));
    }
    let mut representations: Vec<String> = before
        .iter()
        // A retired file a manifest still names goes with the edit (T-4600).
        .filter(|r| known.contains(&r.as_str()))
        .filter(|r| {
            !params
                .remove_representations
                .iter()
                .any(|gone| gone.trim() == r.as_str())
        })
        .cloned()
        .collect();
    for added in &params.add_representations {
        let added = added.trim();
        if !representations.iter().any(|r| r == added) {
            representations.push(added.to_owned());
        }
    }
    if representations.is_empty() {
        return Err(format!(
            "'{name}' would serve no representation; an endpoint serves at least one"
        ));
    }
    spec["enabledRepresentations"] = json!(representations);
    // A calendar goes and comes with `ical`: a mapping without it fails the manifest, `ical`
    // without one serves nothing (EP-159). No compiled model means `Event`; the propose check
    // reads the model source then.
    let ical = Representation::Ical.as_str();
    if representations.iter().any(|r| r == ical) {
        if spec["representations"].get("ical").is_none() {
            spec["representations"]["ical"] = calendar(&served)?;
        }
    } else if let Some(block) = spec["representations"].as_object_mut() {
        block.remove("ical");
        if block.is_empty() {
            if let Some(object) = spec.as_object_mut() {
                object.remove("representations");
            }
        }
    }
    record(
        "enabledRepresentations",
        json!(before),
        json!(representations),
    );

    let hidden_before = spec["projection"]
        .get("hiddenAttributes")
        .cloned()
        .unwrap_or_else(|| json!([]));
    if let Some(hidden) = &params.hidden_attributes {
        if let Some(bad) = hidden.iter().find(|a| !is_identifier(a)) {
            return Err(format!(
                "hiddenAttributes entry '{bad}' is not an attribute name"
            ));
        }
        if hidden.is_empty() {
            if let Some(projection) = spec["projection"].as_object_mut() {
                projection.remove("hiddenAttributes");
                if projection.is_empty() {
                    if let Some(object) = spec.as_object_mut() {
                        object.remove("projection");
                    }
                }
            }
        } else {
            spec["projection"]["hiddenAttributes"] = json!(hidden);
        }
        record("hiddenAttributes", hidden_before, json!(hidden));
    }

    if let Some(per_minute) = params.requests_per_minute {
        if per_minute == 0 || per_minute > MAX_REQUESTS_PER_MINUTE {
            return Err(format!(
                "requestsPerMinute {per_minute} is outside 1..={MAX_REQUESTS_PER_MINUTE}"
            ));
        }
        let before = spec["rateLimits"]
            .get("requestsPerMinute")
            .cloned()
            .unwrap_or(Value::Null);
        spec["rateLimits"]["requestsPerMinute"] = json!(per_minute);
        record("requestsPerMinute", before, json!(per_minute));
    }

    if changes.is_empty() {
        return Err(format!("the request changes nothing on '{name}'"));
    }
    Ok(Edit { endpoint, changes })
}

/// The values the endpoint form opens with for an endpoint that exists (`existing: true`).
pub fn form_values(endpoint: &Value) -> Value {
    let spec = &endpoint["spec"];
    let mut prefill = json!({
        "existing": true,
        "name": endpoint["metadata"]["name"],
        "contextSpaceRef": spec["contextSpaceRef"],
        "slug": spec["slug"],
        "audience": spec["audience"],
        "enabledRepresentations": spec["enabledRepresentations"],
        "allowedProjects": spec.get("allowedProjects").cloned().unwrap_or_else(|| json!([])),
        "hiddenAttributes": spec["projection"].get("hiddenAttributes").cloned().unwrap_or_else(|| json!([])),
    });
    if let Some(title) = endpoint["metadata"].get("title") {
        prefill["title"] = json!(crate::resource::title_text(title).unwrap_or_default());
    }
    for field in ["rateLimits", "caching"] {
        if let Some(value) = spec.get(field) {
            prefill[field] = value.clone();
        }
    }
    prefill
}

/// The `edit_endpoint` call in a model answer, when the answer is one.
pub fn edit_call(answer: &str) -> Option<Result<EditEndpoint, String>> {
    for fence in TOOL_FENCE.captures_iter(answer) {
        let Ok(value) = serde_json::from_str::<Value>(&fence[1]) else {
            continue;
        };
        if value.get("tool").and_then(Value::as_str) != Some("edit_endpoint") {
            continue;
        }
        return Some(
            serde_json::from_value::<EditEndpoint>(value)
                .map_err(|err| format!("the edit_endpoint call does not parse: {err}")),
        );
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{
        edit_call, form_values, prose_of, render, slug, tool_call, Edit, EditEndpoint, FieldChange,
        Preset, ProposeEndpoint, DEFAULT_CREATES_PER_DAY,
    };
    use crate::change::Lane;
    use jc_core::kinds::Representation;
    use serde_json::json;

    fn request() -> ProposeEndpoint {
        ProposeEndpoint {
            context_space: "helsinki".into(),
            name: "bikes-regional-transport".into(),
            allowed_projects: vec!["regional-transport".into()],
            hidden_attributes: vec!["maintenanceNote".into()],
            entity_types: vec!["BikeHireDockingStation".into()],
            ..Default::default()
        }
    }

    #[test]
    fn a_slug_is_26_base32_characters_and_fresh_every_time() {
        let a = slug();
        let b = slug();
        assert_eq!(a.len(), 26);
        assert!(a
            .bytes()
            .all(|c| c.is_ascii_lowercase() || (b'2'..=b'7').contains(&c)));
        assert_ne!(a, b);
    }

    #[test]
    fn the_defaults_are_project_list_ngsi_ld_and_csv_and_the_lane_is_yellow() {
        let proposal = render("helsinki", &request(), &[]).expect("renders");
        assert_eq!(proposal.lane, Lane::Yellow);
        let spec = &proposal.endpoint["spec"];
        assert_eq!(spec["audience"], "project-list");
        assert_eq!(spec["allowedProjects"], json!(["regional-transport"]));
        assert_eq!(spec["enabledRepresentations"], json!(["ngsi-ld", "csv"]));
        assert_eq!(
            spec["projection"]["hiddenAttributes"],
            json!(["maintenanceNote"])
        );
        assert_eq!(spec["slug"].as_str().map(str::len), Some(26));
        assert_eq!(proposal.endpoint["metadata"]["namespace"], "helsinki");
        assert_eq!(proposal.policies.len(), 1);
        let policy = &proposal.policies[0]["spec"];
        assert_eq!(
            policy["assignee"],
            json!({ "kind": "group", "id": "regional-transport" })
        );
        assert_eq!(policy["assigner"], "did:web:{orgDomain}");
        assert_eq!(policy["operations"], json!(["retrieveOps"]));
        assert_eq!(
            policy["information"][0]["entities"][0]["type"],
            "BikeHireDockingStation"
        );
        assert_eq!(
            proposal.prefill["hiddenAttributes"],
            json!(["maintenanceNote"])
        );
        assert_eq!(proposal.prefill["slug"], proposal.endpoint["spec"]["slug"]);
        // The form ticks the classes the proposal names: a new endpoint over a modelled space
        // is refused without one (T-0895).
        assert_eq!(
            proposal.prefill["entityTypes"],
            json!(["BikeHireDockingStation"])
        );
    }

    #[test]
    fn a_public_audience_is_the_red_lane_with_the_public_role() {
        let mut params = request();
        params.audience = Some("public".into());
        params.allowed_projects.clear();
        let proposal = render("helsinki", &params, &[]).expect("renders");
        assert_eq!(proposal.lane, Lane::Red);
        assert!(proposal.endpoint["spec"].get("allowedProjects").is_none());
        assert_eq!(
            proposal.policies[0]["spec"]["assignee"],
            json!({ "kind": "role", "id": "public" })
        );
    }

    #[test]
    fn what_cannot_be_rendered_is_named() {
        let mut params = request();
        params.allowed_projects.clear();
        assert!(render("helsinki", &params, &[])
            .unwrap_err()
            .contains("allowedProjects"));
        let mut params = request();
        params.audience = Some("everyone".into());
        assert!(render("helsinki", &params, &[])
            .unwrap_err()
            .contains("audience"));
        let mut params = request();
        params.name = "Bikes!".into();
        assert!(render("helsinki", &params, &[])
            .unwrap_err()
            .contains("DNS-1123"));
        let mut params = request();
        params.representations = vec!["pdf".into()];
        assert!(render("helsinki", &params, &[])
            .unwrap_err()
            .contains("representation"));
        let mut params = request();
        params.hidden_attributes = vec!["<script>".into()];
        assert!(render("helsinki", &params, &[])
            .unwrap_err()
            .contains("hiddenAttributes"));
    }

    fn endpoints() -> Vec<serde_json::Value> {
        vec![
            json!({
                "apiVersion": "joinedcontext.com/v1alpha1",
                "kind": "Endpoint",
                "metadata": {
                    "name": "helsinki-kpi",
                    "namespace": "helsinki",
                    "labels": { "joinedcontext.com/space": "helsinki-kpi" },
                    "title": { "en": "Helsinki indicators" }
                },
                "spec": {
                    "contextSpaceRef": "helsinki-kpi",
                    "slug": "abcdefghijklmnopqrstuvwxyz",
                    "audience": "project-list",
                    "allowedProjects": ["helsinki-mobility"],
                    "enabledRepresentations": ["ngsi-ld", "csv"],
                    "rateLimits": { "requestsPerMinute": 600, "burst": 50 }
                },
                "status": { "phase": "Live" }
            }),
            json!({
                "metadata": { "name": "helsinki-weather", "namespace": "helsinki" },
                "spec": { "contextSpaceRef": "helsinki", "slug": "weatherweatherweatherweath", "audience": "public", "enabledRepresentations": ["ngsi-ld"] }
            }),
        ]
    }

    /// An edit that adds no calendar reads no served type.
    fn edit(endpoints: &[serde_json::Value], params: &EditEndpoint) -> Result<Edit, String> {
        super::edit(endpoints, params, |_| Vec::new())
    }

    fn edit_of(value: serde_json::Value) -> EditEndpoint {
        serde_json::from_value(value).expect("an edit call")
    }

    #[test]
    fn an_edit_adds_and_removes_representations_and_keeps_the_slug_space_and_labels() {
        let edited = edit(
            &endpoints(),
            &edit_of(json!({ "name": "helsinki-kpi", "addRepresentations": ["ogc-features", "csv"], "removeRepresentations": ["ngsi-ld"] })),
        )
        .expect("edits");
        let spec = &edited.endpoint["spec"];
        assert_eq!(
            spec["enabledRepresentations"],
            json!(["csv", "ogc-features"])
        );
        assert_eq!(spec["slug"], "abcdefghijklmnopqrstuvwxyz");
        assert_eq!(spec["contextSpaceRef"], "helsinki-kpi");
        assert_eq!(spec["rateLimits"]["burst"], 50);
        assert_eq!(
            edited.endpoint["metadata"]["labels"]["joinedcontext.com/space"],
            "helsinki-kpi"
        );
        assert!(
            edited.endpoint.get("status").is_none(),
            "status is never written"
        );
        assert_eq!(
            edited.changes,
            vec![FieldChange {
                field: "enabledRepresentations".into(),
                before: json!(["ngsi-ld", "csv"]),
                after: json!(["csv", "ogc-features"]),
            }]
        );
        let form = form_values(&edited.endpoint);
        assert_eq!(form["existing"], true);
        assert_eq!(form["slug"], "abcdefghijklmnopqrstuvwxyz");
        assert_eq!(form["title"], "Helsinki indicators");
        assert_eq!(form["allowedProjects"], json!(["helsinki-mobility"]));
    }

    #[test]
    fn going_public_drops_the_project_list_and_hiding_sets_the_projection() {
        let edited = edit(
            &endpoints(),
            &edit_of(json!({ "name": "helsinki-kpi", "audience": "public", "hiddenAttributes": ["address"], "requestsPerMinute": 1200 })),
        )
        .expect("edits");
        let spec = &edited.endpoint["spec"];
        assert_eq!(spec["audience"], "public");
        assert!(spec.get("allowedProjects").is_none());
        assert_eq!(spec["projection"]["hiddenAttributes"], json!(["address"]));
        assert_eq!(
            spec["rateLimits"],
            json!({ "requestsPerMinute": 1200, "burst": 50 })
        );
        let fields: Vec<&str> = edited.changes.iter().map(|c| c.field.as_str()).collect();
        assert_eq!(
            fields,
            [
                "audience",
                "allowedProjects",
                "hiddenAttributes",
                "requestsPerMinute"
            ]
        );
        let form = form_values(&edited.endpoint);
        assert_eq!(form["allowedProjects"], json!([]));
        assert_eq!(form["hiddenAttributes"], json!(["address"]));
    }

    /// The list follows the manifest: each name the Endpoint kind takes in
    /// `enabledRepresentations` drafts, and the draft passes the kind's own validation.
    #[test]
    fn every_representation_the_manifest_lists_drafts_and_feed_stays_refused() {
        for representation in Representation::ALL {
            let name = representation.as_str();
            let mut params = request();
            params.entity_types = vec!["Event".to_owned()];
            params.representations = vec![name.to_owned()];
            match render("helsinki", &params, &[]) {
                Ok(rendered) => {
                    let spec: jc_core::kinds::EndpointSpec =
                        serde_json::from_value(rendered.endpoint["spec"].clone())
                            .unwrap_or_else(|e| panic!("{name}: {e}"));
                    spec.validate().unwrap_or_else(|e| panic!("{name}: {e}"));
                    assert!(spec.served_representations().contains(&representation));
                }
                Err(reason) => {
                    assert_eq!(representation, Representation::Feed, "{reason}");
                    assert!(reason.contains("'feed'"), "{reason}");
                }
            }
        }
    }

    #[test]
    fn a_representation_nothing_serves_stays_refused() {
        for bad in ["xlsx", "parquet", "zip", "json", "ODATA", "ical/v1", ""] {
            let mut params = request();
            params.representations = vec![bad.to_owned()];
            let refused = render("helsinki", &params, &[]).expect_err(bad);
            assert!(refused.contains("is not one of"), "{refused}");
        }
    }

    /// T-5962 S-13: the person's words were NGSI-LD, OData and GeoJSON.
    #[test]
    fn geojson_reads_as_the_surface_that_answers_it() {
        let mut params = request();
        params.representations = ["ngsi-ld", "odata", "geojson", "ogc-features"]
            .map(str::to_owned)
            .to_vec();
        let rendered = render("helsinki", &params, &[]).expect("renders");
        assert_eq!(
            rendered.endpoint["spec"]["enabledRepresentations"],
            json!(["ngsi-ld", "odata", "ogc-features"])
        );
    }

    /// T-5962 S-37: a listed `ical` without an output would serve no calendar (EP-159).
    #[test]
    fn ical_drafts_one_calendar_over_the_types_a_preset_maps() {
        let draft = |types: &[&str]| {
            let mut params = request();
            params.representations = vec!["ngsi-ld".to_owned(), "ical".to_owned()];
            params.entity_types = types.iter().map(|t| (*t).to_owned()).collect();
            render("helsinki", &params, &[])
        };
        let event = draft(&["Event", "Place"]).expect("renders");
        assert_eq!(
            event.endpoint["spec"]["representations"]["ical"],
            json!([{ "name": "events", "sources": [{ "type": "Event", "preset": "sdm-event" }] }])
        );
        let refused = draft(&["BikeCounter"]).expect_err("no preset maps it");
        assert!(refused.contains("BikeCounter"), "{refused}");
        assert!(refused.contains("endpoint form"), "{refused}");
        let plain = render("helsinki", &request(), &[]).expect("renders");
        assert!(plain.endpoint["spec"].get("representations").is_none());
    }

    #[test]
    fn an_edit_keeps_what_the_endpoint_serves_and_the_calendar_follows_ical() {
        let mut all = endpoints();
        all[0]["spec"]["enabledRepresentations"] = json!(["ngsi-ld", "odata", "websocket"]);
        let added = edit(
            &all,
            &edit_of(json!({ "name": "helsinki-kpi", "addRepresentations": ["ical"] })),
        )
        .expect("edits");
        let spec = &added.endpoint["spec"];
        assert_eq!(
            spec["enabledRepresentations"],
            json!(["ngsi-ld", "odata", "websocket", "ical"])
        );
        assert_eq!(spec["representations"]["ical"][0]["name"], "events");

        all[0] = added.endpoint;
        let mapped = json!([{ "name": "meetings", "sources": [{ "type": "CouncilMeeting" }] }]);
        all[0]["spec"]["representations"]["ical"] = mapped.clone();
        let kept = edit(
            &all,
            &edit_of(json!({ "name": "helsinki-kpi", "addRepresentations": ["csv"] })),
        )
        .expect("edits");
        assert_eq!(kept.endpoint["spec"]["representations"]["ical"], mapped);

        let removed = edit(
            &all,
            &edit_of(json!({ "name": "helsinki-kpi", "removeRepresentations": ["ical"] })),
        )
        .expect("edits");
        assert!(removed.endpoint["spec"].get("representations").is_none());
    }

    /// A review bounce: `ical` on an Endpoint of BikeCounter drew an `Event` calendar that stays
    /// empty, the silent case `render` refuses.
    #[test]
    fn an_edit_adds_a_calendar_only_over_a_type_the_endpoint_serves() {
        let add = |all: &[serde_json::Value], types: &[&str]| {
            let served: Vec<String> = types.iter().map(|t| (*t).to_owned()).collect();
            super::edit(
                all,
                &edit_of(json!({ "name": "helsinki-kpi", "addRepresentations": ["ical"] })),
                |_| served.clone(),
            )
        };
        let refused = add(&endpoints(), &["BikeCounter"]).expect_err("no preset maps it");
        assert!(refused.contains("BikeCounter"), "{refused}");
        assert!(refused.contains("endpoint form"), "{refused}");

        let mixed = add(&endpoints(), &["BikeCounter", "Event"]).expect("edits");
        assert_eq!(
            mixed.endpoint["spec"]["representations"]["ical"][0]["sources"],
            json!([{ "type": "Event", "preset": "sdm-event" }])
        );

        let no_model = add(&endpoints(), &[]).expect("edits");
        assert_eq!(
            no_model.endpoint["spec"]["representations"]["ical"][0]["sources"][0]["type"],
            "Event"
        );

        // A calendar mapped by hand stands: `ical` is listed already, so no preset is asked for.
        let mut all = endpoints();
        all[0]["spec"]["enabledRepresentations"] = json!(["ngsi-ld", "ical"]);
        let mapped = json!([{ "name": "meetings", "sources": [{ "type": "CouncilMeeting" }] }]);
        all[0]["spec"]["representations"] = json!({ "ical": mapped });
        let kept = super::edit(
            &all,
            &edit_of(json!({ "name": "helsinki-kpi", "addRepresentations": ["csv"] })),
            |_| vec!["CouncilMeeting".to_owned()],
        )
        .expect("edits");
        assert_eq!(kept.endpoint["spec"]["representations"]["ical"], mapped);
    }

    #[test]
    fn what_an_edit_cannot_do_is_named() {
        let unknown = edit(
            &endpoints(),
            &edit_of(json!({ "name": "helsinki-parking", "audience": "public" })),
        )
        .unwrap_err();
        assert!(
            unknown.contains("helsinki-kpi, helsinki-weather"),
            "{unknown}"
        );
        assert!(edit(&[], &edit_of(json!({ "name": "x" })))
            .unwrap_err()
            .contains("no endpoints"));
        let bad_audience = edit(
            &endpoints(),
            &edit_of(json!({ "name": "helsinki-kpi", "audience": "everyone" })),
        )
        .unwrap_err();
        assert!(bad_audience.contains("audience"), "{bad_audience}");
        let bad_representation = edit(
            &endpoints(),
            &edit_of(json!({ "name": "helsinki-kpi", "addRepresentations": ["pdf"] })),
        )
        .unwrap_err();
        assert!(
            bad_representation.contains("representation 'pdf'"),
            "{bad_representation}"
        );
        let none_left = edit(
            &endpoints(),
            &edit_of(json!({ "name": "helsinki-weather", "removeRepresentations": ["ngsi-ld"] })),
        )
        .unwrap_err();
        assert!(none_left.contains("at least one"), "{none_left}");
        let narrowing_public = edit(
            &endpoints(),
            &edit_of(
                json!({ "name": "helsinki-weather", "allowedProjects": ["helsinki-mobility"] }),
            ),
        )
        .unwrap_err();
        assert!(
            narrowing_public.contains("project-list"),
            "{narrowing_public}"
        );
        let empty_list = edit(
            &endpoints(),
            &edit_of(json!({ "name": "helsinki-weather", "audience": "project-list" })),
        )
        .unwrap_err();
        assert!(empty_list.contains("allowedProjects"), "{empty_list}");
        let nothing = edit(
            &endpoints(),
            &edit_of(json!({ "name": "helsinki-weather", "audience": "public" })),
        )
        .unwrap_err();
        assert!(nothing.contains("changes nothing"), "{nothing}");
        let limit = edit(
            &endpoints(),
            &edit_of(json!({ "name": "helsinki-weather", "requestsPerMinute": 0 })),
        )
        .unwrap_err();
        assert!(limit.contains("requestsPerMinute"), "{limit}");
    }

    #[test]
    fn an_edit_call_is_told_from_a_share_call() {
        let answer = "Adding CSV.\n\n```json\n{\"tool\": \"edit_endpoint\", \"name\": \"helsinki-news\", \"addRepresentations\": [\"csv\"]}\n```\n";
        let call = edit_call(answer).expect("an edit call").expect("parses");
        assert_eq!(call.name, "helsinki-news");
        assert_eq!(call.add_representations, vec!["csv"]);
        assert!(tool_call(answer).is_none());
        assert!(edit_call("```json\n{\"tool\": \"propose_endpoint\"}\n```").is_none());
    }

    #[test]
    fn a_tool_answer_is_told_from_a_kit_answer() {
        let answer = "I will share the stations.\n\n```json\n{\"tool\": \"propose_endpoint\", \"contextSpace\": \"helsinki\", \"name\": \"bikes\", \"allowedProjects\": [\"transport\"], \"extra\": 1}\n```\n";
        let call = tool_call(answer).expect("a tool call").expect("parses");
        assert_eq!(call.name, "bikes");
        assert_eq!(call.allowed_projects, vec!["transport"]);
        assert_eq!(prose_of(answer), "I will share the stations.");
        assert!(
            tool_call("```text\nspec.json\n<<<<<<< SEARCH\n=======\n{}\n>>>>>>> REPLACE\n```")
                .is_none()
        );
        assert!(tool_call("```json\n{\"title\": \"x\"}\n```").is_none());
    }

    /// T-1042: a Policy naming a group the repository does not declare does not validate, so
    /// the share drafts the missing group beside it; a group that already exists is not
    /// proposed twice.
    #[test]
    fn a_share_drafts_the_consumer_group_it_names_and_no_other() {
        let mut params = request();
        params.audience = Some("project-list".to_owned());
        params.allowed_projects = vec!["helsinki-mobility".to_owned(), "espoo".to_owned()];

        let proposal = render("helsinki", &params, &["espoo".to_owned()]).expect("renders");
        let drafted: Vec<&str> = proposal
            .groups
            .iter()
            .filter_map(|g| g["metadata"]["name"].as_str())
            .collect();
        assert_eq!(
            drafted,
            vec!["helsinki-mobility"],
            "only the group the repository is missing: {:?}",
            proposal.groups
        );
        assert_eq!(
            proposal.groups[0]["spec"]["members"],
            serde_json::json!([]),
            "the consumer's administrators say who belongs to it (PF-62)"
        );
        assert_eq!(proposal.groups[0]["kind"], "Group");

        // Every named group already declared: nothing to draft.
        let known = vec!["helsinki-mobility".to_owned(), "espoo".to_owned()];
        assert!(render("helsinki", &params, &known)
            .expect("renders")
            .groups
            .is_empty());
    }

    /// T-6464: an organization share grants its Organization and the project's own group; a grant
    /// to the domain matched nobody.
    #[test]
    fn an_organization_share_grants_people_a_token_can_name() {
        let mut params = request();
        params.audience = Some("organization".to_owned());
        params.allowed_projects = Vec::new();
        let proposal = render("helsinki", &params, &[]).expect("renders");
        let assignees: Vec<&serde_json::Value> = proposal
            .policies
            .iter()
            .map(|policy| &policy["spec"]["assignee"])
            .collect();
        assert_eq!(
            assignees,
            [
                &json!({ "kind": "organization" }),
                &json!({ "kind": "group", "id": "helsinki" })
            ]
        );
    }

    /// An audience that names no project names no group either.
    #[test]
    fn an_organization_or_public_share_drafts_no_group() {
        for audience in ["organization", "public"] {
            let mut params = request();
            params.audience = Some(audience.to_owned());
            params.allowed_projects = Vec::new();
            assert!(
                render("helsinki", &params, &[])
                    .expect("renders")
                    .groups
                    .is_empty(),
                "{audience}"
            );
        }
    }

    /// The preset → operations table of AP-132: each preset is the one before plus its writes.
    #[test]
    fn a_preset_names_exactly_the_operations_ap_132_lists() {
        let reads = [
            "queryEntity",
            "retrieveEntity",
            "queryTemporal",
            "retrieveTemporal",
        ];
        assert_eq!(Preset::Read.operations(), reads);
        assert_eq!(
            Preset::Update.operations(),
            [&reads[..], &["updateAttrs", "appendAttrs"]].concat()
        );
        assert_eq!(
            Preset::Full.operations(),
            [
                &reads[..],
                &["updateAttrs", "appendAttrs", "createEntity", "deleteEntity"]
            ]
            .concat()
        );
    }

    fn app_endpoint(access: Preset) -> ProposeEndpoint {
        ProposeEndpoint {
            context_space: "mobility".into(),
            name: "bikes-app".into(),
            entity_types: vec!["BikeHireDockingStation".into()],
            access: Some(access),
            ..Default::default()
        }
    }

    fn public_form() -> ProposeEndpoint {
        let mut form = app_endpoint(Preset::Create);
        form.audience = Some("public".into());
        form.write_attributes = vec!["freeSlots".into(), "name".into(), "name".into()];
        form.write_relationships = vec!["refDevice".into()];
        form
    }

    /// T-3103, T-3172: a public form's endpoint grants the public role `createEntity` on its one
    /// type and the form's fields alone, and takes the red lane like every public endpoint.
    #[test]
    fn a_public_form_grants_anyone_the_create_of_its_one_type_alone() {
        let form = public_form();
        let proposal = render("helsinki", &form, &[]).expect("renders");
        assert_eq!(proposal.lane, Lane::Red);
        let [policy] = proposal.policies.as_slice() else {
            panic!("one policy: {:?}", proposal.policies);
        };
        assert_eq!(
            policy["spec"]["assignee"],
            json!({ "kind": "role", "id": "public" })
        );
        assert_eq!(policy["spec"]["operations"], json!(["createEntity"]));
        assert_eq!(
            policy["spec"]["information"],
            json!([{
                "entities": [{ "type": "BikeHireDockingStation" }],
                "propertyNames": ["freeSlots", "name"],
                "relationshipNames": ["refDevice"],
            }])
        );
        assert_eq!(proposal.endpoint["spec"]["audience"], "public");
        // T-3177: the gateway mints the id and counts the day, and the pinned jc-core takes it.
        assert_eq!(
            proposal.endpoint["spec"]["creates"],
            json!({ "mintIds": true, "perDay": DEFAULT_CREATES_PER_DAY })
        );
        let spec: jc_core::kinds::EndpointSpec =
            serde_json::from_value(proposal.endpoint["spec"].clone()).expect("an Endpoint spec");
        spec.validate().expect("a valid Endpoint");
    }

    /// T-3177: a form's daily count is its own within 1..=10000; anything else is refused, and
    /// only a form names one.
    #[test]
    fn a_public_form_names_its_daily_count_within_bounds() {
        let mut form = public_form();
        form.creates_per_day = Some(25);
        let proposal = render("helsinki", &form, &[]).expect("renders");
        assert_eq!(proposal.endpoint["spec"]["creates"]["perDay"], 25);
        for bad in [0, 10_001] {
            form.creates_per_day = Some(bad);
            assert!(render("helsinki", &form, &[])
                .expect_err("out of bounds")
                .contains("createsPerDay"));
        }
        let mut not_a_form = app_endpoint(Preset::Full);
        not_a_form.creates_per_day = Some(25);
        assert!(render("helsinki", &not_a_form, &[])
            .expect_err("not a form")
            .contains("public form"));
        let read = render("helsinki", &app_endpoint(Preset::Read), &[]).expect("read");
        assert!(read.endpoint["spec"].get("creates").is_none());
    }

    /// EP-101 (T-3266): a form names the sites that may frame it by the Endpoint's own rule; a
    /// site that is not an https origin is refused by name, and only a form names any.
    #[test]
    fn a_public_form_names_the_sites_that_may_frame_it() {
        let mut form = public_form();
        let plain = render("helsinki", &form, &[]).expect("renders");
        assert!(plain.endpoint["spec"]["creates"]
            .get("embedOrigins")
            .is_none());
        form.embed_origins = vec!["https://www.hel.fi".into()];
        let framed = render("helsinki", &form, &[]).expect("renders");
        assert_eq!(
            framed.endpoint["spec"]["creates"]["embedOrigins"],
            json!(["https://www.hel.fi"])
        );
        let spec: jc_core::kinds::EndpointSpec =
            serde_json::from_value(framed.endpoint["spec"].clone()).expect("an Endpoint spec");
        spec.validate().expect("a valid Endpoint");
        for bad in [
            "http://www.hel.fi",
            "https://*.hel.fi",
            "https://www.hel.fi/form",
        ] {
            form.embed_origins = vec![bad.into()];
            assert!(render("helsinki", &form, &[])
                .expect_err(bad)
                .contains("embedOrigins"));
        }
        let mut not_a_form = app_endpoint(Preset::Full);
        not_a_form.embed_origins = vec!["https://www.hel.fi".into()];
        assert!(render("helsinki", &not_a_form, &[])
            .expect_err("not a form")
            .contains("public form"));
    }

    /// T-3172: a form that names no field would grant every attribute of the type, a field that
    /// is not an attribute name is refused, and the fields belong to a form alone.
    #[test]
    fn a_public_form_names_its_fields_and_only_a_form_may() {
        let mut no_fields = public_form();
        no_fields.write_attributes.clear();
        no_fields.write_relationships.clear();
        assert!(render("helsinki", &no_fields, &[])
            .expect_err("no fields")
            .contains("writeAttributes"));
        let mut bad = public_form();
        bad.write_attributes.push("not a name".into());
        assert!(render("helsinki", &bad, &[])
            .expect_err("bad field")
            .contains("'not a name'"));
        let mut not_a_form = app_endpoint(Preset::Full);
        not_a_form.write_attributes = vec!["name".into()];
        assert!(render("helsinki", &not_a_form, &[])
            .expect_err("not a form")
            .contains("public form"));
    }

    /// T-3103: a public form is public and names exactly one type; anything else is refused with
    /// the reason, never rendered as a wider write.
    #[test]
    fn a_public_form_that_is_not_public_or_not_one_type_is_refused() {
        let not_public = app_endpoint(Preset::Create);
        assert!(render("helsinki", &not_public, &[])
            .expect_err("project-list")
            .contains("audience public"));
        for types in [vec![], vec!["A".to_owned(), "B".to_owned()]] {
            let mut form = public_form();
            form.entity_types = types;
            assert!(render("helsinki", &form, &[])
                .expect_err("one type")
                .contains("exactly one entity type"));
        }
    }

    /// A read preset grants nothing: the endpoint alone, read with the grants already held.
    #[test]
    fn a_read_preset_is_the_endpoint_alone_for_this_project() {
        let proposal = render("helsinki", &app_endpoint(Preset::Read), &[]).expect("renders");
        assert_eq!(proposal.lane, Lane::Yellow);
        assert!(proposal.policies.is_empty(), "{:?}", proposal.policies);
        assert!(proposal.groups.is_empty(), "{:?}", proposal.groups);
        let spec = &proposal.endpoint["spec"];
        assert_eq!(spec["audience"], "project-list");
        assert_eq!(spec["allowedProjects"], json!(["helsinki"]));
    }

    /// A write preset grants its operations on the named types to the project, and is red.
    #[test]
    fn a_write_preset_grants_its_operations_on_the_named_types_in_the_red_lane() {
        for preset in [Preset::Update, Preset::Full] {
            let proposal = render("helsinki", &app_endpoint(preset), &[]).expect("renders");
            assert_eq!(proposal.lane, Lane::Red, "{preset:?}");
            assert_eq!(proposal.policies.len(), 1, "{preset:?}");
            let policy = &proposal.policies[0];
            assert_eq!(policy["metadata"]["name"], "bikes-app-helsinki");
            assert_eq!(policy["spec"]["operations"], json!(preset.operations()));
            assert_eq!(
                policy["spec"]["assignee"],
                json!({ "kind": "group", "id": "helsinki" })
            );
            assert_eq!(
                policy["spec"]["information"],
                json!([{ "entities": [{ "type": "BikeHireDockingStation" }] }])
            );
        }
    }

    /// A preset is for this project's own apps, and a write names what it writes.
    #[test]
    fn a_preset_for_another_audience_or_a_write_on_no_type_is_refused() {
        let mut elsewhere = app_endpoint(Preset::Read);
        elsewhere.allowed_projects = vec!["espoo".into()];
        let mut public = app_endpoint(Preset::Update);
        public.audience = Some("public".into());
        let mut both = app_endpoint(Preset::Read);
        both.allowed_projects = vec!["helsinki".into(), "espoo".into()];
        for params in [elsewhere, public, both] {
            let refusal = render("helsinki", &params, &[]).expect_err("refused");
            assert!(
                refusal.contains("allowedProjects ['helsinki'] alone"),
                "{refusal}"
            );
        }
        let mut untyped = app_endpoint(Preset::Full);
        untyped.entity_types.clear();
        let refusal = render("helsinki", &untyped, &[]).expect_err("refused");
        assert!(refusal.contains("needs entityTypes"), "{refusal}");
        // Reading needs no type: the builder narrows what the app reads.
        let mut read = app_endpoint(Preset::Read);
        read.entity_types.clear();
        assert!(render("helsinki", &read, &[]).is_ok());
    }
}
