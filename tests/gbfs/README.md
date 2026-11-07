# Conformance pack: GBFS of a view

The test list of the `gbfs` surface (`crates/antares-gbfs`), one row per
requirement: the clause it comes from and the test that proves it. Run it with
`cargo test -p antares-gbfs`.

| Requirement | Source | Test |
|---|---|---|
| Each file reads through the view: a hidden Attribute is absent; each equals its golden file | ADR-0027, surfaces-threat-model 9.1 | `each_file_is_the_validated_golden_one_without_what_the_view_hides` |
| Every golden file validates against the GBFS 3.0 JSON schemas | MobilityData gbfs-json-schema v3.0 | `python3 tests/gbfs/validate.py` |
| `last_updated` and `ttl` are the same in every file of a read: the newest report and the output's ttl | GBFS 3.0 Output Format | `each_file_is_the_validated_golden_one_without_what_the_view_hides` |
| A station with no live status is listed in `station_information`, left out of `station_status` and counted | GBFS 3.0 station_status.json, EP-126 | `a_station_without_a_live_status_is_counted_and_left_out_of_station_status` |
| The SDM fields map one to one; what a station lacks stays out | docs/src/gbfs.md section 2 | `a_station_maps_to_its_information_and_status`, `what_a_station_lacks_stays_out_or_leaves_it_out` |
| The output's members are checked; one out of bounds refuses the list with 500 | docs/src/gbfs.md section 1 | `an_output_holds_or_the_member_is_refused_whole` |
| A hidden type, an unknown view, output or file and a view without `query_entities` answer 404 alike | surfaces-threat-model 9.2 | `a_hidden_type_an_unknown_view_output_or_file_and_a_view_without_reads_are_404` |
| The Policy decides every read; a denied read serves no data | ADR-0020 | `the_policy_decides_every_read` |
| Read only: a write method is 405 and changes nothing | ADR-0027 section 1 | `the_surface_writes_nothing` |
| An empty Tenant is a feed without stations | TS 104 175 5.5.10 | `an_empty_tenant_is_a_feed_without_stations` |
| A request naming another Tenant than the view's is 400 | ADR-0023, a view serves one Tenant | `a_request_naming_another_tenant_is_refused` |
| The crate reaches data only through `antares-ops` | module rule M2 | `python3 dev/check-module-rules.py` |
