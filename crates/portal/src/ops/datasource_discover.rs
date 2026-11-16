//! What a person can pick for an `http` source's preset (T-7076, G-08): the organizations of a
//! CKAN portal, the layers of an ArcGIS FeatureServer, the entity sets of an OData service, the
//! feature types of a WFS, the collections of an OGC API Features service or the Things of a
//! SensorThings service. The Portal reads the service's own listing once on the project's runner,
//! as a connection test reads a source (`api/datasource_test.rs`): held to the public internet
//! first (AG-65), the runner's own principal, and at most `ANSWER_BYTES` of the answer.
//!
//! The listing's address is built from the scheme, host and path of the URL the person typed and
//! the listing's own query: a credential written into the URL's user part is refused and its query
//! is dropped, so no token the person pasted reaches the runner or comes back in the answer.

use jc_core::kinds::data_source::refused_target;
use jc_core::kinds::{HttpPresetKind, PipelineSpec};
use jcctl::pipeline_test::{harness, Sample, SampleFormat, TestTrace, FETCH_FAILED};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use url::Url;
use utoipa::ToSchema;

use super::{bounds, parse_input, Operation, OperationAnnotations};
use crate::change::Lane;
use crate::error::ApiError;
use jc_core::kinds::Verb;

/// The bytes of a service's listing the runner hands back. A longer XML listing is read up to
/// the cut, a longer JSON one is refused, and either way the answer says it was cut.
pub const ANSWER_BYTES: usize = 256 * 1024;
/// The choices one answer lists.
pub const MAX_CHOICES: usize = 1000;
/// The characters of a choice's value or label the answer keeps.
const MAX_TEXT: usize = 300;

/// What to list: the preset kind and the service's URL as the person typed it.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[schema(example = json!({ "kind": "wfs", "url": "https://kartta.hel.fi/ws/geoserver/avoindata/wfs" }))]
pub struct DiscoverRequest {
    /// `ckan`, `arcgisFeature`, `odata`, `wfs`, `ogcFeatures` or `sensorThings`.
    #[schema(value_type = String, example = "arcgisFeature")]
    pub kind: HttpPresetKind,
    /// The service or any address under it, `https://geodata.example/rest/services/X/FeatureServer/0/query`.
    #[schema(example = "https://kartta.hel.fi/ws/geoserver/avoindata/wfs")]
    pub url: String,
}

/// One thing the person can pick.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct Choice {
    /// What the form writes: an organization name, a layer id, an entity set, a feature type
    /// name, a collection id or a Thing id.
    pub value: String,
    /// What the person reads.
    pub label: String,
}

/// The listing of one service, or why there is none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Discovery {
    /// The form field a choice fills: `organization`, `layer`, `entitySet`, `typeNames`,
    /// `collection` or `thing`.
    pub field: String,
    /// The listing's address the runner read.
    pub url: String,
    /// What the service lists, in its order, at most `MAX_CHOICES`.
    pub choices: Vec<Choice>,
    /// The answer was longer than the Portal reads, or listed more than `MAX_CHOICES`.
    pub truncated: bool,
    /// Why nothing is listed: the service did not answer, or answered something else.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
}

/// The form field a choice of `kind` fills.
pub const fn field_of(kind: HttpPresetKind) -> &'static str {
    match kind {
        HttpPresetKind::Ckan => "organization",
        HttpPresetKind::ArcgisFeature => "layer",
        HttpPresetKind::Odata => "entitySet",
        HttpPresetKind::Wfs => "typeNames",
        HttpPresetKind::OgcFeatures => "collection",
        HttpPresetKind::SensorThings => "thing",
    }
}

