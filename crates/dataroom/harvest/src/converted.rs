#![forbid(unsafe_code)]
//! The platform's own distributions of a converted dataset (EP-118, T-4279): prod's
//! `charts/ckan/files/od-standards.py`. A harvested item whose rows the platform converted to
//! NGSI-LD lists its Endpoint's standard APIs and its data model in every formalism in place of
//! the source's file links; an item that was not converted, or whose Endpoint's model lacks one
//! of the seven artefacts, is not listed.

use dataroom_core::{ConvertedDataset, Distribution};
use serde_json::Value;
use std::collections::BTreeMap;

/// (path under the Endpoint, title, format, media type, conformsTo, how it keeps the dataset's
/// rows, what its rows must have for it to be listed): prod's `OD_STANDARDS`.
type Standard = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    Option<&'static str>,
    Option<Filter>,
    Option<Needs>,
);

#[derive(Clone, Copy)]
enum Filter {
    Q,
    Ogc,
    OData,
    Sta,
}

#[derive(Clone, Copy)]
enum Needs {
    Location,
    Series,
}

const STANDARDS: [Standard; 7] = [
    (
        "ngsi-ld/v1/entities",
        "NGSI-LD API",
        "NGSI-LD",
        "application/json",
        Some("https://www.etsi.org/deliver/etsi_gs/CIM/001_099/009/"),
        Some(Filter::Q),
        None,
    ),
    (
        "file.csv",
        "CSV",
        "CSV",
        "text/csv",
        None,
        Some(Filter::Q),
        None,
    ),
    (
        "ogc/features/collections/{type}/items",
        "OGC API - Features",
        "OGCFEAT",
        "application/geo+json",
        Some("http://www.opengis.net/spec/ogcapi-features-1/1.0/conf/core"),
        Some(Filter::Ogc),
        Some(Needs::Location),
    ),
    (
        "odata/v4/{type}",
        "OData v4",
        "OData",
        "application/json",
        Some("https://docs.oasis-open.org/odata/odata/v4.01/"),
        Some(Filter::OData),
        None,
    ),
    (
        "sta/v1.1/Datastreams",
        "SensorThings API",
        "STA",
        "application/json",
        Some("http://www.opengis.net/spec/iot_sensing/1.1"),
        Some(Filter::Sta),
        Some(Needs::Series),
    ),
    (
        "mcp",
        "Model Context Protocol",
        "MCP",
        "application/json",
        Some("https://modelcontextprotocol.io/specification"),
        None,
        None,
    ),
    (
        "ws",
        "WebSocket",
        "WebSocket",
        "application/json",
        None,
        None,
        None,
    ),
];

/// The data model's files under `schema/v{n}/` and their labels: prod's `OD_MODEL_FILES`.
const MODEL_FILES: [(&str, &str); 7] = [
    ("model.schema.json", "JSON Schema"),
    ("context.jsonld", "JSON-LD"),
    ("model.linkml.yaml", "LinkML"),
    ("model.md", "Markdown"),
    ("model.owl.ttl", "OWL"),
    ("model.rdf.ttl", "RDF"),
    ("model.shacl.ttl", "SHACL"),
];

/// `pairs` as Python's `urlencode`: `quote_plus`, or `quote` (a space as `%20`) when `odata`.
fn encode(pairs: &[(&str, &str)], odata: bool) -> String {
    let mut s = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in pairs {
        s.append_pair(k, v);
    }
    let out = s.finish();
    if odata {
        out.replace('+', "%20")
    } else {
        out
    }
}

/// An OData string literal's text: a quote written twice.
fn odata(text: &str) -> String {
    text.replace('\'', "''")
}

fn address(root: &str, entry: &ConvertedDataset, path: &str, filter: Option<Filter>) -> String {
    let ty = url::form_urlencoded::byte_serialize(entry.entity_type.as_bytes()).collect::<String>();
    let mut url = format!("{root}{}", path.replace("{type}", &ty));
    if path == "ws" {
        if let Some(rest) = url.strip_prefix("https") {
            url = format!("wss{rest}");
        }
    }
    let dataset = entry.dataset.as_str();
    let query = match filter {
        None => return url,
        Some(Filter::Q) => encode(
            &[
                ("type", &entry.entity_type),
                ("q", &format!("isPartOf==\"{dataset}\"")),
            ],
            false,
        ),
        Some(Filter::Ogc) => encode(&[("isPartOf", dataset)], false),
        Some(Filter::OData) => encode(
            &[("$filter", &format!("isPartOf eq '{}'", odata(dataset)))],
            true,
        ),
        Some(Filter::Sta) => encode(
            &[(
                "$filter",
                &format!(
                    "Thing/properties/type eq '{}' and Thing/properties/isPartOf eq '{}'",
                    odata(&entry.entity_type),
                    odata(dataset)
                ),
            )],
            true,
        ),
    };
    format!("{url}?{query}")
}

