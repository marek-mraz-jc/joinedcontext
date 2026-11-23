//! What each profile's own shapes admit to its feed: a complete dataset, and not one missing a
//! field the profile requires (C17). The EU feed keeps what its own shapes allow.

use dataroom_core::{Catalogue, Dataset};
use dataroom_rdf::{catalogue_to_graph, dataset_to_graph, DcatProfile};
use dataroom_shacl::{validate_graph_against_shapes, ProfileRegistry};
use serde_json::Value;

fn complete() -> (Catalogue, Dataset) {
    let mut fixture: Value =
        serde_json::from_str(include_str!("../../rdf/tests/fixtures/complete.json"))
            .expect("fixture");
    let mut dataset = fixture["dataset"].take();
    // The terms follow from the declared licence and personal-data statement alone.
    dataset["distributions"][0]
        .as_object_mut()
        .expect("a distribution")
        .remove("termsOfUse");
    dataset["extras"] = serde_json::json!({
        "personal_data": "does-not-contain",
        // Read by the profiles that name it as an additional property.
        "topic_category": "http://inspire.ec.europa.eu/metadata-codelist/TopicCategory/environment",
    });
    (
        serde_json::from_value(fixture["catalogue"].take()).expect("catalogue"),
        serde_json::from_value(dataset).expect("dataset"),
    )
}

/// The violations of the whole feed of `dataset`: its catalogue node included. With
/// `PROFILE_FEEDS_OUT` set, each feed is left there for another SHACL engine to read.
fn feed_violations(catalogue: &Catalogue, dataset: &Dataset, profile: DcatProfile) -> Vec<String> {
    let stack = ProfileRegistry::new()
        .stack((catalogue.org_id, catalogue.id), profile.id(), None)
        .expect("a built-in profile");
    let datasets = std::slice::from_ref(dataset);
    let publishers = std::collections::BTreeMap::new();
    let graph = catalogue_to_graph(catalogue, datasets, &publishers, profile, None);
    if let Some(dir) = option_env!("PROFILE_FEEDS_OUT") {
        let path = std::path::Path::new(dir).join(format!("{}.ttl", profile.id()));
        std::fs::write(path, dataroom_rdf::turtle::serialize_turtle(&graph)).expect("feed written");
    }
    validate_graph_against_shapes(
        &graph,
        &stack.shapes,
        catalogue.org_id,
        catalogue.id,
        "catalog",
        profile.id(),
        "1",
    )
    .findings
    .into_iter()
    .filter(|f| f.severity == dataroom_core::ShaclSeverity::Violation)
    .map(|f| f.result_path.unwrap_or(f.source_shape))
    .collect()
}

/// The paths of `dataset`'s violations in `profile`; empty when it stays in that feed.
fn violations(catalogue: &Catalogue, dataset: &Dataset, profile: DcatProfile) -> Vec<String> {
    let stack = ProfileRegistry::new()
        .stack((catalogue.org_id, catalogue.id), profile.id(), None)
        .expect("a built-in profile");
    let shapes = &stack.shapes;
    let graph = dataset_to_graph(dataset, catalogue, None, profile);
    let report = validate_graph_against_shapes(
        &graph,
        shapes,
        catalogue.org_id,
        catalogue.id,
        &dataset.id,
        profile.id(),
        "1",
    );
    if report.conforms {
        return Vec::new();
    }
    report.findings.iter().map(|f| format!("{f:?}")).collect()
}

#[test]
fn a_complete_dataset_passes_every_profiles_shapes() {
    let (catalogue, dataset) = complete();
    // Every profile a catalogue may choose, so a new directory is under test with no edit here.
    for profile in DcatProfile::installed().filter(|profile| profile.selectable) {
        let found = violations(&catalogue, &dataset, profile);
        assert!(found.is_empty(), "{}: {found:#?}", profile.id());
        let found = feed_violations(&catalogue, &dataset, profile);
        assert!(found.is_empty(), "{} feed: {found:#?}", profile.id());
    }
}