/// The listing's address for a service of `kind` the person named by `typed`, or what to fix.
pub fn listing_url(kind: HttpPresetKind, typed: &str) -> Result<Url, String> {
    let mut url = Url::parse(typed.trim()).map_err(|e| format!("the URL does not parse: {e}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("the URL must start with https:// or http://".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(
            "the URL carries a user name or password; a listing reads public services only, so remove it"
                .into(),
        );
    }
    url.set_query(None);
    url.set_fragment(None);
    let path = url.path().trim_end_matches('/').to_owned();
    let cut = |marker: &str, keep: bool| {
        path.find(marker)
            .map(|at| path[..at + if keep { marker.len() } else { 0 }].to_owned())
    };
    let (path, query) = match kind {
        HttpPresetKind::Ckan => (
            format!(
                "{}/api/3/action/organization_list",
                cut("/api/3/action", false).unwrap_or(path)
            ),
            None,
        ),
        HttpPresetKind::ArcgisFeature => (
            cut("/FeatureServer", true).ok_or(
                "the URL names no ArcGIS FeatureServer, for example https://services.example/arcgis/rest/services/Trees/FeatureServer",
            )?,
            Some("f=json"),
        ),
        HttpPresetKind::Odata => (cut(".svc", true).unwrap_or(path), None),
        HttpPresetKind::Wfs => (
            path,
            Some("service=WFS&version=2.0.0&request=GetCapabilities"),
        ),
        HttpPresetKind::OgcFeatures => (
            format!("{}/collections", cut("/collections", false).unwrap_or(path)),
            Some("f=json"),
        ),
        HttpPresetKind::SensorThings => (
            format!(
                "{}/Things",
                cut("/v1.1", true).or_else(|| cut("/v1.0", true)).ok_or(
                    "the URL names no SensorThings version, for example https://iot.example/v1.1/Things",
                )?
            ),
            Some("$select=@iot.id,name&$top=1000"),
        ),
    };
    url.set_path(&path);
    url.set_query(query);
    Ok(url)
}

/// The runner configuration that reads `url` once and posts its first `ANSWER_BYTES` and its
/// whole length to `capture`.
pub(crate) fn listing_read(url: &Url, capture: &str) -> Result<Value, ApiError> {
    if let Some(reason) = refused_target(url.as_str()) {
        return Err(ApiError::BadRequest(format!(
            "the service's address: {reason}"
        )));
    }
    let envelope = format!(
        concat!(
            "root = {{}}\n",
            "root.input = if errored() {{ \"\" }} else {{ content().string().slice(0, {}) }}\n",
            "root.output = if errored() {{ deleted() }} else {{ {{\"bytes\": content().length()}} }}\n",
            "root.error = if errored() {{ \"{}\" + error() }} else {{ null }}"
        ),
        ANSWER_BYTES, FETCH_FAILED
    );
    let probe: PipelineSpec = serde_json::from_value(json!({
        "class": "auto",
        "targetEndpoint": "urn:ngsi-ld:Endpoint:probe.local:probe:probe"
    }))
    .map_err(|e| ApiError::Internal(e.to_string()))?;
    let sample = Sample {
        text: Some(String::new()),
        url: None,
        format: SampleFormat::Text,
        records: None,
    };
    let mut config =
        harness(&probe, &sample, capture).map_err(|e| ApiError::Internal(e.to_string()))?;
    config["input"] =
        json!({ "generate": { "count": 1, "interval": "", "mapping": "root = \"\"" } });
    config["pipeline"] = json!({ "processors": [
        { "http": { "url": url.as_str(), "verb": "GET", "timeout": "5s", "retries": 0,
                    "headers": { "Accept": "application/json, application/xml;q=0.9" } } },
        { "mapping": envelope },
        { "catch": [] },
    ] });
    Ok(config)
}

/// The answer of one listing read: its choices, or the probe's words for why there are none.
pub(crate) fn discovery_of(kind: HttpPresetKind, url: &Url, trace: TestTrace) -> Discovery {
    let bytes = trace
        .mapping
        .first()
        .and_then(|out| out["bytes"].as_u64())
        .unwrap_or(0) as usize;
    let sample = trace.input.sample.clone();
    let probe = crate::api::pipeline_test::answer_of(trace);
    let listed = match (probe.skipped, sample) {
        (Some(reason), _) => Err(reason),
        (None, Some(sample)) => choices(kind, &sample, bytes <= ANSWER_BYTES),
        (None, None) => Err("the service answered nothing".into()),
    };
    let (choices, truncated, skipped) = match listed {
        Ok((choices, truncated)) => (choices, truncated, None),
        Err(reason) => (Vec::new(), false, Some(reason)),
    };
    Discovery {
        field: field_of(kind).into(),
        url: url.to_string(),
        choices,
        truncated,
        skipped,
    }
}

