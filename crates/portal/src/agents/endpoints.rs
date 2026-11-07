//! The endpoints an application run reads (AP-44, SDK-02): one to five of the project, the
//! primary first. Each data need belongs to every listed endpoint whose context space is the
//! need's, the primary when none is; a type is read through each endpoint of the needs that name
//! it, and two endpoints of one space (an operations and a public one) join their rows by id.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use utoipa::ToSchema;

use crate::agents::run::AgentRun;
use crate::api::assistant::ref_name;
use crate::error::ApiError;
use crate::store::Mirror;

/// How many endpoints one application may read.
pub const MAX_ENDPOINTS: usize = 5;

/// One endpoint of a run: its manifest name, the slug the gateway addresses it by (EP-02) and
/// the context space it serves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunEndpoint {
    pub name: String,
    pub slug: String,
    pub space: String,
}

/// The endpoint names a create request asks for: `endpointNames`, or the one `endpointName`.
/// Both, neither, a duplicate or more than [`MAX_ENDPOINTS`] is a message for a `400`.
pub fn requested(one: Option<&str>, many: &[String]) -> Result<Vec<String>, String> {
    let names: Vec<String> = match (one, many.is_empty()) {
        (Some(_), false) => {
            return Err("give endpointName or endpointNames, not both".to_owned());
        }
        (None, true) => return Err("endpointNames must name at least one endpoint".to_owned()),
        (Some(name), true) => vec![name.to_owned()],
        (None, false) => many.to_vec(),
    };
    if names.len() > MAX_ENDPOINTS {
        return Err(format!(
            "an application reads at most {MAX_ENDPOINTS} endpoints, {} given",
            names.len()
        ));
    }
    for (i, name) in names.iter().enumerate() {
        if name.trim().is_empty() {
            return Err(format!("endpointNames[{i}] is empty"));
        }
        if names[..i].contains(name) {
            return Err(format!("endpoint '{name}' is named twice"));
        }
    }
    Ok(names)
}

/// Every name resolved from the mirror, in order. An endpoint the project does not have is a
/// `404`, one without a slug a `400` (EP-02).
pub fn resolve(
    mirror: &Mirror,
    project: &str,
    names: &[String],
) -> Result<Vec<RunEndpoint>, ApiError> {
    resolve_in(mirror, project, names, false)
}

/// [`resolve`] for a conversation, which also reads the Endpoints other projects share with
/// this one (`assistant::shared_endpoints`, T-6104). A build stays on the project's
/// own Endpoints: what it publishes is checked against them (AP-44).
pub fn resolve_readable(
    mirror: &Mirror,
    project: &str,
    names: &[String],
) -> Result<Vec<RunEndpoint>, ApiError> {
    resolve_in(mirror, project, names, true)
}

fn resolve_in(
    mirror: &Mirror,
    project: &str,
    names: &[String],
    shared: bool,
) -> Result<Vec<RunEndpoint>, ApiError> {
    names
        .iter()
        .map(|name| {
            let endpoint = mirror
                .get(project, "Endpoint", name)
                .or_else(|| {
                    if !shared {
                        return None;
                    }
                    crate::api::assistant::shared_endpoints(mirror, project)
                        .into_iter()
                        .map(|(_, endpoint)| endpoint)
                        .find(|endpoint| endpoint.metadata.name == *name)
                })
                .ok_or_else(|| {
                    ApiError::NotFound(format!(
                        "endpoint '{name}' not found in project '{project}'"
                    ))
                })?;
            let slug = endpoint
                .spec
                .get("slug")
                .and_then(Value::as_str)
                .filter(|slug| !slug.is_empty())
                .ok_or_else(|| {
                    ApiError::BadRequest(format!("endpoint '{name}' has no slug (EP-02)"))
                })?
                .to_owned();
            Ok(RunEndpoint {
                name: name.clone(),
                slug,
                space: ref_name(&endpoint.spec["contextSpaceRef"]).unwrap_or_default(),
            })
        })
        .collect()
}

/// The endpoints of a stored run; a run recorded before several were possible has its one.
pub fn of_run(run: &AgentRun) -> Vec<RunEndpoint> {
    let stored: Vec<RunEndpoint> =
        serde_json::from_value(run.endpoints.clone()).unwrap_or_default();
    if stored.is_empty() {
        // A conversation started without endpoints reads none.
        if run.endpoint_slug.is_empty() {
            return Vec::new();
        }
        return vec![RunEndpoint {
            name: run.endpoint_name.clone(),
            slug: run.endpoint_slug.clone(),
            space: String::new(),
        }];
    }
    stored
}

