# Dashboard

<!-- Generated from the jc-core schema by crates/portal/tests/kind_reference_tests.rs; do not edit. -->

- apiVersion: `joinedcontext.com/v1alpha1`, kind: `Dashboard`
- Lives in a project (namespace: the project's slug)
- API: `/api/v1/projects/{project}/dashboards`
- Repository path: `projects/{project}/dashboards/{name}.yaml`

Every field below is checked with `deny_unknown_fields`: a field this page does not name is refused at propose, approve and load (`jcctl validate --as-portal`).

## spec (DashboardSpec)

`spec` of a Dashboard (UI-17).

| Field | Type | Required | Description |
|---|---|---|---|
| `pages` | list of [Page](#page) | yes | At least one page. |
| `title` | [Text](#text) | yes | The title (UI-50): one string, or the legacy map per locale (PF-24). |
| `visibility` | [DashboardVisibility](#dashboardvisibility) | no | Who may open it. |

## Page

One page: a map of layers, or a grid of widgets, or both.

| Field | Type | Required | Description |
|---|---|---|---|
| `layers` | list of string | no | Names of `Layer` manifests of the same project, drawn bottom to top. |
| `layout` | string | no | `full-map`, `grid-2x2`, …; the Portal's default when absent. |
| `title` | string | no | Shown as the page's tab. |
| `widgets` | list of [Widget](#widget) | no | The widgets of the page. |

## Text

A human label or description (UI-50): one plain string in the author's language. The legacy language map is still read, through release `v0.9`, and resolves to one string.

Type: string or [MultiLanguageMap](#multilanguagemap)

## DashboardVisibility

Who may open the dashboard (UI-19); `private` when undeclared.

Type: one of `"private"`, `"project"`, `"organization"`, `"public"`

## Widget

A widget of an analytics page (Architecture/10 §1).

| Field | Type | Required | Description |
|---|---|---|---|
| `endpointRef` | string | no | The Endpoint it reads through. |
| `entityId` | string | no | The entity it shows. |
| `entityType` | string | no | The entity type a `grid` widget shows, the type decides its columns (UI-71), or the type a `bar-chart` or `histogram` counts (UI-93). |
| `grid` | [GridConfig](#gridconfig) | no | The configuration of a `grid` widget: the same object the explorer and a generated application are configured by, without the source and the type those two fields decide (SDK-30, T-1440). |
| `property` | string | no | The property it shows. |
| `q` | string | no | The NGSI-LD `q` a `bar-chart` or `histogram` reads the type with: the explorer's filter when the chart came from there (UI-93). |
| `widgetType` | string | yes | The widget, e.g. `temporal-chart` or `grid`. |

## MultiLanguageMap

Multi-language map keyed by ISO 639-1 two-letter lowercase language codes

Type: map of string

## GridConfig

The configuration of a `grid` view: `EntityGridConfig` of the SDK without the two fields the application decides for it (SDK-30). It reads through the app's own endpoint and shows the type of the view's source, so a spec can neither point the grid at another endpoint nor at another type; everything else — the columns, the page size, the filters, the edit — is the spec's.

| Field | Type | Required | Description |
|---|---|---|---|
| `columns` | list of [GridColumn](#gridcolumn) | no | The columns shown, in order; every attribute of the loaded rows when empty. |
| `density` | [GridDensity](#griddensity) | no | How much room a row takes. |
| `editableAttrs` | list of string | no | The attributes a correction may be typed into, which `mode: edit` requires. |
| `entityTimestamps` | boolean | no | Whether the entity's own `createdAt` and `modifiedAt` are columns too. |
| `filters` | [GridFilters](#gridfilters) | no | What may be filtered, and what is filtered from the start. |
| `history` | [GridHistory](#gridhistory) | no | Whether an attribute's history may be opened from its column. |
| `mode` | [GridMode](#gridmode) | no | Whether a correction may be typed into a cell; read only when absent. |
| `pageSize` | integer (uint32) | no | Entities per page, 1 to 1000; 50 when absent. |
| `rowActions` | list of string | no | The row actions the host offers, by name; the host renders them. |

## GridColumn

One column of a `grid` view (SDK-30, UI-71).

| Field | Type | Required | Description |
|---|---|---|---|
| `attr` | string | yes | The attribute shown, which has to be one the source asked for. |
| `editable` | boolean | no | Whether this column takes a correction, in `mode: edit`. |
| `format` | [GridFormat](#gridformat) | no | How the value is drawn, when the rows alone do not say. |
| `label` | string | no | The heading, in place of the attribute's own name. |
| `pinned` | boolean | no | Whether the column stays in view while the grid scrolls sideways. |
| `show` | [GridColumnShow](#gridcolumnshow) | no | Which of the value's metadata columns start open. |
| `width` | integer (uint32) | no | The column's width in pixels. |

## GridDensity

How much room a row takes.

Type: one of `"compact"`, `"comfortable"`

## GridFilters

What the grid may be filtered by, and what it is filtered by from the start.

| Field | Type | Required | Description |
|---|---|---|---|
| `allowed` | list of string | no | The columns whose filter row is offered; every filterable one when absent. |
| `preset` | [GridPreset](#gridpreset) | no | What the grid asks the endpoint for before anyone filters on screen. |

## GridHistory

Whether one attribute's history may be opened, and how much of it is read.

| Field | Type | Required | Description |
|---|---|---|---|
| `enabled` | boolean | no | Whether an attribute's history may be opened at all. |
| `maxPoints` | integer (uint32) | no | Points one history read returns at most. |

## GridMode

Whether the grid takes a correction at all.

Type: one of `"view"`, `"edit"`

## GridFormat

How a cell is drawn when the loaded rows do not say by themselves.

Type: one of `"text"`, `"number"`, `"date"`, `"link"`

## GridColumnShow

Which metadata columns of one attribute start open beside its value.

| Field | Type | Required | Description |
|---|---|---|---|
| `createdAt` | boolean | no | When the attribute was first written. |
| `datasetId` | boolean | no | Which dataset the instance belongs to. |
| `modifiedAt` | boolean | no | When the attribute last changed. |
| `observedAt` | boolean | no | When the value was observed. |
| `unit` | boolean | no | What the value is measured in. |

## GridPreset

The query the grid always asks the endpoint with, before anyone filters on screen.

| Field | Type | Required | Description |
|---|---|---|---|
| `attrs` | list of string | no | The attributes asked for, when not every one of them. |
| `idPattern` | string | no | A regular expression the ids must match. |
| `q` | string | no | An NGSI-LD `q` the grid always asks with. |
| `scopeQ` | string | no | A scope expression the entities must be in. |
