# The Žilina seed: five spaces, one model each

The `zilina` project (T-3137): the city of Žilina and the University of Žilina, seeded the same way
as `bbsk/` and `banskabystrica/`. `index.yaml` maps each file to its path in the configuration
repository (jc-core `PATH_TEMPLATE`s), and the gateway mounts each under its own name.

Neither the city nor the university publishes in the national catalogue, and the city's own GIS
layers carry no licence, so the project started from what national publishers release about the
city and from the university's own repository. Since 2026-05-04 the city runs its own open data
portal, `opendata.zilina.sk` (Invipo, ODbL, credited "Mesto Žilina"): its car parks, buses and
trolleybuses, traffic counters and street lights reach `zilina-doprava` live (T-5232), and
`../../../../seeds/zilina-opendata/inventory.csv` records every one of its 145 dataset pages as
taken, merged as a copy, or skipped with the reason. Which feed answered with which status, and which were
refused and why, is `joinedcontext-docs/Research/zilina-open-data-sources.md`.

| space | model | holds |
|---|---|---|
| `zilina-mesto` | `statistical-observation` | ŠÚ SR rows for the city `SK031B517402`, one entity per cube cell, codes verbatim |
| `zilina-verejne` | `zilina-verejne` | the EEA station SK0020A, the city's monuments placed by address, its railway stations |
| `zilina-uniza` | `zilina-uniza` | DREPO's openly licensed works, without author names |
| `zilina-kpi` | `key-performance-indicator` | the indicators computed from the two city spaces (PF-54, PF-55) |
| `zilina-doprava` | `zilina-doprava` | the city portal's car parks (every 5 min), buses and trolleybuses with their delay (every 5 min), traffic counts of the last full hour, street lights (daily) |

`statistical-observation` and `key-performance-indicator` are the same models as in `../bbsk/`
(copied, not edited: `tests/test_bystrica_seed.py` holds the copies identical); only their
examples name this project's spaces. Regenerate the artifacts of the three own models after editing
their LinkML:

```sh
T=../../../../../joinedcontext-platform/tools/model-tools/src   # the platform checkout
for m in zilina-verejne zilina-uniza zilina-doprava; do
  python3 $T/gen_json_schema.py $m.linkml.yaml -o $m.v1.schema.json
  python3 $T/gen_context.py $m.linkml.yaml -o $m.v1.context.jsonld
  python3 $T/gen_docs.py $m.linkml.yaml -o $m.v1.md
  python3 $T/gen_example.py $m.linkml.yaml -o $m.v1.example.jsonld
done
```
