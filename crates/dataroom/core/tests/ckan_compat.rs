#![forbid(unsafe_code)]
//! CKAN Action API dictionaries: what `package_show` writes, what `package_create/update` reads,
//! and that a show -> update cycle keeps what it showed (jcctl, the Portal, od-standards.py).

use chrono::NaiveDate;
use dataroom_core::ckan_compat::{
    ckan_license_list, dataset_from_ckan, dataset_to_ckan, distribution_from_ckan,
    distribution_to_ckan, resource_view_to_ckan,
};
use dataroom_core::{
    AgentDetails, CatalogueId, ContactKind, ContactPoint, DataService, Dataset, OrgId, Publisher,
    RecordState, ResourceView, TemporalPeriod, Visibility,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use uuid::Uuid;

fn ids() -> (OrgId, CatalogueId) {
    (
        OrgId::from_uuid(Uuid::from_u128(0xc1)),
        CatalogueId::from_uuid(Uuid::from_u128(0xc2)),
    )
}

fn texts(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

fn minimal(name: &str) -> Dataset {
    let (org, cat) = ids();
    serde_json::from_value(json!({
        "orgId": org, "catalogueId": cat, "id": format!("pkg-{name}"), "name": name,
        "title": {"sk": format!("Dataset {name}")}, "description": {}, "publisherId": "mesto",
        "visibility": "public", "state": "active",
        "issued": "2026-11-01T10:00:00Z", "modified": "2026-11-02T10:00:00Z"
    }))
    .unwrap()
}

fn full() -> Dataset {
    let mut ds = minimal("ovzdusie");
    ds.title = texts(&[("sk", "Kvalita ovzdušia"), ("en", "Air quality")]);
    ds.description = texts(&[("sk", "Merania"), ("en", "Measurements")]);
    ds.identifier = Some("00313271-ovz".into());
    ds.frequency = Some("http://publications.europa.eu/resource/authority/frequency/DAILY".into());
    ds.themes = vec!["http://publications.europa.eu/resource/authority/data-theme/ENVI".into()];
    ds.spatial = vec![
        "http://data.europa.eu/nuts/code/SK032".into(),
        "http://data.europa.eu/nuts/code/SK031".into(),
    ];
    ds.temporal = Some(TemporalPeriod {
        start: NaiveDate::from_ymd_opt(2020, 1, 1),
        end: NaiveDate::from_ymd_opt(2025, 12, 31),
    });
    ds.contact_point = Some(ContactPoint {
        uri: None,
        kind: ContactKind::Organization,
        name: texts(&[("sk", "Oddelenie dát")]),
        email: "mailto:data@mesto.sk".into(),
        url: None,
        identifier: None,
    });
    ds.publisher = Some(AgentDetails {
        uri: Some("https://data.gov.sk/id/legal-subject/00313271".into()),
        name: texts(&[("sk", "Mesto")]),
        email: None,
        homepage: None,
        agent_type: None,
        identifier: None,
    });
    ds.endpoint_url = Some("https://api.example.org/endpoint/x/".into());
    ds.generated_by = Some("urn:jc:generator".into());
    ds.licence_id = Some("cc-by".into());
    ds.licence_url = Some("https://creativecommons.org/licenses/by/4.0/".into());
    ds.harvest_source_id = Some("hs-nkod".into());
    ds.harvest_source_item_id = Some("remote-1".into());
    ds.harvest_hash = Some("abc123".into());
    ds.tags = vec!["ovzdušie".into(), "PM10".into()];
    ds.landing_page = Some("https://mesto.sk/ovzdusie".into());
    ds.version = Some("2.0".into());
    ds.visibility = Visibility::Restricted;
    ds.extras.insert("custom".into(), "kept".into());
    let mut dist = serde_json::from_value::<dataroom_core::Distribution>(json!({
        "id": "res-1", "datasetId": "pkg-ovzdusie",
        "title": {"sk": "CSV", "en": "CSV file"}, "description": {"sk": "Denné merania"},
        "accessUrl": "https://mesto.sk/ovzdusie", "downloadUrl": "https://mesto.sk/ovzdusie.csv",
        "format": "CSV", "mediaType": "text/csv", "byteSize": 1234,
        "hash": "deadbeef", "hashAlgorithm": "sha-256", "resourceType": "file",
        "conformsTo": ["https://example.org/schema-a", "https://example.org/schema-b"]
    }))
    .unwrap();
    dist.uri = Some("https://mesto.sk/id/res-1".into());
    dist.compress_format =
        Some("http://www.iana.org/assignments/media-types/application/gzip".into());
    dist.package_format =
        Some("http://publications.europa.eu/resource/authority/file-type/TAR".into());
    dist.licence = Some("https://creativecommons.org/licenses/by/4.0/".into());
    dist.status = Some("http://purl.org/adms/status/Completed".into());
    dist.availability = Some("STABLE".into());
    dist.temporal_resolution = Some("P1D".into());
    dist.spatial_resolution_in_meters = Some(10.5);
    dist.access_services = vec![DataService {
        id: "svc-res-1-0".into(),
        uri: Some("https://mesto.sk/api".into()),
        title: texts(&[("sk", "API")]),
        endpoint_urls: vec!["https://mesto.sk/api/v1".into()],
        ..DataService::default()
    }];
    dist.extras.insert("url_type".into(), json!("datastore"));
    ds.distributions = vec![dist];
    ds
}

fn publisher() -> Publisher {
    let (org, cat) = ids();
    serde_json::from_value(json!({
        "orgId": org, "catalogueId": cat, "id": "pub-mesto", "name": "mesto",
        "title": {"sk": "Mesto Banská Bystrica"}, "description": {"sk": "Popis mesta"},
        "uri": "https://example.org/p/mesto", "logoUrl": "https://example.org/logo.png",
        "state": "active", "editorGrants": [],
        "created": "2026-11-01T10:00:00Z", "modified": "2026-11-01T10:00:00Z"
    }))
    .unwrap()
}

fn extra<'a>(pkg: &'a Value, key: &str) -> Option<&'a str> {
    pkg["extras"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["key"] == key)
        .and_then(|e| e["value"].as_str())
}

