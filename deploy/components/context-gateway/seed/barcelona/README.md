# The Barcelona seed: one Organization, one project, one space

Ajuntament de Barcelona is an Organization of its own (ADR-N-058, T-5260), set up as Mesto Žilina
is in `../zilina/` (T-5242): `organizations/barcelona.yaml` (barcelona.cat, locales ca, es, en),
its org-admin `demo.barcelona` and its readers, the members of the Keycloak group `/barcelona`.
`index.yaml` maps each file to its path in the configuration repository.

| space | model | holds |
|---|---|---|
| `barcelona` | `barcelona` | Open Data BCN (CC BY 4.0) as Smart Data Models: `AirQualityObserved`, `AdministrativeArea` (10 districts, 73 barris), `CityWork` (works on the public space), `PedestrianCrossing` (the street inventory's crossings), `StreetFurniture` (its speed reducers), `PointOfInterest` (facilities, drinking and ornamental fountains), `TouristRental` (the licensed tourist flats, at building level) |

The pipelines' account `pipelines` writes the space through the Endpoint `barcelona` and the
Policy `pipelines-write`, which lists the model's types and no other. Each Open Data BCN batch
(T-5262..T-5266) adds its classes to `barcelona.linkml.yaml`, the same types to both Policies and
its retention and `in-area` rows to the space. The datasets publish to the CKAN organisation
`barcelona`, titled "Ajuntament de Barcelona": jcctl opens it with the first Endpoint that
publishes. Which Open Data BCN dataset goes where, and why, is the ranked matrix of T-5261.

Batch 2 (T-5263) adds the pipeline `air-quality`: Open Data BCN's hourly file
Qualitat_Aire_Detall.csv, twice an hour, one `AirQualityObserved` per station of the city's network
with the newest valid hour of each pollutant, published as the CKAN dataset of the Endpoint
`air-quality`. Black carbon arrives in ng/m³ under a µg/m³ label and is divided by 1000; the
mapping's header lists what it drops and why. The pipeline `weather` reads Meteocat's daily values
of the three stations in the city (`WeatherObserved`, Endpoint `weather`), the pipeline
`groundwater` the city's well readings (`PhreaticObserved`, one per well read in the last five
years, Endpoint `groundwater`).

T-5413 adds the pipeline `dog-areas`: the city's fenced dog areas (Open Data BCN
arees-esbarjo-gossos-bcn, about twice a year), read weekly as `PointOfInterest` with the category
`animals-plants, dog-area` and the id prefix `dog-area-`, published as the Endpoint `dog-areas`,
whose Policy reads them by that prefix. The pipeline `local-trees` reads the 208 trees of local interest
(arbres-interes-local) from the DataStore of the register the same way, with the category
`environment, local-interest-tree` and the prefix `local-tree-` (Endpoint `local-trees`).

T-6570 adds the pipeline `heritage`: the city's architectural heritage, 1,484 buildings with a code
of the Generalitat's inventory (IPAC) as Wikidata lists them (CC0, one SPARQL query, weekly), as
`PointOfInterest` with the category `heritage` and each designation's code (`bcin`, `bcil`, `bpcc`,
`urban-protection`, `world-heritage`...), the prefix `heritage-` and the Wikidata item id (Endpoint
`heritage`). The Ajuntament's own catalogue (patrimoni-arquitectonic-protegit) is a zipped
Shapefile in ETRS89 UTM 31N with no address, which a pipeline cannot read.

The pipeline `weather-forecast` reads Meteocat's forecast for the comarca of Barcelonès from Open
Data BCN's XML file (prediccio-meteorologica-del-barcelones) every half hour, one `WeatherForecast`
per day ahead (`barcelones-day-1`, `-day-2`) with its issue time, validity, extremes and sky,
replacing the type's entities on each good run (Endpoint `weather-forecast`).

Regenerate the model's artifacts after editing its LinkML as `../zilina/README.md` shows, with
`m=barcelona`.
