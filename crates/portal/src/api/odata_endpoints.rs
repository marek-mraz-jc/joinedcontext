//! The entity set `endpointList` of the project's OData service (T-6645): the rows of the
//! Endpoints page, which the SDK grid draws (T-6644). The set `endpoints` answers
//! `jc_resource_list`, the project's own Endpoints alone; this one also holds what the page
//! lists beside them (T-6169):
//!
//! - `own`, an Endpoint of this project, keyed by its name;
//! - `shared`, an Endpoint another project shares with this one (EP-14), keyed `{source}/{name}`;
//! - `gone`, this project's SharedSpaceReference whose Endpoint no longer shows (EP-15), keyed
//!   `reference:{name}`.
//!
//! A manifest name holds neither `/` nor `:`, so the three key spaces never meet. Every row
//! passes the read check of the project it lives in before an OData option sees it, and
//! [`decide`] answers `$allowed` and the grid's writes from the check the proposal runs.

use std::collections::{HashMap, HashSet};

use jc_core::kinds::Verb;
use odata_table::{Field, FieldKind};
use serde_json::{json, Map, Value};

use crate::auth::Identity;
use crate::error::ApiError;
use crate::permissions::Effective;
use crate::resource::ResourceEnvelope;
use crate::state::AppState;
use crate::store::ListOptions;

/// The set's name, its entity type and its key property.
pub const SET: &str = "endpointList";
pub const TYPE: &str = "EndpointRow";
pub const KEY: &str = "id";
const KIND: &str = "Endpoint";

/// A row's kind, the `kind` property.
const OWN: &str = "own";
const SHARED: &str = "shared";
const GONE: &str = "gone";

/// The properties of a row, in order: name, type, nullable, description. The allow-list: the
/// parser refuses any other name in `$filter`, `$select` and `$orderby`.
const PROPERTIES: [(&str, FieldKind, bool, &str); 15] = [
    ("id", FieldKind::String, false, "The row's key: the name, {source}/{name} for a shared row, reference:{name} for a gone one"),
    ("name", FieldKind::String, false, "The Endpoint's name, or the reference's for a gone row"),
    ("title", FieldKind::String, true, "The Endpoint's title in English, or the reference's alias"),
    ("description", FieldKind::String, true, "The Endpoint's description in English"),
    ("contextSpace", FieldKind::String, true, "The Context Space the Endpoint serves"),
    ("slug", FieldKind::String, true, "The Endpoint's slug, its address on the gateway"),
    ("audience", FieldKind::String, true, "Who may call it: project, project-list, organization or public"),
    ("mcp", FieldKind::Boolean, false, "Whether it serves MCP (EP-24)"),
    ("source", FieldKind::String, true, "The project that publishes it"),
    ("kind", FieldKind::String, false, "own, shared or gone"),
    ("reference", FieldKind::String, true, "This project's SharedSpaceReference to it"),
    ("phase", FieldKind::String, true, "Its lifecycle phase, e.g. Ready"),
    ("representations", FieldKind::String, true, "The representations it serves, comma-separated"),
    ("calls7d", FieldKind::Integer, true, "Its calls in the last 7 days, for an own row where counted"),
    // odata-table serves no tolower, so the page's search in any case reads this one.
    ("search", FieldKind::String, false, "The name and the title in lower case, for a search in any case, e.g. contains(search,'noise')"),
];

/// The values a row is built from: every property but the last, `search`, derived from them.
const GIVEN: usize = PROPERTIES.len() - 1;

/// The properties a `PATCH` may change, each with the manifest field it writes: what the
/// Endpoints grid edits in place, as its ManifestGrid did (T-6169). The rest is the form's.
const EDITABLE: [(&str, &str); 3] = [
    ("contextSpace", "contextSpaceRef"),
    ("audience", "audience"),
    ("mcp", "mcp"),
];

/// The set's fields, for its entity type.
pub fn fields() -> Vec<Field> {
    PROPERTIES
        .iter()
        .map(|(name, kind, nullable, description)| {
            let mut field = Field::new(name, kind.clone(), description);
            field.nullable = *nullable;
            field
        })
        .collect()
}

fn text(t: Option<&jc_core::Text>) -> Value {
    t.map_or(Value::Null, |t| {
        Value::String(t.resolve(&[], "en").to_owned())
    })
}

fn spec_str(env: &ResourceEnvelope, key: &str) -> Value {
    env.spec
        .get(key)
        .and_then(Value::as_str)
        .map_or(Value::Null, |s| Value::String(s.to_owned()))
}

