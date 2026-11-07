//! A public Endpoint's query standards, the fields each one filters on and two examples per
//! standard built from one real entity (API/01 §16a, TS-19, T-4484).
//!
//! Every read is anonymous, through the Endpoint on the gateway, so the answer holds what a
//! visitor without a token may read and nothing a view hides. Each example is run once and kept
//! only when it answers with at least one entity: the page never shows a query that fails, and
//! no example is written by hand.

use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};
use std::time::{Duration, Instant};

use axum::extract::{Path, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures_util::future::join_all;
use serde::Serialize;
use serde_json::Value;
use url::Url;
use utoipa::ToSchema;

use crate::api::catalogue::{endpoint_url, public_endpoint, served_representations};
use crate::api::endpoint_preview::encode;
use crate::error::ApiError;
use crate::state::AppState;

/// How long one Endpoint's answer is kept, here and in a browser.
const MAX_AGE: Duration = Duration::from_secs(3600);
/// How long an answer built while the gateway did not answer is kept: long enough that a burst
/// of visitors asks a failing gateway once per Endpoint, short enough that the page heals soon.
const INCOMPLETE_MAX_AGE: Duration = Duration::from_secs(30);
/// The most Endpoints whose answers are kept at once.
const KEPT_MAX: usize = 512;
/// The most types tried for an entity to build the examples from.
const TYPES_TRIED: usize = 5;
/// The most fields listed per standard.
const FIELDS_MAX: usize = 50;
/// The quotable texts an equality example tries, in field order, before a number.
const EQUALITY_TRIES: usize = 3;
/// The query standards a page explains, in the order it shows them.
const STANDARDS: [&str; 4] = ["ngsi-ld", "ogc-features", "odata", "sta"];

static HTTP: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .user_agent("joinedcontext-portal endpoint standards")
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_default()
});

/// Each Endpoint's answer, with the moment it was built and how long it is kept.
type Kept = (Instant, Duration, EndpointStandards);

static KEPT: LazyLock<RwLock<HashMap<String, Kept>>> = LazyLock::new(RwLock::default);

/// The answer kept for `slug`, or the one `build` makes, and how long it may be kept: an hour,
/// or [`INCOMPLETE_MAX_AGE`] when the gateway did not answer.
async fn kept_or_built<F>(slug: &str, build: F) -> (EndpointStandards, Duration)
where
    F: std::future::Future<Output = (EndpointStandards, bool)>,
{
    let kept = KEPT
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(slug)
        .filter(|(at, age, _)| at.elapsed() < *age)
        .map(|(at, age, answer)| (answer.clone(), age.saturating_sub(at.elapsed())));
    if let Some(kept) = kept {
        return kept;
    }
    let (answer, complete) = build.await;
    let age = if complete {
        MAX_AGE
    } else {
        INCOMPLETE_MAX_AGE
    };
    let mut kept = KEPT.write().unwrap_or_else(|e| e.into_inner());
    if kept.len() >= KEPT_MAX && !kept.contains_key(slug) {
        kept.clear();
    }
    kept.insert(slug.to_owned(), (Instant::now(), age, answer.clone()));
    (answer, age)
}

/// What a public Endpoint's page shows under "Standards".
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EndpointStandards {
    pub slug: String,
    /// The entity types of the Endpoint's data model.
    pub types: Vec<String>,
    /// The type of the entity the examples were built from; absent when none was read.
    pub entity_type: Option<String>,
    pub standards: Vec<StandardGuide>,
    /// The Endpoint's public address, ending in `/`: every sample a developer page shows starts here.
    pub base: String,
    /// Every representation the Endpoint serves, query standards and files alike.
    pub served: Vec<String>,
    /// The licence the Endpoint's catalogue entry names; absent when it names none.
    pub licence: Option<EndpointLicence>,
    /// The requests a minute the edge lets one anonymous caller send (ADR-N-035, class
    /// `publicEndpoint`); beyond it the edge answers 429.
    pub requests_per_minute: Option<u32>,
}