fn distribution(
    dataset_id: &str,
    n: usize,
    lang: &str,
    title: String,
    url: String,
    format: &str,
    media: Option<String>,
) -> Distribution {
    Distribution {
        id: format!("res-{dataset_id}-{n}"),
        dataset_id: dataset_id.to_owned(),
        uri: None,
        title: BTreeMap::from([(lang.to_owned(), title)]),
        description: BTreeMap::new(),
        access_url: url,
        download_url: None,
        format: Some(format.to_owned()),
        media_type: media,
        compress_format: None,
        package_format: None,
        byte_size: None,
        hash: None,
        hash_algorithm: None,
        licence: None,
        rights: BTreeMap::new(),
        status: None,
        availability: None,
        resource_type: None,
        conforms_to: Vec::new(),
        documentation: Vec::new(),
        languages: Vec::new(),
        applicable_legislation: Vec::new(),
        temporal_resolution: None,
        spatial_resolution_in_meters: None,
        terms_of_use: None,
        access_services: Vec::new(),
        issued: None,
        modified: None,
        extras: BTreeMap::new(),
    }
}

/// The model files of the newest model in `index` (an Endpoint's `schema/index.json`) that
/// defines `kind`, or none when that model lacks one of the seven artefacts.
fn model_files(
    root: &str,
    index: &Value,
    kind: &str,
    dataset_id: &str,
    lang: &str,
    first: usize,
) -> Vec<Distribution> {
    let models = index.get("models").and_then(Value::as_array);
    let Some(model) = models
        .into_iter()
        .flatten()
        .filter(|m| {
            m.get("types")
                .and_then(Value::as_array)
                .is_some_and(|t| t.iter().any(|t| t.as_str() == Some(kind)))
        })
        .max_by_key(|m| m.get("version").and_then(Value::as_u64).unwrap_or(0))
    else {
        return Vec::new();
    };
    let version = model.get("version").and_then(Value::as_u64).unwrap_or(0);
    let artefacts = model.get("artifacts").cloned().unwrap_or(Value::Null);
    if MODEL_FILES.iter().any(|(f, _)| artefacts.get(f).is_none()) {
        return Vec::new();
    }
    MODEL_FILES
        .iter()
        .enumerate()
        .map(|(i, (file, label))| {
            let a = &artefacts[file];
            let sha = a.get("sha256").and_then(Value::as_str).map(str::to_owned);
            let mut d = distribution(
                dataset_id,
                first + i,
                lang,
                format!("Data model: {label}"),
                format!("{root}schema/v{version}/{file}"),
                label,
                a.get("type").and_then(Value::as_str).map(str::to_owned),
            );
            d.description = BTreeMap::from([(
                lang.to_owned(),
                format!(
                    "The data model of {kind} as {label}; sha256 {}.",
                    sha.as_deref().unwrap_or("None")
                ),
            )]);
            d.byte_size = a.get("bytes").and_then(Value::as_u64);
            d.hash = sha;
            d.hash_algorithm = Some("sha256".into());
            d
        })
        .collect()
}

