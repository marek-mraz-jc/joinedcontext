# The Bratislava seed: one Organization, one project, one space

Hlavné mesto SR Bratislava is an Organization of its own (ADR-N-058, T-5271), set up as Mesto
Žilina is in `../zilina/` (T-5242): `organizations/bratislava.yaml` (bratislava.sk, locales sk, en,
DCAT-AP-SK), its org-admin `demo.bratislava` and its readers, the members of the Keycloak group
`/bratislava`. `index.yaml` maps each file to its path in the configuration repository.

| space | model | holds |
|---|---|---|
| `bratislava` | `bratislava` | data.bratislava.sk (CC BY 4.0) as Smart Data Models: `AirQualityObserved`; the counter sites as `TrafficFlowObserved` (cyclists, vehicles) and `CrowdFlowObserved` (pedestrians, T-5273); the Slovnaft BAjk stations of nextbike's GBFS feed (CC0 1.0) as `BikeHireDockingStation` |

The pipelines' account `pipelines` writes the space through the Endpoint `bratislava` and the
Policy `pipelines-write`, which lists the model's types and no other. Each batch adds its classes
to `bratislava.linkml.yaml`, the same types to both Policies and its retention and `in-area` rows
to the space. The datasets publish to the CKAN organisation `bratislava`, titled "Hlavné mesto SR
Bratislava": jcctl opens it with the first Endpoint that publishes. Which data.bratislava.sk
dataset goes where, and why, is the ranked matrix of T-5272.

Regenerate the model's artifacts after editing its LinkML as `../zilina/README.md` shows, with
`m=bratislava`.

T-5273's pipelines: `counters` and `pedestrian-counters` read the day the hub's import closed from
the Feature Service tables `cyklisti2_0`, `vozidla_00` and `chodci10_01` once a morning, summed per
site by the service; `bike-stations` (nightly) and `bike-availability` (every five minutes) read
nextbike's GBFS feed `nextbike_bi`. Each catalogue dataset (`bratislava-cyklisti`,
`bratislava-vozidla`, `bratislava-chodci`, `bratislava-bajk`) has its own Endpoint and public
Policy; the checks each mapping makes are in its header and in tests/test_bratislava_counters.py
and tests/test_bratislava_bikes.py.
