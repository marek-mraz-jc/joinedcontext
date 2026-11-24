//! The DCAT-AP record, its slices, its feeds, its Turtle and its page, edge cases included
//! (T-7794): every representation, a full catalogue block, a restricted audience, a schema index
//! with models and mappings, and the helpers each serialization shares.

use jc_core::kinds::catalog_datasets::CatalogDataset;
use jc_core::kinds::{Audience, Catalog, Duty, ProjectCatalog, Representation};
use jc_dcat::{
    classification, classified, dataset, dataset_html, dataset_slice, duties, escape, feed,
    feed_turtle, literal, localized, major_of, path_of, plain, publisher_feed, slice_catalog,
    turtle, ws_address, CatalogueOwner, Subject, SHARE_ALIKE,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

const BASE: &str = "https://data.example.sk";
const IRI: &str = "https://data.example.sk/api/endpoint/udalosti";

const ALL: [Representation; 9] = [
    Representation::NgsiLd,
    Representation::Mcp,
    Representation::Csv,
    Representation::OgcFeatures,
    Representation::Sta,
    Representation::Odata,
    Representation::WebSocket,
    Representation::Ical,
    Representation::Feed,
];

fn texts(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

fn full_catalog() -> Catalog {
    serde_json::from_value(json!({
        "publisher": {"name": {"en": "City of Example", "sk": "Mesto Príklad"}, "uri": "https://city.example.org/"},
        "contactPoint": {"name": "Open data desk", "email": "opendata@city.example.org"},
        "license": "CC_BYSA_4_0",
        "attribution": {"en": "Source: City of Example"},
        "themes": ["TRAN"],
        "keywords": {"en": ["events"], "sk": ["podujatia"]},
        "spatial": ["SK032"],
        "temporal": {"start": "2019-01-01", "end": "2026-12-31"},
        "frequency": "DAILY",
        "source": [{"url": "https://opendata.example.org/dataset/events", "title": {"en": "Events"}, "description": {"en": "The feed"}}],
        "pipelineRef": {"kind": "Pipeline", "name": "city-events"},
        "applicableLegislation": ["http://data.europa.eu/eli/reg_impl/2023/138/oj"]
    }))
    .expect("the catalogue block parses")
}

fn index() -> Value {
    json!({
        "models": [
            {"name": "events", "version": 2, "artifacts": {
                "model.schema.json": {"type": "application/schema+json", "bytes": 120, "sha256": "ab12"},
                "model.linkml.yaml": {"type": "application/yaml"},
                "context.jsonld": {},
                "model.shacl.ttl": {},
                "model.owl.ttl": {},
                "model.rdf.ttl": {},
                "model.md": {},
                "notes.txt": {}
            }},
            {"name": "old", "version": "1", "artifacts": {"model.md": {}}},
            {"name": "unversioned", "artifacts": {"model.md": {}}},
            {"name": "no-artifacts", "version": 3}
        ],
        "mappings": [
            {"name": "m", "version": 1, "artifacts": {
                "m.linkml-map.yaml": {}, "m.blobl": {}, "m.ir.json": {}
            }}
        ]
    })
}

struct Fixture {
    title: BTreeMap<String, String>,
    description: BTreeMap<String, String>,
    space_title: BTreeMap<String, String>,
    space_description: BTreeMap<String, String>,
    catalog: Catalog,
    feeds: Vec<jc_core::kinds::FeedOutput>,
    calendars: Vec<jc_core::kinds::IcalOutput>,
}

impl Fixture {
    fn new() -> Self {
        Self {
            title: texts(&[("en", "City events"), ("sk", "Podujatia"), ("fi", " ")]),
            description: texts(&[("en", "Every event")]),
            space_title: texts(&[("en", "Events space")]),
            space_description: texts(&[("en", "The space of events")]),
            catalog: full_catalog(),
            feeds: serde_json::from_value(json!([
                {"name": "kultura", "sources": [{"type": "Event", "preset": "event"}]}
            ]))
            .expect("a feed output"),
            calendars: serde_json::from_value(json!([
                {"name": "kalendar", "description": {"sk": "Všetko"}, "sources": [{"type": "Event", "preset": "sdm-event"}]}
            ]))
            .expect("a calendar output"),
        }
    }

    fn subject<'a>(
        &'a self,
        audience: Audience,
        representations: &'a [Representation],
        catalog: Option<&'a Catalog>,
        assigners: &'a [&'a str],
    ) -> Subject<'a> {
        Subject {
            slug: "udalosti",
            space: "udalosti",
            base_path: "/api/endpoint/udalosti",
            title: &self.title,
            description: &self.description,
            space_title: Some(&self.space_title),
            space_description: Some(&self.space_description),
            audience,
            representations,
            catalog,
            classes: &["Event"],
            assigners,
            feeds: &self.feeds,
            calendars: &self.calendars,
        }
    }
}