fn opt(value: Option<&str>) -> Value {
    value.map_or(Value::Null, |s| Value::String(s.to_owned()))
}

/// A row from its values in [`PROPERTIES`] order, `search` added from its name and title.
fn row(values: [Value; GIVEN]) -> Value {
    let mut row: Map<String, Value> = PROPERTIES
        .iter()
        .map(|(name, ..)| (*name).to_owned())
        .zip(values)
        .collect();
    let search = ["name", "title"]
        .iter()
        .filter_map(|name| row.get(*name).and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase();
    row.insert("search".into(), Value::String(search));
    Value::Object(row)
}

/// An Endpoint row: `source` the project publishing it, `kind` own or shared, `reference` this
/// project's reference to it, `calls` its calls in the last 7 days where counted.
fn endpoint_row(
    env: &ResourceEnvelope,
    source: &str,
    kind: &'static str,
    reference: Option<&str>,
    calls: Option<u64>,
) -> Value {
    let name = env.metadata.name.clone();
    let id = if kind == OWN {
        name.clone()
    } else {
        format!("{source}/{name}")
    };
    let representations = env
        .spec
        .get("enabledRepresentations")
        .and_then(Value::as_array)
        .map(|reps| {
            reps.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(",")
        });
    let phase = env
        .status
        .as_ref()
        .and_then(|status| serde_json::to_value(status.phase).ok())
        .and_then(|phase| phase.as_str().map(str::to_owned));
    row([
        Value::String(id),
        Value::String(name),
        text(env.metadata.title.as_ref()),
        text(env.metadata.description.as_ref()),
        crate::permissions::spec_space(&env.spec).map_or(Value::Null, Value::String),
        spec_str(env, "slug"),
        spec_str(env, "audience"),
        // Every Endpoint serves MCP unless it says `mcp: false` (EP-24).
        Value::Bool(env.spec.get("mcp").and_then(Value::as_bool) != Some(false)),
        Value::String(source.to_owned()),
        Value::String(kind.to_owned()),
        opt(reference),
        opt(phase.as_deref()),
        opt(representations.as_deref()),
        calls.map_or(Value::Null, |n| json!(n)),
    ])
}

/// A reference of this project whose Endpoint no longer shows (EP-15): a row still, because a
/// grant nobody lists is a grant nobody can remove (T-0706).
fn gone_row(reference: &ResourceEnvelope) -> Value {
    let name = reference.metadata.name.clone();
    row([
        Value::String(format!("reference:{name}")),
        Value::String(name.clone()),
        opt(reference.spec.get("alias").and_then(Value::as_str)),
        Value::Null,
        Value::Null,
        spec_str(reference, "endpointSlug"),
        Value::Null,
        Value::Bool(false),
        Value::Null,
        Value::String(GONE.to_owned()),
        Value::String(name),
        Value::Null,
        Value::Null,
        Value::Null,
    ])
}

/// The read check of the set: a project where the caller reads no Endpoint has no list.
fn gate(state: &AppState, identity: &Identity, project: &str) -> Result<Effective, ApiError> {
    let effective = crate::permissions::for_request(state, identity, project);
    if !effective.may_read_some(KIND) {
        return Err(ApiError::NotFound(format!(
            "no {SET} in project '{project}': you read no Endpoint there"
        )));
    }
    Ok(effective)
}

/// The manifests of `kind` in `project` the caller may read: the kind check, then each manifest's
/// own (a grant bound to one context space reads that space's Endpoints alone).
fn readable_of(
    state: &AppState,
    effective: &Effective,
    project: &str,
    kind: &str,
) -> Vec<ResourceEnvelope> {
    if !effective.may_read_some(kind) {
        return Vec::new();
    }
    state
        .mirror
        .list(project, kind, &ListOptions::default())
        .items
        .into_iter()
        .filter(|env| effective.may_read_envelope(kind, env))
        .collect()
}

/// Whether `project` may call an Endpoint `owner` publishes: the gateway's own rule (EP-14), as
/// the Endpoints page reads it (ui sharing.tsx `admits`).
fn admits(env: &ResourceEnvelope, owner: &str, project: &str) -> bool {
    if owner == project {
        return true;
    }
    match env.spec.get("audience").and_then(Value::as_str) {
        Some("organization" | "public") => true,
        None | Some("project-list") => env
            .spec
            .get("allowedProjects")
            .and_then(Value::as_array)
            .is_some_and(|list| list.iter().any(|p| p.as_str() == Some(project))),
        Some(_) => false,
    }
}

/// The reference of `references` to `source`'s Endpoint `env`: by `endpointRef` where it names
/// one, else by slug (ui sharing.tsx `referenceTo`).
fn reference_to<'a>(
    references: &'a [ResourceEnvelope],
    env: &ResourceEnvelope,
    source: &str,
) -> Option<&'a ResourceEnvelope> {
    let slug = env.spec.get("slug").and_then(Value::as_str).unwrap_or("");
    references
        .iter()
        .find(|reference| match reference.spec.get("endpointRef") {
            Some(target) => {
                target.get("project").and_then(Value::as_str) == Some(source)
                    && target.get("name").and_then(Value::as_str)
                        == Some(env.metadata.name.as_str())
            }
            None => {
                !slug.is_empty()
                    && reference.spec.get("endpointSlug").and_then(Value::as_str) == Some(slug)
            }
        })
}

