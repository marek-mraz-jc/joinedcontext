# One computed indicator (`helsinki-kpi` 1.0.0)

The model of Helsinki's indicator space `helsinki-kpi`, generated from `helsinki-kpi.linkml.yaml`,
which is the regions' indicator contract under the `hel.fi` id. One class,
`KeyPerformanceIndicator`, specialising the shared `Entity` of `ngsi-ld-core`. It states the
contract the platform already enforces: `schemas/kinds/KeyPerformanceIndicator.json` closes the
object and requires every attribute below (PF-54), and the indicator pipelines the assistant
drafts write exactly this set.

| Attribute | NGSI-LD kind | Range | Unit | IRI |
|---|---|---|---|---|
| `id` | Property | uri | | `ngsi-ld:hasId` |
| `type` | Property | string | | `ngsi-ld:hasType` |
| `location` | GeoProperty | GeoJSON geometry | | `geojson:geometry` |
| `observedAt` | Property | datetime | | `ngsi-ld:observedAt` |
| `name` | Property | string | | `jc:name` |
| `title` | LanguageProperty | string per language | | `jc:title` |
| `currentValue` | Property | number or string | on the Property as `unitCode` | `jc:currentValue` |
| `calculationPeriod` | Property | `{ start, end }` | RFC 3339 instants | `jc:calculationPeriod` |
| `calculationFormula` | Property | string | | `jc:calculationFormula` |
| `derivedFrom` | Property | uri | | `jc:derivedFrom` |
| `computedBy` | Property | uri | | `jc:computedBy` |
| `updatedAt` | Property | datetime | | `jc:updatedAt` |

Seven own attributes are required, as are `id` and `type`; `title`, the human name per language, is optional.

`state` and `threshold` are absent by decision. A threshold is the judgement of whoever reads the
number: it changes without the number changing, and two departments may hold different ones for
the same indicator. It lives in the configuration of the view, and the lane a view paints is what
that view computes from the value and its own threshold. An entity carrying either is rejected by
`jc-core` and by the gateway before it is stored.