fn distribution<'a>(record: &'a Value, url: &str) -> &'a Value {
    record["dcat:distribution"]
        .as_array()
        .expect("distributions")
        .iter()
        .find(|d| d["dcat:accessURL"] == url)
        .unwrap_or_else(|| panic!("no distribution {url} in {record:#}"))
}

fn included<'a>(record: &'a Value, id: &str) -> Vec<&'a Value> {
    record["@included"]
        .as_array()
        .expect("included")
        .iter()
        .filter(|n| n["@id"] == id)
        .collect()
}

#[test]
fn every_representation_is_one_distribution_with_its_format_and_standard() {
    let f = Fixture::new();
    let record = dataset(
        &f.subject(Audience::Public, &ALL, Some(&f.catalog), &[]),
        &json!({}),
        BASE,
    );
    let cases = [
        (
            "ngsi-ld/v1/",
            "JSON_LD",
            Some("https://www.etsi.org/deliver/etsi_gs/CIM/001_099/009/"),
        ),
        (
            "mcp",
            "JSON",
            Some("https://modelcontextprotocol.io/specification"),
        ),
        ("file.csv", "CSV", None),
        (
            "ogc/features/",
            "GEOJSON",
            Some("http://www.opengis.net/spec/ogcapi-features-1/1.0/conf/core"),
        ),
        (
            "sta/v1.1/",
            "JSON",
            Some("http://www.opengis.net/spec/sensorthings/1.1"),
        ),
        (
            "odata/v4/",
            "JSON",
            Some("https://docs.oasis-open.org/odata/odata/v4.01/"),
        ),
    ];
    for (path, code, standard) in cases {
        let d = distribution(&record, &format!("{IRI}/{path}"));
        assert_eq!(
            d["dct:format"],
            format!("http://publications.europa.eu/resource/authority/file-type/{code}"),
            "{path}"
        );
        assert_eq!(d["dct:conformsTo"].as_str(), standard, "{path}");
        assert_eq!(d["dcat:accessService"], format!("{IRI}#service"));
        assert_eq!(
            d["dct:license"],
            "http://publications.europa.eu/resource/authority/licence/CC_BYSA_4_0",
            "{path}"
        );
        assert_eq!(
            d["dct:rights"]["rdfs:label"][0]["@value"],
            "Source: City of Example"
        );
    }
    assert_eq!(
        distribution(&record, &format!("{IRI}/file.csv"))["dcat:mediaType"],
        "https://www.iana.org/assignments/media-types/text/csv"
    );
    let calendar = distribution(&record, &format!("{IRI}/feeds/kalendar.ics"));
    assert_eq!(calendar["dct:title"], "kalendar (iCalendar feed)");
    assert_eq!(calendar["dct:description"][0]["@value"], "Všetko");
    assert_eq!(
        calendar["dct:license"],
        "http://publications.europa.eu/resource/authority/licence/CC_BYSA_4_0"
    );
}

#[test]
fn the_paths_of_the_representations_are_the_record_s() {
    assert_eq!(path_of(Representation::NgsiLd), "ngsi-ld/v1/");
    assert_eq!(path_of(Representation::Mcp), "mcp");
    assert_eq!(path_of(Representation::WebSocket), "ws");
    assert_eq!(path_of(Representation::Ical), "ical/v1/events.ics");
    assert_eq!(path_of(Representation::Feed), "feed/v1/");
}

#[test]
fn the_websocket_explains_itself_in_each_language_the_endpoint_has_a_title_in() {
    let f = Fixture::new();
    let record = dataset(
        &f.subject(Audience::Public, &[Representation::WebSocket], None, &[]),
        &json!({}),
        BASE,
    );
    let ws = distribution(&record, &format!("{IRI}/ws"));
    let languages: Vec<&str> = ws["dct:description"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["@language"].as_str().unwrap())
        .collect();
    // fi has only a blank title, so it gets no sentence.
    assert_eq!(languages, vec!["en", "sk"]);
    assert!(ws["dct:description"][0]["@value"]
        .as_str()
        .unwrap()
        .contains("wss://data.example.sk/api/endpoint/udalosti/ws"));
    assert_eq!(ws["foaf:page"], format!("{BASE}/docs/API/07-websocket"));
    assert_eq!(
        ws["dct:conformsTo"],
        format!("{BASE}/docs/API/07-websocket")
    );
    assert!(ws.get("dct:license").is_none(), "no catalogue, no licence");
}

