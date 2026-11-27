use crate::common;

use common::*;
use jcctl::commands::plan::{compute, Action};
use jcctl::loader::RawManifest;
use jcctl::platform::{InMemory, Platform, PlatformError};
use serde_json::json;

#[test]
fn a_fresh_platform_makes_every_manifest_a_create_in_wave_order() {
    let dir = demo_repo("plan-fresh");
    let changes = compute(&load(&dir), &InMemory::new()).expect("plan computes");

    assert_eq!(changes.count(Action::Create), 4);
    assert_eq!(changes.count(Action::Update), 0);
    assert_eq!(changes.count(Action::Delete), 0);
    assert!(!changes.is_clean());

    let waves: Vec<u8> = changes.waves.iter().map(|(wave, _)| *wave).collect();
    assert_eq!(waves, vec![0, 1, 3]);
    assert_eq!(
        changes
            .iter()
            .map(|c| c.id.kind.as_str())
            .collect::<Vec<_>>(),
        vec!["Organization", "Project", "ContextSpace", "Endpoint"]
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The property the whole reconciler rests on: a platform that already matches the
/// repository has nothing pending (CC-17, CC-18).
#[test]
fn a_platform_that_matches_the_repository_is_clean() {
    let dir = demo_repo("plan-clean");
    let platform = InMemory::new()
        .with(manifest(ORG))
        .with(manifest(PROJECT))
        .with(manifest(SPACE))
        .with(manifest(ENDPOINT));

    let changes = compute(&load(&dir), &platform).expect("plan computes");

    assert!(changes.is_clean());
    assert_eq!(changes.count(Action::Unchanged), 4);
    assert_eq!(changes.to_json()["changes"], json!([]));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_changed_member_is_an_update_carrying_its_field_diff() {
    let dir = demo_repo("plan-update");
    let live = ENDPOINT.replace(
        r#"enabledRepresentations: ["ngsi-ld", "csv"]"#,
        r#"enabledRepresentations: ["ngsi-ld"]"#,
    );
    let platform = InMemory::new()
        .with(manifest(ORG))
        .with(manifest(PROJECT))
        .with(manifest(SPACE))
        .with(manifest(&live));

    let changes = compute(&load(&dir), &platform).expect("plan computes");

    assert_eq!(changes.count(Action::Update), 1);
    let update = changes
        .iter()
        .find(|c| c.action == Action::Update)
        .expect("the endpoint changed");
    assert_eq!(update.id.name, "public-air");
    assert_eq!(update.diff.len(), 1);
    assert_eq!(update.diff[0].path, "spec.enabledRepresentations");
    let text = changes.render();
    assert!(
        text.contains("  update    Endpoint/ovzdusie/public-air\n"),
        "{text}"
    );
    assert!(
        text.contains(
            "      spec.enabledRepresentations: [\"ngsi-ld\"] -> [\"ngsi-ld\",\"csv\"]\n"
        ),
        "{text}"
    );
    assert!(text.contains("  unchanged "), "{text}");

    let bare = ENDPOINT.replace("  enabledRepresentations: [\"ngsi-ld\", \"csv\"]\n", "");
    let platform = InMemory::new()
        .with(manifest(ORG))
        .with(manifest(PROJECT))
        .with(manifest(SPACE))
        .with(manifest(&bare));
    let text = compute(&load(&dir), &platform)
        .expect("plan computes")
        .render();
    assert!(
        text.contains("      spec.enabledRepresentations: (absent) -> "),
        "{text}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A resource the platform has and the repository no longer declares is reported, never
/// removed by `plan` itself (CC-19).
#[test]
fn a_resource_missing_from_the_repository_is_reported_for_deletion() {
    let dir = demo_repo("plan-delete");
    std::fs::remove_file(dir.join(ENDPOINT_PATH)).expect("drop the endpoint from Git");

    let platform = InMemory::new()
        .with(manifest(ORG))
        .with(manifest(PROJECT))
        .with(manifest(SPACE))
        .with(manifest(ENDPOINT));

    let changes = compute(&load(&dir), &platform).expect("plan computes");

    assert_eq!(changes.count(Action::Delete), 1);
    assert_eq!(platform.len(), 4, "plan writes nothing");

    let deletion = changes
        .iter()
        .find(|c| c.action == Action::Delete)
        .expect("the endpoint is gone from Git");
    assert_eq!(deletion.id.name, "public-air");
    assert!(deletion.declared.is_none());
    assert!(
        changes.render().contains("  delete    Endpoint/"),
        "{}",
        changes.render()
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The `--json` contract of API/03 section 2.
#[test]
fn the_json_contract_carries_the_summary_and_the_nested_diff() {
    let dir = demo_repo("plan-json");
    let live = ENDPOINT.replace("audience: public", "audience: organization");
    let platform = InMemory::new()
        .with(manifest(ORG))
        .with(manifest(PROJECT))
        .with(manifest(SPACE))
        .with(manifest(&live));

    let json = compute(&load(&dir), &platform)
        .expect("plan computes")
        .to_json();

    assert_eq!(
        json["summary"],
        json!({"to_add": 0, "to_change": 1, "to_delete": 0})
    );
    assert_eq!(
        json["changes"],
        json!([{
            "kind": "Endpoint",
            "id": "public-air",
            "action": "UPDATE",
            "diff": { "spec": { "audience": "public" } },
        }])
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_rendered_diff_names_the_wave_the_resource_converges_in() {
    let dir = demo_repo("plan-render");
    let rendered = compute(&load(&dir), &InMemory::new())
        .expect("plan computes")
        .render();

    assert!(rendered.contains("wave 0"), "{rendered}");
    assert!(
        rendered.contains("create    Endpoint/ovzdusie/public-air"),
        "{rendered}"
    );
    assert!(
        rendered.contains("4 to add, 0 to change, 0 to delete"),
        "{rendered}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

const BLUEPRINT: &str = r#"apiVersion: joinedcontext.com/v1alpha1
kind: Blueprint
metadata:
  name: threshold-alert
  namespace: org
spec:
  version: 1.0.0
  riskClass: green
  allowedRoles: [domain-editor]
  parameterSchema: { type: object }
  templates: []
"#;

/// A platform that answers every query with the same list, whatever the plural asked for.
struct Fixed(Vec<RawManifest>);

impl Platform for Fixed {
    fn list(&self, _project: &str, _plural: &str) -> Result<Vec<RawManifest>, PlatformError> {
        Ok(self.0.clone())
    }

    fn put(&mut self, _manifest: &RawManifest) -> Result<(), PlatformError> {
        Ok(())
    }

    fn delete(&mut self, _id: &jcctl::loader::ResourceId) -> Result<(), PlatformError> {
        Ok(())
    }
}

/// A kind with no wave carries no live state: declared in Git it is not planned, and a platform
/// that answers with one is not asked to delete it.
#[test]
fn a_kind_with_no_wave_is_neither_planned_nor_deleted() {
    let dir = demo_repo("plan-no-wave");
    let path = jc_core::registry::by_kind("Blueprint")
        .expect("a kind")
        .repo_path("org", "", "threshold-alert");
    write(&dir, &path, BLUEPRINT);
    let platform = Fixed(vec![manifest(BLUEPRINT)]);
    let changes = compute(&load(&dir), &platform).expect("plan computes");
    assert!(
        changes.iter().all(|c| c.id.kind != "Blueprint"),
        "{}",
        changes.render()
    );
    assert_eq!(changes.count(Action::Delete), 0);
    assert_eq!(changes.count(Action::Create), 4);
}

/// DM-48: a registration naming an Endpoint this repository lacks is flagged, and the flag is
/// in the rendered plan as a note.
#[test]
fn a_registration_naming_a_missing_endpoint_is_flagged_in_the_plan() {
    let dir = demo_repo("plan-flag");
    let path = jc_core::registry::by_kind("ContextSourceRegistration")
        .expect("a kind")
        .repo_path("ovzdusie", "ovzdusie", "mesto");
    write(
        &dir,
        &path,
        "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSourceRegistration\nmetadata:\n  name: mesto\n  namespace: ovzdusie\nspec:\n  contextSpaceRef: ovzdusie\n  endpointRef: { kind: Endpoint, name: nowhere }\n  information:\n    - entities: [{ type: AirQualityObserved }]\n  federation: { identity: caller }\n",
    );
    let changes = compute(&load(&dir), &InMemory::new()).expect("plan computes");
    let rendered = changes.render();
    assert!(
        rendered.contains("note: ContextSourceRegistration/") && rendered.contains("nowhere"),
        "{rendered}"
    );
}