/// The choices a service of `kind` lists in `answer` (JSON parsed, anything else as text), and
/// whether the list is cut; `whole` is false when the runner read only the first `ANSWER_BYTES`.
pub fn choices(
    kind: HttpPresetKind,
    answer: &Value,
    whole: bool,
) -> Result<(Vec<Choice>, bool), String> {
    if kind == HttpPresetKind::Wfs {
        let text = answer
            .as_str()
            .ok_or("the service did not answer a WFS capabilities document")?;
        let mut found = feature_types(text);
        if found.is_empty() && text.contains("ExceptionReport") {
            return Err(format!(
                "the WFS refused the request: {}",
                short(&elements(text, "ExceptionText").join(" "))
            ));
        }
        if found.is_empty() {
            return Err("the answer lists no FeatureType; check the URL names a WFS".into());
        }
        let cut = !whole || found.len() > MAX_CHOICES;
        found.truncate(MAX_CHOICES);
        return Ok((found, cut));
    }
    if !whole || answer.is_string() {
        return Err(format!(
            "the answer is not JSON{}; check the URL names the service",
            if whole {
                ""
            } else {
                " the Portal can read whole (over 256 KiB)"
            }
        ));
    }
    let text = |v: &Value| match v {
        Value::String(s) => Some(short(s)),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    };
    let pick = |items: Option<&Vec<Value>>, value: &str, label: &str| -> Vec<Choice> {
        items
            .into_iter()
            .flatten()
            .filter_map(|item| {
                let value = text(&item[value])?;
                let label = text(&item[label]).unwrap_or_else(|| value.clone());
                Some(Choice { value, label })
            })
            .collect()
    };
    let mut found = match kind {
        HttpPresetKind::Ckan => answer["result"]
            .as_array()
            .ok_or("the answer has no CKAN result list; check the URL names a CKAN portal")?
            .iter()
            .filter_map(|name| {
                text(name).map(|n| Choice {
                    value: n.clone(),
                    label: n,
                })
            })
            .collect(),
        HttpPresetKind::ArcgisFeature => {
            let mut layers = pick(answer["layers"].as_array(), "id", "name");
            layers.extend(pick(answer["tables"].as_array(), "id", "name"));
            if answer.get("layers").is_none() {
                return Err(
                    "the answer has no layers; check the URL names an ArcGIS FeatureServer".into(),
                );
            }
            layers
        }
        HttpPresetKind::Odata => {
            let sets: Vec<Value> = answer["value"]
                .as_array()
                .ok_or("the answer is no OData service document; check the URL names the service root, …/Service.svc")?
                .iter()
                .filter(|item| item["kind"].as_str().is_none_or(|k| k == "EntitySet"))
                .cloned()
                .collect();
            pick(Some(&sets), "url", "name")
        }
        HttpPresetKind::OgcFeatures => pick(
            Some(answer["collections"].as_array().ok_or(
                "the answer has no collections; check the URL names an OGC API Features service",
            )?),
            "id",
            "title",
        ),
        HttpPresetKind::SensorThings => {
            pick(
                Some(answer["value"].as_array().ok_or(
                    "the answer has no Things; check the URL names a SensorThings service",
                )?),
                "@iot.id",
                "name",
            )
        }
        HttpPresetKind::Wfs => Vec::new(),
    };
    let cut = found.len() > MAX_CHOICES;
    found.truncate(MAX_CHOICES);
    Ok((found, cut))
}

/// `text` cut to `MAX_TEXT` characters.
fn short(text: &str) -> String {
    text.chars().take(MAX_TEXT).collect()
}