#[test]
fn schema_artifacts_carry_size_digest_and_formalism_and_skip_unversioned_models() {
    let f = Fixture::new();
    let record = dataset(
        &f.subject(Audience::Public, &[], Some(&f.catalog), &[]),
        &index(),
        BASE,
    );
    let json_schema = distribution(&record, &format!("{IRI}/schema/v2/model.schema.json"));
    assert_eq!(json_schema["dct:title"], "events model.schema.json");
    assert_eq!(json_schema["dcat:byteSize"], "120");
    assert_eq!(json_schema["spdx:checksum"]["spdx:checksumValue"], "ab12");
    assert_eq!(
        json_schema["dcat:mediaType"],
        "https://www.iana.org/assignments/media-types/application/schema+json"
    );
    assert_eq!(
        json_schema["dct:conformsTo"],
        "http://json-schema.org/draft-07/schema#"
    );
    let formalisms = [
        ("v2/model.linkml.yaml", "https://w3id.org/linkml/"),
        ("v2/context.jsonld", "https://www.w3.org/TR/json-ld11/"),
        ("v2/model.shacl.ttl", "https://www.w3.org/TR/shacl/"),
        ("v2/model.owl.ttl", "https://www.w3.org/TR/owl2-overview/"),
        ("v2/model.rdf.ttl", "https://www.w3.org/TR/rdf11-concepts/"),
        ("v2/model.md", "https://commonmark.org/"),
        ("v1/model.md", "https://commonmark.org/"),
        (
            "v1/mappings/m.linkml-map.yaml",
            "https://w3id.org/linkml/mapper/",
        ),
        (
            "v1/mappings/m.blobl",
            "https://warpstreamlabs.github.io/bento/docs/guides/bloblang/about",
        ),
        (
            "v1/mappings/m.ir.json",
            "https://joinedcontext.com/ns/mapping-ir",
        ),
    ];
    for (path, standard) in formalisms {
        let d = distribution(&record, &format!("{IRI}/schema/{path}"));
        assert_eq!(d["dct:conformsTo"], standard, "{path}");
    }
    let notes = distribution(&record, &format!("{IRI}/schema/v2/notes.txt"));
    assert!(notes.get("dct:conformsTo").is_none());
    assert!(notes.get("dcat:mediaType").is_none());
    let urls: Vec<&str> = record["dcat:distribution"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d["dcat:accessURL"].as_str())
        .collect();
    assert!(
        !urls
            .iter()
            .any(|u| u.contains("unversioned") || u.contains("/v3/")),
        "{urls:?}"
    );
    // The dataset conforms to each formalism once.
    let standards = record["dct:conformsTo"].as_array().unwrap();
    assert_eq!(
        standards
            .iter()
            .filter(|s| *s == "https://commonmark.org/")
            .count(),
        1
    );
}

