// SPDX-License-Identifier: EUPL-1.2
//! The CIM 009 Table 4.20-1 names the access surface speaks (README section
//! 4), each with the operation-matrix rows that serve it through a view:
//! the platform Portal's table (`reconciler/views.rs`), less the operations
//! no view serves (purge, Entity Maps, the source identity).

use antares_ops::OperationId;

/// `(name, rows)`, in Table 4.20-1 order.
pub const OPERATIONS: [(&str, &[&str]); 36] = [
    ("createEntity", &["create_entity"]),
    ("updateEntity", &["update_attributes"]),
    ("appendAttrs", &["append_attributes"]),
    ("updateAttrs", &["partial_attribute_update"]),
    ("deleteAttrs", &["delete_attribute"]),
    ("deleteEntity", &["delete_entity"]),
    ("createBatch", &["batch_create"]),
    ("upsertBatch", &["batch_upsert"]),
    ("updateBatch", &["batch_update"]),
    ("deleteBatch", &["batch_delete"]),
    ("upsertTemporal", &["temporal_upsert"]),
    ("appendAttrsTemporal", &["temporal_add_attributes"]),
    ("deleteAttrsTemporal", &["temporal_delete_attribute"]),
    ("updateAttrInstanceTemporal", &["temporal_modify_instance"]),
    ("deleteAttrInstanceTemporal", &["temporal_delete_instance"]),
    ("deleteTemporal", &["temporal_delete"]),
    ("mergeEntity", &["merge_entity"]),
    ("replaceEntity", &["replace_entity"]),
    ("replaceAttrs", &["replace_attribute"]),
    ("mergeBatch", &["batch_merge"]),
    ("retrieveEntity", &["retrieve_entity"]),
    ("queryEntity", &["query_entities"]),
    ("queryBatch", &["query_entities_post"]),
    ("retrieveTemporal", &["temporal_retrieve"]),
    ("queryTemporal", &["temporal_query", "temporal_query_post"]),
    ("retrieveEntityTypes", &["retrieve_entity_types"]),
    (
        "retrieveEntityTypeDetails",
        &["retrieve_entity_type_details"],
    ),
    ("retrieveEntityTypeInfo", &["retrieve_entity_type_info"]),
    ("retrieveAttrTypes", &["retrieve_attributes"]),
    ("retrieveAttrTypeDetails", &["retrieve_attribute_details"]),
    ("retrieveAttrTypeInfo", &["retrieve_attribute_info"]),
    ("createSubscription", &["create_subscription"]),
    ("updateSubscription", &["update_subscription"]),
    ("retrieveSubscription", &["retrieve_subscription"]),
    ("querySubscription", &["query_subscriptions"]),
    ("deleteSubscription", &["delete_subscription"]),
];

/// The clause the engine is asked for the operation `name`: the gate clause
/// of its rows (operation matrix), which every row of a name shares. Type
/// and Attribute details are asked as 5.7.5 and 5.7.8, as the broker
/// serves them on the same routes.
#[must_use]
pub fn clause(name: &str) -> Option<&'static str> {
    let (_, rows) = OPERATIONS.iter().find(|(n, _)| *n == name)?;
    rows.first()
        .and_then(|row| OperationId::from_id(row))
        .and_then(OperationId::gate_clause)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_name_is_once_and_its_rows_share_one_gate_clause() {
        let mut names: Vec<&str> = OPERATIONS.iter().map(|(n, _)| *n).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), OPERATIONS.len());
        for (name, rows) in OPERATIONS {
            for row in rows {
                let op = OperationId::from_id(row);
                assert!(op.is_some(), "{name}: row {row} is not an operation");
                assert_eq!(
                    op.and_then(OperationId::gate_clause),
                    clause(name),
                    "{name}: {row}"
                );
            }
        }
        assert_eq!(clause("queryBatch"), Some("5.7.2"));
        assert_eq!(clause("retrieveEntityTypeDetails"), Some("5.7.5"));
        assert_eq!(clause("purgeEntity"), None);
        assert_eq!(clause("nosuch"), None);
    }
}
