# Layer

<!-- Generated from the jc-core schema by crates/portal/tests/kind_reference_tests.rs; do not edit. -->

- apiVersion: `joinedcontext.com/v1alpha1`, kind: `Layer`
- Lives in a project (namespace: the project's slug)
- API: `/api/v1/projects/{project}/layers`
- Repository path: `projects/{project}/dashboards/{name}.yaml`

Every field below is checked with `deny_unknown_fields`: a field this page does not name is refused at propose, approve and load (`jcctl validate --as-portal`).

## spec (LayerSpec)

`spec` of a Layer (UI-18).

| Field | Type | Required | Description |
|---|---|---|---|
| `colorBy` | [ColorBy](#colorby) | no | The colour encoding. |
| `entityType` | string | yes | The NGSI-LD entity type it shows. |
| `filter` | [LayerFilter](#layerfilter) | no | The static filter. |
| `popupProperties` | list of string | no | The properties a click shows, in order; the first few of the feature when empty. |
| `sizeBy` | [SizeBy](#sizeby) | no | The size encoding. |
| `sourceEndpointRef` | string | yes | The Endpoint of the same project the layer reads through, by `metadata.name`. |
| `style` | [LayerStyle](#layerstyle) | no | How it draws. |
| `visible` | boolean | no | Drawn when the dashboard opens; `false` leaves it in the legend, switched off. |

## ColorBy

Colour by one numeric property over a domain (UI-18).

| Field | Type | Required | Description |
|---|---|---|---|
| `domain` | list of number (double) | no | `[min, max]` of the property mapped onto the ramp. |
| `palette` | string | no | A ColorBrewer ramp name, e.g. `YlOrRd`; the Portal's default when absent. |
| `property` | string | yes | The property read from every feature. |

## LayerFilter

The static NGSI-LD filter every request of the layer carries (UI-18, UI-22).

| Field | Type | Required | Description |
|---|---|---|---|
| `geoQ` | string | no | A fixed `geoQ`; the map adds its own viewport bbox on top (UI-22). |
| `q` | string | no | The `q` expression, e.g. `pm10>0`. |
| `scopeQ` | string | no | The `scopeQ` expression, e.g. `/geo/FI/HKI/#`. |

## SizeBy

Size by one numeric property over a pixel range (UI-18).

| Field | Type | Required | Description |
|---|---|---|---|
| `property` | string | yes | The property read from every feature. |
| `range` | list of number (double) | no | `[min, max]` radius in pixels. |

## LayerStyle

How a layer draws (UI-20, UI-21): the three MapLibre styles and the aggregations only deck.gl renders; `icon` draws as `circle` until the Portal has an icon renderer.

Type: one of `"circle"`, `"line"`, `"fill"`, `"heatmap"`, `"hexagon"`, `"icon"`