/// A licence of the EU licence table.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EndpointLicence {
    /// The EU licence table code, such as `CC_BY_4_0`.
    pub code: String,
    pub title: String,
    /// The licence's IRI in the EU licence table.
    pub url: String,
}

/// One query standard the Endpoint serves.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StandardGuide {
    /// `ngsi-ld`, `ogc-features`, `odata` or `sta`.
    pub representation: String,
    /// The names this standard filters on.
    pub fields: Vec<String>,
    pub examples: Vec<Example>,
    /// The first five entities of the example type in this standard, kept only when that call
    /// answered with entities.
    pub first: Option<String>,
}

/// A query that answered with at least one entity when the page was built.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Example {
    /// `equality` or `range`.
    pub kind: String,
    pub field: String,
    pub url: String,
    /// The entities on the first page of its answer.
    pub answered: u64,
}

#[utoipa::path(
    get,
    path = "/api/v1/catalogue/endpoints/{slug}/standards",
    summary = "Explain a public Endpoint's standards",
    description = "The query standards a public Endpoint serves, the fields each filters on and two examples per standard built from one real entity, read anonymously through the Endpoint (TS-19, T-4484).",
    tag = "ckan",
    params(("slug" = String, Path, description = "The Endpoint's slug")),
    responses(
        (status = 200, description = "The standards", body = EndpointStandards),
        (status = 404, description = "No public Endpoint with this slug", body = crate::error::ProblemDetails),
    )
)]
pub async fn get_standards(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> Result<Response, ApiError> {
    let (project, endpoint) = public_endpoint(&state, &slug)
        .ok_or_else(|| ApiError::NotFound(format!("no public endpoint '{slug}'")))?;
    let (answer, age) = kept_or_built(&slug, build(&state, &project, &slug, &endpoint.spec)).await;
    // What the Organization and the catalogue entry say now, never an hour old.
    let answer = EndpointStandards {
        licence: licence_of(&endpoint.spec),
        requests_per_minute: crate::reconciler::edge_file::EdgeLimits::of(
            crate::api::organization_limits::limits_in(&state.mirror).as_ref(),
            &state.config.organization_bounds,
        )
        .of_class("publicEndpoint"),
        ..answer
    };
    Ok((
        [(
            header::CACHE_CONTROL,
            format!("public, max-age={}", age.as_secs()),
        )],
        Json(answer),
    )
        .into_response())
}

/// The answer, and whether the gateway answered the entity read (an answer built while it was
/// down is not kept).
async fn build(
    state: &AppState,
    project: &str,
    slug: &str,
    spec: &Value,
) -> (EndpointStandards, bool) {
    // In the cluster the gateway answers directly; the public host is the same Endpoint.
    let gateway = state
        .config
        .gateway_url
        .as_deref()
        .and_then(|g| Url::parse(g).ok())
        .unwrap_or_else(|| state.config.public_base_url.clone());
    let at = Endpoint {
        read: endpoint_url(&gateway, slug),
        shown: endpoint_url(&state.config.api_base_url(), slug),
    };
    let served = served_representations(spec);
    let types = classes_of(state, project, spec);
    let mut entity = None;
    let mut complete = true;
    for name in types.iter().take(TYPES_TRIED) {
        let path = format!(
            "ngsi-ld/v1/entities?type={}&limit=1&options=keyValues",
            encode(name)
        );
        match at.read(&path).await {
            Some(Value::Array(list)) => {
                if let Some(first) = list.into_iter().next() {
                    entity = Some((name.clone(), first));
                    break;
                }
            }
            _ => complete = false,
        }
    }
    let mut standards = Vec::new();
    for representation in STANDARDS {
        if served.iter().any(|r| r == representation) {
            standards.push(guide(&at, representation, entity.as_ref()).await);
        }
    }
    let answer = EndpointStandards {
        slug: slug.to_owned(),
        types,
        entity_type: entity.map(|(name, _)| name),
        standards,
        base: at.shown.clone(),
        served,
        licence: None,
        requests_per_minute: None,
    };
    (answer, complete)
}

/// One standard's fields and examples.
async fn guide(
    at: &Endpoint,
    representation: &str,
    entity: Option<&(String, Value)>,
) -> StandardGuide {
    let mut out = StandardGuide {
        representation: representation.to_owned(),
        fields: Vec::new(),
        examples: Vec::new(),
        first: None,
    };
    let Some((name, entity)) = entity else {
        return out;
    };
    let attributes = attributes(entity);
    let (fields, base) = match representation {
        "ngsi-ld" => (
            attributes.clone(),
            Some(format!("ngsi-ld/v1/entities?type={}&", encode(name))),
        ),
        "sta" => (attributes.clone(), Some("sta/v1.1/Things?".to_owned())),
        "ogc-features" => ogc(at, name).await,
        "odata" => odata(at, name).await,
        _ => (Vec::new(), None),
    };
    out.fields = fields.into_iter().take(FIELDS_MAX).collect();
    let Some(base) = base else {
        return out;
    };
    let first = format!("{base}{}", first_page(representation));
    if at
        .read(&first)
        .await
        .is_some_and(|a| count(representation, &a) > 0)
    {
        out.first = Some(format!("{}{first}", at.shown));
    }
    // Equality on the first text the endpoint answers, else on a number when none does (air
    // quality: numbers, and station names the old ASCII-only rule refused, T-4571). Texts are
    // tried in turn because a source may match a name and never an address.
    let mut equality = text_fields(entity, &out.fields);
    equality.truncate(EQUALITY_TRIES);
    equality.extend(number_field(entity, &out.fields));
    let candidates = equality
        .into_iter()
        .map(|found| ("equality", found))
        .chain(number_field(entity, &out.fields).map(|found| ("range", found)));
    let urls: Vec<(&str, String, String)> = candidates
        .filter_map(|(kind, (field, value))| {
            let query = query(representation, kind, &field, &value)?;
            Some((kind, field, format!("{base}{query}")))
        })
        .collect();
    let answers = join_all(urls.iter().map(|(_, _, path)| at.read(path))).await;
    for ((kind, field, path), answer) in urls.into_iter().zip(answers) {
        let answered = answer.as_ref().map_or(0, |a| count(representation, a));
        let shown = out.examples.iter().any(|e| e.kind == kind);
        if answered > 0 && !shown {
            out.examples.push(Example {
                kind: kind.to_owned(),
                field,
                url: format!("{}{path}", at.shown),
                answered,
            });
        }
    }
    out
}

/// One Endpoint, as the Portal reads it and as a visitor sees it.
struct Endpoint {
    read: String,
    shown: String,
}

impl Endpoint {
    /// The JSON at `path` below the Endpoint, read anonymously; `None` for anything but a `200`.
    async fn read(&self, path: &str) -> Option<Value> {
        let response = HTTP
            .get(format!("{}{path}", self.read))
            .header(header::ACCEPT, "application/json")
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        response.json().await.ok()
    }

    async fn read_text(&self, path: &str) -> Option<String> {
        let response = HTTP.get(format!("{}{path}", self.read)).send().await.ok()?;
        if !response.status().is_success() {
            return None;
        }
        response.text().await.ok()
    }
}

/// The collection's queryables, and the items path when the Endpoint serves a collection of
/// the type.
async fn ogc(at: &Endpoint, name: &str) -> (Vec<String>, Option<String>) {
    let Some(collections) = at.read("ogc/features/collections").await else {
        return (Vec::new(), None);
    };
    let found = collections["collections"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| c["id"].as_str())
        .any(|id| id == name);
    if !found {
        return (Vec::new(), None);
    }
    let collection = format!("ogc/features/collections/{}", encode(name));
    let fields = at
        .read(&format!("{collection}/queryables"))
        .await
        .and_then(|q| q["properties"].as_object().cloned())
        .map(|properties| {
            properties
                .keys()
                .filter(|key| *key != "id" && *key != "geometry")
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    (fields, Some(format!("{collection}/items?")))
}

/// The entity type's properties in `$metadata`, and the entity set's path when the service
/// document lists one of the type.
async fn odata(at: &Endpoint, name: &str) -> (Vec<String>, Option<String>) {
    let set = at.read("odata/v4/").await.and_then(|service| {
        service["value"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|set| set["name"].as_str() == Some(name))
            .and_then(|set| set["url"].as_str().map(str::to_owned))
    });
    let Some(set) = set else {
        return (Vec::new(), None);
    };
    let fields = at
        .read_text("odata/v4/$metadata")
        .await
        .map(|metadata| odata_properties(&metadata, name))
        .unwrap_or_default();
    (fields, Some(format!("odata/v4/{}?", encode(&set))))
}

/// The `Property` names of `EntityType` `name` in a CSDL XML document.
fn odata_properties(metadata: &str, name: &str) -> Vec<String> {
    let opening = format!("<EntityType Name=\"{name}\"");
    let Some(body) = metadata.split_once(&opening).map(|(_, rest)| rest) else {
        return Vec::new();
    };
    let body = body.split_once("</EntityType>").map_or(body, |(b, _)| b);
    body.split("<Property Name=\"")
        .skip(1)
        .filter_map(|tail| tail.split_once('"').map(|(name, _)| name.to_owned()))
        .collect()
}

/// The query string of one example in the standard's own syntax; `None` for a standard with
/// no filter syntax here. An equality on a number is written unquoted in every syntax.
fn query(representation: &str, kind: &str, field: &str, value: &str) -> Option<String> {
    let equality = kind == "equality";
    let number = value.parse::<f64>().is_ok_and(f64::is_finite);
    let (double, single) = if number {
        (value.to_owned(), value.to_owned())
    } else {
        (format!("\"{value}\""), format!("'{value}'"))
    };
    Some(match representation {
        "ngsi-ld" if equality => format!("q={}", encode(&format!("{field}=={double}"))),
        "ngsi-ld" => format!("q={}", encode(&format!("{field}>={value}"))),
        "ogc-features" if equality => format!("{}={}", encode(field), encode(value)),
        "ogc-features" => format!("filter={}", encode(&format!("{field} >= {value}"))),
        "odata" if equality => format!("$filter={}", encode(&format!("{field} eq {single}"))),
        "odata" => format!("$filter={}", encode(&format!("{field} ge {value}"))),
        "sta" if equality => format!(
            "$filter={}",
            encode(&format!("properties/{field} eq {single}"))
        ),
        "sta" => format!(
            "$filter={}",
            encode(&format!("properties/{field} ge {value}"))
        ),
        _ => return None,
    })
}

/// The query string of a first page of five in the standard's own syntax.
fn first_page(representation: &str) -> &'static str {
    match representation {
        "odata" | "sta" => "$top=5",
        _ => "limit=5",
    }
}

/// The licence `spec.catalog.license` names, as the EU licence table writes it.
fn licence_of(spec: &Value) -> Option<EndpointLicence> {
    let licence: jc_core::kinds::catalog::Licence =
        serde_json::from_value(spec["catalog"]["license"].clone()).ok()?;
    Some(EndpointLicence {
        code: licence.code().to_owned(),
        title: licence.title().to_owned(),
        url: licence.iri(),
    })
}

/// The entities on the first page of an answer in the standard's own shape.
fn count(representation: &str, answer: &Value) -> u64 {
    let list = match representation {
        "ngsi-ld" => answer.as_array(),
        "ogc-features" => answer["features"].as_array(),
        _ => answer["value"].as_array(),
    };
    list.map_or(0, |l| l.len() as u64)
}

/// The attribute names of a `keyValues` entity.
fn attributes(entity: &Value) -> Vec<String> {
    entity
        .as_object()
        .into_iter()
        .flatten()
        .map(|(name, _)| name)
        .filter(|name| !matches!(name.as_str(), "id" | "type" | "@context"))
        .cloned()
        .collect()
}

/// The fields `fields` lists whose value is text every standard can quote, in order: up to 64 characters,
/// spaces and diacritics included, never a quote or a control character (each syntax's encoding
/// percent-encodes the rest). A text that reads as a number is left to [`number_field`], since
/// it would be written unquoted.
fn text_fields(entity: &Value, fields: &[String]) -> Vec<(String, String)> {
    fields
        .iter()
        .filter_map(|field| {
            let value = entity[field.as_str()].as_str()?;
            let quotable = !value.trim().is_empty()
                && value.chars().count() <= 64
                && !value
                    .chars()
                    .any(|c| c == '"' || c == '\'' || c.is_control())
                && value.parse::<f64>().is_err();
            quotable.then(|| (field.clone(), value.to_owned()))
        })
        .collect()
}

/// A field `fields` lists whose value is a number.
fn number_field(entity: &Value, fields: &[String]) -> Option<(String, String)> {
    fields
        .iter()
        .find_map(|field| match &entity[field.as_str()] {
            Value::Number(n) => Some((field.clone(), n.to_string())),
            _ => None,
        })
}

/// The classes of the data model of the space the Endpoint serves.
fn classes_of(state: &AppState, project: &str, spec: &Value) -> Vec<String> {
    crate::resource::reference_name(&spec["contextSpaceRef"])
        .map(|space| crate::api::assistant_context::model_classes(&state.mirror, project, space))
        .unwrap_or_default()
}

pub fn router() -> axum::Router<AppState> {
    axum::Router::new().route(
        "/catalogue/endpoints/{slug}/standards",
        axum::routing::get(get_standards),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn a_failing_gateway_is_asked_once_per_endpoint_in_the_short_window() {
        let builds = std::sync::atomic::AtomicUsize::new(0);
        let failing = || async {
            builds.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let answer = EndpointStandards {
                slug: "t5728-down".into(),
                types: vec![],
                entity_type: None,
                standards: vec![],
                base: String::new(),
                served: vec![],
                licence: None,
                requests_per_minute: None,
            };
            (answer, false)
        };
        for _ in 0..5 {
            let (_, age) = kept_or_built("t5728-down", failing()).await;
            assert!(age <= INCOMPLETE_MAX_AGE, "{age:?}");
        }
        assert_eq!(builds.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn each_standard_writes_its_examples_in_its_own_syntax() {
        let q = |r, k, f, v| query(r, k, f, v).expect("a filter syntax");
        assert_eq!(
            q("ngsi-ld", "equality", "name", "Zvolen"),
            "q=name%3D%3D%22Zvolen%22"
        );
        assert_eq!(q("ngsi-ld", "range", "no2", "21.5"), "q=no2%3E%3D21.5");
        assert_eq!(
            q("ogc-features", "equality", "name", "Zvolen"),
            "name=Zvolen"
        );
        assert_eq!(
            q("ogc-features", "range", "no2", "21.5"),
            "filter=no2%20%3E%3D%2021.5"
        );
        assert_eq!(
            q("odata", "equality", "name", "Zvolen"),
            "$filter=name%20eq%20%27Zvolen%27"
        );
        assert_eq!(q("odata", "range", "no2", "3"), "$filter=no2%20ge%203");
        assert_eq!(
            q("sta", "range", "no2", "3"),
            "$filter=properties%2Fno2%20ge%203"
        );
        assert_eq!(query("csv", "range", "no2", "3"), None);
    }

    /// T-4571: an equality on a number is unquoted in every syntax.
    #[test]
    fn a_numeric_equality_is_unquoted_in_each_syntax() {
        let q = |r| query(r, "equality", "no2", "12").expect("a filter syntax");
        assert_eq!(q("ngsi-ld"), "q=no2%3D%3D12");
        assert_eq!(q("ogc-features"), "no2=12");
        assert_eq!(q("odata"), "$filter=no2%20eq%2012");
        assert_eq!(q("sta"), "$filter=properties%2Fno2%20eq%2012");
        assert_eq!(
            query("odata", "equality", "name", "Námestie SNP").as_deref(),
            Some("$filter=name%20eq%20%27N%C3%A1mestie%20SNP%27")
        );
    }

    /// T-4571: an air-quality entity, numbers and a station name with a space and diacritics,
    /// yields an equality on the name and a range on a number; one with numbers alone an
    /// equality on a number.
    #[test]
    fn an_entity_of_numbers_and_diacritic_text_yields_two_candidates() {
        let entity = json!({"id": "urn:ngsi-ld:AirQualityObserved:1", "type": "AirQualityObserved",
                            "name": "Banská Bystrica, Štefánikovo nábrežie", "no2": 21.5});
        let fields = attributes(&entity);
        assert_eq!(
            text_fields(&entity, &fields).first().cloned(),
            Some((
                "name".to_owned(),
                "Banská Bystrica, Štefánikovo nábrežie".to_owned()
            ))
        );
        assert_eq!(
            number_field(&entity, &fields),
            Some(("no2".to_owned(), "21.5".to_owned()))
        );
        let numbers = json!({"no2": 21.5, "pm10": 9});
        let fields = attributes(&numbers);
        assert_eq!(text_fields(&numbers, &fields).first().cloned(), None);
        assert!(number_field(&numbers, &fields).is_some());
    }

    #[test]
    fn only_quotable_text_and_numbers_become_examples() {
        let entity = json!({
            "id": "urn:ngsi-ld:A:1", "type": "A",
            "address": "Hlavná 1", "quoted": "it's", "refStation": "urn:ngsi-ld:S:1",
            "name": "Zvolen-1", "no2": 21.5, "count": 3, "flag": true, "code": "42",
            "line": "a\nb"
        });
        let fields = attributes(&entity);
        assert!(
            !fields.iter().any(|f| f == "id" || f == "type"),
            "{fields:?}"
        );
        // sorted names: `address` is the first quotable text
        assert_eq!(
            text_fields(&entity, &fields).first().cloned(),
            Some(("address".to_owned(), "Hlavná 1".to_owned()))
        );
        // sorted names: `count` before `no2`
        assert_eq!(
            number_field(&entity, &fields),
            Some(("count".to_owned(), "3".to_owned()))
        );
        // T-4571: a space and diacritics are quotable; a quote, a control character and a
        // number written as text are not.
        assert_eq!(
            text_fields(&entity, &["address".to_owned()])
                .first()
                .cloned(),
            Some(("address".to_owned(), "Hlavná 1".to_owned()))
        );
        for field in ["quoted", "line", "code"] {
            assert_eq!(
                text_fields(&entity, &[field.to_owned()]).first().cloned(),
                None,
                "{field}"
            );
        }
        assert_eq!(number_field(&entity, &["name".to_owned()]), None);
        assert_eq!(text_fields(&json!({}), &fields), []);
    }

    #[test]
    fn odata_properties_are_read_from_the_entity_types_own_element() {
        let metadata = r#"<edmx:Edmx><Schema>
            <EntityType Name="B"><Property Name="other" Type="Edm.String"/></EntityType>
            <EntityType Name="A" BaseType="NgsiLd.Entity">
              <Property Name="name" Type="Edm.String"/><Property Name="no2" Type="Edm.Double"/>
            </EntityType></Schema></edmx:Edmx>"#;
        assert_eq!(odata_properties(metadata, "A"), ["name", "no2"]);
        assert_eq!(odata_properties(metadata, "B"), ["other"]);
        assert!(odata_properties(metadata, "C").is_empty());
        assert!(odata_properties("", "A").is_empty());
    }

    #[test]
    fn answers_are_counted_in_each_standards_shape() {
        assert_eq!(count("ngsi-ld", &json!([{}, {}])), 2);
        assert_eq!(count("ogc-features", &json!({"features": [{}]})), 1);
        assert_eq!(count("odata", &json!({"value": [{}, {}, {}]})), 3);
        assert_eq!(count("sta", &json!({"value": []})), 0);
        assert_eq!(count("odata", &json!({"error": {}})), 0);
    }
}
