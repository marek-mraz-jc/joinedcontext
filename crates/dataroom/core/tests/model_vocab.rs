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
fn the_public_filter_pages_by_twenty() {
    let f = DatasetFilter::public_default();
    assert_eq!(f.limit, 20);
    assert_eq!(f.offset, 0);
    assert!(!f.include_drafts && !f.include_withdrawn);
    assert!(f.q.is_empty());
}
