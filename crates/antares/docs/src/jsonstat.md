# JSON-stat of a view

A build with the `jsonstat` feature (`cargo build -p antares-broker --features jsonstat`)
serves the `KeyPerformanceIndicator` Entities of one source table of a view
as a JSON-stat 2.0 dataset (T-4490), so a statistics tool reads a ŠÚ SR
table as the cube it came from:

```text
GET /x/views/{view}/jsonstat/v1/{table}
GET /x/views/{view}/jsonstat/v1/om7014rr?lang=en
```

Mount it with `ANTARES_API_SURFACES=admin,views,jsonstat`. `{table}` is the
Entities' `dataSet`, the DATAcube code (1 to 64 of `A-Z a-z 0-9 . _ -`).
`lang` means what it means for NGSI-LD Query Entities: a LanguageProperty
`name` comes back in that language.

## 1. Mapping

The cells are the Entities the pipeline's `datacube` step writes (jc-core
`datacube.blobl`), one per cell DATAcube filled, read with one NGSI-LD Query
Entities `q=dataSet=="{table}"` through the view.

| JSON-stat member | From |
|---|---|
| `id`, `size` | `refArea`, `refPeriod`, `indicator` in DATAcube's order (territory, period, indicator), then `dimensionKey` and `unitText` when an Entity of the table carries them |
| `role` | `geo` `refArea`, `time` `refPeriod`, `metric` `indicator` |
| `dimension.refPeriod.category.index` | the periods, newest first as DATAcube lists them |
| `dimension.refArea`, `indicator`, `dimensionKey`, `unitText` `category.index` | the codes, sorted |
| `dimension.indicator.category.label` | `name` of the indicator's first Entity |
| `value` | `kpiValue` in row-major order; a cell no Entity holds is `null`, never 0 |
| `label` | the table code |
| `updated` | the newest `kpiValue.observedAt`, else `dateObserved` |
| `source` | the `source` URLs, joined |
| `extension.source`, `extension.isPartOf` | every `source` and `isPartOf` |
| `note` | that an empty cell is `null` |

A key dimension an Entity lacks while another carries it is the category
`_`. A cube larger than a million cells, or less than a quarter full, writes
`value` as an object of the cells it holds by index (JSON-stat 2.0 allows
both forms). The pipeline stores no territory label, so `refArea` has codes
alone. An Entity without a territory, period, indicator or numeric
`kpiValue`, and a second Entity of a cell already held, is left out and
counted in the `antares-skipped-records` header and in
`antares_surface_records_skipped_total{surface="jsonstat"}`.

## 2. Refusals

A view that is unknown, does not allow `query_entities` or does not show
`KeyPerformanceIndicator`, a table code out of bounds and a table the view
holds no cell of answer `404` alike. Any parameter but one `lang` is `400`.
A read the Policy refuses answers the NGSI-LD refusal (`403`). `HEAD` answers
like `GET` without the body; every other method but `OPTIONS` is `405`.

The round-trip test reads the ŠÚ SR answer for `om7014rr`
(`tests/jsonstat/om7014rr.json`), writes its Entities as the pipeline does
and compares the dataset the surface serves: the same values in the same
order and the same dimension order. The pack is
[`tests/jsonstat/README.md`](https://github.com/marek-mraz-jc/Antares-NGSI-LD-Context-Broker/blob/dev/tests/jsonstat/README.md).

## Related

- [Surfaces](surfaces.md)
- [Views](views.md)
- [New surface](new-surface.md)
