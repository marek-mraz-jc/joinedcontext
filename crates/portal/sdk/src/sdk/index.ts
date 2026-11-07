// @joinedcontext/sdk entry point barrel

// NGSI-LD & Schema types
export type { Cell, Column, Geo, LanguageMap, RelationshipObject, Row, WriteValue } from "../ngsi";
export { isLanguageMap, isRelationshipObject, targetsOf } from "../ngsi";
export { fieldOf } from "../write";
export { enumOptions, enumsOf, optionLabel } from "../enums";
export type { EnumOption } from "../enums";
export { relationsOf } from "../relations";
export type { RelationEnd, TargetOption } from "../relations";
export { NGSI_LD_NULL, RelationPicker } from "../grid/RelationPicker";
export type { PickerLabels } from "../grid/RelationPicker";
export type { Field, FieldSchema, Schema, TypeSchema } from "../write";

// Client
export { jc, ProblemError, ServiceRefusedError } from "./client";
export type { AiMessage, Client, Completion, DataClient, Email, EndpointOption, Feature, FeatureCollection, FeatureCollectionInfo, FeatureQuery, FileInfo, JobStatus, Query, TemporalPoint, TemporalQuery, TemporalRow } from "./client";

// Config (read by startApp from the document the Portal serves; app code never builds a client or a
// transport, SDK-02 and SDK-06)
export type { JcConfig, JcEndpoint, JcUser } from "./config";

// Access & Permissions
export { can } from "./access";
export type { AccessDocument, Decision, Grant } from "./access";

// Design Tokens
export { applyTokens, currentTokens, DEFAULT_TOKENS, echartsTheme, mapColors, rechartsPalette, resolveTokens } from "./tokens";
export type { DesignTokens } from "./tokens";

// Hooks
export { JcProvider, useAccess, useClient, useEntities, useEntity, useFunction, useMe, useOptionalClient, useSave, useSchema } from "./hooks";
export type { Loaded } from "./hooks";

// Filters logic
export { filterRows, useFilters } from "./filters";
export type { FilterBinding, FilterDef, FilterValue } from "./filters";

// Helpers
export { aggregate, columnKind, displayName, distinct, extent, format, groupBy, pointOf, toFeatureCollection } from "./helpers";
export type { Agg, Group } from "./helpers";

// The entity grid (UI-64, UI-71, SDK-29): one component for the Portal, Dashboards and apps
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
export { GeoView, boundsOf, featuresOf, positionsOf } from "../geo/GeoView";
export type { GeoFeature } from "../geo/GeoView";
export { DEFAULT_GEO_LABELS, DRAW_MODES, GeoEditor, MAX_GEOJSON_BYTES, modesFor, rowLabel, rowsOf, terraDrawEngine, withPosition, withoutPosition } from "../geo/GeoEditor";
export type { CoordinateRow, DrawEngine, DrawMode, GeoLabels } from "../geo/GeoEditor";
export { GEOMETRY_TYPES, MAX_VERTICES, checkGeometry, geometryOf } from "../geo/validate";
export type { GeoCheck, GeoFinding, Geometry, GeometryType, Position } from "../geo/validate";
export type { EntitySource, GridPage, GridQuery, HistoryPoint, HistoryWindow } from "../grid/source";
export { attributesOf, cellText, projectRow, toRichCell, toRichRow } from "../grid/model";
export type { CellKind, RichCell, RichRow } from "../grid/model";
export { andQ, opsForKind, queryFromFilters, queryFromGroup, searchQuery } from "../grid/filters";
export type { ColumnFilter, FilterColumn, FilterCondition, FilterGroup, FilterKind, FilterOp } from "../grid/filters";
export { applyChanges, attrsBody, MAX_ENTITIES } from "../grid/apply";
export type { ApplyResult, AttributeChange, EntityChange, Observed } from "../grid/apply";
export { problemOf } from "../grid/rules";
export { matchesQ, parseQ } from "../grid/qmatch";
export type { RuleLabels, ValueRule } from "../grid/rules";
export { asCsv, EntityHistory, MAX_POINTS } from "../grid/EntityHistory";
export type { HistoryLabels } from "../grid/EntityHistory";
// The host's own transport, for a page that renders the grid itself (T-1439).
export { originTransport, transportFor } from "./transport";

// Layout primitives: every screen responsive without the app writing CSS (T-2777, UI-84)
export { Card, Grid, Header, Page, Sidebar, Split, Tabs } from "./layout";
export type { Tab } from "./layout";
// The one shell and the entity panel every App shares (SDK-39, SDK-40).
export { AppShell, Empty, ErrorBoundary, Loading, Problem } from "./shell";
export type { ShellLanguage, ShellPage } from "./shell";
export { EntityPanel, EntitySelectionProvider, attributeOrder, labelOf, parseValue, portalLinkOf, selectable, useEntitySelection } from "./panel";
export type { PanelSource, SelectedEntity } from "./panel";
export { SDK_WORDS, sdkLanguage, sdkWord } from "./words";
export type { SdkLanguage, SdkWord } from "./words";

// Tables, maps, exports: what the template's components stand on
export { compare } from "../views/Table";
export { mapWorkerReady, NO_BASEMAP, NO_LOCATIONS, styleFor } from "./map";
export { download, toCsv, toGeoJson, toPdf, toPng } from "../artifact";

// Application Bootstrap
export { startApp } from "./start";
export { reportError } from "./report";

// UN/CEFACT units (DM-06): the platform's code list, and a value written with its unit's symbol.
export { UNITS, conversion, convertible, formatValue, unitLabel, unitOf, unitSymbol, unitTitle } from "./units";
export type { Unit } from "./units";