#[test]
fn the_full_catalogue_block_describes_the_dataset_and_types_its_values() {
    let f = Fixture::new();
    let record = dataset(
        &f.subject(
            Audience::Public,
            &[Representation::NgsiLd],
            Some(&f.catalog),
            &[],
        ),
        &json!({}),
        BASE,
    );
    assert_eq!(record["dct:publisher"]["@id"], "https://city.example.org/");
    assert_eq!(record["dcat:contactPoint"]["vcard:fn"], "Open data desk");
    assert_eq!(
        record["dct:license"],
        "http://publications.europa.eu/resource/authority/licence/CC_BYSA_4_0"
    );
    assert_eq!(record["dct:temporal"]["dcat:startDate"], "2019-01-01");
    assert_eq!(record["dct:temporal"]["dcat:endDate"], "2026-12-31");
    assert_eq!(
        record["dct:accrualPeriodicity"],
        "http://publications.europa.eu/resource/authority/frequency/DAILY"
    );
    assert_eq!(
        record["dct:source"],
        json!(["https://opendata.example.org/dataset/events"])
    );
    assert_eq!(
        record["prov:wasGeneratedBy"]["rdfs:label"],
        "pipeline city-events"
    );
    assert_eq!(
        record["dcatap:applicableLegislation"],
        json!(["http://data.europa.eu/eli/reg_impl/2023/138/oj"])
    );
    assert_eq!(record["dcat:keyword"].as_array().unwrap().len(), 2);
    assert_eq!(
        included(&record, "http://data.europa.eu/eli/reg_impl/2023/138/oj")[0]["@type"],
        "eli:LegalResource"
    );
    assert_eq!(
        included(&record, "https://opendata.example.org/dataset/events")[0]["@type"],
        "dcat:Dataset"
    );
    // A public dataset under a licence: the offer alone, attribution and share-alike duties.
    let offer = &record["odrl:hasPolicy"];
    assert_eq!(offer["@id"], format!("{IRI}#offer"));
    assert_eq!(offer["odrl:assigner"], "https://city.example.org/");
    assert_eq!(
        offer["odrl:permission"]["odrl:duty"],
        json!([{"odrl:action": "odrl:attribute"}, {"odrl:action": SHARE_ALIKE}])
    );
    assert!(offer["odrl:permission"].get("odrl:constraint").is_none());
    let service = included(&record, &format!("{IRI}#service"));
    assert_eq!(
        service[0]["dct:license"],
        "http://publications.europa.eu/resource/authority/licence/CC_BYSA_4_0"
    );
    assert_eq!(
        service[0]["dcat:endpointDescription"],
        format!("{IRI}/openapi.json")
    );
}

#[test]
fn a_restricted_endpoint_points_at_its_grant_and_constrains_the_offer_to_its_organization() {
    let f = Fixture::new();
    let mut catalog = f.catalog.clone();
    catalog.publisher = None;
    let record = dataset(
        &f.subject(
            Audience::Organization,
            &[Representation::NgsiLd],
            Some(&catalog),
            &["did:web:city.example.org", "did:web:city.example.org"],
        ),
        &json!({}),
        BASE,
    );
    assert_eq!(
        record["dct:accessRights"],
        "http://publications.europa.eu/resource/authority/access-right/RESTRICTED"
    );
    let policies = record["odrl:hasPolicy"]
        .as_array()
        .expect("grant and offer");
    assert_eq!(policies[0], format!("{IRI}/access"));
    let offer = &policies[1];
    assert_eq!(offer["odrl:assigner"], "did:web:city.example.org");
    let constraints = offer["odrl:permission"]["odrl:constraint"]
        .as_array()
        .unwrap();
    assert_eq!(constraints.len(), 2);
    assert_eq!(constraints[0]["odrl:operator"], "odrl:isPartOf");
    assert_eq!(
        constraints[0]["odrl:rightOperand"]["@id"],
        "did:web:city.example.org"
    );
    assert_eq!(
        constraints[1]["odrl:rightOperand"],
        Audience::Organization.as_str()
    );
}

#[test]
fn two_assigners_name_no_organization_in_the_offer() {
    let f = Fixture::new();
    let mut catalog = f.catalog.clone();
    catalog.publisher = None;
    let record = dataset(
        &f.subject(
            Audience::Organization,
            &[],
            Some(&catalog),
            &["did:web:a", "did:web:b"],
        ),
        &json!({}),
        BASE,
    );
    let offer = &record["odrl:hasPolicy"][1];
    assert!(offer.get("odrl:assigner").is_none());
    let constraints = offer["odrl:permission"]["odrl:constraint"]
        .as_array()
        .unwrap();
    assert_eq!(
        constraints.len(),
        1,
        "only the audience class: {constraints:?}"
    );
}

#[test]
fn a_restricted_endpoint_without_licence_lists_its_grant_alone() {
    let f = Fixture::new();
    let record = dataset(
        &f.subject(Audience::Organization, &[], None, &[]),
        &json!({}),
        BASE,
    );
    assert_eq!(record["odrl:hasPolicy"], format!("{IRI}/access"));
}

#[test]
fn a_public_endpoint_without_licence_has_no_policy() {
    let f = Fixture::new();
    let record = dataset(
        &f.subject(Audience::Public, &[], None, &[]),
        &json!({}),
        BASE,
    );
    assert!(record.get("odrl:hasPolicy").is_none());
}