/// The distributions of a converted dataset on `root` (`{gateway}/api/endpoint/{slug}/`) whose
/// Endpoint's model index is `index`; none when the model lacks an artefact (prod's
/// `od_standards`).
pub fn distributions(
    root: &str,
    entry: &ConvertedDataset,
    index: &Value,
    dataset_id: &str,
    lang: &str,
) -> Vec<Distribution> {
    let mut out = Vec::new();
    for (path, title, format, media, conforms, filter, needs) in STANDARDS {
        let listed = match needs {
            None => true,
            Some(Needs::Location) => entry.location,
            Some(Needs::Series) => entry.series,
        };
        if !listed {
            continue;
        }
        // A dataset an older seed reads whole names no catalogue dataset; one whose type holds
        // its rows alone on the Endpoint is kept by the type (T-4291, T-4540).
        let filter = filter.filter(|_| entry.filtered && !entry.sole);
        let what = if filter.is_some() || entry.sole {
            "These rows".to_owned()
        } else {
            format!(
                "Every {} of the space, these rows among them",
                entry.entity_type
            )
        };
        let mut d = distribution(
            dataset_id,
            out.len(),
            lang,
            format!("{title} ({})", entry.entity_type),
            address(root, entry, path, filter),
            format,
            Some(media.to_owned()),
        );
        d.description = BTreeMap::from([(
            lang.to_owned(),
            format!("{what}, through the platform's {title} (NGSI-LD data, read-only)."),
        )]);
        d.resource_type = Some("api".into());
        d.conforms_to = conforms.iter().map(|c| (*c).to_owned()).collect();
        out.push(d);
    }
    let model = model_files(root, index, &entry.entity_type, dataset_id, lang, out.len());
    if model.is_empty() {
        return Vec::new();
    }
    out.extend(model);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(filtered: bool, sole: bool, location: bool) -> ConvertedDataset {
        ConvertedDataset {
            endpoint: "6b4yq".into(),
            entity_type: "Person".into(),
            dataset: "https://opendata.example/dataset/bbsk-clenovia-samospravy-efc2f3".into(),
            location,
            series: false,
            filtered,
            sole,
        }
    }

    fn index() -> Value {
        let artefacts: serde_json::Map<String, Value> = MODEL_FILES
            .iter()
            .map(|(f, _)| {
                (
                    (*f).to_owned(),
                    json!({"type": "text/plain", "bytes": 10, "sha256": "ab"}),
                )
            })
            .collect();
        json!({"models": [
            {"version": 1, "types": ["Person"], "artifacts": artefacts.clone()},
            {"version": 2, "types": ["Person"], "artifacts": artefacts},
            {"version": 3, "types": ["Bridge"], "artifacts": {}}
        ]})
    }

    #[test]
    fn a_converted_dataset_lists_prods_doors_and_the_newest_models_seven_files() {
        let root = "https://api.example/api/endpoint/6b4yq/";
        let d = distributions(root, &entry(true, true, false), &index(), "ds1", "sk");
        // prod's bbsk-clenovia-samospravy-efc2f3: NGSI-LD, CSV, OData, MCP, WebSocket and the
        // seven model files (no location: no OGC; no series: no STA).
        let formats: Vec<_> = d.iter().map(|d| d.format.clone().unwrap()).collect();
        assert_eq!(
            formats,
            [
                "NGSI-LD",
                "CSV",
                "OData",
                "MCP",
                "WebSocket",
                "JSON Schema",
                "JSON-LD",
                "LinkML",
                "Markdown",
                "OWL",
                "RDF",
                "SHACL"
            ]
        );
        // `sole`: kept by the type, no filter (prod: .../ngsi-ld/v1/entities, no query).
        assert_eq!(d[0].access_url, format!("{root}ngsi-ld/v1/entities"));
        assert_eq!(d[2].access_url, format!("{root}odata/v4/Person"));
        assert_eq!(d[4].access_url, "wss://api.example/api/endpoint/6b4yq/ws");
        assert_eq!(
            d[5].access_url,
            format!("{root}schema/v2/model.schema.json")
        );
        assert_eq!(d[5].hash.as_deref(), Some("ab"));
    }

    #[test]
    fn a_filtered_dataset_keeps_its_rows_in_each_standards_syntax() {
        let root = "https://api.example/api/endpoint/6b4yq/";
        let d = distributions(root, &entry(true, false, true), &index(), "ds1", "sk");
        assert_eq!(
            d[0].access_url,
            format!("{root}ngsi-ld/v1/entities?type=Person&q=isPartOf%3D%3D%22https%3A%2F%2Fopendata.example%2Fdataset%2Fbbsk-clenovia-samospravy-efc2f3%22")
        );
        assert_eq!(
            d[2].access_url,
            format!("{root}ogc/features/collections/Person/items?isPartOf=https%3A%2F%2Fopendata.example%2Fdataset%2Fbbsk-clenovia-samospravy-efc2f3")
        );
        assert_eq!(
            d[3].access_url,
            format!("{root}odata/v4/Person?%24filter=isPartOf%20eq%20%27https%3A%2F%2Fopendata.example%2Fdataset%2Fbbsk-clenovia-samospravy-efc2f3%27")
        );
    }

    #[test]
    fn a_model_without_all_seven_files_lists_nothing() {
        let mut idx = index();
        idx["models"][1]["artifacts"]
            .as_object_mut()
            .unwrap()
            .remove("model.owl.ttl");
        assert!(distributions("https://a/", &entry(true, true, false), &idx, "d", "sk").is_empty());
        assert!(distributions(
            "https://a/",
            &entry(true, true, false),
            &json!({}),
            "d",
            "sk"
        )
        .is_empty());
    }
}