#[test]
fn package_show_carries_every_field_jcctl_and_the_portal_read() {
    let ds = full();
    let views = BTreeMap::from([(
        "res-1".to_string(),
        vec![serde_json::from_value::<ResourceView>(json!({
            "orgId": ds.org_id, "catalogueId": ds.catalogue_id,
            "id": "v1", "resourceId": "res-1", "viewType": "datatables_view",
            "title": "Tabuľka", "description": "", "order": 0, "config": {"filterable": true}
        }))
        .unwrap()],
    )]);
    let pkg = dataset_to_ckan(&ds, Some(&publisher()), &views, "sk");
    assert_eq!(pkg["title"], "Kvalita ovzdušia");
    assert_eq!(pkg["title_translated"]["en"], "Air quality");
    assert_eq!(pkg["notes"], "Merania");
    assert_eq!(pkg["owner_org"], "mesto");
    assert_eq!(pkg["organization"]["title"], "Mesto Banská Bystrica");
    assert_eq!(pkg["organization"]["description"], "Popis mesta");
    assert_eq!(
        pkg["organization"]["image_url"],
        "https://example.org/logo.png"
    );
    assert_eq!(pkg["private"], true);
    assert_eq!(pkg["visibility"], "restricted");
    assert_eq!(pkg["state"], "active");
    assert_eq!(pkg["license_id"], "cc-by");
    assert_eq!(
        pkg["license_url"],
        "https://creativecommons.org/licenses/by/4.0/"
    );
    assert_eq!(pkg["license_link"], pkg["license_url"]);
    assert!(pkg["license_title"].as_str().is_some_and(|t| !t.is_empty()));
    assert_eq!(pkg["maintainer"], "Oddelenie dát");
    assert_eq!(pkg["maintainer_email"], "data@mesto.sk");
    assert_eq!(pkg["url"], "https://mesto.sk/ovzdusie");
    assert_eq!(pkg["version"], "2.0");
    assert_eq!(pkg["num_tags"], 2);
    assert_eq!(pkg["tags"][1]["name"], "PM10");
    assert_eq!(pkg["num_resources"], 1);
    for (key, value) in [
        ("identifier", "00313271-ovz"),
        (
            "frequency",
            "http://publications.europa.eu/resource/authority/frequency/DAILY",
        ),
        ("temporal_start", "2020-01-01"),
        ("temporal_end", "2025-12-31"),
        ("contact_name", "Oddelenie dát"),
        ("contact_email", "data@mesto.sk"),
        ("publisher_name", "Mesto"),
        (
            "publisher_uri",
            "https://data.gov.sk/id/legal-subject/00313271",
        ),
        ("endpoint", "https://api.example.org/endpoint/x/"),
        ("generated_by", "urn:jc:generator"),
        (
            "license_url",
            "https://creativecommons.org/licenses/by/4.0/",
        ),
        ("harvest_source", "hs-nkod"),
        ("source_id", "remote-1"),
        ("harvest_hash", "abc123"),
        ("custom", "kept"),
    ] {
        assert_eq!(extra(&pkg, key), Some(value), "extra {key}");
    }
    let spatial: Vec<String> = serde_json::from_str(extra(&pkg, "spatial_uri").unwrap()).unwrap();
    assert_eq!(spatial, ds.spatial);
    let themes: Vec<String> = serde_json::from_str(extra(&pkg, "theme").unwrap()).unwrap();
    assert_eq!(themes, ds.themes);

    let res = &pkg["resources"][0];
    assert_eq!(res["url"], "https://mesto.sk/ovzdusie.csv");
    assert_eq!(res["access_url"], "https://mesto.sk/ovzdusie");
    assert_eq!(res["name"], "CSV");
    assert_eq!(res["description"], "Denné merania");
    assert_eq!(res["package_id"], "pkg-ovzdusie");
    assert_eq!(res["hash_algorithm"], "sha-256");
    assert_eq!(res["conforms_to"].as_array().unwrap().len(), 2);
    assert_eq!(res["has_views"], true);
    assert_eq!(res["url_type"], "datastore");
    assert_eq!(
        res["access_services"][0]["endpoint_url"][0],
        "https://mesto.sk/api/v1"
    );
    assert_eq!(res["spatial_resolution_in_meters"], 10.5);
}