#[test]
fn blank_texts_fall_back_to_the_space_and_then_to_its_name() {
    let mut f = Fixture::new();
    f.title = texts(&[("en", "  ")]);
    f.description = BTreeMap::new();
    let record = dataset(
        &f.subject(Audience::Public, &[], None, &[]),
        &json!({}),
        BASE,
    );
    assert_eq!(record["dct:title"][0]["@value"], "Events space");
    assert_eq!(
        record["dct:description"][0]["@value"],
        "The space of events"
    );
    let none = BTreeMap::new();
    let subject = Subject {
        space_title: None,
        space_description: None,
        title: &none,
        description: &none,
        ..f.subject(Audience::Public, &[], None, &[])
    };
    let record = dataset(&subject, &json!({}), BASE);
    assert_eq!(record["dct:title"], "udalosti");
    assert_eq!(
        record["dct:description"],
        "Data of the context space udalosti, served by this endpoint."
    );
}

#[test]
fn themes_and_keywords_come_from_the_types_unless_set_by_hand() {
    let f = Fixture::new();
    let derived = classified(&f.subject(Audience::Public, &[], None, &[]));
    let classification = classification(&f.subject(Audience::Public, &[], None, &[]));
    assert_eq!(derived.keywords, classification.keywords);
    assert_eq!(
        derived.themes,
        classification.themes.into_iter().collect::<Vec<_>>()
    );
    // Both set by hand: kept as written.
    let kept = classified(&f.subject(Audience::Public, &[], Some(&f.catalog), &[]));
    assert_eq!(kept.themes, f.catalog.themes);
    assert_eq!(kept.keywords, f.catalog.keywords);
    // Themes set, keywords derived.
    let mut themes_only = f.catalog.clone();
    themes_only.keywords.clear();
    let mixed = classified(&f.subject(Audience::Public, &[], Some(&themes_only), &[]));
    assert_eq!(mixed.themes, f.catalog.themes);
    assert_eq!(mixed.keywords, derived.keywords);
}

#[test]
fn duties_follow_the_licence() {
    assert!(duties(Duty::None).is_empty());
    assert_eq!(
        duties(Duty::Attribute),
        vec![json!({"odrl:action": "odrl:attribute"})]
    );
    assert_eq!(duties(Duty::AttributeShareAlike).len(), 2);
}

fn slice(q: Option<&str>, license: Option<&str>, keywords: bool) -> CatalogDataset {
    let mut value = json!({
        "name": "koncerty",
        "title": {"en": "Concerts"},
        "description": {"en": "Every concert"},
        "types": ["Event", "Place"],
    });
    if let Some(q) = q {
        value["q"] = json!(q);
    }
    if let Some(license) = license {
        value["license"] = json!(license);
    }
    if keywords {
        value["keywords"] = json!({"en": ["music"]});
    }
    serde_json::from_value(value).expect("the slice parses")
}

#[test]
fn a_slice_catalog_takes_the_entry_s_keywords_and_licence_and_lists_no_datasets() {
    let f = Fixture::new();
    let mut catalog = f.catalog.clone();
    catalog.datasets = vec![slice(None, None, false)];
    let narrowed = slice_catalog(Some(&catalog), &slice(None, Some("CC0"), true));
    assert_eq!(
        narrowed.keywords,
        BTreeMap::from([("en".to_owned(), vec!["music".to_owned()])])
    );
    assert_eq!(
        narrowed.license.unwrap().iri(),
        "http://publications.europa.eu/resource/authority/licence/CC0"
    );
    assert!(narrowed.datasets.is_empty());
    let kept = slice_catalog(Some(&catalog), &slice(None, None, false));
    assert_eq!(kept.keywords, f.catalog.keywords);
    assert_eq!(kept.license, f.catalog.license);
    assert_eq!(
        slice_catalog(None, &slice(None, None, false)),
        Catalog::default()
    );
}