/// T-7746: an Organization's own dataset fields add their properties to the feed under its
/// namespace, typed by their definitions, and every profile's shapes still admit the feed.
#[test]
fn the_organization_dataset_fields_keep_every_feed_valid() {
    let (catalogue, mut dataset) = complete();
    dataset.extras.insert("x_funding".into(), "eu".into());
    dataset.extras.insert("x_reviewed".into(), "2026-11-23".into());
    dataset.extras.insert("x_reviewed_by".into(), "not a field".into());
    let fields = dataroom_core::branding::BrandingDatasetFields {
        organization: "bbsk".into(),
        namespace: "https://bbsk.sk/ns/dataset-fields/bbsk/".into(),
        fields: serde_json::from_value(serde_json::json!([
            { "name": "funding", "label": { "sk": "Financovanie" }, "type": "enum",
              "values": ["eu", "city"] },
            { "name": "reviewed", "label": { "sk": "Revízia" }, "type": "date" }
        ]))
        .expect("definitions"),
    };
    for profile in DcatProfile::installed().filter(|profile| profile.selectable) {
        let stack = ProfileRegistry::new()
            .stack((catalogue.org_id, catalogue.id), profile.id(), None)
            .expect("a built-in profile");
        let datasets = std::slice::from_ref(&dataset);
        let mut graph = catalogue_to_graph(
            &catalogue,
            datasets,
            &std::collections::BTreeMap::new(),
            profile,
            None,
        );
        dataroom_rdf::add_field_triples(&mut graph, &catalogue, datasets, &fields);
        let ttl = dataroom_rdf::turtle::serialize_turtle(&graph);
        assert!(
            ttl.contains("https://bbsk.sk/ns/dataset-fields/bbsk/funding"),
            "{}: {ttl}",
            profile.id()
        );
        assert!(
            ttl.contains("\"2026-11-23\"^^xsd:date")
                || ttl.contains("\"2026-11-23\"^^<http://www.w3.org/2001/XMLSchema#date>"),
            "the date typed: {ttl}"
        );
        assert!(!ttl.contains("not a field"), "an undefined extra stays out");
        // The JSON-LD feed declares the Organization's namespace in its context.
        let doc: Value = serde_json::from_str(&dataroom_rdf::jsonld::serialize_graph_jsonld(&graph))
            .expect("JSON-LD");
        assert_eq!(
            doc["@context"]["fields-bbsk"],
            "https://bbsk.sk/ns/dataset-fields/bbsk/",
            "{}",
            profile.id()
        );
        assert!(doc["@graph"]
            .as_array()
            .expect("graph")
            .iter()
            .any(|node| node["fields-bbsk:funding"] == "eu"));
        let report = validate_graph_against_shapes(
            &graph,
            &stack.shapes,
            catalogue.org_id,
            catalogue.id,
            "catalog",
            profile.id(),
            "1",
        );
        let violations: Vec<_> = report
            .findings
            .into_iter()
            .filter(|f| f.severity == dataroom_core::ShaclSeverity::Violation)
            .collect();
        assert!(violations.is_empty(), "{}: {violations:#?}", profile.id());
    }
}

#[test]
fn an_undeclared_required_field_leaves_the_national_feeds_only() {
    type Undeclare = fn(&mut Dataset);
    let cases: [(&str, Undeclare, &[DcatProfile]); 6] = [
        (
            "personal data",
            |d| {
                d.extras.remove("personal_data");
            },
            &[
                DcatProfile::from_token("dcat-ap-cz").expect("installed"),
                DcatProfile::from_token("dcat-ap-sk-2.0").expect("installed"),
            ],
        ),
        (
            "licence",
            |d| d.licence_id = None,
            &[
                DcatProfile::from_token("dcat-ap-cz").expect("installed"),
                DcatProfile::from_token("dcat-ap-sk-2.0").expect("installed"),
            ],
        ),
        (
            "theme",
            |d| d.themes.clear(),
            &[
                DcatProfile::from_token("dcat-ap-cz").expect("installed"),
                DcatProfile::from_token("dcat-ap-sk-2.0").expect("installed"),
            ],
        ),
        (
            "frequency",
            |d| d.frequency = None,
            &[
                DcatProfile::from_token("dcat-ap-cz").expect("installed"),
                DcatProfile::from_token("dcat-ap-sk-2.0").expect("installed"),
            ],
        ),
        (
            "publisher",
            |d| d.publisher = None,
            &[
                DcatProfile::from_token("dcat-ap-cz").expect("installed"),
                DcatProfile::from_token("dcat-ap-sk-2.0").expect("installed"),
            ],
        ),
        (
            "legislation",
            |d| {
                d.applicable_legislation.clear();
                d.distributions[0].applicable_legislation.clear();
            },
            &[DcatProfile::from_token("dcat-ap-cz").expect("installed")],
        ),
    ];
    for (what, undeclare, leaves) in cases {
        let (catalogue, mut dataset) = complete();
        undeclare(&mut dataset);
        for profile in [
            DcatProfile::eu(),
            DcatProfile::from_token("dcat-ap-cz").expect("installed"),
            DcatProfile::from_token("dcat-ap-sk-2.0").expect("installed"),
        ] {
            let found = violations(&catalogue, &dataset, profile);
            assert_eq!(
                !found.is_empty(),
                leaves.contains(&profile),
                "without {what}, {}: {found:#?}",
                profile.id()
            );
        }
    }
}