/// The `Name` and `Title` of each `FeatureType` of a WFS capabilities document, as far as `text`
/// holds whole elements.
fn feature_types(text: &str) -> Vec<Choice> {
    elements(text, "FeatureType")
        .into_iter()
        .filter_map(|body| {
            let value = elements(body, "Name").into_iter().next()?.trim().to_owned();
            let label = elements(body, "Title")
                .into_iter()
                .next()
                .map(|t| t.trim().to_owned())
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| value.clone());
            Some(Choice {
                value: short(&unescaped(&value)),
                label: short(&unescaped(&label)),
            })
        })
        .collect()
}

/// The text inside each whole `<local>` or `<prefix:local …>` element of `text`, outermost first;
/// an element cut off by the end of `text` is left out.
fn elements<'a>(text: &'a str, local: &str) -> Vec<&'a str> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        rest = &rest[open + 1..];
        let name_end = rest
            .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
            .unwrap_or(rest.len());
        let name = &rest[..name_end];
        if name.rsplit(':').next() != Some(local) {
            continue;
        }
        let Some(tag_end) = rest.find('>') else { break };
        if rest[..tag_end].ends_with('/') {
            rest = &rest[tag_end + 1..];
            continue;
        }
        let body = &rest[tag_end + 1..];
        let close = format!("</{name}>");
        let Some(end) = body.find(&close) else { break };
        found.push(&body[..end]);
        rest = &body[end + close.len()..];
    }
    found
}

