// SPDX-License-Identifier: EUPL-1.2
//! antares-api's integration tests as ONE binary (T-3540): 105 files linked as 105 executables
//! cost more link time than all their code took to compile. Each file is a module here and keeps
//! its path, so every reference to `crates/antares-api/tests/<file>.rs` still holds.
//!
//! A file that changes process-wide state (environment variables, the global tracing subscriber)
//! stays its own binary, declared in Cargo.toml with the reason, because `cargo test` runs this
//! binary's tests as threads of one process. Cargo.toml sets `autotests = false`, so a new file
//! runs only once it is a `mod` here or a `[[test]]` there; the guard below makes forgetting red.

mod common;

mod admin_auth;
mod api_surface;
mod attr_path_name_5_6_4;
mod batch_run_phases_t5503;
mod behaviour_profiles_t5497;
mod bounded_reads_5_14_5_16;
#[cfg(feature = "channels")]
mod channels_adr0031;
mod collation_4_23;
mod contexts_5_13;
mod contexts_fleet_5_13;
mod core_member_iris_5_5_7;
mod cross_org_write_5_6;
mod csf_forwarding_5_7;
mod csource_concurrency_5_9_2;
mod csource_subscription_ignored_members;
mod csource_tenant_in_csi_9_5_1;
mod csource_tenant_stall_5_9_2;
mod csr_attribute_names_9_4_2;
mod dataset_none_4_5_5_1;
mod dead_letters;
mod discovery;
mod discovery_5_7_5;
mod durable_state;
mod egress_breaker_6_3_8;
mod entity_map_paging_5_5_9;
mod entity_map_store_failure_5_14;
mod entity_maps_5_14;
mod entity_maps_usage_5_7;
mod entity_member_null_5_5_12;
mod expiry_sub_attr_4_22;
mod fanout_bookkeeping_t5159;
mod fed_entity_map_shape_5_2_39;
mod fed_fanout_4_3_6;
mod fed_query_entity_types_4_3_6_1;
mod fed_warning_bounds_6_3_17;
mod federation_loop;
mod federation_store_failure_5_7;
mod federation_topology;
mod filter_pushdown_5_7_2;
mod forward_context_4_3_6_6;
mod id_routing_5_12;
mod internal_tenant_leaks;
mod join_budget_4_5_23;
mod json_property_value_5_2_6_4_10;
mod merge_kept_types_10_2_9_4;
mod modified_by_dm138;
mod module_boundaries;
mod narrowing_through_proxies_5_7_2;
mod no_change_no_notify_5_6;
mod no_overwrite_5_6_3;
mod notification_context_5_2_12;
mod notification_expand_values_5_2_12;
mod notification_geojson;
mod notification_headers_6_3_8;
mod notification_triggers_5_2_12;
mod notifier_stall;
mod notify_in_process_adr0025;
mod notify_join_flat_4_5_23;
mod notify_linked_q_4_9;
mod notify_projection_4_21;
mod notify_seed_failure;
mod ordered_read_4_23;
mod pagination_links_6_3_10;
mod panic_isolation;
mod pg_query_parity;
mod plugin_sink_5_2_15;
mod policy_allow_if_g12;
mod policy_allow_if_temporal;
mod policy_contexts_5_13;
mod policy_filter_5_7;
mod policy_filter_by_id;
mod policy_filter_by_type;
mod policy_gate_seam;
mod policy_operation_members;
mod policy_pre_image;
mod policy_pre_image_fail_closed;
mod policy_temporal_upsert_5_6_11;
mod policy_temporal_window_g6;
mod pool_overload_6_3_2;
mod prefer_body_cap;
mod projection_absolute_iri_4_21;
mod query_binding_parity_5_7_2;
mod query_json_keys_4_9;
mod query_link_context_6_3_6;
mod reg_index;
mod reg_mirror_local_bus;
mod reg_narrowing_5_12;
mod registration_counters_5_2_9;
mod registration_expiry_5_9;
mod registration_listing_5_10_2;
mod response_compression_t5847;
mod response_key_order;
mod scope_merge_null_5_5_12;
mod scope_q_grammar_4_19;
mod scope_target_5_6_5;
mod security_regression;
mod set_attribute_value_10_2_6;
mod snapshots_5_16;
mod split_entities_5_7_2;
mod streaming_lists;
mod sub_attribute_null_5_6_4;
mod sub_index;
mod subscription_jsonld_context_5_8_1_4;
mod subscription_listing_5_8_4;
mod subscription_status_5_2_12;
mod subscriptions_secrets;
mod temporal_aggregated_query_4_5_19;
mod temporal_attr_writes_5_6_13;
mod temporal_autorecord;
mod temporal_bikes_week;
mod temporal_content_range_6_3_10;
mod temporal_dedup;
mod temporal_drain;
mod temporal_driver_6_3_2;
mod temporal_expand_values_5_7_4;
mod temporal_fed_paging_5_7_4;
mod temporal_id_pattern_pg;
mod temporal_instance_patch_5_6_14;
mod temporal_lang_5_7_4;
mod temporal_matrix;
mod temporal_on_change;
mod temporal_pagination_7_4_2_5;
mod temporal_projection_5_7_3;
mod temporal_rules_window;
mod temporal_scope_5_7_4;
mod temporal_sub_attr_sys_4_8;
mod temporal_values_4_5_9;
mod temporal_versions_2_4;
mod tenant_header_6_3_14;
mod tenant_isolation;
mod tenant_known_t5500;
mod tenant_move;
mod tenant_purge_large;
mod tenants_admin;
mod throttling_5_2_12;
mod type_selector_context_5_6_6;
mod types_list_pg;
mod types_list_without_bodies;
mod vanilla_jc_extensions;
mod write_race_5_6;

/// Every file under tests/ runs: a `mod` line here, or its own `[[test]]` in Cargo.toml.
#[test]
fn every_test_file_is_compiled_into_a_binary() {
    let here = include_str!("it.rs");
    let manifest = include_str!("../Cargo.toml");
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut missing = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("tests/") {
        let path = entry.expect("entry").path();
        let (Some(stem), Some("rs")) = (
            path.file_stem().and_then(|s| s.to_str()),
            path.extension().and_then(|s| s.to_str()),
        ) else {
            continue;
        };
        if stem == "it" {
            continue;
        }
        let declared = here.lines().any(|l| l == format!("mod {stem};"))
            || manifest.contains(&format!("path = \"tests/{stem}.rs\""));
        if !declared {
            missing.push(stem.to_owned());
        }
    }
    assert!(
        missing.is_empty(),
        "tests/ files no binary compiles: {missing:?}"
    );
}
