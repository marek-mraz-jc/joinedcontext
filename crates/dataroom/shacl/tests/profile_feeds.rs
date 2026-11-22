//! What each profile's own shapes admit to its feed: a complete dataset, and not one missing a
//! field the profile requires (C17). The EU feed keeps what its own shapes allow.

use dataroom_core::{Catalogue, Dataset};
use dataroom_rdf::{dataset_to_graph, DcatProfile};
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
    dataset["extras"] = serde_json::json!({"personal_data": "does-not-contain"});
    (
        serde_json::from_value(fixture["catalogue"].take()).expect("catalogue"),
        serde_json::from_value(dataset).expect("dataset"),
    )
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
    for profile in [
        DcatProfile::eu(),
        DcatProfile::from_token("dcat-ap-cz").expect("installed"),
        DcatProfile::from_token("dcat-ap-sk-2.0").expect("installed"),
    ] {
        let found = violations(&catalogue, &dataset, profile);
        assert!(found.is_empty(), "{}: {found:#?}", profile.id());
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
