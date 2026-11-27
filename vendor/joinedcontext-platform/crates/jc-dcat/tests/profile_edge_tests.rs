//! The national profiles over records of an odd shape (coverage T-7896): a record is whatever
//! the gateway handed in, so every part the profile reads may be missing.

#[cfg(any(feature = "dcat-ap-sk", feature = "dcat-ap-cz"))]
use jc_core::kinds::Catalog;
use jc_dcat::profile::{render, render_catalog, Profile};
use serde_json::json;
#[cfg(any(feature = "dcat-ap-sk", feature = "dcat-ap-cz"))]
use serde_json::Value;

#[cfg(any(feature = "dcat-ap-sk", feature = "dcat-ap-cz"))]
fn bare_catalog() -> Catalog {
    Catalog::default()
}

#[test]
fn dcat_ap_hands_a_record_and_a_catalogue_back_unchanged() {
    let record = json!({"dcat:distribution": [{"dct:title": "x"}]});
    assert_eq!(render(record.clone(), Profile::DcatAp, None), record);
    assert_eq!(render_catalog(record.clone(), Profile::DcatAp), record);
}

#[cfg(feature = "dcat-ap-sk")]
mod slovak {
    use super::*;

    const FILE_TYPE: &str = "http://publications.europa.eu/resource/authority/file-type/";

    #[test]
    fn a_record_with_nothing_to_read_stays_without_distributions_or_nodes() {
        let out = render(json!({"@id": "https://x/d"}), Profile::DcatApSk, None);
        assert_eq!(out, json!({"@id": "https://x/d"}));
        let out = render(json!({}), Profile::DcatApSk, Some(&bare_catalog()));
        assert_eq!(out, json!({}));
    }

    #[test]
    fn a_title_that_is_already_tagged_or_missing_is_left_and_a_distribution_without_url_is_skipped()
    {
        let record = json!({
            "@id": "https://x/d",
            "dcat:distribution": [
                {"dct:title": [{"@value": "Hotovo", "@language": "en"}]},
                "not an object",
                {"dct:title": "Bez adresy"}
            ],
            "dcat:contactPoint": {"vcard:hasEmail": "a@b.sk"}
        });
        let out = render(record, Profile::DcatApSk, None);
        let distributions = out["dcat:distribution"].as_array().expect("array");
        assert_eq!(distributions[0]["dct:title"][0]["@language"], "en");
        assert_eq!(distributions[2]["dct:title"][0]["@language"], "sk");
        assert!(distributions[2].get("dcat:downloadURL").is_none());
        assert!(out["dcat:contactPoint"].get("vcard:fn").is_none());
    }

    #[test]
    fn a_file_of_an_unknown_media_type_or_with_a_format_gets_no_eu_code() {
        let record = json!({
            "@id": "https://x/d",
            "dcat:distribution": [
                {"dcat:accessURL": "https://x/file.bin", "dcat:mediaType": "application/octet-stream"},
                {"dcat:accessURL": "https://x/file.dat"},
                {"dcat:accessURL": "https://x/file.csv", "dcat:mediaType": "text/csv", "dct:format": "own"},
                {"dcat:accessURL": "https://x/file.json", "dcat:mediaType": "application/json"}
            ]
        });
        let out = render(record, Profile::DcatApSk, None);
        let d = out["dcat:distribution"].as_array().expect("array");
        assert!(d[0].get("dct:format").is_none());
        assert!(d[1].get("dct:format").is_none());
        assert_eq!(d[2]["dct:format"], "own");
        assert_eq!(d[3]["dct:format"], format!("{FILE_TYPE}JSON"));
        assert_eq!(d[0]["dcat:downloadURL"], "https://x/file.bin");
        // The record names no `@included`: the code list entry has nowhere to go and no
        // service node is made for a file.
        assert!(out.get("@included").is_none());
    }