/// Calls per own Endpoint in the last 7 days, as the gateways count them (T-5118); `None` when
/// the usage database does not answer, so the rows still show and the count reads as unknown.
async fn calls_7d(state: &AppState, project: &str) -> Option<HashMap<String, u64>> {
    let (from, to) = crate::endpoint_usage::window(chrono::Utc::now().date_naive(), 7);
    match state.endpoint_usage.read(project, from, to).await {
        Ok(rows) => Some(rows.iter().fold(HashMap::new(), |mut totals, row| {
            *totals.entry(row.name.clone()).or_insert(0) += row.calls;
            totals
        })),
        Err(e) => {
            tracing::warn!(error = %e, project, "the OData endpointList rows carry no calls: the usage rows were not read");
            None
        }
    }
}

/// The rows of `project` the caller may read, in the page's order: own Endpoints, then those
/// other projects share with it, then references whose Endpoint went away (T-6169).
pub async fn rows(
    state: &AppState,
    identity: &Identity,
    project: &str,
) -> Result<Vec<Value>, ApiError> {
    let effective = gate(state, identity, project)?;
    let calls = calls_7d(state, project).await;
    let references = readable_of(state, &effective, project, "SharedSpaceReference");
    let mut rows: Vec<Value> = readable_of(state, &effective, project, KIND)
        .iter()
        .map(|env| {
            let counted = calls
                .as_ref()
                .map(|calls| calls.get(&env.metadata.name).copied().unwrap_or(0));
            endpoint_row(env, project, OWN, None, counted)
        })
        .collect();
    let mut used = HashSet::new();
    for source in state.mirror.namespaces() {
        if source == project || !crate::api::projects::readable(state, identity, &source) {
            continue;
        }
        let theirs = crate::permissions::for_request(state, identity, &source);
        for env in readable_of(state, &theirs, &source, KIND) {
            if !admits(&env, &source, project) {
                continue;
            }
            let reference = reference_to(&references, &env, &source);
            if let Some(reference) = reference {
                used.insert(reference.metadata.name.clone());
            }
            let name = reference.map(|r| r.metadata.name.as_str());
            rows.push(endpoint_row(&env, &source, SHARED, name, None));
        }
    }
    rows.extend(
        references
            .iter()
            .filter(|reference| !used.contains(&reference.metadata.name))
            .map(gone_row),
    );
    Ok(rows)
}

/// What `$allowed` and the write doors decide about one call, from the decision the proposal path
/// runs, so the answer never drifts from the call: `not_found` for a key the caller does not
/// read, which reads like a missing one.
#[derive(Debug, PartialEq)]
pub struct Decision {
    pub allowed: bool,
    pub reason: Option<String>,
    pub not_found: bool,
}

impl Decision {
    fn yes() -> Self {
        Decision {
            allowed: true,
            reason: None,
            not_found: false,
        }
    }

    fn no(reason: impl Into<String>) -> Self {
        Decision {
            allowed: false,
            reason: Some(reason.into()),
            not_found: false,
        }
    }

    fn of(check: Result<(), ApiError>) -> Self {
        match check {
            Ok(()) => Decision::yes(),
            Err(ApiError::Denied(m) | ApiError::NotFound(m) | ApiError::BadRequest(m)) => {
                Decision::no(m)
            }
            Err(other) => Decision::no(other.to_string()),
        }
    }
}

