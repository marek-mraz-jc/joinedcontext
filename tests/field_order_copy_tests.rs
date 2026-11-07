//! The Portal UI labels columns from a byte copy of jc-core's field-order.json (T-5192): the one
//! label table the assistant widget, the CKAN map popup and the publisher read. This keeps the
//! copy equal to the table of the jc-core rev the Portal pins, so a label never differs by host.

#[test]
fn ui_field_order_is_the_pinned_jc_core_table() {
    assert_eq!(
        include_str!("../ui/src/data/field-order.json"),
        jc_core::field_order::JSON,
        "ui/src/data/field-order.json differs from jc-core's data/field-order.json: copy it again"
    );
}