    #[test]
    fn two_files_of_one_type_list_its_eu_code_once() {
        let record = json!({
            "dcat:distribution": [
                {"dcat:accessURL": "https://x/file.csv", "dcat:mediaType": "text/csv"},
                {"dcat:accessURL": "https://x/file.csv?a=1", "dcat:mediaType": "text/csv"}
            ],
            "@included": []
        });
        let out = render(record, Profile::DcatApSk, None);
        let code = format!("{FILE_TYPE}CSV");
        let listed = out["@included"]
            .as_array()
            .expect("array")
            .iter()
            .filter(|n| n["@id"] == code.as_str())
            .count();
        assert_eq!(listed, 1);
    }

    #[test]
    fn an_interface_without_conformance_licence_or_rights_has_a_bare_data_service() {
        let record = json!({
            "@id": "https://x/d",
            "dcat:distribution": [
                {"dcat:accessURL": "https://x/api/", "dcat:accessService": "old", "dct:title": "API"}
            ],
            "@included": [{"@id": "dcat-dataset", "@type": "dcat:Dataset"}]
        });
        let out = render(record, Profile::DcatApSk, None);
        let service = out["@included"]
            .as_array()
            .expect("array")
            .iter()
            .find(|n| n["@type"] == "dcat:DataService")
            .expect("a data service");
        assert_eq!(service["@id"], "https://x/api/#service");
        assert_eq!(service["dcat:endpointURL"], "https://x/api/");
        assert!(service.get("dct:conformsTo").is_none());
        assert!(service.get("dct:license").is_none());
        assert!(service.get("dct:accessRights").is_none());
        assert_eq!(service["dct:title"][0]["@language"], "sk");
    }

    #[test]
    fn a_source_dataset_is_untyped_only_when_the_record_names_it() {
        let record = json!({
            "@id": "https://x/d",
            "dct:source": ["https://other/d"],
            "@included": [
                {"@id": "https://other/d", "@type": "dcat:Dataset"},
                {"@id": "https://x/mine", "@type": "dcat:Dataset"},
                {"@id": "https://other/d", "@type": "dcat:DataService"}
            ]
        });
        let out = render(record, Profile::DcatApSk, None);
        let nodes = out["@included"].as_array().expect("array");
        assert!(nodes[0].get("@type").is_none());
        assert_eq!(nodes[1]["@type"], "dcat:Dataset");
        assert_eq!(nodes[2]["@type"], "dcat:DataService");
        // A record naming no source untypes nothing.
        let none = render(
            json!({"@included": [{"@id": "a", "@type": "dcat:Dataset"}]}),
            Profile::DcatApSk,
            None,
        );
        assert_eq!(none["@included"][0]["@type"], "dcat:Dataset");
    }

    #[test]
    fn the_catalogue_contact_is_tagged_when_there_is_one() {
        let with = render_catalog(
            json!({"dcat:contactPoint": {"vcard:fn": "Desk"}}),
            Profile::DcatApSk,
        );
        assert_eq!(with["dcat:contactPoint"]["vcard:fn"][0]["@language"], "sk");
        let without: Value = render_catalog(json!({"@id": "c"}), Profile::DcatApSk);
        assert_eq!(without, json!({"@id": "c"}));
    }
}

#[cfg(feature = "dcat-ap-cz")]
mod czech {
    use super::*;

    #[test]
    fn a_record_with_nothing_to_read_stays_as_it_is() {
        let out = render(json!({"@id": "https://x/d"}), Profile::DcatApCz, None);
        assert_eq!(out, json!({"@id": "https://x/d"}));
        let out = render(json!({}), Profile::DcatApCz, Some(&bare_catalog()));
        assert_eq!(out, json!({}));
    }

