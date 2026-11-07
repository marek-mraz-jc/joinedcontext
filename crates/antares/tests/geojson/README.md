# Conformance pack: GeoJSON of a view

The test list of the `geojson` surface (`crates/antares-geojson`), one row per
requirement: the clause it comes from and the test that proves it. Run it with
`cargo test -p antares-geojson`. The first rows are the template's and hold for
every generated surface; the standard's own rows follow them.

| Requirement | Source | Test |
|---|---|---|
| Reads only what the view shows: a hidden Attribute is absent | ADR-0027, surfaces-threat-model 9.1 | `a_type_reads_through_the_view_without_what_it_hides` |
| A hidden type, an unknown view and a view without `query_entities` answer 404 alike | surfaces-threat-model 9.2 | `a_hidden_type_an_unknown_view_and_a_view_without_reads_are_404` |
| The Policy decides every read; a denied read serves no data | ADR-0020 | `the_policy_decides_every_read` |
| Read only: a write method is 405 and changes nothing | ADR-0027 section 1 | `the_surface_writes_nothing` |
| `limit`, `offset` and `q` narrow the answer | TS 104 175 6.4.3.2 | `pages_and_q_narrow_the_answer` |
| A bad parameter is 400; an empty Tenant is an empty page | TS 104 175 5.5.10 | `bad_parameters_are_400_and_an_empty_tenant_an_empty_page` |
| A request naming another Tenant than the view's is 400 | ADR-0023, a view serves one Tenant | `a_request_naming_another_tenant_is_refused` |
| The crate reaches data only through `antares-ops` | module rule M2 | `python3 dev/check-module-rules.py` |
| The answer is an RFC 7946 `FeatureCollection` of `Feature`s, `application/geo+json` | RFC 7946 3.3, 12 | `a_page_is_an_rfc_7946_feature_collection` |
| A Feature's `id` is the Entity's, its geometry the `location` value | RFC 7946 3.2 | `a_page_is_an_rfc_7946_feature_collection` |
| An Entity without `location` has a `null` geometry | RFC 7946 3.2 | `every_attribute_kind_is_a_plain_property_and_no_location_a_null_geometry` |
| A multi-instance `location` gives its first instance, a valid single geometry | RFC 7946 3.1 | `every_attribute_kind_is_a_plain_property_and_no_location_a_null_geometry` |
| Properties are plain values: a Relationship's `object`, a LanguageProperty's `languageMap`, each instance of a multi-instance Attribute | TS 104 175 4.5 | `every_attribute_kind_is_a_plain_property_and_no_location_a_null_geometry` |
| The next page's `offset` is the foreign member `nextOffset`, `null` on the last page | RFC 7946 6.1 | `a_page_is_an_rfc_7946_feature_collection` |
