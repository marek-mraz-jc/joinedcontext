Recorded answers of public sources, trimmed (T-7427), 2026-11-19:

- `arcgis-rest/`: `https://sampleserver6.arcgisonline.com/arcgis/rest/services` (Esri's public
  sample server): the root directory cut to one folder and four services, the `Elevation` folder
  cut to two, and three services' `?f=json` answers as read.
- `ogc-records/page1.json`: `https://demo.pygeoapi.io/master/collections/dutch-metadata/items?limit=2&f=json`
  as read, its `numberMatched` set to 4 and its `next` link pointed at `page2.json`, a written page
  of two malformed records (no title, no id).