#[test]
fn package_show_then_update_keeps_what_it_showed() {
    let (org, cat) = ids();
    let ds = full();
    let pkg = dataset_to_ckan(&ds, Some(&publisher()), &BTreeMap::new(), "sk");
    let back = dataset_from_ckan(org, cat, &pkg, None, "sk");
    assert_eq!(back.id, ds.id);
    assert_eq!(back.name, ds.name);
    assert_eq!(back.title, ds.title);
    assert_eq!(back.description, ds.description);
    assert_eq!(back.publisher_id, "mesto");
    assert_eq!(back.visibility, Visibility::Restricted);
    assert_eq!(back.state, RecordState::Active);
    assert_eq!(back.tags, ds.tags);
    assert_eq!(back.keywords.get("sk"), Some(&ds.tags));
    assert_eq!(back.themes, ds.themes);
    assert_eq!(back.spatial, ds.spatial);
    assert_eq!(back.temporal, ds.temporal);
    let cp = back.contact_point.unwrap();
    assert_eq!(cp.email, "data@mesto.sk");
    assert_eq!(cp.name.get("sk").map(String::as_str), Some("Oddelenie dát"));
    assert_eq!(back.publisher.unwrap().uri, ds.publisher.unwrap().uri);
    assert_eq!(back.licence_id, ds.licence_id);
    assert_eq!(back.licence_url, ds.licence_url);
    assert_eq!(back.identifier, ds.identifier);
    assert_eq!(back.frequency, ds.frequency);
    assert_eq!(back.endpoint_url, ds.endpoint_url);
    assert_eq!(back.generated_by, ds.generated_by);
    assert_eq!(back.harvest_source_id, ds.harvest_source_id);
    assert_eq!(back.harvest_source_item_id, ds.harvest_source_item_id);
    assert_eq!(back.harvest_hash, ds.harvest_hash);
    assert_eq!(back.landing_page, ds.landing_page);
    assert_eq!(back.version, ds.version);
    assert_eq!(back.extras.get("custom").map(String::as_str), Some("kept"));

    // The resource comes back whole, unknown CKAN keys (url_type) included.
    let mut want = ds.distributions[0].clone();
    let got = &back.distributions[0];
    // package_show writes the download URL as `url`; the access URL survives as `access_url`
    // only when no download URL exists, as CKAN has one `url`.
    want.access_url = want.download_url.clone().unwrap();
    assert_eq!(got, &want);
}