#[test]
fn a_slice_narrows_the_query_surfaces_and_hands_the_rest_to_the_service() {
    let f = Fixture::new();
    let subject = f.subject(Audience::Public, &ALL, Some(&f.catalog), &[]);
    let record = dataset_slice(&subject, &slice(None, Some("CC0"), false), &index(), BASE);
    let own = format!("{IRI}/datasets/koncerty");
    assert_eq!(record["@id"], own);
    assert_eq!(record["dct:identifier"], "koncerty");
    assert_eq!(record["dct:title"][0]["@value"], "Concerts");
    let urls: Vec<&str> = record["dcat:distribution"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d["dcat:accessURL"].as_str())
        .collect();
    assert!(
        urls.contains(&format!("{IRI}/ngsi-ld/v1/entities?type=Event%2CPlace").as_str()),
        "{urls:?}"
    );
    assert!(urls.contains(&format!("{IRI}/file.csv?type=Event%2CPlace").as_str()));
    assert!(urls.contains(&format!("{IRI}/ogc/features/collections/Event/items").as_str()));
    assert!(urls.contains(&format!("{IRI}/ogc/features/collections/Place/items").as_str()));
    assert!(!urls
        .iter()
        .any(|u| u.ends_with("/mcp") || u.ends_with("/ws") || u.contains("odata")));
    let schema = record["dcat:distribution"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["@id"] == format!("{own}#schema/v2/model.md"))
        .expect("the schema artifacts stay, under the dataset's own id");
    assert_eq!(
        schema["dct:license"],
        "http://publications.europa.eu/resource/authority/licence/CC0"
    );
    let service = included(&record, &format!("{IRI}#service"));
    assert_eq!(service.len(), 1);
    assert_eq!(service[0]["dcat:servesDataset"], own);
    // The service keeps the Endpoint's licence, and that licence is typed here too.
    assert_eq!(
        service[0]["dct:license"],
        "http://publications.europa.eu/resource/authority/licence/CC_BYSA_4_0"
    );
    assert_eq!(
        included(
            &record,
            "http://publications.europa.eu/resource/authority/licence/CC_BYSA_4_0"
        )[0]["@type"],
        "dct:LicenseDocument"
    );
    assert_eq!(
        included(
            &record,
            &format!("{IRI}/ogc/features/collections/Place/items")
        )[0]["@type"],
        "rdfs:Resource"
    );
}

#[test]
fn a_slice_with_a_filter_drops_the_ogc_items_and_carries_q() {
    let f = Fixture::new();
    let subject = f.subject(Audience::Public, &ALL, None, &[]);
    let record = dataset_slice(
        &subject,
        &slice(Some("genre==\"jazz\""), None, false),
        &json!({}),
        BASE,
    );
    let urls: Vec<&str> = record["dcat:distribution"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d["dcat:accessURL"].as_str())
        .collect();
    assert_eq!(urls.len(), 2, "{urls:?}");
    assert!(urls.iter().all(|u| u.contains("q=genre")), "{urls:?}");
}

#[test]
fn the_feed_gathers_records_once_and_skips_what_is_no_record() {
    let f = Fixture::new();
    let one = dataset(
        &f.subject(
            Audience::Public,
            &[Representation::NgsiLd],
            Some(&f.catalog),
            &[],
        ),
        &json!({}),
        BASE,
    );
    let catalog = feed(
        vec![
            one.clone(),
            one.clone(),
            json!("not a record"),
            json!({"no": "id"}),
        ],
        BASE,
        "city.example.org",
    );
    assert_eq!(catalog["@type"], "dcat:Catalog");
    assert_eq!(catalog["@id"], format!("{BASE}/catalog"));
    assert_eq!(catalog["dct:title"], "Open data of city.example.org");
    assert_eq!(catalog["dcat:dataset"].as_array().unwrap().len(), 2);
    let service = format!("{IRI}#service");
    assert_eq!(included(&catalog, &service).len(), 1, "a shared node once");
    let text = feed_turtle(&catalog);
    assert!(
        text.contains(&format!("<{BASE}/catalog> a dcat:Catalog")),
        "{text}"
    );
}

#[test]
fn a_publisher_feed_names_its_publisher_desk_and_home_page() {
    let project: ProjectCatalog = serde_json::from_value(json!({
        "publisher": {"name": {"en": "Transport office"}, "uri": "https://doprava.example.org/"},
        "contactPoint": {"name": "Desk", "email": "desk@example.org"}
    }))
    .expect("the project catalogue parses");
    let title = texts(&[("en", "Transport")]);
    let none = BTreeMap::new();
    let owner = CatalogueOwner {
        project: "doprava",
        title: &title,
        description: &none,
        domain: Some("doprava.example.org"),
        catalog: &project,
    };
    let catalog = publisher_feed(vec![], BASE, &owner);
    assert_eq!(catalog["@id"], format!("{BASE}/catalog/doprava"));
    assert_eq!(catalog["dct:title"][0]["@value"], "Transport");
    assert_eq!(
        catalog["dct:description"][0]["@value"], "Transport",
        "the title stands in"
    );
    assert_eq!(
        catalog["dct:publisher"]["@id"],
        "https://doprava.example.org/"
    );
    assert_eq!(
        catalog["dcat:contactPoint"]["vcard:hasEmail"],
        "mailto:desk@example.org"
    );
    assert_eq!(catalog["foaf:homepage"], "https://doprava.example.org/");
    assert_eq!(
        included(&catalog, "https://doprava.example.org/")[0]["@type"],
        "foaf:Document"
    );

    let bare: ProjectCatalog =
        serde_json::from_value(json!({"publisher": {"name": {"en": "Office"}}}))
            .expect("a publisher alone parses");
    let description = texts(&[("en", "What it publishes")]);
    let owner = CatalogueOwner {
        project: "urad",
        title: &none,
        description: &description,
        domain: None,
        catalog: &bare,
    };
    let catalog = publisher_feed(vec![], BASE, &owner);
    assert_eq!(
        catalog["dct:title"][0]["@value"], "Office",
        "the publisher's name stands in"
    );
    assert_eq!(catalog["dct:description"][0]["@value"], "What it publishes");
    assert!(catalog["dct:publisher"].get("@id").is_none());
    assert!(catalog.get("dcat:contactPoint").is_none());
    assert!(catalog.get("foaf:homepage").is_none());
}

