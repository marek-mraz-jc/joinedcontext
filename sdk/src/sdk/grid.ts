// @joinedcontext/sdk/grid (UI-64, UI-71, SDK-29; G-R2GRID2-001): the entity grid as its own entry,
// for a Portal page, an App or a public page. Data comes through an `EntitySource` (NGSI-LD's
// `endpointSource`/`spaceSource`, or any adapter with `query` and `get`), the caller's decisions
// through `access` (prop or `source.access`), every visible string through `labels`. Nothing here
// imports outside the SDK (tests/grid_boundary.test.ts); the root entry re-exports the same names.
export { EntityGrid } from "../grid/EntityGrid";
export * from "../grid/parts";
export type { EntityGridProps } from "../grid/EntityGrid";
export { DEFAULT_LABELS, readableName, useEntityGrid } from "../grid/useEntityGrid";
export type { ColumnLayout, GridLabels, GridLabelsInput, GridState, MetaKey, UseEntityGridOptions, VisibleColumn } from "../grid/useEntityGrid";
export { targetName } from "../grid/cells";
export type { CellLabels, ColumnKind } from "../grid/cells";
export { DEFAULT_PAGE_SIZE, gridConfigSchema, MAX_PAGE_SIZE, parseGridConfig } from "../grid/config";
export type { ConfigFinding, EntityGridConfig, GridColumn, GridSource, ResolvedGridConfig } from "../grid/config";
export { Dashboard } from "../grid/Dashboard";
export { parseDashboardConfig } from "../grid/dashboard";
export type { DashboardConfig } from "../grid/dashboard";
export { DEFAULT_GRID_MAP_LABELS, GridMap, summaryOf } from "../grid/GridMap";
export type { GridMapLabels, GridMapProps } from "../grid/GridMap";
export { diffGeometry, featuresOfRows, geometryOfRow, mapAttrOf, metresBetween, rowsOffTheMap, verticesOf } from "../grid/mapRows";
export type { GeometryDiff } from "../grid/mapRows";
export { areaQuery, decimate, MAX_COORDINATES_CHARS, ringOf, ringOfBounds } from "../grid/geoarea";
export type { GeoArea, GeoQuery } from "../grid/geoarea";
export { EntityCompare, DEFAULT_COMPARE_LABELS, pageSource } from "../grid/EntityCompare";
export type { CompareLabels, EntityCompareProps } from "../grid/EntityCompare";
export { endpointSource, fixtureSource, historyOf, idChunks, SourceError, sourceFor, spaceSource } from "../grid/source";
export type { AccessReason, AttributeAccess, EntitySource, GridPage, GridQuery, HistoryPoint, HistoryWindow } from "../grid/source";
export { attributesOf, cellText, projectRow, toRichCell, toRichRow } from "../grid/model";
export type { CellKind, RichCell, RichRow } from "../grid/model";
export { andQ, opsForKind, queryFromFilters, queryFromGroup, searchQuery } from "../grid/filters";
export type { ColumnFilter, FilterColumn, FilterCondition, FilterGroup, FilterKind, FilterOp } from "../grid/filters";
export { applyChanges, attrsBody, MAX_ENTITIES } from "../grid/apply";
export type { ApplyResult, AttributeChange, EntityChange, Observed } from "../grid/apply";
export { problemOf } from "../grid/rules";
export type { RuleLabels, ValueRule } from "../grid/rules";
export { matchesQ, parseQ } from "../grid/qmatch";
export { asCsv, EntityHistory, MAX_POINTS } from "../grid/EntityHistory";
export type { HistoryLabels } from "../grid/EntityHistory";
export { NGSI_LD_NULL, RelationPicker } from "../grid/RelationPicker";
export type { PickerLabels } from "../grid/RelationPicker";
export type { DrawEngine, GeoLabels } from "../geo/GeoEditor";
// A host page's own transport, for the built-in sources (T-1439).
export { originTransport, transportFor } from "./transport";
export type { Transport } from "./transport";
