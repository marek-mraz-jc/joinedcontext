//! `POST /api/v1/projects/{project}/catalogue/drafts`: what the one-step publish flow proposes
//! for an Endpoint (EP-83, Architecture/21 §7, API/01 §16b).
//!
//! It writes nothing. The flow shows the draft, a person edits it, and the UI proposes one Change
//! on the Endpoint through the resource API with the two blocks merged in; that Change is the
//! only write, and the publication stays the reconciler's (EP-62).

use std::collections::{BTreeMap, BTreeSet};

use axum::extract::{Path, State};
use axum::routing::post;
use axum::{Json, Router};
use jc_core::kinds::catalog::Catalog;
use jc_core::kinds::Verb;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use utoipa::ToSchema;

use crate::auth::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::permissions::ORG_NAMESPACE;
use crate::state::AppState;
use crate::store::ListOptions;

/// The licence a draft names when nothing else does (Architecture/21 §7).
const DEFAULT_LICENCE: &str = "CC_BY_4_0";
/// The most keywords a draft proposes.
const KEYWORDS: usize = 20;
/// The catalogue members a person may still have to fill, in the order the form shows them.
const MEMBERS: [&str; 8] = [
    "publisher",
    "contactPoint",
    "license",
    "themes",
    "keywords",
    "spatial",
    "temporal",
    "frequency",
];

/// Which Endpoint of the project to draft for.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CatalogueDraftRequest {
    /// The Endpoint's `metadata.name`.
    pub endpoint: String,
}

/// The two blocks the flow proposes, and what nothing could fill.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CatalogueDraft {
    pub endpoint: String,
    /// The Endpoint's audience is not `public`: the proposal makes it public, in the red lane.
    pub makes_public: bool,
    /// The drafted `spec.catalog` (EP-78), or the one the Endpoint already declares.
    #[schema(value_type = Object)]
    pub catalog: Value,
    /// The drafted `spec.publish` (EP-62), or the one the Endpoint already declares.
    #[schema(value_type = Object)]
    pub publish: Value,
    /// Catalogue members nothing could fill.
    pub missing: Vec<String>,
    /// The model's attributes per type, and whether the public dataset can carry each one.
    pub attributes: Vec<DraftAttribute>,
}

/// Why an attribute stays out of a public dataset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum Withheld {
    /// The model marks it `personalData` (DM-122): a public projection of it is refused.
    Personal,
    /// The Endpoint's projection hides it already (EP-61).
    Hidden,
}

/// One attribute of one type the Endpoint serves, as the publish form lists it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DraftAttribute {
    /// The class (entity type) the attribute belongs to.
    #[serde(rename = "type")]
    pub type_name: String,
    pub name: String,
    /// Why it is not offered; absent when the public dataset may carry it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub withheld: Option<Withheld>,
}

/// What the platform knows that a draft is made from; gathered from the mirror and the forge by
/// the route, so [`draft`] is a pure function every rule is tested on.
#[derive(Debug, Default)]
pub struct Known {
    /// The Endpoint's `spec`.
    pub endpoint: Value,
    /// The LinkML source of the space's model, when the forge gave it.
    pub linkml: Option<String>,
    /// The model's classes, from its manifest.
    pub classes: Vec<String>,
    /// The space's language, else the organization's.
    pub language: String,
    /// The project's title per language: the CKAN organization the dataset lands in is the
    /// project's (Architecture/21 §1), so it is the publisher.
    pub publisher: BTreeMap<String, String>,
    /// The Organization's `open-data` contact: `(name, email)` (EP-80).
    pub contact: Option<(String, String)>,
    /// The `CkanInstance` the project publishes to.
    pub instance: Option<String>,
}

/// Smart Data Models domains whose EU data theme is plain; any other domain names none.
const DOMAIN_THEMES: [(&str, &str); 14] = [
    ("Agrifood", "AGRI"),
    ("Aquaculture", "AGRI"),
    ("Battery", "ENER"),
    ("Building", "REGI"),
    ("Device", "TECH"),
    ("Energy", "ENER"),
    ("Environment", "ENVI"),
    ("KeyPerformanceIndicator", "ECON"),
    ("Parking", "TRAN"),
    ("PointOfInterest", "REGI"),
    ("Transportation", "TRAN"),
    ("UrbanMobility", "TRAN"),
    ("WaterQuality", "ENVI"),
    ("Weather", "ENVI"),
];