fn str_of<'a>(row: &'a Value, name: &str) -> Option<&'a str> {
    row.get(name).and_then(Value::as_str)
}

/// Whether the caller may `op` the row keyed `key` of `rows` (or the set, without a key).
pub fn decide(
    state: &AppState,
    identity: &Identity,
    project: &str,
    rows: &[Value],
    op: &str,
    key: Option<&str>,
) -> Decision {
    let row = match key {
        None => None,
        Some(key) => match rows.iter().find(|row| str_of(row, KEY) == Some(key)) {
            Some(row) => Some(row),
            None => {
                return Decision {
                    allowed: false,
                    reason: Some(format!("no Endpoint '{key}' in project {project}")),
                    not_found: true,
                }
            }
        },
    };
    match op {
        "read" => Decision::yes(),
        "export" => Decision::no(
            "the list serves no $export; read the rows with $top and $skip, or use Export on the Endpoints page",
        ),
        "create" => Decision::no(
            "an Endpoint is created with New endpoint on the Endpoints page, which checks the whole manifest",
        ),
        "update" | "delete" => {
            let Some(row) = row else {
                return Decision::no(format!("name the row: {op} takes a key"));
            };
            if let Some(refused) = elsewhere(row) {
                return refused;
            }
            let name = str_of(row, "name").unwrap_or_default();
            let Some(manifest) = state
                .mirror
                .get(project, KIND, name)
                .and_then(|env| serde_json::to_value(env).ok())
            else {
                return Decision::no(format!(
                    "Endpoint '{name}' is no longer in project {project}; reload the list"
                ));
            };
            Decision::of(if op == "update" {
                crate::groups::for_proposal(state, identity, project, KIND, true, &manifest).check(
                    KIND,
                    Verb::Propose,
                    Some(&manifest),
                )
            } else {
                crate::permissions::for_request(state, identity, project).check(
                    KIND,
                    Verb::Delete,
                    Some(&manifest),
                )
            })
        }
        other => Decision::no(format!(
            "op is one of read, export, create, update and delete, not '{other}'"
        )),
    }
}

/// The refusal of a write on a row this project does not change: a shared row is changed by its
/// publisher, a gone row has only its reference left.
fn elsewhere(row: &Value) -> Option<Decision> {
    match (str_of(row, "kind"), str_of(row, "source")) {
        (Some(OWN), _) => None,
        (Some(SHARED), Some(source)) => Some(Decision::no(format!(
            "project {source} publishes this Endpoint: it is changed there; Stop using in the row menu removes this project's reference"
        ))),
        _ => Some(Decision::no(
            "its Endpoint is gone and only this project's reference is left: Stop using in the row menu removes it",
        )),
    }
}