#[test]
fn an_update_keeps_the_stored_values_it_does_not_name() {
    let (org, cat) = ids();
    let stored = full();
    let patch = json!({"name": "ovzdusie"});
    let back = dataset_from_ckan(org, cat, &patch, Some(&stored), "sk");
    assert_eq!(back.id, stored.id);
    assert_eq!(back.title, stored.title);
    assert_eq!(back.description, stored.description);
    assert_eq!(back.publisher_id, stored.publisher_id);
    assert_eq!(back.extras, stored.extras);
    assert_eq!(back.visibility, stored.visibility);
    assert_eq!(back.state, stored.state);
    assert_eq!(back.tags, stored.tags);
    assert_eq!(back.temporal, stored.temporal);
    assert_eq!(back.contact_point, stored.contact_point);
    assert_eq!(back.publisher, stored.publisher);
    assert_eq!(back.licence_id, stored.licence_id);
    assert_eq!(back.licence_url, stored.licence_url);
    assert_eq!(back.distributions, stored.distributions);
    assert_eq!(back.issued, stored.issued);
    assert_eq!(back.restricted_grants, stored.restricted_grants);

    // New resources keep the stored resource ids by position; extra ones get fresh ids.
    let with_res = json!({"resources": [{"url": "https://a/1"}, {"url": "https://a/2"}]});
    let back = dataset_from_ckan(org, cat, &with_res, Some(&stored), "sk");
    assert_eq!(back.name, stored.name);
    assert_eq!(back.distributions[0].id, "res-1");
    assert_eq!(back.distributions[1].id, "res-pkg-ovzdusie-1");
    // Without a title or an existing record, the name is the title.
    let fresh = dataset_from_ckan(org, cat, &json!({"name": "novy"}), None, "sk");
    assert_eq!(fresh.title.get("sk").map(String::as_str), Some("novy"));
    assert!(fresh.id.starts_with("pkg-"));
    assert_eq!(fresh.visibility, Visibility::Public);
    assert_eq!(fresh.state, RecordState::Active);
}