/// The EU themes of the Smart Data Models domains the model's IRIs name (`dataModel.Weather/…`).
pub fn themes(linkml: &str) -> Vec<String> {
    let mut found = BTreeSet::new();
    for (at, _) in linkml.match_indices("dataModel.") {
        let domain: String = linkml[at + "dataModel.".len()..]
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect();
        if let Some((_, theme)) = DOMAIN_THEMES.iter().find(|(d, _)| *d == domain) {
            found.insert((*theme).to_owned());
        }
    }
    found.into_iter().collect()
}

/// A LinkML title in `language`: the entry of a map, a plain string, else nothing.
fn title_in(node: &serde_norway::Value, language: &str) -> Option<String> {
    node.as_str()
        .or_else(|| node[language].as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// `KeyPerformanceIndicator` → `key performance indicator`, `observed_at` → `observed at`.
fn words(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c == '_' || c == '-' {
            out.push(' ');
        } else if c.is_uppercase() && i > 0 && !out.ends_with(' ') {
            out.push(' ');
            out.extend(c.to_lowercase());
        } else {
            out.extend(c.to_lowercase());
        }
    }
    out
}

/// Keywords from the model's classes and their slots, in the space's language: a slot's or a
/// class's title in that language where the model gives one, else its name in words.
pub fn keywords(linkml: &str, classes: &[String], language: &str) -> Vec<String> {
    let model = serde_norway::from_str::<serde_norway::Value>(linkml).unwrap_or_default();
    let mut out: Vec<String> = Vec::new();
    let mut push = |word: String| {
        let word = word.to_lowercase();
        if !word.is_empty() && !out.contains(&word) && out.len() < KEYWORDS {
            out.push(word);
        }
    };
    for class in classes {
        let node = &model["classes"][class.as_str()];
        push(title_in(&node["title"], language).unwrap_or_else(|| words(class)));
        for slot in node["slots"].as_sequence().into_iter().flatten() {
            let Some(slot) = slot.as_str() else { continue };
            // Identity and time are on every entity; they say nothing about this dataset.
            if matches!(
                slot,
                "id" | "type" | "observedAt" | "observed_at" | "dateObserved"
            ) {
                continue;
            }
            push(title_in(&model["slots"][slot]["title"], language).unwrap_or_else(|| words(slot)));
        }
    }
    out
}

/// Every attribute of `classes` with the reason a public dataset leaves it out (DM-122, EP-61):
/// a slot, a `slot_usage` or an `attributes` entry annotated `personalData`, or a whole class so
/// marked, is personal; a name in the projection's `hiddenAttributes` is hidden already.
pub fn attributes(linkml: &str, classes: &[String], hidden: &[String]) -> Vec<DraftAttribute> {
    use serde_norway::Value as Yaml;
    let model = serde_norway::from_str::<Yaml>(linkml).unwrap_or_default();
    let personal = |node: &Yaml| {
        let mark = &node["annotations"]["personalData"];
        let mark = if mark.get("value").is_some() {
            &mark["value"]
        } else {
            mark
        };
        mark.as_bool() == Some(true)
            || mark
                .as_str()
                .is_some_and(|s| s.eq_ignore_ascii_case("true"))
    };
    let mut out = Vec::new();
    for class in classes {
        let node = &model["classes"][class.as_str()];
        let listed = node["slots"]
            .as_sequence()
            .into_iter()
            .flatten()
            .filter_map(Yaml::as_str);
        let own = node["attributes"]
            .as_mapping()
            .into_iter()
            .flatten()
            .filter_map(|(k, _)| k.as_str());
        let mut seen = BTreeSet::new();
        for slot in listed.chain(own) {
            // Identity and type are on every entity and served whatever a projection says.
            if matches!(slot, "id" | "type") || !seen.insert(slot) {
                continue;
            }
            let withheld = if personal(node)
                || personal(&model["slots"][slot])
                || personal(&node["slot_usage"][slot])
                || personal(&node["attributes"][slot])
            {
                Some(Withheld::Personal)
            } else if hidden.iter().any(|h| h == slot) {
                Some(Withheld::Hidden)
            } else {
                None
            };
            out.push(DraftAttribute {
                type_name: class.clone(),
                name: slot.to_owned(),
                withheld,
            });
        }
    }
    out
}

/// The draft for Endpoint `name` (EP-83): a block the Endpoint already declares is returned as
/// it is, so running the flow again never overwrites what a steward wrote.
pub fn draft(name: &str, known: &Known) -> CatalogueDraft {
    let makes_public = known.endpoint["audience"].as_str() != Some("public");
    let catalog = match known.endpoint.get("catalog") {
        Some(existing @ Value::Object(_)) => existing.clone(),
        _ => drafted_catalog(known),
    };
    let publish = match known.endpoint.get("publish") {
        Some(existing) if existing.get("ckan").is_some() => {
            let mut existing = existing.clone();
            // The one member dropped from a declared block: jc-core refuses the removed row
            // mirror (T-5060), so the draft a steward submits must not carry it back.
            if let Some(ckan) = existing.get_mut("ckan").and_then(Value::as_object_mut) {
                ckan.remove("datastore");
            }
            existing
        }
        _ => match &known.instance {
            Some(instance) => {
                json!({ "ckan": { "instanceRef": { "kind": "CkanInstance", "name": instance } } })
            }
            None => json!({}),
        },
    };
    let missing = MEMBERS
        .iter()
        .filter(|member| match catalog.get(**member) {
            None | Some(Value::Null) => true,
            Some(Value::Array(items)) => items.is_empty(),
            Some(Value::Object(fields)) => fields.is_empty(),
            Some(_) => false,
        })
        .map(|member| (*member).to_owned())
        .collect();
    let hidden: Vec<String> = known.endpoint["projection"]["hiddenAttributes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|h| h.as_str().map(str::to_owned))
        .collect();
    CatalogueDraft {
        endpoint: name.to_owned(),
        makes_public,
        catalog,
        publish,
        missing,
        attributes: known
            .linkml
            .as_deref()
            .map(|linkml| attributes(linkml, &known.classes, &hidden))
            .unwrap_or_default(),
    }
}

fn drafted_catalog(known: &Known) -> Value {
    let mut catalog = Map::new();
    if !known.publisher.is_empty() {
        catalog.insert("publisher".into(), json!({ "name": known.publisher }));
    }
    if let Some((name, email)) = &known.contact {
        catalog.insert(
            "contactPoint".into(),
            json!({ "name": name, "email": email }),
        );
    }
    catalog.insert("license".into(), json!(DEFAULT_LICENCE));
    if let Some(linkml) = known.linkml.as_deref() {
        let themes = themes(linkml);
        if !themes.is_empty() {
            catalog.insert("themes".into(), json!(themes));
        }
        let keywords = keywords(linkml, &known.classes, &known.language);
        if !keywords.is_empty() {
            catalog.insert(
                "keywords".into(),
                json!({ known.language.as_str(): keywords }),
            );
        }
    }
    let catalog = Value::Object(catalog);
    // Only what the manifest itself accepts is ever proposed: a member the table does not know
    // is dropped here rather than failing the proposal the person makes from it.
    match serde_json::from_value::<Catalog>(catalog.clone()) {
        Ok(_) => catalog,
        Err(err) => {
            tracing::warn!(error = %err, "drafted catalogue block does not parse; proposing the licence alone");
            json!({ "license": DEFAULT_LICENCE })
        }
    }
}

/// A title as its manifest holds it (one string or a map per language), as a map.
fn texts(title: Option<&Value>, language: &str) -> BTreeMap<String, String> {
    match title {
        Some(Value::String(s)) if !s.trim().is_empty() => {
            BTreeMap::from([(language.to_owned(), s.trim().to_owned())])
        }
        Some(Value::Object(map)) => map
            .iter()
            .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.trim().to_owned())))
            .filter(|(_, v)| !v.is_empty())
            .collect(),
        _ => BTreeMap::new(),
    }
}