    #[test]
    fn legislation_goes_to_distributions_and_services_only_when_the_record_has_it() {
        let with = json!({
            "dcatap:applicableLegislation": "https://eli/x",
            "dcat:distribution": [{"dcat:accessURL": "https://x/a"}],
            "@included": [
                {"@id": "s", "@type": "dcat:DataService"},
                {"@id": "t"}
            ]
        });
        let out = render(with, Profile::DcatApCz, None);
        assert_eq!(
            out["dcat:distribution"][0]["dcatap:applicableLegislation"],
            "https://eli/x"
        );
        assert_eq!(
            out["@included"][0]["dcatap:applicableLegislation"],
            "https://eli/x"
        );
        assert!(out["@included"][1]
            .get("dcatap:applicableLegislation")
            .is_none());

        let without = json!({
            "dcat:distribution": [{"dcat:accessURL": "https://x/a"}],
            "@included": [{"@id": "s", "@type": "dcat:DataService"}]
        });
        let out = render(without, Profile::DcatApCz, None);
        assert!(out["dcat:distribution"][0]
            .get("dcatap:applicableLegislation")
            .is_none());
        assert!(out["@included"][0]
            .get("dcatap:applicableLegislation")
            .is_none());
    }

    #[test]
    fn a_distribution_keeps_one_description_czech_then_english_then_the_first() {
        let record = json!({
            "dcat:distribution": [
                {"dct:description": [
                    {"@value": "en", "@language": "en"}, {"@value": "cs", "@language": "cs"}]},
                {"dct:description": [
                    {"@value": "de", "@language": "de"}, {"@value": "en", "@language": "en"}]},
                {"dct:description": [{"@value": "fr", "@language": "fr"}]},
                {"dct:description": []},
                {"dct:description": "plain", "dct:title": "Titul"},
                {"dct:title": [{"@value": "t", "@language": "en"}]}
            ],
            "dct:title": "plain title",
            "dct:description": [{"@value": "de", "@language": "de"}]
        });
        let out = render(record, Profile::DcatApCz, None);
        let d = out["dcat:distribution"].as_array().expect("array");
        assert_eq!(d[0]["dct:description"]["@value"], "cs");
        assert_eq!(d[1]["dct:description"]["@value"], "en");
        assert_eq!(d[2]["dct:description"]["@value"], "fr");
        assert_eq!(d[3]["dct:description"], Value::Null);
        assert_eq!(d[4]["dct:description"], "plain");
        assert_eq!(d[4]["dct:title"][0]["@language"], "cs");
        assert_eq!(d[5]["dct:title"][0]["@language"], "en");
        assert_eq!(out["dct:title"], "plain title");
        assert_eq!(out["dct:description"]["@value"], "de");
    }

    #[test]
    fn a_data_service_title_and_the_contact_are_made_czech_when_present() {
        let record = json!({
            "dcat:contactPoint": {"vcard:fn": "Linka"},
            "@included": [
                {"@id": "s", "@type": "dcat:DataService",
                 "dct:title": [{"@value": "x", "@language": "en"}, {"@value": "y", "@language": "cs"}]},
                {"@id": "u", "@type": "dcat:DataService"}
            ]
        });
        let out = render(record, Profile::DcatApCz, None);
        assert_eq!(out["dcat:contactPoint"]["vcard:fn"][0]["@language"], "cs");
        assert_eq!(out["@included"][0]["dct:title"]["@value"], "y");
        assert!(out["@included"][1].get("dct:title").is_none());
    }

    #[test]
    fn the_catalogue_is_one_language_and_a_contact_less_one_is_left() {
        let feed = json!({
            "dct:title": [{"@value": "a", "@language": "en"}, {"@value": "b", "@language": "cs"}],
            "dct:description": "plain",
            "dcat:contactPoint": {"vcard:fn": "Linka"}
        });
        let out = render_catalog(feed, Profile::DcatApCz);
        assert_eq!(out["dct:title"]["@value"], "b");
        assert_eq!(out["dct:description"], "plain");
        assert_eq!(out["dcat:contactPoint"]["vcard:fn"][0]["@language"], "cs");
        let bare = render_catalog(json!({"@id": "c"}), Profile::DcatApCz);
        assert_eq!(bare, json!({"@id": "c"}));
    }
}