#[test]
fn package_payload_variants_parse_like_ckan() {
    let (org, cat) = ids();
    let pkg = json!({
        "name": "var",
        "title_translated": {"sk": "Názov", "en": "Title", "cs": ""},
        "notes_translated": {"sk": "Starý popis", "en": "Description", "cs": ""},
        "notes": "Nový popis",
        "organization": {"name": "kraj"},
        "visibility": "internal",
        "state": "draft",
        "tags": [{"name": "a"}, "b", {"name": "a"}, "b", 7],
        "tags_translated": {"en": ["one", "two"], "cs": [], "de": "not-a-list"},
        "theme": ["http://example.org/theme/T1"],
        "extras": [
            {"key": "spatial", "value": "[\"{\\\"type\\\":\\\"Point\\\"}\", \"http://example.org/place\"]"},
            {"key": "temporal_start", "value": "2021-03-04T00:00:00"},
            {"key": "issued", "value": "2020-05-06T07:08:09Z"},
            {"key": "language", "value": "http://publications.europa.eu/resource/authority/language/SLK"},
            {"key": "conforms_to", "value": "[\"https://example.org/s1\", \"https://example.org/s2\"]"},
            {"key": "spatial_resolution_in_meters", "value": "25"},
            {"key": "publisher_email", "value": "p@kraj.sk"},
            {"key": "source_uri", "value": "https://source.example.org/var"},
            {"key": "numeric", "value": 42},
            {"key": "nothing", "value": null},
            {"value": "no key"}
        ],
        "maintainer": "Správca",
        "maintainer_email": "spravca@kraj.sk",
        "license_link": "https://example.org/licence",
        "restricted_grants": ["group-a", 3],
        "resources": [{
            "access_url": "https://kraj.sk/data",
            "name_translated": {"en": "Data", "sk": ""},
            "name": "Dáta",
            "description_translated": {"en": "All data", "sk": ""},
            "description": "Všetky dáta",
            "conforms_to": "https://example.org/schema",
            "access_services": [{"title": "Služba"}, {}],
            "datastore_active": true,
            "dropped": null
        }]
    });
    let ds = dataset_from_ckan(org, cat, &pkg, None, "sk");
    assert_eq!(ds.title, texts(&[("sk", "Názov"), ("en", "Title")]));
    assert_eq!(
        ds.description,
        texts(&[("sk", "Nový popis"), ("en", "Description")])
    );
    assert_eq!(ds.publisher_id, "kraj");
    assert_eq!(ds.visibility, Visibility::Internal);
    assert_eq!(ds.state, RecordState::Draft);
    assert_eq!(ds.tags, vec!["a".to_string(), "b".to_string()]);
    assert_eq!(ds.keywords.len(), 1);
    assert_eq!(
        ds.keywords["en"],
        vec!["one".to_string(), "two".to_string()]
    );
    assert_eq!(ds.themes, vec!["http://example.org/theme/T1".to_string()]);
    // GeoJSON in `spatial` is not a place IRI.
    assert_eq!(ds.spatial, vec!["http://example.org/place".to_string()]);
    let t = ds.temporal.unwrap();
    assert_eq!(t.start, NaiveDate::from_ymd_opt(2021, 3, 4));
    assert_eq!(t.end, None);
    assert_eq!(ds.issued.to_rfc3339(), "2020-05-06T07:08:09+00:00");
    assert_eq!(ds.languages.len(), 1);
    assert_eq!(ds.conforms_to.len(), 2);
    assert_eq!(ds.spatial_resolution_in_meters, Some(25.0));
    assert_eq!(
        ds.publisher, None,
        "an email alone names no publisher agent"
    );
    assert_eq!(ds.uri.as_deref(), Some("https://source.example.org/var"));
    assert_eq!(ds.extras.get("numeric").map(String::as_str), Some("42"));
    assert!(!ds.extras.contains_key("nothing"));
    let cp = ds.contact_point.unwrap();
    assert_eq!(cp.email, "spravca@kraj.sk");
    assert_eq!(cp.name["sk"], "Správca");
    assert_eq!(
        ds.licence_url.as_deref(),
        Some("https://example.org/licence")
    );
    assert_eq!(ds.restricted_grants, vec!["group-a".to_string()]);
    let dist = &ds.distributions[0];
    assert_eq!(dist.id, format!("res-{}-0", ds.id));
    assert_eq!(dist.access_url, "https://kraj.sk/data");
    assert_eq!(dist.title, texts(&[("en", "Data"), ("sk", "Dáta")]));
    assert_eq!(
        dist.description,
        texts(&[("en", "All data"), ("sk", "Všetky dáta")])
    );
    assert_eq!(
        dist.conforms_to,
        vec!["https://example.org/schema".to_string()]
    );
    assert_eq!(dist.access_services.len(), 2);
    assert_eq!(dist.access_services[0].title["sk"], "Služba");
    assert!(dist.access_services[1].endpoint_urls.is_empty());
    assert_eq!(dist.extras.get("datastore_active"), Some(&json!(true)));
    assert!(!dist.extras.contains_key("dropped"));

    // `private` decides when `visibility` is absent; unknown visibility and state words fall back.
    for (payload, vis, state) in [
        (
            json!({"name": "p", "private": true, "state": "deleted"}),
            Visibility::Internal,
            RecordState::Withdrawn,
        ),
        (
            json!({"name": "p", "private": false, "state": "withdrawn"}),
            Visibility::Public,
            RecordState::Withdrawn,
        ),
        (
            json!({"name": "p", "visibility": "restricted", "state": "active"}),
            Visibility::Restricted,
            RecordState::Active,
        ),
        (
            json!({"name": "p", "visibility": "odd", "state": "odd"}),
            Visibility::Public,
            RecordState::Active,
        ),
    ] {
        let ds = dataset_from_ckan(org, cat, &payload, None, "sk");
        assert_eq!((ds.visibility, ds.state), (vis, state), "{payload}");
    }
}