#[test]
fn turtle_writes_typed_literals_language_tags_blank_nodes_and_escaped_iris() {
    let record = json!({
        "@id": "https://example.org/d",
        "@type": ["dcat:Dataset", "https://example.org/Other"],
        "dcat:byteSize": "12",
        "dct:title": [
            {"@value": "Názov", "@language": "sk"},
            {"@value": "no tag", "@language": "not a tag!"},
            {"@value": "2026-01-01", "@type": "xsd:date"}
        ],
        "dct:issued": 3,
        "dcat:hasVersion": true,
        "dct:creator": null,
        "dct:spatial": "dct:Location",
        "dct:temporal": {"@type": "dct:PeriodOfTime", "dcat:startDate": "2019-01-01"},
        "dct:publisher": {"@id": "https://example.org/p", "foaf:name": "P"},
        "dct:rightsHolder": {"@id": "https://example.org/bare"},
        "pu:osobní-údaje": {"@id": "https://example.org/x"},
        "unknown:key": "dropped",
        "dcat:accessURL": "https://example.org/a b<c>",
        "@included": [
            {"@id": "https://example.org/d", "@type": ["dcat:Dataset", "https://example.org/Other"], "dcat:byteSize": "12",
             "dct:title": [{"@value": "Názov", "@language": "sk"}, {"@value": "no tag", "@language": "not a tag!"}, {"@value": "2026-01-01", "@type": "xsd:date"}],
             "dct:issued": 3, "dcat:hasVersion": true, "dct:creator": null, "dct:spatial": "dct:Location",
             "dct:temporal": {"@type": "dct:PeriodOfTime", "dcat:startDate": "2019-01-01"},
             "dct:publisher": {"@id": "https://example.org/p", "foaf:name": "P"},
             "dct:rightsHolder": {"@id": "https://example.org/bare"},
             "pu:osobní-údaje": {"@id": "https://example.org/x"}, "unknown:key": "dropped",
             "dcat:accessURL": "https://example.org/a b<c>"},
            {"no": "id"},
            {"@id": "https://example.org/empty"}
        ]
    });
    let text = turtle(&record);
    assert!(text.starts_with("@prefix dcat: <http://www.w3.org/ns/dcat#> .\n"));
    assert!(
        text.contains("dcat:byteSize \"12\"^^xsd:nonNegativeInteger"),
        "{text}"
    );
    assert!(text.contains("\"Názov\"@sk"));
    assert!(
        text.contains("dct:title \"no tag\" ;"),
        "a bad tag is dropped: {text}"
    );
    assert!(text.contains("\"2026-01-01\"^^xsd:date"));
    assert!(text.contains("dct:issued \"3\""));
    assert!(text.contains("dcat:hasVersion true"));
    assert!(!text.contains("dct:creator"));
    assert!(text.contains("dct:spatial <http://purl.org/dc/terms/Location>"));
    assert!(text.contains("dct:temporal [ a dct:PeriodOfTime ;"));
    assert!(text.contains("dct:publisher <https://example.org/p>"));
    assert!(
        text.contains("<https://example.org/p> foaf:name \"P\" ."),
        "a named node is its own statement: {text}"
    );
    assert!(
        !text.contains("\n<https://example.org/bare>"),
        "an id alone is no statement"
    );
    assert!(text.contains(
        "<https://data.gov.cz/slovník/podmínky-užití/osobní-údaje> <https://example.org/x>"
    ));
    assert!(!text.contains("unknown"));
    assert!(text.contains("<https://example.org/a%20b%3Cc%3E>"));
    assert!(
        text.contains("a dcat:Dataset ;\n    a <https://example.org/Other>"),
        "{text}"
    );
    assert_eq!(
        text.matches("<https://example.org/d> a").count(),
        1,
        "the same node once: {text}"
    );
    assert!(!text.contains("example.org/empty"));
}