/// The indexes of the endpoints one data need belongs to: every endpoint of its context space,
/// the primary when none is.
pub fn of_need(endpoints: &[RunEndpoint], need: &Value) -> Vec<usize> {
    let space = ref_name(&need["contextSpaceRef"]);
    let at: Vec<usize> = endpoints
        .iter()
        .enumerate()
        .filter(|(_, endpoint)| {
            !endpoint.space.is_empty() && space.as_deref() == Some(endpoint.space.as_str())
        })
        .map(|(i, _)| i)
        .collect();
    if at.is_empty() {
        vec![0]
    } else {
        at
    }
}

/// The types each endpoint's needs name, aligned with `endpoints`, each once, in need order.
pub fn types_by_endpoint(endpoints: &[RunEndpoint], data_needs: &Value) -> Vec<Vec<String>> {
    let mut types = vec![Vec::<String>::new(); endpoints.len().max(1)];
    for need in data_needs.as_array().into_iter().flatten() {
        for at in of_need(endpoints, need) {
            for entity_type in need["types"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                if !types[at].iter().any(|known| known == entity_type) {
                    types[at].push(entity_type.to_owned());
                }
            }
        }
    }
    types
}

/// The indexes of the endpoints a type is read through, in endpoint order; the primary when no
/// need names the type.
pub fn serving(endpoints: &[RunEndpoint], data_needs: &Value, entity_type: &str) -> Vec<usize> {
    let at: Vec<usize> = types_by_endpoint(endpoints, data_needs)
        .iter()
        .enumerate()
        .filter(|(_, types)| types.iter().any(|known| known == entity_type))
        .map(|(i, _)| i)
        .collect();
    if at.is_empty() {
        vec![0]
    } else {
        at
    }
}

/// The endpoint a single read of a type goes through: the first that serves it.
pub fn of_type(endpoints: &[RunEndpoint], data_needs: &Value, entity_type: &str) -> usize {
    serving(endpoints, data_needs, entity_type)[0]
}

/// Adds the rows one more endpoint answered for a type to those already read: a row with a known
/// `id` gains the attributes it did not carry, a new `id` is a row of its own.
pub fn join_by_id(rows: &mut Vec<Value>, read: Vec<Value>) {
    for entity in read {
        let known = entity["id"]
            .as_str()
            .and_then(|id| rows.iter().position(|row| row["id"].as_str() == Some(id)));
        match (known, entity) {
            (Some(at), Value::Object(attributes)) => {
                if let Value::Object(row) = &mut rows[at] {
                    for (name, value) in attributes {
                        row.entry(name).or_insert(value);
                    }
                }
            }
            (_, entity) => rows.push(entity),
        }
    }
}