#[test]
fn resources_views_and_licences_convert_alone() {
    // A resource with no title and no download URL shows its access URL for both.
    let dist = distribution_from_ckan(
        &json!({"url": "https://x/a.csv", "id": ""}),
        "pkg-x",
        "res-fallback",
        "sk",
    );
    assert_eq!(dist.id, "res-fallback");
    let shown = distribution_to_ckan(&dist, "pkg-x", 3, &[], "sk");
    assert_eq!(shown["name"], "https://x/a.csv");
    assert_eq!(shown["url"], "https://x/a.csv");
    assert_eq!(shown["position"], 3);
    assert_eq!(shown["has_views"], false);
    assert_eq!(shown["format"], "");
    assert!(shown.get("download_url").is_none());
    assert!(shown.get("conforms_to").is_none());
    assert!(shown.get("access_services").is_none());
    let single = distribution_from_ckan(
        &json!({"url": "https://x/b", "conforms_to": ["https://s/1"]}),
        "pkg-x",
        "res-b",
        "sk",
    );
    assert_eq!(
        distribution_to_ckan(&single, "pkg-x", 0, &[], "sk")["conforms_to"],
        "https://s/1"
    );

    let view: ResourceView = serde_json::from_value(json!({
        "orgId": Uuid::nil(), "catalogueId": Uuid::nil(),
        "id": "v1", "resourceId": "r1", "viewType": "image_view", "title": "Obrázok",
        "description": "Popis", "order": 2, "config": {"image_url": "https://x/i.png"}
    }))
    .unwrap();
    let v = resource_view_to_ckan(&view);
    assert_eq!(v["view_type"], "image_view");
    assert_eq!(v["order"], 2);
    assert_eq!(v["image_url"], "https://x/i.png");

    let licences = ckan_license_list();
    let ccby = licences.iter().find(|l| l["id"] == "cc-by").expect("cc-by");
    assert_eq!(ccby["od_conformance"], "approved");
    assert!(licences.iter().any(|l| l["is_generic"] == true));

    // A dataset without publisher record or licence shows its publisher id and no licence link.
    let mut bare = minimal("bare");
    bare.licence_url = Some("https://creativecommons.org/licenses/by/4.0/".into());
    let pkg = dataset_to_ckan(&bare, None, &BTreeMap::new(), "sk");
    assert_eq!(pkg["organization"]["name"], "mesto");
    assert_eq!(pkg["notes"], Value::Null);
    assert_eq!(pkg["maintainer_email"], Value::Null);
    assert!(pkg["license_id"].as_str().is_some(), "{pkg}");
    let mut one_place = minimal("place");
    one_place.spatial = vec!["http://data.europa.eu/nuts/code/SK032".into()];
    let pkg = dataset_to_ckan(&one_place, None, &BTreeMap::new(), "sk");
    assert_eq!(
        extra(&pkg, "spatial_uri"),
        Some("http://data.europa.eu/nuts/code/SK032")
    );
    assert_eq!(pkg["license_id"], Value::Null);
}