async fn known(state: &AppState, project: &str, endpoint: &Value) -> Known {
    let organization = state
        .mirror
        .list(ORG_NAMESPACE, "Organization", &ListOptions::default())
        .items
        .into_iter()
        .next();
    let space = crate::resource::reference_name(&endpoint["contextSpaceRef"])
        .and_then(|space| state.mirror.get(project, "ContextSpace", space));
    let language = space
        .as_ref()
        .and_then(|s| s.spec["defaultLocale"].as_str())
        .or_else(|| organization.as_ref()?.spec["defaultLocale"].as_str())
        .unwrap_or("en")
        .to_owned();
    let model = space
        .as_ref()
        .and_then(|s| crate::resource::reference_name(&s.spec["dataModelRef"]).map(str::to_owned));
    let classes = model
        .as_deref()
        .and_then(|m| state.mirror.get(project, "DataModel", m))
        .map(|m| {
            m.spec["classes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|c| c.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let linkml = match model.as_deref() {
        Some(m) => crate::api::datamodels::read_source(state, project, m)
            .await
            .ok(),
        None => None,
    };
    let project_manifest = state.mirror.get(ORG_NAMESPACE, "Project", project);
    let mut publisher = texts(
        project_manifest
            .as_ref()
            .and_then(|p| serde_json::to_value(p.metadata.title.as_ref()?).ok())
            .as_ref(),
        &language,
    );
    if publisher.is_empty() {
        publisher.insert(language.clone(), project.to_owned());
    }
    let contact = organization.as_ref().and_then(|o| {
        o.spec["contacts"].as_array()?.iter().find_map(|c| {
            (c["role"].as_str() == Some("open-data")).then(|| {
                Some((
                    c["name"].as_str()?.to_owned(),
                    c["email"].as_str()?.to_owned(),
                ))
            })?
        })
    });
    let mut instances: Vec<(bool, String)> = state
        .mirror
        .list(project, "CkanInstance", &ListOptions::default())
        .items
        .into_iter()
        .map(|i| {
            let own = i.spec["organizationDefault"].as_str() == Some(project);
            (!own, i.metadata.name)
        })
        .collect();
    // The instance whose default organization is this project first, then by name.
    instances.sort();
    Known {
        endpoint: endpoint.clone(),
        linkml,
        classes,
        language,
        publisher,
        contact,
        instance: instances.into_iter().next().map(|(_, name)| name),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/catalogue/drafts",
    summary = "Draft a Catalogue Publication",
    description = "What the one-step publish flow proposes for an Endpoint: its `spec.catalog` drafted from the model and the organization, its `spec.publish.ckan`, and whether publishing makes it public (EP-83). Writes nothing.",
    tag = "ckan",
    params(("project" = String, Path, description = "Project slug")),
    request_body(content = CatalogueDraftRequest, example = json!({ "endpoint": "bbsk-kpi" })),
    responses(
        (status = 200, description = "The draft", body = CatalogueDraft),
        (status = 400, description = "The body names no Endpoint", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "No grant proposes an Endpoint in this project", body = ProblemDetails),
        (status = 404, description = "No such Endpoint in this project", body = ProblemDetails),
        (status = 409, description = "The project has no CkanInstance to publish to", body = ProblemDetails),
    )
)]
pub async fn draft_publication(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
    Json(request): Json<CatalogueDraftRequest>,
) -> Result<Json<CatalogueDraft>, ApiError> {
    let name = request.endpoint.trim();
    if name.is_empty() || name.len() > 63 {
        return Err(ApiError::BadRequest(
            "endpoint: name the Endpoint to publish".into(),
        ));
    }
    // Who may ask, before anything of the project is looked up (PF-59, R20): a project the caller
    // does not read answers like one that is not there, and a caller who reads it but not its
    // Endpoints gets the answer of a missing name, so no status tells an Endpoint name apart.
    let effective = crate::permissions::for_request(&state, &user.0.identity, &project);
    if !effective.may_read_project() {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    let missing = || {
        ApiError::NotFound(format!(
            "endpoint '{name}' not found in project '{project}'"
        ))
    };
    if !effective.may_read("Endpoint") {
        return Err(missing());
    }
    let endpoint = state
        .mirror
        .get(&project, "Endpoint", name)
        .ok_or_else(missing)?;
    effective.check(
        "Endpoint",
        Verb::Propose,
        Some(&serde_json::to_value(&endpoint).unwrap_or(Value::Null)),
    )?;
    let known = known(&state, &project, &endpoint.spec).await;
    let drafted = draft(name, &known);
    if drafted.publish.get("ckan").is_none() {
        return Err(ApiError::Conflict(format!(
            "project '{project}' has no CkanInstance to publish to; add one on its Open data page first"
        )));
    }
    Ok(Json(drafted))
}

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/projects/{project}/catalogue/drafts",
        post(draft_publication),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINKML: &str = r#"
id: https://example.org/bbsk
prefixes:
  sdm: https://smartdatamodels.org/dataModel.KeyPerformanceIndicator/
classes:
  KeyPerformanceIndicator:
    class_uri: https://smartdatamodels.org/dataModel.KeyPerformanceIndicator/KeyPerformanceIndicator
    title:
      sk: Ukazovateľ
      en: Indicator
    slots: [id, kpiValue, observedAt, calculationMethod]
  Station:
    class_uri: https://smartdatamodels.org/dataModel.Environment/AirQualityObserved
    slots: [dataProvider]
slots:
  kpiValue:
    title:
      sk: Hodnota
  calculationMethod: {}
  dataProvider: {}
"#;

    fn known() -> Known {
        Known {
            endpoint: json!({ "audience": "organization", "contextSpaceRef": "kpi" }),
            linkml: Some(LINKML.to_owned()),
            classes: vec!["KeyPerformanceIndicator".into(), "Station".into()],
            language: "sk".into(),
            publisher: BTreeMap::from([("sk".into(), "Banskobystrický samosprávny kraj".into())]),
            contact: Some(("Open data desk".into(), "opendata@example.org".into())),
            instance: Some("bbsk".into()),
        }
    }

    #[test]
    fn a_personal_slot_is_withheld_and_a_hidden_one_named() {
        let linkml = r#"
classes:
  Visit:
    slots: [id, visitor, count, place]
    slot_usage:
      place:
        annotations: { personalData: { tag: personalData, value: "true" } }
    attributes:
      note:
        annotations: { personalData: true }
  Person:
    annotations: { personalData: true }
    slots: [age]
slots:
  visitor:
    annotations: { personalData: true }
"#;
        let found = attributes(
            linkml,
            &["Visit".into(), "Person".into(), "Missing".into()],
            &["count".into()],
        );
        let of = |name: &str| found.iter().find(|a| a.name == name).map(|a| a.withheld);
        assert_eq!(of("visitor"), Some(Some(Withheld::Personal)));
        assert_eq!(of("place"), Some(Some(Withheld::Personal)));
        assert_eq!(of("note"), Some(Some(Withheld::Personal)));
        assert_eq!(of("age"), Some(Some(Withheld::Personal)));
        assert_eq!(of("count"), Some(Some(Withheld::Hidden)));
        assert_eq!(of("id"), None, "identity is no attribute a person picks");
        assert_eq!(found.len(), 5);
        assert!(attributes(": broken [", &["Visit".into()], &[]).is_empty());
    }

    #[test]
    fn the_draft_offers_the_public_attributes_and_drops_a_removed_mirror() {
        let mut known = known();
        known.endpoint["projection"] = json!({ "hiddenAttributes": ["calculationMethod"] });
        let draft = draft("bbsk-kpi", &known);
        assert_eq!(
            draft.publish["ckan"],
            json!({ "instanceRef": { "kind": "CkanInstance", "name": "bbsk" } })
        );
        let offered: Vec<&str> = draft
            .attributes
            .iter()
            .filter(|a| a.withheld.is_none())
            .map(|a| a.name.as_str())
            .collect();
        assert_eq!(offered, vec!["kpiValue", "observedAt", "dataProvider"]);
        // What the Endpoint already declares is kept, but the removed row mirror (T-5060).
        known.endpoint["publish"] = json!({ "ckan": {
            "instanceRef": "bbsk", "name": "kpi", "datastore": { "representation": "csv" }
        } });
        assert_eq!(
            super::draft("bbsk-kpi", &known).publish,
            json!({ "ckan": { "instanceRef": "bbsk", "name": "kpi" } })
        );
    }

    #[test]
    fn the_draft_fills_what_the_model_and_the_organization_know_and_names_the_rest() {
        let draft = draft("bbsk-kpi", &known());
        assert!(draft.makes_public);
        assert_eq!(
            draft.catalog,
            json!({
                "publisher": { "name": { "sk": "Banskobystrický samosprávny kraj" } },
                "contactPoint": { "name": "Open data desk", "email": "opendata@example.org" },
                "license": "CC_BY_4_0",
                "themes": ["ECON", "ENVI"],
                "keywords": { "sk": ["ukazovateľ", "hodnota", "calculation method", "station", "data provider"] },
            })
        );
        assert_eq!(
            draft.publish,
            json!({ "ckan": { "instanceRef": { "kind": "CkanInstance", "name": "bbsk" } } })
        );
        assert_eq!(draft.missing, vec!["spatial", "temporal", "frequency"]);
        // The drafted block is one the Endpoint manifest accepts.
        serde_json::from_value::<Catalog>(draft.catalog).expect("a valid spec.catalog");
    }

    #[test]
    fn what_the_endpoint_already_declares_is_kept_as_it_is() {
        let mut known = known();
        known.endpoint = json!({
            "audience": "public",
            "catalog": { "license": "CC0", "themes": [] },
            "publish": { "ckan": { "instanceRef": "other", "license": "cc-zero" } },
        });
        let draft = draft("bbsk-kpi", &known);
        assert!(!draft.makes_public);
        assert_eq!(draft.catalog, json!({ "license": "CC0", "themes": [] }));
        assert_eq!(draft.publish["ckan"]["instanceRef"], "other");
        assert!(draft.missing.contains(&"themes".to_owned()));
        assert!(draft.missing.contains(&"publisher".to_owned()));
    }

    #[test]
    fn with_no_model_no_contact_and_no_catalogue_the_draft_is_the_licence_and_nothing_to_publish_to(
    ) {
        let draft = draft(
            "x",
            &Known {
                endpoint: json!({ "audience": "public" }),
                language: "en".into(),
                ..Known::default()
            },
        );
        assert_eq!(draft.catalog, json!({ "license": "CC_BY_4_0" }));
        assert_eq!(draft.publish, json!({}));
        assert_eq!(
            draft.missing,
            vec![
                "publisher",
                "contactPoint",
                "themes",
                "keywords",
                "spatial",
                "temporal",
                "frequency"
            ]
        );
    }

    #[test]
    fn a_domain_outside_the_table_names_no_theme_and_a_broken_source_no_keyword() {
        assert!(themes("class_uri: https://smartdatamodels.org/dataModel.Alert/Alert").is_empty());
        assert!(themes("").is_empty());
        assert_eq!(
            themes("dataModel.Weather/A dataModel.Weather/B dataModel.Parking/C"),
            vec!["ENVI", "TRAN"]
        );
        assert!(keywords(": not [ yaml", &["X".into()], "en") == vec!["x".to_owned()]);
        assert_eq!(
            words("KeyPerformanceIndicator"),
            "key performance indicator"
        );
        assert_eq!(words("observed_at"), "observed at");
    }

    #[test]
    fn keywords_stop_at_twenty() {
        let slots: Vec<String> = (0..40).map(|i| format!("slot{i}")).collect();
        let linkml = format!("classes:\n  A:\n    slots: [{}]\n", slots.join(", "));
        assert_eq!(keywords(&linkml, &["A".into()], "en").len(), KEYWORDS);
    }

    #[test]
    fn a_title_is_a_map_whatever_the_manifest_holds() {
        assert_eq!(
            texts(Some(&json!("Kraj")), "sk"),
            BTreeMap::from([("sk".into(), "Kraj".into())])
        );
        assert_eq!(
            texts(Some(&json!({ "sk": "Kraj", "en": " " })), "sk"),
            BTreeMap::from([("sk".into(), "Kraj".into())])
        );
        assert!(texts(None, "sk").is_empty());
        assert!(texts(Some(&json!("  ")), "sk").is_empty());
    }
}