/// The attribute names the rows carry, besides `id` and `type`, each once, sorted.
pub fn attributes_of(rows: &[Value]) -> Vec<String> {
    let mut names: Vec<String> = rows
        .iter()
        .filter_map(Value::as_object)
        .flat_map(|row| row.keys())
        .filter(|name| !matches!(name.as_str(), "id" | "type" | "@context"))
        .cloned()
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Where one endpoint's data is read through the agent proxy (Architecture/19 §4): `/v1/data`
/// for the primary, `/v1/data/endpoints/{slug}` for the others.
pub fn data_base(proxy_base: &str, endpoints: &[RunEndpoint], index: usize) -> String {
    let proxy_base = proxy_base.trim_end_matches('/');
    match endpoints.get(index) {
        Some(endpoint) if index > 0 => format!("{proxy_base}/v1/data/endpoints/{}", endpoint.slug),
        _ => format!("{proxy_base}/v1/data"),
    }
}

/// The served configuration's `endpoints` (SDK-02): name, slug, space and the types the needs
/// read there.
pub fn config(endpoints: &[RunEndpoint], data_needs: &Value) -> Value {
    let types = types_by_endpoint(endpoints, data_needs);
    Value::Array(
        endpoints
            .iter()
            .zip(types)
            .map(|(endpoint, types)| {
                json!({
                    "name": endpoint.name,
                    "slug": endpoint.slug,
                    "space": endpoint.space,
                    "types": types,
                })
            })
            .collect(),
    )
}

/// The section of a code pack that says which endpoint serves which type (SDK-13); empty for a
/// run of one endpoint, whose pack reads as it always did.
pub fn pack_section(endpoints: &[RunEndpoint], data_needs: &Value) -> String {
    if endpoints.len() < 2 {
        return String::new();
    }
    let types = types_by_endpoint(endpoints, data_needs);
    let mut section = String::from(
        "## THE ENDPOINTS\n\nThis application reads several endpoints. Pass `{ endpoint: \"<name>\" }` \
         to a data call only for a type more than one of them serves; every other type is read \
         from the endpoint listed with it.\n\n",
    );
    for (i, (endpoint, types)) in endpoints.iter().zip(&types).enumerate() {
        section.push_str(&format!(
            "- `{}`{} — context space `{}`, types: {}\n",
            endpoint.name,
            if i == 0 { " (primary)" } else { "" },
            endpoint.space,
            if types.is_empty() {
                "none named by the data needs".to_owned()
            } else {
                types.join(", ")
            }
        ));
    }
    let mut shared: Vec<&String> = Vec::new();
    for (i, list) in types.iter().enumerate() {
        for entity_type in list {
            if types[i + 1..]
                .iter()
                .any(|later| later.contains(entity_type))
                && !shared.contains(&entity_type)
            {
                shared.push(entity_type);
            }
        }
    }
    if !shared.is_empty() {
        section.push_str(&format!(
            "\nServed by more than one endpoint, so every call names the endpoint; when they carry \
             different attributes of the same entities, read the type from each and join the rows \
             by `id`: {}\n",
            shared
                .iter()
                .map(|t| t.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    section.push('\n');
    section
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two() -> Vec<RunEndpoint> {
        vec![
            RunEndpoint {
                name: "transportation".into(),
                slug: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
                space: "transportation".into(),
            },
            RunEndpoint {
                name: "transportation-kpis".into(),
                slug: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
                space: "transportation-kpi".into(),
            },
        ]
    }

    fn needs() -> Value {
        json!([
            { "contextSpaceRef": { "kind": "ContextSpace", "name": "transportation" }, "types": ["Vehicle", "BusStop"], "operations": ["queryEntity"] },
            { "contextSpaceRef": { "kind": "ContextSpace", "name": "transportation-kpi" }, "types": ["KeyPerformanceIndicator"], "operations": ["queryEntity"] },
            { "contextSpaceRef": { "kind": "ContextSpace", "name": "elsewhere" }, "types": ["Alert"], "operations": ["queryEntity"] }
        ])
    }

    #[test]
    fn a_request_names_one_endpoint_or_a_list_never_both() {
        assert_eq!(requested(Some("a"), &[]).unwrap(), vec!["a"]);
        assert_eq!(
            requested(None, &["a".into(), "b".into()]).unwrap(),
            vec!["a", "b"]
        );
        assert!(requested(Some("a"), &["b".into()]).is_err());
        assert!(requested(None, &[]).is_err());
        assert!(requested(None, &["a".into(), "a".into()]).is_err());
        let six: Vec<String> = (0..6).map(|i| format!("e{i}")).collect();
        assert!(requested(None, &six).is_err());
    }

    #[test]
    fn a_need_belongs_to_the_endpoint_of_its_space_and_otherwise_to_the_primary() {
        let endpoints = two();
        let needs = needs();
        assert_eq!(of_need(&endpoints, &needs[0]), vec![0]);
        assert_eq!(of_need(&endpoints, &needs[1]), vec![1]);
        assert_eq!(of_need(&endpoints, &needs[2]), vec![0]);
        assert_eq!(of_type(&endpoints, &needs, "KeyPerformanceIndicator"), 1);
        assert_eq!(of_type(&endpoints, &needs, "Unknown"), 0);
        assert_eq!(
            types_by_endpoint(&endpoints, &needs),
            vec![
                vec![
                    "Vehicle".to_owned(),
                    "BusStop".to_owned(),
                    "Alert".to_owned()
                ],
                vec!["KeyPerformanceIndicator".to_owned()]
            ]
        );
    }

    #[test]
    fn the_primary_reads_under_v1_data_and_the_others_under_their_slug() {
        let endpoints = two();
        assert_eq!(
            data_base("http://proxy/", &endpoints, 0),
            "http://proxy/v1/data"
        );
        assert_eq!(
            data_base("http://proxy", &endpoints, 1),
            "http://proxy/v1/data/endpoints/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        );
        assert_eq!(
            data_base("http://proxy", &endpoints, 9),
            "http://proxy/v1/data"
        );
    }

    #[test]
    fn the_pack_lists_every_endpoint_and_the_types_two_of_them_serve() {
        let endpoints = two();
        let mut needs = needs();
        needs.as_array_mut().unwrap().push(json!({
            "contextSpaceRef": { "kind": "ContextSpace", "name": "transportation-kpi" },
            "types": ["Vehicle"], "operations": ["queryEntity"]
        }));
        let section = pack_section(&endpoints, &needs);
        assert!(section.contains("`transportation` (primary)"), "{section}");
        assert!(section.contains("`transportation-kpis` — context space `transportation-kpi`, types: KeyPerformanceIndicator, Vehicle"), "{section}");
        assert!(
            section.contains("join the rows by `id`: Vehicle"),
            "{section}"
        );
        assert_eq!(pack_section(&endpoints[..1], &needs), "");
        let config = config(&endpoints, &needs);
        assert_eq!(
            config[1]["types"],
            json!(["KeyPerformanceIndicator", "Vehicle"])
        );
        assert_eq!(config[0]["slug"], json!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));
    }

    fn one_space() -> Vec<RunEndpoint> {
        vec![
            RunEndpoint {
                name: "helsinki-bikes-ops".into(),
                slug: "cccccccccccccccccccccccccccccccc".into(),
                space: "helsinki".into(),
            },
            RunEndpoint {
                name: "helsinki-bikes".into(),
                slug: "dddddddddddddddddddddddddddddddd".into(),
                space: "helsinki".into(),
            },
        ]
    }

    #[test]
    fn two_endpoints_of_one_space_both_serve_its_types_and_the_pack_says_to_join_them() {
        let endpoints = one_space();
        let needs = json!([
            { "contextSpaceRef": { "kind": "ContextSpace", "name": "helsinki" }, "types": ["BikeHireDockingStation"], "operations": ["queryEntity"] }
        ]);
        assert_eq!(of_need(&endpoints, &needs[0]), vec![0, 1]);
        assert_eq!(
            serving(&endpoints, &needs, "BikeHireDockingStation"),
            vec![0, 1]
        );
        assert_eq!(of_type(&endpoints, &needs, "BikeHireDockingStation"), 0);
        assert_eq!(serving(&endpoints, &needs, "Unknown"), vec![0]);
        assert_eq!(serving(&[], &needs, "BikeHireDockingStation"), vec![0]);
        let config = config(&endpoints, &needs);
        assert_eq!(config[0]["types"], json!(["BikeHireDockingStation"]));
        assert_eq!(config[1]["types"], json!(["BikeHireDockingStation"]));
        let section = pack_section(&endpoints, &needs);
        assert!(
            section.contains("join the rows by `id`: BikeHireDockingStation"),
            "{section}"
        );
    }

    #[test]
    fn rows_of_several_endpoints_join_by_id_and_keep_what_was_read_first() {
        let mut rows = vec![
            json!({ "id": "urn:a", "type": "S", "status": "working" }),
            json!({ "id": "urn:b", "type": "S", "status": "outOfService" }),
        ];
        join_by_id(
            &mut rows,
            vec![
                json!({ "id": "urn:a", "type": "S", "name": "Kaivopuisto", "status": "ignored", "location": { "type": "Point", "coordinates": [24.95, 60.16] } }),
                json!({ "id": "urn:c", "type": "S", "name": "Laivasillankatu" }),
                json!("not an entity"),
            ],
        );
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0]["name"], json!("Kaivopuisto"));
        assert_eq!(rows[0]["status"], json!("working"));
        assert!(rows[0]["location"].is_object());
        assert_eq!(rows[1].get("name"), None);
        assert_eq!(rows[2]["id"], json!("urn:c"));
        assert_eq!(attributes_of(&rows), vec!["location", "name", "status"]);
        assert!(attributes_of(&[]).is_empty());
    }
}
