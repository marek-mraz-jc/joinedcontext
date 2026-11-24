#![forbid(unsafe_code)]
//! Identifiers, enum spellings, text fallback, vocabulary IRIs and error mapping of the core model.

use dataroom_core::vocab::{
    eu_iri_to_language, iana_iri_to_media_type, language_to_eu_iri, media_type_to_iana_iri,
};
use dataroom_core::{
    text_in_lang, CatalogueId, DataroomError, DatasetFilter, HarvestRunState, HarvestSourceKind,
    OrgId, ShaclSeverity,
};
use serde_json::json;
use std::collections::BTreeMap;

#[test]
fn verified_ids_refuse_bad_and_nil_uuids() {
    let id = "0190f3a2-7c4e-7000-8000-00000000abcd";
    assert_eq!(OrgId::parse_verified(id).unwrap().to_string(), id);
    assert_eq!(CatalogueId::parse_verified(id).unwrap().to_string(), id);
    for (bad, msg) in [
        ("nope", "invalid UUID"),
        (
            "00000000-0000-0000-0000-000000000000",
            "nil UUID is not allowed",
        ),
    ] {
        match OrgId::parse_verified(bad).unwrap_err() {
            DataroomError::Validation { fields, .. } => {
                assert!(fields["org_id"][0].contains(msg), "{fields:?}")
            }
            other => panic!("{other:?}"),
        }
        match CatalogueId::parse_verified(bad).unwrap_err() {
            DataroomError::Validation { fields, .. } => {
                assert!(fields["catalogue_id"][0].contains(msg), "{fields:?}")
            }
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn enum_spellings_match_their_wire_names() {
    for kind in [
        HarvestSourceKind::DcatAp,
        HarvestSourceKind::NkodSparql,
        HarvestSourceKind::CkanAction,
        HarvestSourceKind::ArcGisHub,
        HarvestSourceKind::ArcGisRest,
        HarvestSourceKind::OgcRecords,
    ] {
        // JSON, the Postgres `kind` column and OData rows all use as_str().
        assert_eq!(serde_json::to_value(kind).unwrap(), json!(kind.as_str()));
        assert_eq!(
            serde_json::from_value::<HarvestSourceKind>(json!(kind.as_str())).unwrap(),
            kind
        );
    }
    // Documents written before the spelling was fixed still read.
    assert_eq!(
        serde_json::from_value::<HarvestSourceKind>(json!("arc-gis-hub")).unwrap(),
        HarvestSourceKind::ArcGisHub
    );
    for state in [
        HarvestRunState::Running,
        HarvestRunState::Succeeded,
        HarvestRunState::Failed,
    ] {
        assert_eq!(serde_json::to_value(state).unwrap(), json!(state.as_str()));
    }
    for sev in [
        ShaclSeverity::Info,
        ShaclSeverity::Warning,
        ShaclSeverity::Violation,
    ] {
        assert!(!sev.as_str().is_empty());
        assert_eq!(
            serde_json::from_value::<ShaclSeverity>(serde_json::to_value(sev).unwrap()).unwrap(),
            sev
        );
    }
    assert!(ShaclSeverity::Violation > ShaclSeverity::Warning);
}

#[test]
fn dataset_language_keys_must_be_language_tags() {
    let base = json!({
        "orgId": uuid::Uuid::nil(), "catalogueId": uuid::Uuid::nil(), "id": "", "name": "air",
        "title": {"sk": "Ovzdušie", "en-GB": "Air"}, "description": {},
        "publisherId": "city", "visibility": "public", "state": "active",
        "issued": "2026-11-01T10:00:00Z", "modified": "2026-11-02T10:00:00Z"
    });
    let ds: dataroom_core::Dataset = serde_json::from_value(base.clone()).unwrap();
    ds.validate_basic().unwrap();
    for (field, value, problem) in [
        (
            "title",
            json!({"en . <http://x/s> <http://x/p> \"x\"@en": "x"}),
            "language",
        ),
        ("description", json!({"": "x"}), "language"),
        ("keywords", json!({"s k": ["x"]}), "keywords"),
    ] {
        let mut v = base.clone();
        v[field] = value;
        let ds: dataroom_core::Dataset = serde_json::from_value(v).unwrap();
        let err = ds.validate_basic().unwrap_err();
        assert!(
            matches!(&err, dataroom_core::DataroomError::Validation { fields, .. } if fields.contains_key(problem)),
            "{field}: {err:?}"
        );
    }
}

#[test]
fn text_falls_back_through_en_sk_cs_then_any_non_empty() {
    let t = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect()
    };
    assert_eq!(
        text_in_lang(&t(&[("de", "Hallo"), ("sk", "Ahoj")]), Some("de")),
        Some("Hallo")
    );
    assert_eq!(
        text_in_lang(&t(&[("de", ""), ("sk", "Ahoj"), ("en", "Hi")]), Some("de")),
        Some("Hi")
    );
    assert_eq!(
        text_in_lang(&t(&[("cs", "Ahoj cz"), ("sk", "")]), None),
        Some("Ahoj cz")
    );
    assert_eq!(
        text_in_lang(&t(&[("de", ""), ("fr", "Salut")]), Some("it")),
        Some("Salut")
    );
    assert_eq!(text_in_lang(&t(&[("de", "")]), None), None);
}

#[test]
fn vocabulary_iris_go_both_ways() {
    for (code, iri) in [
        ("sk", "SLK"),
        ("cs", "CES"),
        ("en", "ENG"),
        ("fi", "FIN"),
        ("ca", "CAT"),
        ("de", "DEU"),
        ("hu", "HUN"),
        ("uk", "UKR"),
        ("fr", "FRA"),
        ("es", "SPA"),
    ] {
        let full = language_to_eu_iri(code);
        assert_eq!(
            full,
            format!("http://publications.europa.eu/resource/authority/language/{iri}")
        );
        assert_eq!(eu_iri_to_language(&full), code);
        assert_eq!(eu_iri_to_language(&format!("{full}/")), code);
    }
    assert_eq!(language_to_eu_iri("cz"), language_to_eu_iri("cs"));
    assert_eq!(
        eu_iri_to_language("http://publications.europa.eu/resource/authority/language/POL"),
        "pol"
    );
    assert_eq!(eu_iri_to_language("ita"), "ita");

    for iri in [
        "http://www.iana.org/assignments/media-types/text/csv",
        "https://www.iana.org/assignments/media-types/text/csv",
        "  text/csv ",
    ] {
        assert_eq!(iana_iri_to_media_type(iri), "text/csv");
    }
    assert_eq!(
        iana_iri_to_media_type(&media_type_to_iana_iri("application/json")),
        "application/json"
    );
}

#[test]
fn table_errors_map_to_dataroom_errors() {
    use odata_table::TableError;
    assert!(
        matches!(DataroomError::from(TableError::NotImplemented("$apply".into())), DataroomError::NotSupported(ref w) if w == "$apply")
    );
    match DataroomError::from(TableError::UnknownProperty("colour".into())) {
        DataroomError::Validation { fields, .. } => {
            assert_eq!(fields["colour"], vec!["unknown property".to_string()])
        }
        other => panic!("{other:?}"),
    }
    match DataroomError::from(TableError::BadRequest("bad $filter".into())) {
        DataroomError::Validation { message, fields } => {
            assert_eq!(message, "bad $filter");
            assert!(fields.is_empty());
        }
        other => panic!("{other:?}"),
    }
    let storage = DataroomError::from(TableError::Source("db down".into()));
    assert!(matches!(storage, DataroomError::Storage(ref m) if m == "db down"));
    assert_eq!(storage.status_code(), 500);
}

#[test]
fn every_error_names_its_problem_slug_and_status() {
    let cases = [
        (DataroomError::NotFound("x".into()), "not-found", 404),
        (DataroomError::Gone("x".into()), "gone", 410),
        (DataroomError::Forbidden("x".into()), "forbidden", 403),
        (DataroomError::Unauthorized("x".into()), "unauthorized", 401),
        (
            DataroomError::field_error("name", "bad"),
            "validation-error",
            422,
        ),
        (
            DataroomError::ShaclViolation {
                profile: "dcat-ap".into(),
                violations: 2,
                details: vec!["title".into()],
            },
            "shacl-violation",
            422,
        ),
        (DataroomError::Conflict("x".into()), "conflict", 409),
        (
            DataroomError::NotSupported("x".into()),
            "not-supported",
            501,
        ),
        (
            DataroomError::OutboundRefused("x".into()),
            "outbound-refused",
            400,
        ),
        (DataroomError::Rdf("x".into()), "rdf-error", 422),
        (
            DataroomError::RdfLimitExceeded {
                kind: "triples".into(),
                message: "over 100000".into(),
            },
            "rdf-limit-exceeded",
            422,
        ),
        (
            DataroomError::FeedUnavailable("x".into()),
            "feed-unavailable",
            503,
        ),
        (DataroomError::Storage("x".into()), "storage-error", 500),
    ];
    for (err, slug, status) in cases {
        assert_eq!((err.slug(), err.status_code()), (slug, status), "{err:?}");
        assert_eq!(
            err.problem_type_uri(),
            format!("urn:joinedcontext:problem:{slug}")
        );
    }
}

#[test]
fn vocabulary_spellings_fold_to_their_authority_iris() {
    use dataroom_core::vocab::{format_to_eu_iri, theme_label};
    assert_eq!(theme_label("ENVI", "cs"), "Životní prostředí");
    assert_ne!(theme_label("ENVI", "cs"), theme_label("ENVI", "sk"));
    assert_eq!(theme_label("nope", "cs"), "nope");
    let ft = "http://publications.europa.eu/resource/authority/file-type/";
    for (label, code) in [
        ("rdf/xml", "RDF_XML"),
        ("RDF", "RDF_XML"),
        ("ttl", "RDF_TURTLE"),
        ("Turtle", "RDF_TURTLE"),
        ("nt", "RDF_N_TRIPLES"),
        ("N-Triples", "RDF_N_TRIPLES"),
        ("json schema", "JSON"),
        ("geo json", "GEO_JSON"),
    ] {
        assert_eq!(format_to_eu_iri(label), format!("{ft}{code}"), "{label}");
    }
    assert_eq!(
        format_to_eu_iri(" https://example.org/csv "),
        "https://example.org/csv"
    );
    assert_eq!(
        language_to_eu_iri("pl"),
        "http://publications.europa.eu/resource/authority/language/PL"
    );
}

fn catalogue(extra: serde_json::Value) -> dataroom_core::Catalogue {
    let mut v = json!({
        "orgId": "0190f3a2-7c4e-7000-8000-0000000000a1",
        "id": "0190f3a2-7c4e-7000-8000-0000000000c1", "name": "city",
        "host": "data.city.example", "title": {"sk": "Mesto", "en": ""}, "description": {},
        "defaultLanguage": "sk", "languages": ["sk", "en"], "mainProfile": "dcat-ap-3.0.1",
        "activeProfiles": [], "modified": "2026-11-01T10:00:00Z"
    });
    for (k, val) in extra.as_object().cloned().unwrap_or_default() {
        v[k] = val;
    }
    serde_json::from_value(v).unwrap()
}

fn dataset(extra: serde_json::Value) -> dataroom_core::Dataset {
    let mut v = json!({
        "orgId": "0190f3a2-7c4e-7000-8000-0000000000a1",
        "catalogueId": "0190f3a2-7c4e-7000-8000-0000000000c1", "id": "d-1", "name": "ovzdusie",
        "title": {"sk": "Ovzdušie"}, "description": {}, "publisherId": "city",
        "visibility": "public", "state": "active", "tags": ["air"],
        "issued": "2026-11-01T10:00:00Z", "modified": "2026-11-02T10:00:00Z"
    });
    for (k, val) in extra.as_object().cloned().unwrap_or_default() {
        v[k] = val;
    }
    serde_json::from_value(v).unwrap()
}

#[test]
fn a_portal_catalogue_id_is_stable_per_organization_and_path() {
    let org = OrgId::parse_verified("0190f3a2-7c4e-7000-8000-0000000000a1").unwrap();
    let other = OrgId::parse_verified("0190f3a2-7c4e-7000-8000-0000000000a2").unwrap();
    let a = CatalogueId::of_portal(org, "/opendata");
    assert_eq!(a, CatalogueId::of_portal(org, "/opendata"));
    assert_ne!(a, CatalogueId::of_portal(org, "/maps"));
    assert_ne!(a, CatalogueId::of_portal(other, "/opendata"));
}

#[test]
fn a_catalogue_address_and_its_branding_layer_over_the_installation() {
    assert_eq!(catalogue(json!({})).base_url(), "https://data.city.example");
    let plain = catalogue(json!({"host": "http://localhost:8080/"}));
    assert_eq!(plain.base_url(), "http://localhost:8080");
    let mut based = catalogue(json!({}));
    based.public_base = Some("https://city.example/opendata/".into());
    assert_eq!(based.base_url(), "https://city.example/opendata");

    let themed = catalogue(json!({"theme": {
        "instanceName": "Mestské dáta", "organisation": "Mesto Žilina",
        "primary": "#112233", "secondary": "#223344", "accent": "#334455",
        "background": "#ffffff", "text": "#000000",
        "logo": "/assets/logo.svg", "tagline": "Otvorené dáta mesta"
    }}));
    let block = themed.effective_branding(None);
    assert_eq!(block.instance_name, "Mestské dáta");
    assert_eq!(block.organisation, "Mesto Žilina");
    assert_eq!(
        [
            &block.colours.primary,
            &block.colours.secondary,
            &block.colours.accent
        ],
        ["#112233", "#223344", "#334455"]
    );
    assert_eq!(
        (
            block.colours.background.as_str(),
            block.colours.text.as_str()
        ),
        ("#ffffff", "#000000")
    );
    assert_eq!(
        (block.logo.as_str(), block.tagline.as_str()),
        ("/assets/logo.svg", "Otvorené dáta mesta")
    );
    assert_eq!(block.languages.default, "sk");
    assert_eq!(block.languages.offered, ["sk", "en"]);
    assert_eq!(
        block.site_title.get("sk").map(String::as_str),
        Some("Mesto")
    );
    assert!(
        !block.site_title.contains_key("en"),
        "an empty title is not a title"
    );

    // a catalogue without languages keeps the installation's
    let bare = catalogue(json!({"defaultLanguage": "", "languages": []}));
    let installation = dataroom_core::BrandingBlock::default();
    let kept = bare.effective_branding(Some(&installation));
    assert_eq!(kept.languages, installation.languages);

    // tokens that are not colours, or the defaults, leave the installation's values
    let base = dataroom_core::BrandingBlock::default();
    let odd = catalogue(json!({"theme": {
        "instanceName": "dataroom", "organisation": "Open Data",
        "primary": "red", "secondary": "", "accent": "#12", "background": "url(x)", "text": "#ggg"
    }}));
    let kept = odd.effective_branding(Some(&base));
    assert_eq!(kept.instance_name, base.instance_name);
    assert_eq!(kept.organisation, base.organisation);
    assert_eq!(kept.colours, base.colours);
}

#[test]
fn a_publisher_iri_on_a_left_host_is_not_its_own() {
    let publisher: dataroom_core::Publisher = serde_json::from_value(json!({
        "orgId": "0190f3a2-7c4e-7000-8000-0000000000a1",
        "catalogueId": "0190f3a2-7c4e-7000-8000-0000000000c1", "id": "pub-city", "name": "city",
        "title": {"sk": "Mesto"}, "description": {},
        "uri": "https://old.city.example/publisher/city", "state": "active",
        "created": "2026-11-01T10:00:00Z", "modified": "2026-11-01T10:00:00Z"
    }))
    .unwrap();
    let mut undeclared = publisher.clone();
    undeclared.uri = "not an iri".into();
    assert!(!undeclared.has_own_iri(&catalogue(json!({}))));
    let moved = catalogue(json!({"leftHosts": ["old.city.example"]}));
    assert!(
        !publisher.has_own_iri(&moved),
        "minted on a host the catalogue left"
    );
    let elsewhere = catalogue(json!({"leftHosts": ["other.example"]}));
    assert!(publisher.has_own_iri(&elsewhere), "minted by someone else");
}

#[test]
fn a_declaration_reports_whether_it_changed_the_catalogue() {
    use dataroom_core::{CatalogueDeclaration, ProfilePin};
    let mut cat =
        catalogue(json!({"publisherUri": "https://data.gov.sk/id/legal-subject/00313271"}));
    assert_eq!(
        cat.declared_publisher_uri(),
        Some("https://data.gov.sk/id/legal-subject/00313271")
    );
    let pins = vec![ProfilePin {
        profile_id: "dcat-ap-sk".into(),
        version: "1".into(),
    }];
    let declared = CatalogueDeclaration {
        active_profiles: Some(pins.clone()),
        main_profile: Some("dcat-ap-sk".into()),
        ..CatalogueDeclaration::default()
    };
    assert!(declared.apply(&mut cat), "new pins and a new main profile");
    assert_eq!(
        (cat.active_profiles.clone(), cat.main_profile.as_str()),
        (pins, "dcat-ap-sk")
    );
    assert!(
        !declared.apply(&mut cat),
        "the same declaration again changes nothing"
    );
    let only_main = CatalogueDeclaration {
        main_profile: Some("dcat-ap-3.0.1".into()),
        ..CatalogueDeclaration::default()
    };
    assert!(only_main.apply(&mut cat));
    let only_pins = CatalogueDeclaration {
        active_profiles: Some(Vec::new()),
        ..CatalogueDeclaration::default()
    };
    assert!(only_pins.apply(&mut cat));
    assert!(!CatalogueDeclaration::default().apply(&mut cat));
    // a registry placeholder declares no publisher
    let placeholder = catalogue(json!({"publisherUri": "urn:placeholder"}));
    assert_eq!(placeholder.declared_publisher_uri(), None);
    assert_eq!(catalogue(json!({})).declared_publisher_uri(), None);
}

#[test]
fn language_tags_take_digits_only_after_the_first_subtag() {
    use dataroom_core::is_language_tag;
    for ok in ["sk", "en-GB", "es-419", "zh-Hant-TW"] {
        assert!(is_language_tag(ok), "{ok}");
    }
    for bad in [
        "",
        "1en",
        "e1",
        "en-",
        "toolonglang",
        "en-123456789",
        "en_GB",
    ] {
        assert!(!is_language_tag(bad), "{bad}");
    }
    assert_eq!(
        dataset(json!({})).dataset_type,
        "dataset",
        "a record without a type is a dataset"
    );
}

#[test]
fn a_dataset_iri_falls_back_to_its_extras_then_the_catalogue() {
    let mut d = dataset(json!({"uri": "  ", "extras": {"uri": " https://data.gov.sk/set/1 "}}));
    assert_eq!(
        d.canonical_uri("https://x.example/"),
        "https://data.gov.sk/set/1"
    );
    d.extras.insert("uri".into(), " ".into());
    assert_eq!(
        d.canonical_uri("https://x.example/"),
        "https://x.example/dataset/ovzdusie"
    );
}

#[test]
fn basic_validation_names_the_field_it_refuses() {
    assert!(dataset(json!({})).validate_basic().is_ok());
    for (extra, field, why) in [
        (
            json!({"title": {"sk": "  "}}),
            "title",
            "at least one non-empty language title",
        ),
        (
            json!({"publisherId": " "}),
            "owner_org",
            "Organization does not exist",
        ),
        (
            json!({"tags": ["a"]}),
            "tags",
            "between 2 and 100 characters",
        ),
        (
            json!({"tags": ["x".repeat(101)]}),
            "tags",
            "between 2 and 100 characters",
        ),
        (
            json!({"tags": ["air;drop"]}),
            "tags",
            "contains invalid characters",
        ),
        (
            json!({"keywords": {"not a tag": ["x"]}}),
            "keywords",
            "is not a language tag",
        ),
    ] {
        match dataset(extra.clone()).validate_basic().unwrap_err() {
            DataroomError::Validation { fields, .. } => {
                assert!(fields[field][0].contains(why), "{extra}: {fields:?}")
            }
            other => panic!("{extra}: {other:?}"),
        }
    }
}

#[test]
fn the_public_filter_pages_by_twenty() {
    let f = DatasetFilter::public_default();
    assert_eq!(f.limit, 20);
    assert_eq!(f.offset, 0);
    assert!(!f.include_drafts && !f.include_withdrawn);
    assert!(f.q.is_empty());
}