/// The merge patch of an OData `PATCH` body: only the properties the row edits (`EDITABLE`), each of its type; the
/// refusal is a sentence for a 400.
pub fn spec_patch(body: &[u8]) -> Result<Value, String> {
    let Ok(Value::Object(fields)) = serde_json::from_slice::<Value>(body) else {
        return Err("the body is a JSON object of the properties to change, e.g. {\"audience\": \"public\"}".into());
    };
    let mut spec = Map::new();
    for (name, value) in fields {
        if name.starts_with('@') {
            continue;
        }
        let Some((_, field)) = EDITABLE.iter().find(|(n, _)| *n == name) else {
            return Err(format!(
                "{name} is not changed here: the row changes {}; edit the rest in the Endpoint's form",
                EDITABLE.map(|(n, _)| n).join(", ")
            ));
        };
        let (fits, wanted) = if name == "mcp" {
            (value.is_boolean(), "true or false")
        } else {
            (
                value.as_str().is_some_and(|s| !s.is_empty()),
                "a non-empty text",
            )
        };
        if !fits {
            return Err(format!("{name} takes {wanted}"));
        }
        spec.insert((*field).to_owned(), value);
    }
    if spec.is_empty() {
        return Err(
            "the body changes nothing: name at least one property, e.g. {\"mcp\": false}".into(),
        );
    }
    Ok(json!({ "spec": spec }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_patch_changes_only_the_editable_properties_each_of_its_type() {
        assert_eq!(
            spec_patch(br#"{"audience":"public","mcp":false,"@odata.etag":"x"}"#),
            Ok(json!({ "spec": { "audience": "public", "mcp": false } }))
        );
        assert_eq!(
            spec_patch(br#"{"contextSpace":"air"}"#),
            Ok(json!({ "spec": { "contextSpaceRef": "air" } }))
        );
        for (body, says) in [
            (&br#"{"title":"x"}"#[..], "title is not changed here"),
            (br#"{"mcp":"no"}"#, "mcp takes true or false"),
            (br#"{"audience":""}"#, "audience takes a non-empty text"),
            (br#"{}"#, "changes nothing"),
            (br#"[1]"#, "a JSON object"),
        ] {
            let Err(message) = spec_patch(body) else {
                panic!("{body:?} was taken");
            };
            assert!(message.contains(says), "{message}");
        }
    }

    fn manifest(name: &str, spec: Value) -> ResourceEnvelope {
        serde_json::from_value(json!({
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": "Endpoint",
            "metadata": { "name": name, "namespace": "espoo" },
            "spec": spec,
        }))
        .unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn a_project_reads_what_another_shares_with_it_by_the_gateways_rule() {
        let open = manifest("open", json!({ "audience": "public" }));
        let org = manifest("org", json!({ "audience": "organization" }));
        let listed = manifest(
            "listed",
            json!({ "audience": "project-list", "allowedProjects": ["p"] }),
        );
        let unlisted = manifest("unlisted", json!({ "allowedProjects": ["q"] }));
        let own = manifest("own", json!({ "audience": "project" }));
        assert!(admits(&open, "espoo", "p"));
        assert!(admits(&org, "espoo", "p"));
        assert!(admits(&listed, "espoo", "p"));
        assert!(!admits(&unlisted, "espoo", "p"));
        assert!(!admits(&own, "espoo", "p"));
        assert!(admits(&own, "p", "p"));
    }

    #[test]
    fn a_reference_finds_its_endpoint_by_ref_or_else_by_slug() {
        let endpoint = manifest("air", json!({ "slug": "s1" }));
        let by_ref = manifest(
            "r1",
            json!({ "endpointRef": { "project": "espoo", "name": "air" } }),
        );
        let other_project = manifest(
            "r2",
            json!({ "endpointRef": { "project": "vantaa", "name": "air" } }),
        );
        let by_slug = manifest("r3", json!({ "endpointSlug": "s1" }));
        let refs = [other_project.clone(), by_ref];
        assert_eq!(
            reference_to(&refs, &endpoint, "espoo").map(|r| r.metadata.name.as_str()),
            Some("r1")
        );
        assert_eq!(
            reference_to(&[other_project, by_slug], &endpoint, "espoo")
                .map(|r| r.metadata.name.as_str()),
            Some("r3")
        );
        let unslugged = manifest("air", json!({}));
        assert!(reference_to(
            &[manifest("r4", json!({ "endpointSlug": "" }))],
            &unslugged,
            "espoo"
        )
        .is_none());
    }

    #[test]
    fn a_shared_and_a_gone_row_say_where_they_are_changed() {
        let shared = endpoint_row(
            &manifest("air", json!({ "slug": "s1" })),
            "espoo",
            SHARED,
            Some("r1"),
            None,
        );
        assert_eq!(str_of(&shared, "id"), Some("espoo/air"));
        assert_eq!(str_of(&shared, "reference"), Some("r1"));
        assert_eq!(shared.get("calls7d"), Some(&Value::Null));
        assert!(elsewhere(&shared).is_some_and(|d| d
            .reason
            .is_some_and(|r| r.contains("project espoo publishes"))));
        let gone = gone_row(&manifest(
            "r9",
            json!({ "alias": "Old air", "endpointSlug": "s9" }),
        ));
        assert_eq!(str_of(&gone, "id"), Some("reference:r9"));
        assert_eq!(str_of(&gone, "title"), Some("Old air"));
        assert_eq!(str_of(&gone, "search"), Some("r9\nold air"));
        assert_eq!(str_of(&gone, "kind"), Some(GONE));
        assert!(elsewhere(&gone).is_some());
        let own = endpoint_row(&manifest("air", json!({})), "p", OWN, None, Some(3));
        assert_eq!(str_of(&own, "id"), Some("air"));
        assert_eq!(own.get("calls7d"), Some(&json!(3)));
        assert!(elsewhere(&own).is_none());
        let names: Vec<String> = fields().into_iter().map(|f| f.name).collect();
        let keys: Vec<&String> = own
            .as_object()
            .map(|o| o.keys().collect())
            .unwrap_or_default();
        assert_eq!(keys.len(), names.len(), "a row carries every property");
    }
}
