# Preset fixtures (T-7067)

Pages recorded from public open data APIs on 2026-11-15, one folder per `spec.http.preset` kind:
`source.yaml` is the DataSource, `render.json` the URL and `paginate` option jc-manifest renders
from it (`bento_preset_fixture_tests.rs` checks it), `page-N.json` the pages the walk asks for in
order (`preset_fixture_tests.rs` walks them). Extra URL parameters (`returnGeometry=false`,
`propertyName`, `$select`) keep the recordings small; the CKAN pages drop each package's `extras` key/value list (gitleaks reads `"key": "..."` as a secret).

| Folder | Source | Records |
|---|---|---|
| arcgis | ČSÚ RSO FeatureServer/17 (geodata.csu.gov.cz) | 142 |
| wfs | Helsinki kartta.hel.fi avoindata:FMI_Paikkakohtainen_vyorymakerroin_piste | 453 |
| ckan | Barcelona Open Data, organization ocupacio | 2 |
| odata | OData Northwind V4 Customers | 91 |
| sensorthings | Hamburg iot.hamburg.de Things named StadtRad | 368 |
| ogc | pygeoapi demo collection lakes | 25 |