/// XML's five predefined entities written out.
fn unescaped(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// `jc_datasource_discover`: the agent's way to the same listing the form's "Find in the service"
/// reads (`POST /api/v1/projects/{project}/datasource-discovery`).
pub fn operations() -> Vec<Operation> {
    vec![Operation {
        name: "jc_datasource_discover",
        title: "Data Source Discovery",
        description: "Lists what a public service offers an http preset: organizations, layers, \
                      entity sets, feature types, collections or Things",
        input: || {
            json!({
                "type": "object",
                "required": ["kind", "url"],
                "additionalProperties": false,
                "properties": {
                    "kind": {
                        "type": "string",
                        "enum": ["ckan", "arcgisFeature", "odata", "wfs", "ogcFeatures", "sensorThings"],
                        "description": "The preset kind"
                    },
                    "url": bounds::text("The service or any address under it", bounds::URL)
                }
            })
        },
        output: || json!({ "type": "object", "required": ["field", "url", "choices", "truncated"] }),
        annotations: OperationAnnotations {
            read_only_hint: true,
            destructive_hint: false,
            idempotent_hint: true,
        },
        kind: "DataSource",
        verb: Some(Verb::Propose),
        lane: Lane::Green,
        validate: |val| parse_input::<DiscoverRequest>(val.clone()).map(|_| ()),
        run: |caller, state, project, val| {
            Box::pin(async move {
                let request: DiscoverRequest = parse_input(val)?;
                let found = crate::api::datasource_test::discovery(
                    state,
                    &caller.identity,
                    project,
                    request,
                )
                .await?;
                Ok(serde_json::to_value(found)?)
            })
        },
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRESETS: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../ekiben/crates/ekiben-io-http/tests/fixtures/presets"
    );

    fn recorded(file: &str) -> Value {
        let text = std::fs::read_to_string(format!("{PRESETS}/{file}")).expect("fixture");
        serde_json::from_str(&text).unwrap_or(Value::String(text))
    }

    fn listed(kind: HttpPresetKind, file: &str) -> Vec<Choice> {
        let (found, cut) = choices(kind, &recorded(file), true).expect("choices");
        assert!(!cut, "{file}");
        found
    }

    fn choice(value: &str, label: &str) -> Choice {
        Choice {
            value: value.into(),
            label: label.into(),
        }
    }

    #[test]
    fn each_kind_lists_from_its_own_address_and_drops_what_the_person_typed_after_it() {
        let cases = [
            (HttpPresetKind::Ckan, "https://opendata-ajuntament.barcelona.cat/data/api/3/action/package_search?q=x&apikey=s3cret",
             "https://opendata-ajuntament.barcelona.cat/data/api/3/action/organization_list"),
            (HttpPresetKind::Ckan, "https://data.example/", "https://data.example/api/3/action/organization_list"),
            (HttpPresetKind::ArcgisFeature, "https://geodata.csu.gov.cz/server/rest/services/Hosted/Open_data_RSO/FeatureServer/17/query?returnGeometry=false",
             "https://geodata.csu.gov.cz/server/rest/services/Hosted/Open_data_RSO/FeatureServer?f=json"),
            (HttpPresetKind::Odata, "https://services.odata.org/V4/Northwind/Northwind.svc/Customers?$select=CustomerID",
             "https://services.odata.org/V4/Northwind/Northwind.svc"),
            (HttpPresetKind::Wfs, "https://kartta.hel.fi/ws/geoserver/avoindata/wfs?propertyName=id%2Cc",
             "https://kartta.hel.fi/ws/geoserver/avoindata/wfs?service=WFS&version=2.0.0&request=GetCapabilities"),
            (HttpPresetKind::OgcFeatures, "https://demo.pygeoapi.io/master/collections/lakes/items",
             "https://demo.pygeoapi.io/master/collections?f=json"),
            (HttpPresetKind::SensorThings, "https://iot.hamburg.de/v1.1/Things?$filter=startswith(name,'StadtRad')",
             "https://iot.hamburg.de/v1.1/Things?$select=@iot.id,name&$top=1000"),
        ];
        for (kind, typed, listing) in cases {
            assert_eq!(
                listing_url(kind, typed).expect(typed).as_str(),
                listing,
                "{typed}"
            );
        }
    }

    #[test]
    fn a_url_with_a_credential_or_without_the_service_marker_is_refused_with_what_to_fix() {
        let refused = |kind, url| listing_url(kind, url).expect_err(url);
        assert!(refused(
            HttpPresetKind::Ckan,
            "https://user:pw@data.example/api/3/action/x"
        )
        .contains("password"));
        assert!(refused(
            HttpPresetKind::ArcgisFeature,
            "https://maps.example/MapServer/0"
        )
        .contains("FeatureServer"));
        assert!(
            refused(HttpPresetKind::SensorThings, "https://iot.example/Things").contains("v1.1")
        );
        assert!(refused(HttpPresetKind::Wfs, "ftp://maps.example/wfs").contains("https://"));
        assert!(refused(HttpPresetKind::Wfs, "not a url").contains("does not parse"));
    }

    #[test]
    fn a_listing_inside_the_cluster_is_refused_before_the_runner_reads_it() {
        let url = Url::parse("http://169.254.169.254/latest/collections?f=json").expect("url");
        assert!(matches!(
            listing_read(&url, "http://portal/x"),
            Err(ApiError::BadRequest(_))
        ));
        let url = listing_url(
            HttpPresetKind::Odata,
            "https://services.odata.org/V4/Northwind/Northwind.svc",
        )
        .expect("url");
        let config = listing_read(&url, "http://portal/x").expect("config");
        let fetch = &config["pipeline"]["processors"][0]["http"];
        assert_eq!(fetch["url"], url.as_str());
        assert_eq!(fetch["retries"], 0);
        assert!(config["pipeline"]
            .to_string()
            .contains(&format!("slice(0, {ANSWER_BYTES})")));
    }

    #[test]
    fn ckan_lists_the_organizations_of_the_portal() {
        let found = listed(HttpPresetKind::Ckan, "ckan/discover.json");
        assert!(found.len() > 10);
        assert!(found.contains(&choice("ocupacio", "ocupacio")), "{found:?}");
    }

    #[test]
    fn arcgis_lists_the_layers_of_the_feature_server_by_id() {
        let found = listed(HttpPresetKind::ArcgisFeature, "arcgis/discover.json");
        assert_eq!(found[0], choice("1", "Adresní místa"));
        assert_eq!(found.len(), 12);
    }

    #[test]
    fn odata_lists_the_entity_sets_of_the_service_document() {
        let found = listed(HttpPresetKind::Odata, "odata/discover.json");
        assert!(
            found.contains(&choice("Customers", "Customers")),
            "{found:?}"
        );
        assert!(found.iter().all(|c| !c.value.is_empty()));
    }

    #[test]
    fn wfs_lists_the_feature_type_names_with_their_titles() {
        let found = listed(HttpPresetKind::Wfs, "wfs/discover.xml");
        assert_eq!(found.len(), 12);
        assert_eq!(
            found[0].value,
            "avoindata:Ajoneuvoliikenne_liikennemaarat_viiva"
        );
    }

    #[test]
    fn a_wfs_listing_cut_by_the_byte_cap_keeps_the_whole_feature_types_and_says_it_is_cut() {
        let Value::String(text) = recorded("wfs/discover.xml") else {
            panic!("xml")
        };
        // Cut inside the seventh feature type, as the runner's byte cap cuts a long listing.
        let seventh = text
            .match_indices("</FeatureType>")
            .nth(5)
            .expect("six feature types")
            .0;
        let half = &text[..seventh + 40];
        let (found, cut) =
            choices(HttpPresetKind::Wfs, &Value::String(half.into()), false).expect("choices");
        assert!(cut);
        assert_eq!(found.len(), 6);
    }

    #[test]
    fn ogc_lists_the_collections_by_id_and_title() {
        let found = listed(HttpPresetKind::OgcFeatures, "ogc/discover.json");
        assert!(found.contains(&choice("lakes", "Large Lakes")), "{found:?}");
    }

    #[test]
    fn sensorthings_lists_the_things_by_id_and_name() {
        let found = listed(HttpPresetKind::SensorThings, "sensorthings/discover.json");
        assert_eq!(found.len(), 12);
        assert_eq!(found[0].value, "3");
        assert!(found[0].label.starts_with("StadtRad-Station"));
    }

    #[test]
    fn another_answer_than_the_kind_lists_says_what_to_check() {
        let ckan = recorded("ckan/discover.json");
        for kind in [
            HttpPresetKind::ArcgisFeature,
            HttpPresetKind::Odata,
            HttpPresetKind::OgcFeatures,
            HttpPresetKind::SensorThings,
        ] {
            assert!(choices(kind, &ckan, true)
                .expect_err(kind.as_str())
                .contains("check the URL"));
        }
        assert!(choices(HttpPresetKind::Wfs, &ckan, true).is_err());
        assert!(
            choices(HttpPresetKind::Ckan, &Value::String("<html/>".into()), true)
                .expect_err("html")
                .contains("not JSON")
        );
        assert!(
            choices(HttpPresetKind::Ckan, &recorded("ckan/discover.json"), false)
                .expect_err("cut")
                .contains("256 KiB")
        );
        let refused = Value::String("<ows:ExceptionReport><ows:Exception><ows:ExceptionText>No such service</ows:ExceptionText></ows:Exception></ows:ExceptionReport>".into());
        assert!(choices(HttpPresetKind::Wfs, &refused, true)
            .expect_err("refused")
            .contains("No such service"));
    }

    #[test]
    fn more_than_the_choices_an_answer_lists_are_cut_and_said_to_be() {
        let names: Vec<String> = (0..MAX_CHOICES + 5).map(|i| format!("org-{i}")).collect();
        let (found, cut) =
            choices(HttpPresetKind::Ckan, &json!({ "result": names }), true).expect("choices");
        assert!(cut);
        assert_eq!(found.len(), MAX_CHOICES);
    }

    #[test]
    fn a_failed_read_is_an_answer_with_the_reason_and_no_choices() {
        let url = Url::parse("https://data.example/api/3/action/organization_list").expect("url");
        let found = discovery_of(HttpPresetKind::Ckan, &url, TestTrace::default());
        assert!(found.choices.is_empty());
        assert!(found.skipped.is_some());
        assert_eq!(found.field, "organization");
    }
}
