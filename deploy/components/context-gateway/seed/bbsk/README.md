# The Banskobystrický kraj seed: two spaces, one share

The region's half of the Banská Bystrica demonstration (T-2305), seeded the same way as
`helsinki/`: `index.yaml` maps each file to its path in the configuration repository (jc-core
`PATH_TEMPLATE`s), and the gateway mounts each under its own name in the flat `/repo`.

`bbsk-kraj` holds what the publishers published, one entity per cube cell, the publisher's own
codes kept verbatim and the request that produced each row in `source`. `bbsk-kpi` holds what
was decided to be measured: `KeyPerformanceIndicator` entities, one per indicator and territory,
with the formula and the provenance PF-55 requires. Nothing is computed twice and no application
re-derives a number.

One endpoint is public, `bbsk-kpi`: the indicators are what the region publishes. `bbsk-kraj` is
`organization`, because those rows are somebody else's published data mirrored here for the
pipelines that read them, and republishing them under our own name at our own URL is not ours to
do.

`bbsk-share-mesto-kpi.yaml` is how the region's application reads the city's indicators: the
consumer declares the reference and the source Endpoint's audience and Policy decide (EP-15,
EP-77). The region reads and cannot write.

Which feeds are ingested, with the status code each answered with, is
`joinedcontext-docs/Research/banska-bystrica-open-data-sources.md`; the projects, spaces, ids and
KPI entity are frozen in `joinedcontext-docs/Development/10-banska-bystrica-contract.md`, and the
indicators themselves in `…/11-banska-bystrica-kpis.md`.

Regenerate the model artifacts after editing `statistical-observation.linkml.yaml`, and copy the
four of them into `../banskabystrica/` so the city's own copy cannot drift
(`tests/test_bystrica_seed.py` holds the two identical):

```sh
T=../../../../../joinedcontext-platform/tools/model-tools/src   # the platform checkout
python3 $T/gen_json_schema.py statistical-observation.linkml.yaml -o statistical-observation.v1.schema.json
python3 $T/gen_context.py statistical-observation.linkml.yaml -o statistical-observation.v1.context.jsonld
cp statistical-observation.linkml.yaml statistical-observation.v1.schema.json \
   statistical-observation.v1.context.jsonld statistical-observation.v1.md ../banskabystrica/
```

`statistical-observation.v1.md` and each folder's `…example.jsonld` are written by hand: the
generated docs page does not carry the house heading and the example has to name its own space.