#[test]
fn turtle_of_a_value_that_is_no_node_is_the_prefixes_alone() {
    let text = turtle(&json!("text"));
    assert_eq!(text.lines().filter(|l| !l.is_empty()).count(), 15);
    assert!(turtle(&json!({"@id": 3})).ends_with("\n\n"));
}

#[test]
fn the_page_lists_each_distribution_its_publisher_licence_and_alternates() {
    let f = Fixture::new();
    let mut fixture = Fixture::new();
    fixture.title = texts(&[("en", "<b>City</b> & co")]);
    let subject = fixture.subject(Audience::Organization, &ALL, Some(&f.catalog), &[]);
    let page = dataset_html(&subject, &index(), BASE);
    assert!(
        page.contains("<title>&lt;b&gt;City&lt;/b&gt; &amp; co</title>"),
        "{page}"
    );
    assert!(page.contains("access restricted"));
    assert!(page.contains("<p>Published by City of Example.</p>"));
    assert!(page.contains("Licence: <a href=\"http://publications.europa.eu/resource/authority/licence/CC_BYSA_4_0\">"));
    assert!(page.contains(&format!(
        "<li><a href=\"{IRI}/mcp\">Model Context Protocol</a></li>"
    )));
    assert!(page.contains(" <a href=\"https://data.example.sk/docs/API/07-websocket\">Guide</a>"));
    assert!(page.contains("<br>Open wss://"));
    assert!(page.contains("type=\"text/calendar\" title=\"kalendar\""));
    assert!(page.contains("type=\"application/rss+xml\" title=\"kultura (RSS 2.0)\""));
    assert!(page.contains(&format!(
        "<li><a href=\"{IRI}/schema/v2/model.md\">events model.md</a></li>"
    )));
}

#[test]
fn the_page_of_a_public_endpoint_without_catalogue_says_public_and_names_no_publisher() {
    let f = Fixture::new();
    let page = dataset_html(
        &f.subject(Audience::Public, &[Representation::Csv], None, &[]),
        &json!({}),
        BASE,
    );
    assert!(page.contains("access public"));
    assert!(!page.contains("Published by"));
    assert!(!page.contains("Licence:"));
}

#[test]
fn localized_and_plain_pick_the_text_a_reader_gets() {
    assert_eq!(localized(&BTreeMap::new(), "fb"), json!("fb"));
    assert_eq!(localized(&texts(&[("", "plain")]), "fb"), json!("plain"));
    assert_eq!(
        localized(&texts(&[("", "plain"), ("sk", "text")]), "fb"),
        json!(["plain", {"@value": "text", "@language": "sk"}])
    );
    assert_eq!(
        plain(&texts(&[("en", "English"), ("sk", "Slovak")]), "fb"),
        "English"
    );
    assert_eq!(
        plain(&texts(&[("en", " "), ("sk", "Slovak")]), "fb"),
        "Slovak"
    );
    assert_eq!(plain(&texts(&[("en", "")]), "fb"), "fb");
}

#[test]
fn literal_escape_and_ws_address_handle_hostile_text() {
    assert_eq!(literal("a\"b\\c\nd\re"), "\"a\\\"b\\\\c\\nd\\re\"");
    assert_eq!(
        escape("<a href=\"x\">&</a>"),
        "&lt;a href=&quot;x&quot;&gt;&amp;&lt;/a&gt;"
    );
    assert_eq!(ws_address("https://h/ws"), "wss://h/ws");
    assert_eq!(ws_address("http://h/ws"), "ws://h/ws");
    assert_eq!(ws_address("ftp://h/ws"), "ftp://h/ws");
    assert_eq!(ws_address("no scheme"), "no scheme");
}

#[test]
fn major_of_reads_a_number_or_digits_as_text_and_nothing_else() {
    assert_eq!(major_of(&json!({"version": 4})), Some(4));
    assert_eq!(major_of(&json!({"version": "5"})), Some(5));
    assert_eq!(major_of(&json!({"version": "5.1"})), None);
    assert_eq!(major_of(&json!({"version": -1})), None);
    assert_eq!(major_of(&json!({})), None);
}
