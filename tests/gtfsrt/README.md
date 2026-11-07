# Conformance pack: GTFS Realtime of a view

The test list of the `gtfsrt` surface (`crates/antares-gtfsrt`), one row per
requirement: the clause it comes from and the test that proves it. Run it with
`cargo test -p antares-gtfsrt`.

| Requirement | Source | Test |
|---|---|---|
| The protobuf decodes as a `FeedMessage` and equals the JSON; a hidden Attribute is absent | GTFS Realtime 2.0, ADR-0027 | `the_protobuf_decodes_equals_the_json_and_holds_what_the_view_shows` |
| The mirror's field numbers are the reference proto's, and the encoder writes exactly them | gtfs-realtime.proto (google/transit 3c9e7b90) | `the_mirror_carries_the_reference_field_numbers` |
| A vehicle with no route has no `trip`; the header time is the newest position | docs/src/gtfsrt.md section 1 | `the_protobuf_decodes_equals_the_json_and_holds_what_the_view_shows`, `what_a_vehicle_lacks_stays_absent_and_km_h_become_m_s` |
| A position older than `maxAge` is dropped and counted | docs/src/gtfsrt.md section 1, EP-126 | `a_position_outside_the_window_is_dropped_and_counted`, `a_stale_unplaced_or_untimed_vehicle_is_dropped_and_counted` |
| A hidden type, an unknown view or file and a view without `query_entities` answer 404 alike | surfaces-threat-model 9.2 | `a_hidden_type_an_unknown_view_or_file_and_a_view_without_reads_are_404` |
| The Policy decides every read; a denied read serves no data | ADR-0020 | `the_policy_decides_every_read` |
| Read only: a write method is 405 and changes nothing | ADR-0027 section 1 | `the_surface_writes_nothing` |
| A bad parameter is 400; an empty Tenant is an empty feed | TS 104 175 5.5.10 | `bad_parameters_are_400_and_an_empty_tenant_an_empty_feed` |
| A request naming another Tenant than the view's is 400 | ADR-0023, a view serves one Tenant | `a_request_naming_another_tenant_is_refused` |
| The crate reaches data only through `antares-ops` | module rule M2 | `python3 dev/check-module-rules.py` |
