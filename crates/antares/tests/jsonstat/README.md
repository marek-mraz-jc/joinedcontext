# Conformance pack: JSON-stat of a view

The test list of the `jsonstat` surface (`crates/antares-jsonstat`), one row per
requirement: the clause it comes from and the test that proves it. Run it with
`cargo test -p antares-jsonstat`.

| Requirement | Source | Test |
|---|---|---|
| A ŠÚ SR table round-trips: the same values, value order and dimension order as DATAcube's answer | JSON-stat 2.0, T-4402 | `a_ds_sr_table_round_trips_with_its_values_and_dimension_order` |
| A sparse cube holds `null` for a missing cell, never 0; labels follow `lang` (sk, en) | JSON-stat 2.0 value | `a_sparse_cube_holds_null_for_a_missing_cell_and_labels_follow_lang` |
| A key dimension appears only when an Entity carries it; a large or sparse cube is an object | docs/src/jsonstat.md section 1 | `a_key_dimension_appears_when_an_entity_carries_it_and_a_missing_key_is_none`, `a_sparse_or_large_cube_is_an_object_of_its_cells` |
| What is no cell, or a second Entity of a cell, is left out and counted | EP-126 | `what_is_no_cell_or_a_second_entity_of_a_cell_is_left_out` |
| A hidden type, an unknown view or table and a view without `query_entities` answer 404 alike | surfaces-threat-model 9.2 | `a_hidden_type_an_unknown_view_or_table_and_a_view_without_reads_are_404` |
| The Policy decides every read; a denied read serves no data | ADR-0020 | `the_policy_decides_every_read` |
| Read only: a write method is 405 and changes nothing | ADR-0027 section 1 | `the_surface_writes_nothing` |
| A bad parameter is 400 | docs/src/jsonstat.md section 2 | `bad_parameters_are_400` |
| A request naming another Tenant than the view's is 400 | ADR-0023, a view serves one Tenant | `a_request_naming_another_tenant_is_refused` |
| The crate reaches data only through `antares-ops` | module rule M2 | `python3 dev/check-module-rules.py` |
