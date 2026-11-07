// SPDX-License-Identifier: EUPL-1.2
//! The syntax checks over the ETSI NGSI-LD test suite's own Entity fixtures
//! (`ngsi-ld-test-suite/data/entities`, vendored in the workspace; not in the published crate).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use ngsild_model::{check_attr_name, EntityId};
use serde_json::Value;
use std::path::Path;

/// Every suite Entity id is an `EntityId` and every Attribute name it uses passes
/// `check_attr_name`, so the entry-point checks refuse none of the suite's valid data.
#[test]
fn every_suite_entity_id_and_attribute_name_is_accepted() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ngsi-ld-test-suite/data/entities");
    let (mut ids, mut names) = (0, 0);
    for f in std::fs::read_dir(dir).unwrap() {
        let path = f.unwrap().path();
        let Ok(Value::Object(doc)) =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_default())
        else {
            continue;
        };
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        // `RandomUuidToBeReplaced` is a Robot placeholder, refused here as the broker refuses it.
        match doc.get("id").and_then(Value::as_str) {
            Some("RandomUuidToBeReplaced") => assert_eq!(
                EntityId::new("RandomUuidToBeReplaced")
                    .unwrap_err()
                    .status(),
                400
            ),
            Some(id) => {
                assert_eq!(EntityId::new(id).unwrap().as_str(), id, "{name}");
                ids += 1;
            }
            None => {}
        }
        for key in doc
            .keys()
            .filter(|k| !["id", "type", "scope", "@context"].contains(&k.as_str()))
        {
            check_attr_name(key).unwrap_or_else(|e| panic!("{name} {key}: {e}"));
            names += 1;
        }
    }
    assert!(ids >= 60 && names >= 100, "{ids} ids, {names} names");
}
