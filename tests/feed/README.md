# Conformance pack: Feeds of a view

The test list of the `feed` surface (`crates/antares-feed`), one row per
requirement: the clause it comes from and the test that proves it. Run it with
`cargo test -p antares-feed`. The first rows are the template's and hold for
every generated surface; the standard's own rows follow them.

| Requirement | Source | Test |
|---|---|---|
| The three formats list the same entries, newest first, without what the view hides | RFC 4287, RSS 2.0, JSON Feed 1.1, EP-150, ADR-0027 | `three_formats_list_the_same_entries_newest_first_without_what_the_view_hides` |
| Text stays text: `&`, `<`, `]]>` and control characters escaped or dropped; only http(s) links | EP-150, XML 1.0 2.2 | `three_formats_list_the_same_entries_newest_first_without_what_the_view_hides`, `xml_escapes_markup_and_drops_what_xml_excludes`, `only_absolute_http_links_pass` |
| Only a listed output of a readable view has a feed; a hidden type, an unknown view, output or file is 404 | EP-150, surfaces-threat-model 9.2 | `only_a_listed_output_of_a_readable_view_has_a_feed` |
| An output's filter only narrows; a request cannot widen it | EP-151 | `a_filter_only_narrows_and_a_request_cannot_widen_it`, `a_request_text_must_balance_outside_its_strings` |
| RFC 5005 paging links; `limit` at most 200; a bad parameter is 400; an empty Tenant is an empty feed | RFC 5005 3, EP-150 | `pages_link_each_other_and_bad_parameters_are_400` |
| A conditional GET of an unchanged feed is 304 | RFC 9110 13.1 | `a_conditional_get_of_an_unchanged_feed_is_304` |
| More than 20 outputs or a repeated name serves nobody; a broken sibling stops no output | EP-151 | `a_list_of_more_than_twenty_outputs_or_a_repeated_name_serves_nobody` |
| An output without `title` or `updated`, with a bad name or an unknown member does not compile | EP-151, T-5207 | `an_output_without_title_or_updated_or_with_a_bad_name_does_not_compile` |
| The Policy decides every read; a denied read serves no data | ADR-0020 | `the_policy_decides_every_read` |
| Read only: a write method is 405 and changes nothing | ADR-0027 section 1 | `the_surface_writes_nothing` |
| A request naming another Tenant than the view's is 400 | ADR-0023, a view serves one Tenant | `a_request_naming_another_tenant_is_refused` |
| The rendered documents pass the T-5201 validator, alone and across formats | T-5201 | conformance `compliance/feeds/test_antares_fixtures.py` on the files `FEED_FIXTURE_DIR=<dir> cargo test -p antares-feed --test feed` writes |
| The crate reaches data only through `antares-ops` | module rule M2 | `python3 dev/check-module-rules.py` |
