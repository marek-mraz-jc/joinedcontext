# @joinedcontext/sdk

### How an app is built
`src/main.tsx` calls `startApp(App, { tokens })`.
Pages are laid out with this package's layout primitives (`Page`, `Header`, `Grid`, `Card`, `Split`, `Sidebar`, `Tabs`) and compose the template's components with its hooks, so every screen works from a 375 px phone to a 2560 px wall without layout CSS of its own. The components are files of the application in `src/components/` (`AppShell`, `states` with `Problem`, `Loading`, `Empty` and `ErrorBoundary`, `StatTiles`, `TypeGrid`, `EntityDetail`, `filters`, `EntityForm`, `ExportButton`, `charts`, `EntityMap`, `components.css`): read them in the pack and change them like any other file. This reference covers the package they stand on.
Backend functions live in `functions/{name}.ts` using `@joinedcontext/sdk/server`.
Tests verify behaviors using `@joinedcontext/sdk/testing`.
`design-tokens.json` re-themes all component styling, chart palettes, and map layers.
Allowed imports: relative, `react`, `react-dom/client`, `react/jsx-runtime`, `echarts`, `recharts`, `maplibre-gl`, `@deck.gl/core`, `@deck.gl/layers`, `@deck.gl/aggregation-layers`, `@deck.gl/mapbox`, `@joinedcontext/sdk`, `@joinedcontext/sdk/grid`, `@joinedcontext/sdk/server`, `vitest`, `@testing-library/react`, `@joinedcontext/sdk/testing`.

---

## Entry Point: `@joinedcontext/sdk`

### Client
```ts
function jc(): Client
```
Returns the active SDK client singleton, reading configuration on first access.
```ts
class ProblemError extends Error
```
RFC 7807 problem error representation containing HTTP status, title, detail, type, and source location.
```ts
class ServiceRefusedError extends ProblemError { service: "files" | "email" | "jobs" | "ai"; layer?: "organization" | "project" | "app"; quota?: string; resetAt?: string }
```
A platform service the organization, the project or the App switched off (`403`, `layer` names which), or a daily quota used up (`429`, `quota` and `resetAt`). Show it; never retry before `resetAt`.
```ts
jc().features.collections(options?: { endpoint?: string }): Promise<FeatureCollectionInfo[]>
jc().features.items(collection: string, query?: FeatureQuery): Promise<FeatureCollection>
jc().features.get(collection: string, id: string, options?: { endpoint?: string }): Promise<Feature>
type FeatureQuery = { endpoint?: string; bbox?: [number, number, number, number]; datetime?: string; filter?: string; properties?: string[]; sortby?: string[]; limit?: number }
```
The endpoint's OGC API - Features surface (EP-29), only where it lists `ogc-features` (else `404`): a collection is an entity type, a Feature's `id` the URN, `geometry` the `location`, `properties` the other attributes; the entities the endpoint's view and Policy admit. `filter` is CQL2 text; an operator the endpoint lacks is a `400` naming it. `bbox` is `[minLon, minLat, maxLon, maxLat]` (WGS 84), `datetime` an instant or interval (`..` open end), `sortby` names (`-` descending), `limit` 1 to 1000 (default 100). A `FeatureCollection` is a GeoJSON source as it is.
```ts
jc().email.send(message: { to: string[] | "me"; subject: string; text: string; html?: string }): Promise<{ id: string }>
```
Sends a message to people of the organization, named by person id or `"me"` (the signed-in person), never by address; needs `email` in the App's `spec.services`. The platform adds the unsubscribe link. A subject with a line break is refused.
```ts
jc().jobs.list(): Promise<{ name: string; schedule: string; nextRun?: string; lastRun?: { at: string; ok: boolean; message?: string } }[]>
```
Lists the App's scheduled jobs (`spec.server.jobs[]`) with the next minute each runs, absent for a schedule that names none within a year, and its last run as the host reported it. Needs `jobs` on at every layer; the organization's default list has it.
```ts
jc().files.put(path: string, body: Blob | ArrayBuffer | string, opts?: { contentType?: string }): Promise<FileInfo>
jc().files.get(path: string): Promise<Blob>
jc().files.list(prefix?: string): Promise<FileInfo[]>
jc().files.remove(path: string): Promise<void>
jc().files.url(path: string, opts?: { method?: "GET" } | { method: "PUT"; size: number }): Promise<{ url: string; expiresAt: string }>
type FileInfo = { path: string; size: number; contentType: string; modifiedAt: string }
```
Keeps the App's objects under its own prefix; a key is relative, has no `.` or `..` segment and is at most 512 bytes. One object is at most 25 MiB (`413`), all of them at most the App's `filesMiB` (`429`, no `resetAt`: delete to make room). `url` hands out a URL for one key and one method that expires within five minutes; a PUT URL names the `size` it stores, held to 25 MiB and to what `filesMiB` has left, and the store refuses a body of any other length. Needs `files` on at every layer; the organization's default list has it. In the browser on the App's own host; the preview's bridge does not carry bytes.
```ts
jc().ai.complete(request: { messages: { role: "system" | "user" | "assistant"; content: string }[]; maxTokens?: number; schema?: object }): Promise<{ text: string; json?: unknown; tokens: { in: number; out: number } }>
```
Asks the model the platform chooses; the App names no model and holds no key. At most 50 messages and 100 KB, `maxTokens` 1 to 4096 (1024 when absent) and never more than the App's `aiTokensPerDay` has left after the prompt, which counts about one token per four bytes; a prompt the day has no room for is `429`. With `schema`, `json` is the answer as one value of that JSON Schema, or the call fails with `502` (`…/no-json`): ask again. A spent day is `429` with `resetAt`; an organization without a working model key is `503` (`…/no-model-key`). Needs `ai` on at every layer, which the organization's default list does not have.

### Hooks
```ts
function JcProvider({ client, children }: { client?: Client; children: ReactNode }): JSX.Element
```
Context provider supplying an SDK Client to nested hooks and components.
```ts
function useClient(): Client
```
Returns the Client supplied by the nearest JcProvider or the global singleton.
```ts
function useEntities<T extends Row = Row>(type: string, query?: Query, options?: { refreshMs?: number; enabled?: boolean; all?: boolean }): Loaded & { rows: T[] }
```
Loads and polls entities of a given type with query parameters, caching, and race condition protection.
```ts
function useEntity<T extends Row = Row>(id: string | null, attrs?: string[]): Loaded & { entity: T | null }
```
Loads a single entity by NGSI-LD identifier.
```ts
function useSave(): { create: (type: string, attrs: Record<string, Cell>, localId?: string, options?: { endpoint?: string }) => Promise<string | null>; update: (id: string, patch: Record<string, Cell>) => Promise<boolean>; remove: (id: string) => Promise<boolean>; saving: boolean; problem: ProblemError | null; clear: () => void }
```
Provides write operations (`create`, `update`, `remove`) that track saving state and ProblemError refusals without throwing.
```ts
function useSchema(type?: string): { schema: Schema | null; typeSchema: TypeSchema | null; error: ProblemError | null }
```
Loads merged JSON Schema definitions for all models exposed by the endpoint.
```ts
function useAccess(endpoint?: string): { access: AccessDocument | null; error: ProblemError | null; can: (operation: string, type: string, attr?: string) => Decision }
```
Loads user permissions (of the named endpoint, the primary by default) and provides synchronous evaluation of permissions and prohibitions. The endpoint decides; a refusal to a person who holds roles in the application names them: "Your role viewer does not permit updateAttrs on Alert." (SDK-36).
```ts
function useMe(): JcUser | null
```
Returns `{ id, name, email, roles }` of the signed-in person as the host served it, `roles` being the person's roles in this application (`spec.access`), or `null` for an anonymous visitor. It never fetches, decodes or stores a token (SDK-35, AP-95).
```ts
function useFunction<T = unknown>(name: string, body?: unknown, options?: { enabled?: boolean }): Loaded & { data: T | null }
```
Invokes a backend server function and manages its response lifecycle.

### Layout (UI-84)
```ts
function Page(props: { children; width?: "narrow" | "wide" | "full"; label? }): JSX.Element
function Header(props: { title; subtitle?; actions?; level?: 1 | 2 | 3 }): JSX.Element
function Grid(props: { children; columns?: 1 | 2 | 3 | 4 }): JSX.Element
function Card(props: { title?; actions?; children; level?: 1 | 2 | 3; label? }): JSX.Element
function Split(props: { children: [ReactNode, ReactNode]; ratio?: "1:1" | "2:1" | "1:2" }): JSX.Element
function Sidebar(props: { label: string; side: ReactNode; children; position?: "start" | "end" }): JSX.Element
function Tabs(props: { tabs: { id; label; render: () => ReactNode }[]; label: string; initial?; onChange? }): JSX.Element
```
Each one lays out by the width it is given (container queries), so a `Grid` inside a `Split` or a framed app adapts to its own box: `Grid` is 1 column, then 2 from 40rem, 3 from 64rem, `columns` (at most 4) from 90rem; `Split` stacks under 48rem; `Sidebar` is a column beside the content from 48rem and a drawer behind a button named `label` on a phone (Escape closes it); `Tabs` follows the WAI-ARIA pattern (arrow keys, Home, End) and scrolls sideways on a phone. `--jc-gutter` and `--jc-font-size-title` grow with the window; `--jc-target` (44 px) is the least height of a control on a phone or a touch screen.

### Shell and entity panel (SDK-39, SDK-40)
```ts
function AppShell(props: { title; pages: ShellPage[]; actions?; initial?; languages?: ShellLanguage[]; language?; onLanguage?(code); source?: PanelSource; userName? }): JSX.Element
function useOptionalClient(): Client | null
function Loading(props: { label? }): JSX.Element
function Empty(props: { children? }): JSX.Element
function Problem(props: { error: ProblemError | Error | null | undefined; onRetry? }): JSX.Element | null
class ErrorBoundary
function useEntitySelection(): { selected: SelectedEntity | null; select(e: SelectedEntity): void; clear(): void }
function selectable(e: SelectedEntity, select): { role; tabIndex; onClick; onKeyDown }
function EntityPanel(): JSX.Element | null
function EntitySelectionProvider(props: { children }): JSX.Element
function attributeOrder(row: Row, properties?): string[]
function labelOf(name: string, property?): string
function parseValue(field: Field, text: string, language: SdkLanguage): { value: Cell } | { error: string }
function portalLinkOf(portal: string | undefined, space: string, id: string): string | null
const SDK_WORDS; function sdkLanguage(configured?): SdkLanguage; function sdkWord(language, word, slots?): string
```
Every App renders `AppShell`; it holds the selection and the one `EntityPanel`. Make any map feature, table row, chart point or card open an entity with `select({ id, type })` (or spread `selectable(...)` for click and Enter). The panel lists the attributes with the schema's labels; **Edit** shows only to a signed-in reader whose access document allows `updateAttrs` on the type (and the attribute), checks each value against the schema, shows the change and writes it through the App's Endpoint after the reader confirms; a 409 or 403 is said in words. Without the right, or on a `public` App, it links to the entity in the Portal (`config.portal`).

### Tables, forms, maps, exports
```ts
function compare(a: Row, b: Row, attr: string, dir: "asc" | "desc"): number
```
Orders two rows by one attribute: numbers as numbers, text in the document's locale, empty last.
```ts
function fieldOf(name: string, schema: TypeSchema | undefined, kind: Column, defs?: Record<string, unknown>, language?: string): Field
```
The form input for one attribute from the endpoint's JSON Schema: number, text, select, date, checkbox, geo or language (a LanguageProperty). Pass the merged `Schema` from `useSchema` as `defs`, so an enum the model writes as a `$ref` becomes a select too.
```ts
function enumOptions(property: unknown, defs?: Record<string, unknown>, language?: string): EnumOption[] | null
function enumsOf(type: unknown, defs?: Record<string, unknown>, language?: string): Record<string, EnumOption[]>
function optionLabel(option: EnumOption): string
```
The permissible values of an attribute (UI-86): a direct `enum`, or one behind `$ref`, `allOf`, `anyOf` with `null`, or a `oneOf` of `const`s; each `{ value, title?, description? }`, the title in `language` where the model has one. `null` when the attribute is not an enum. `optionLabel` is what a picker shows: the title, else the value. `enumsOf` reads every enum attribute of one type's schema, which is what the grid's `enums` prop takes: such a column is then edited and filtered by picking.
```ts
function relationsOf(type: unknown): Record<string, RelationEnd>
```
The relationship ends of one type's schema (UI-84): every property with `x-ngsi-ld-relationship`, as `{ target, many, required }`. Pass it as the grid's `relations` prop: the column is then edited by picking entities of the target class that the person can read, one on a single end and several on a many end, and a required end keeps its last target.
```ts
function RelationPicker(props: { label: string; value: string[]; end: RelationEnd; search: (text: string) => Promise<TargetOption[]>; labels: PickerLabels; changed?: boolean; invalid?: string; onChange(next: string[]): void }): JSX.Element
const NGSI_LD_NULL: string
function targetsOf(value: Cell | undefined): string[]
```
The control the grid, the spec Form and the template's `EntityForm` edit a relationship end with (UI-84): a group named `label` with the picked targets as chips and a combobox that runs `search` a pause after the last key. A single end's pick replaces its target, a many end's adds one, and a required end's last target has no remove button. `labels` carries every visible string. `targetsOf` reads a row's cell back as targets (a many end's URNs are joined by `", "`); a cleared optional end is written as `{ object: NGSI_LD_NULL }`, which removes the attribute. `Query.idPattern` is the NGSI-LD `idPattern` a search sends.
```ts
function isLanguageMap(value: unknown): value is LanguageMap
```
Whether a value is `{ languageMap }`, the shape `entities.create`/`update` write as a `LanguageProperty`.
```ts
function isRelationshipObject(value: unknown): value is RelationshipObject
```
Whether a value is `{ object }` (one URN, or a list of URNs on a many end), the shape `entities.create`/`update` write as a `Relationship` (DM-64). A relationship end written as a plain URN string becomes a Property that names no target, which the gateway refuses on a required end.
```ts
const NO_BASEMAP: string
function styleFor(basemap?: string): string | StyleSpecification
```
The MapLibre style: the configured basemap URL, or a plain background and `NO_BASEMAP` shown beside it.
```ts
const NO_LOCATIONS: string
```
What a map shows instead of a count when none of its rows has a location: read the location from the endpoint that carries it.
```ts
function mapWorkerReady(doc?: Document): Promise<boolean>
```
Resolves once MapLibre's worker is set up for the preview frame; create a map only after it.
```ts
function toCsv(columns: string[], rows: Row[]): Blob
function toGeoJson(rows: Row[], locationAttr: string): GeoJsonCollection
function toPdf(input: { title: string; endpoint: string; filters: string; takenAt: string; attribution?: string; lines: string[] }): Blob
function toPng(canvas: HTMLCanvasElement): Promise<Blob>
function download(blob: Blob, filename: string): void
```
Exports of what is shown (AP-66) and the browser download of the result.

### Entity grid (UI-64, UI-71, SDK-29)
```ts
function EntityGrid(props: EntityGridProps): JSX.Element
function useGridView(props: EntityGridProps): GridView
function useEntityGrid(options: UseEntityGridOptions): { rows, columns, total, loading, error, state, labels, cellOf, toggleMeta, setOffset, setSort, moveActive, getGridProps, getHeaderProps, getRowProps, getCellProps, reload }
function parseGridConfig(raw: unknown): { config?: ResolvedGridConfig; findings: ConfigFinding[] }
const gridConfigSchema: JSONSchema // published as grid.config.schema.json
const DEFAULT_PAGE_SIZE = 50, MAX_PAGE_SIZE = 1000
const DEFAULT_LABELS: GridLabels
function targetName(urn: string): { type?: string; name: string }
function readableName(attr: string): string

// The filter row as a query: the endpoint filters, never the browser; the Portal's queryFromGroup, searchQuery too
function queryFromFilters(columns: FilterColumn[], filters: Record<string, ColumnFilter>): { q?: string; idPattern?: string }
function opsForKind(kind: FilterKind): FilterOp[]
function andQ(...parts: (string | undefined)[]): string | undefined

// Cells corrected by hand, applied through the source's own `patch` (T-1430; UI-67, AG-78)
function applyChanges(options: { source, entities, observed?, fallback?, now? }): Promise<ApplyResult>
function attrsBody(changes: AttributeChange[], observed: Observed, now?: () => string): Record<string, unknown>
const MAX_ENTITIES = 50

// A typed value checked at its cell against its model's rule before it is pending for Apply
function problemOf(rule: ValueRule | undefined, value: unknown, labels: RuleLabels): string | null

// Whether a loaded row matches an NGSI-LD q, for marks decided in the page (API/01 §30); null when the q is beyond the subset
function matchesQ(row: RichRow, q: string): boolean | null
function parseQ(q: string): Term[][] | null

// One attribute's history: the temporal read as a table and a line
function EntityHistory(props: EntityHistoryProps): JSX.Element
function asCsv(points: HistoryPoint[], attr: string): string
const MAX_POINTS = 1000

// The space beside an endpoint: the same page of ids through both doors
function EntityCompare(props: EntityCompareProps): JSX.Element
function pageSource(rows: RichRow[]): EntitySource
const DEFAULT_COMPARE_LABELS: CompareLabels
```
One spreadsheet-like grid of one entity type (`role="grid"`): an id column pinned, each cell read by its kind (a number in `locale` with its unit, local time, a point as lat, lon, a relationship as a chip), per column an icon and a menu (sort, filter, hide, rename, resize, move; the `observedAt`, unit, `datasetId`, `createdAt`, `modifiedAt` columns under Details). The config is data (`{ source, type, columns, entityTimestamps, filters, pageSize, mode, editableAttrs, history, compareWith, density, rowActions }`); `parseGridConfig` refuses unknown keys with JSON paths and fills defaults. Props: `config`, `source`, `labels` (every visible string; English defaults), `state` + `onStateChange` (controlled per key), `renderers` by attribute name, cell kind or `id`, `onOpenRelationship`, `onRows` (the rows shown and their offset), `onQuery` (the filters' `q` and `idPattern`, for a host that saves a view), `hidden` + `onHide`, `layout` + `onLayout` (`{ order, width, labels }`), `locale`, `rowTone` (a row's tone and reason, as a named swatch), `toolbar`, `empty`, `rules` (`ValueRule` per attribute: a pending value that breaks it is marked with the reason and holds Review back), `classNames`. `useEntityGrid` is the same without markup. Values render as text.

The header's second row filters at the endpoint (UI-66, EP-07): per column the operators its content allows, composed into one `;`-joined `q`, shown under the grid with a copy button and an "edit as text" switch. The footer carries the endpoint's own `NGSILD-Results-Count`, and nothing when a narrowed answer carried none (R22). In `mode: "edit"` the columns of `editableAttrs` take a typed value, reviewed as a batch with one `observedAt` choice and applied one `PATCH …/attrs` per entity through the source's `patch`; a refused entity keeps its cell and the endpoint's own sentence. A source without `patch` has no edit mode, one without `history` no history.

`EntityHistory` shows one attribute's temporal answer by what it holds, with a table of every value; `views` replaces a kind's picture. `heading` names the panel, `locale` the language of times and numbers, `fractionDigits` the precision (2 by default). Copy as CSV keeps the raw values.

A subset of the grid: `const view = useGridView(props)`, then `<GridFrame view={view}>` around any of `GridTable`, `GridPager`, `GridMapPanel`, `GridDetail`, `GridNotices`, `GridStatus`, `GridQueryBox`, `GridReview`, each with `view={view}`. `RowDetail` takes `tabs`; `AttributeView` shows one attribute.

`EntityCompare` puts a space and an endpoint side by side (UI-69, EP-61): the right side is asked for the left's ids, and `marks` paints on the left what the right does not answer.
```ts
function endpointSource(slug: string, transport: Transport, language?: string): EntitySource
function spaceSource(space: string, transport: Transport, language?: string): EntitySource
function sourceFor(source: { kind: "endpoint"; slug } | { kind: "space"; space }, transport: Transport, language?: string): EntitySource
function fixtureSource(entities: object[], language?: string): EntitySource
function idChunks(ids: string[], maxChars?: number): string[][]
function transportFor(config: JcConfig): Transport
function originTransport(fetchImpl?: typeof fetch, doc?: Document): Transport // a host page's own: same origin, the person's session and CSRF header
class SourceError extends Error { status: number }
function historyOf(body: unknown, attr: string): HistoryPoint[]
```
Where a grid reads: `query`, `get`, and where the source offers them `history`, `patch`, `remove`, `access`; any store with these is a source. `@joinedcontext/sdk/grid` exports the grid alone. The two built-in sources read the normalized representation with `options=sysAttrs` through the person's session; a space's 404 says "no grant on this space" whether or not it exists; an Endpoint's 404 on `history` is an empty series (no instances and no grant look alike, T-2972). The Endpoint's Policy decides every write.
```ts
function toRichRow(entity: object, language?: string): RichRow // { id, type, createdAt, modifiedAt, scope, cells, raw }
function toRichCell(attr: unknown, language: string): RichCell | null // { kind, value, unitCode, observedAt, datasetId, createdAt, modifiedAt, object, geometryType, languageMap, sub }
function projectRow(rich: RichRow, language?: string): Row
function attributesOf(rows: RichRow[]): string[]
function cellText(cell: RichCell | RichCell[] | undefined): string
```
The rich cell model: a multi-instance attribute (several `datasetId`s) is an array; `raw` is the entity as received, so an edit never drops a member the grid does not show.

### The grid and the map together (UI-72, UI-66, UI-67)
```ts
function GridMap(props: GridMapProps): JSX.Element          // the panel `config.map.enabled` mounts
function mapAttrOf(rows: RichRow[], named?: string): string | null
function geometryOfRow(row: RichRow, attr: string): Geometry | null
function featuresOfRows(rows, attr, edits?): GeoFeature[]   // pending edits included
function rowsOffTheMap(rows, attr, edits?): string[]         // the rows with no geometry
function diffGeometry(before: Geometry | null, after: Geometry | null): GeometryDiff
function summaryOf(before, after, labels: GridMapLabels): string   // "Point moved 38 m"
function metresBetween(a: Position, b: Position): number
function verticesOf(g: Geometry | null): Position[]
function areaQuery(area: GeoArea | null, maxChars?): GeoQuery | null
function ringOf(g: Geometry): Position[] | null
function ringOfBounds(west, south, east, north): Position[]
function decimate(ring: Position[], maxChars: number): { ring: Position[]; simplified: boolean }
const MAX_COORDINATES_CHARS = 1800
const DEFAULT_GRID_MAP_LABELS: GridMapLabels
```
The map is a second view of the same page, never a second query. `config.map` turns it on
(`{ enabled, attr?, position? }`); the attribute is the one it names or the page's first geometry,
and a type with none gets no panel and no action. A click on a shape moves the grid's active cell to
that row (`setActive`), and the active row's shape is drawn chosen. In edit mode the active row's
geometry is edited in `GeoEditor`, reviewed ("Point moved 38 m") and applied by `applyChanges` as a
**GeoProperty**. `areaQuery` turns the drawn rectangle into `georel=within` + `geometry` +
`coordinates` + `geoproperty`.

### Dashboard
`<Dashboard config={parseDashboardConfig(raw).config!} source={endpointSource(useEndpoint()!.slug, transportFor(useClient().config))} />`, `raw` = `src/dashboard.json`: `{ grid: { source: { kind: "endpoint", slug }, type, columns, pageSize? }, filters?: [{ attr, op, value }], colour?: { attr, bands: [{ below?, tone, label }] }, chart?: { x, y, agg }, kpis?: [{ label, agg, attr?, where? }] }`.

### Geometry: view and edit (UI-72, SDK-29)
```ts
function GeoView(props: { value, selectedId?, onSelect?, accent?, basemap?, label? }): JSX.Element
function GeoEditor(props: { value, onChange, allowed?, snapTo?, basemap?, engine?, label? }): JSX.Element
function checkGeometry(raw: unknown, allowed?: GeometryType[]): { geometry: Geometry | null; findings: GeoFinding[] }
function geometryOf(raw: unknown, allowed?: GeometryType[]): { geometry: Geometry | null; findings: GeoFinding[] }
function rowsOf(g: Geometry | null): CoordinateRow[]            // the coordinate table's rows
function withPosition(g: Geometry, at: number[], p: Position): Geometry
function withoutPosition(g: Geometry, at: number[]): Geometry
function rowLabel(at: number[]): string                          // "Point 1.3"
function modesFor(allowed: GeometryType[]): DrawMode[]           // the toolbar
function positionsOf(coordinates: unknown): Position[]
function boundsOf(features: GeoFeature[]): [[number, number], [number, number]] | null
function featuresOf(value): GeoFeature[]
function terraDrawEngine(): Promise<DrawEngine>                  // the default, loaded on map load
const GEOMETRY_TYPES = ["Point","LineString","Polygon","MultiPoint","MultiLineString","MultiPolygon"]
const MAX_VERTICES = 10000
const DRAW_MODES: DrawMode[]
const DEFAULT_GEO_LABELS: GeoLabels   // every visible string, overridden per key via `labels`
const MAX_GEOJSON_BYTES = 1000000     // a paste or an upload larger than this is refused unparsed
```
`GeoView` draws a geometry, a Feature or a list of them on the kit's base map and fits to it once;
`selectedId` thickens one shape and `onSelect` reports a click. `GeoEditor` is the same map with a
toolbar (only the modes `allowed` permits, plus select, undo, redo and delete) **and the same
geometry as a table of longitude/latitude inputs**, which is the keyboard path: every action of the
map is possible from the table. A paste or an uploaded `.geojson` goes through `geometryOf`, which
also takes a `Feature` or a `FeatureCollection` (one type becomes the `Multi-` form where `allowed`
has it). Nothing reaches `onChange` unchecked: `checkGeometry` closes an open ring, normalises the
winding, keeps altitudes, and refuses a bow tie, a hole outside its shell, a pair out of ±180/±90
(saying so when the two look swapped) and more than `MAX_VERTICES` points — the findings appear on
screen instead. Output is RFC 7946, which is what an NGSI-LD GeoProperty's `value` holds.

### Filters Logic
```ts
function filterRows<T extends Row>(rows: T[], filters: FilterDef[], values: FilterValue[]): T[]
```
Filters an in-memory row array against an active set of filter definitions and values.
```ts
function useFilters<T extends Row>(rows: T[], filters: FilterDef[]): { shown: T[]; values: FilterValue[]; set: (index: number, value: FilterValue) => void; bind: (index: number) => FilterBinding; reset: () => void }
```
Manages filter state, returning filtered rows and two-way control bindings.

### Helpers
```ts
function aggregate(rows: Row[], agg: Agg, attr?: string): number | null
```
Calculates count, sum, avg, min, or max over numeric attributes.
```ts
function columnKind(rows: Row[], attr: string): Column
```
Infers whether an attribute contains numbers, dates, geometry objects, or text.
```ts
function displayName(row: Row): string
```
What a row is called on screen: `name`, else `title`, else the local id at the end of its URN. Use it wherever a row is named; never show `row.id`.
```ts
function distinct(rows: Row[], attr: string): string[]
```
Returns a sorted list of unique non-empty string values for an attribute.
```ts
function extent(rows: Row[], attr: string): [number, number] | null
```
Returns the minimum and maximum numeric values found in an attribute.
```ts
function format(value: Cell, kind?: Column): string
```
Formats a cell value for human-readable display; a missing or non-numeric number is `—`, never `NaN`.
```ts
function pointOf(value: Cell): [number, number] | null
```
Extracts `[longitude, latitude]` coordinates from a GeoProperty.
```ts
function groupBy(rows: Row[], attr: string, agg?: Agg, valueAttr?: string, top?: number): Group[]
```
Groups rows by attribute key and computes an aggregated value for each group.

### Units
A Property's `unitCode` is a UN/CEFACT Recommendation 20 code (`GQ`, `CEL`); the SDK carries the platform's code list.
```ts
function formatValue(value: unknown, unitCode?: string, locale?: string): string
```
A value as people read it: `formatValue(23.4, "GQ", "sk")` is `23,4 µg/m³`. Use it wherever a measured value is shown; never print the code.
```ts
function unitSymbol(code?: string): string
function unitTitle(code?: string): string
```
The symbol (`µg/m³`, empty for a count) and a hover text (`microgram per cubic metre (GQ)`).
```ts
const UNITS: readonly Unit[]
function unitOf(code?: string): Unit | undefined
function unitLabel(unit: Unit): string
function convertible(from: Unit, to: Unit): boolean
function conversion(from: Unit, to: Unit): { factor: number; offset: number } | undefined
```
`Unit` is `{code, name, symbol, deprecated, ucum, qudt, quantityKinds, dimension, factor, offset, frequent}`. `conversion` gives `to = from × factor + offset`, or `undefined` for units of different quantities.
```ts
function toFeatureCollection(rows: Row[], location?: string, properties?: string[]): { type: "FeatureCollection"; features: Array<{ type: "Feature"; id: string; geometry: Geo; properties: Record<string, string | number | boolean | null> }> }
```
Converts entity rows with geometry into a GeoJSON FeatureCollection.

### Tokens
```ts
const DEFAULT_TOKENS: DesignTokens
```
Standard design token values for palette, fonts, spacing, radius, and maps.
```ts
function resolveTokens(input: unknown): { tokens: DesignTokens; warnings: string[] }
```
Validates and merges user design tokens against defaults.
```ts
function applyTokens(input: unknown, root?: HTMLElement): DesignTokens
```
Applies design tokens as CSS variables to the document or target element.
```ts
function currentTokens(): DesignTokens
```
Returns the active design tokens.
```ts
function echartsTheme(tokens?: DesignTokens): Record<string, unknown>
```
Generates an Apache ECharts theme configuration from design tokens.
```ts
function rechartsPalette(tokens?: DesignTokens): string[]
```
Returns the chart color palette array for Recharts.
```ts
function mapColors(tokens?: DesignTokens): DesignTokens["map"]
```
Returns map styling colors for points, highlights, ramps, and strokes.

### Start
```ts
function startApp(App: ComponentType, options?: { tokens?: unknown; root?: HTMLElement; doc?: Document }): { client: Client; tokens: DesignTokens }
```
Bootstraps a generated application by reading configuration, applying tokens, and mounting React.
```ts
function reportError(error: unknown): void
```
Logs an error and, in the preview frame, posts it to the Portal with file and line so the run can fix it.

### Types
- `Row`: Entity object `{ id: string, type: string } & Record<string, Cell>`.
- `Cell`: Scalar value `string | number | boolean | Geo | null`.
- `Geo`: Geometry object `{ type: string, coordinates: unknown }`.
- `LanguageMap`: `{ languageMap: Record<string, string> }`, one LanguageProperty with every language.
- `RelationshipObject`: `{ object: string | string[] }`, one Relationship with its target or targets.
- `WriteValue`: what one attribute of `entities.create`/`update` takes: a `Cell`, a `LanguageMap` or a `RelationshipObject`.
- `Column`: Kind `"number" | "date" | "geo" | "text"`.
- `Agg`: Aggregation `"count" | "sum" | "avg" | "min" | "max"`.
- `Group`: Aggregated group `{ key: string, value: number }`.
- `Page`: Navigation tab `{ id: string, label: string, render: () => ReactNode }`.
- `StatTile`: Summary metric `{ label: string, agg?: Agg, attr?: string, unit?: string, digits?: number }`.
- `ColumnDef<T>`: Table column `{ attr: string, label?: string, render?: (row: T) => ReactNode }`.
- `ExportFormat`: File format `"csv" | "geojson" | "pdf" | "png"`.
- `ChartSpec`: Metric spec `{ x: string, y?: string, agg?: Agg, top?: number }`.
- `Bucket`: Time bucket `"hour" | "day" | "week" | "month"`.
- `MapMode`: Map mode `"auto" | "points" | "hexbin" | "grid"`.

---

## Entry Point: `@joinedcontext/sdk/server`

```ts
interface FnRequest { method: "GET" | "POST"; query: Record<string, string>; body: any; user: JcUser | null }
```
Incoming HTTP request payload passed to backend server functions.
```ts
interface FnResponse { status?: number; body?: unknown }
```
Response status and body returned by backend server functions.
```ts
interface FnContext { jc: DataClient; log(...parts: unknown[]): void }
```
Context providing data access and logging to server functions.
```ts
type FnHandler = (request: FnRequest, ctx: FnContext) => Promise<FnResponse>
```
Standard handler signature implemented by backend function files.
```ts
function requireRole(request: Pick<FnRequest, "user">, role: string): FnResponse | undefined
```
The `403` (or `401` without a signed-in person) a function answers when its caller does not hold the App's `role`, `undefined` when they do. `request.user` is the person the Portal verified for this call, so a patched page changes nothing: `const refused = requireRole(request, "steward"); if (refused) return refused;`.
Backend functions run in QuickJS without browser Web APIs: no `fetch`, `URLSearchParams`, `URL`, `crypto`, DOM or timers. Read data through `ctx.jc` and build strings by hand.

---

## Entry Point: `@joinedcontext/sdk/testing`

```ts
function stubTransport(fixture?: Fixture): StubTransport
```
Creates an in-memory transport holding entity fixtures and recording request calls. The fixture
carries `entities`, `schema`, `access`, `functions`, `temporal` (the temporal read's answer as the
broker's `temporalValues` bodies, filtered by the asked type), `refuse` (a response to send
instead, per request) and `services` (an answer per request for `files`, `email`, `jobs` and `ai`,
by service; a service without one answers `404`). `features` reads the same entities
(`location` the geometry; `bbox`, `properties`, `limit` apply); a CQL2 `filter` is `501`, use `refuse`.
```ts
function stubClient(fixture?: Fixture, config?: Partial<JcConfig>): Client & { transport: StubTransport }
```
Constructs an SDK Client wired to stub transport for unit tests.
```ts
function fakeContext(fixture?: Fixture & { user?: JcUser | null }): { jc: DataClient; log(...parts: unknown[]): void; logs: unknown[][] }
```
Constructs a server function execution context capturing log statements.
```ts
function recordControls(afterAll: (done: () => Promise<void>) => void, dir?: string): void
function controlId(element: Element): string
```
Call `recordControls(afterAll)` in the test setup; the gate fails controls no test uses.

---

## Several endpoints (SDK-02)
An application may read up to five endpoints (e.g. `transportation` and `transportation-kpis`); the pack's THE ENDPOINTS section lists them with their types. A type served by one endpoint is read there with no extra argument. Only a type served by more than one needs `{ endpoint: "<name>" }`: in `Query` (`useEntities("Vehicle", { endpoint: "transportation" })`), in `TemporalQuery`, or as the last argument of `entities.get`/`create`/`update`/`remove`; without it the call throws a `ProblemError` naming the candidates. Writes go to the endpoint of the entity's type (an id's space picks among several). `schema()` merges every endpoint's schemas; `schema(name)` and `access(name)` read one.

## Rows (SDK-03)
Entities read through the SDK are flattened into `Row` objects. `id` (URN) and `type` are always present. KeyValues attributes are simplified into `Cell` values: `Property` becomes primitive value, `GeoProperty` becomes a GeoJSON object, and `LanguageProperty` is resolved to the preferred language string. A write replaces a LanguageProperty whole, so an edit reads every language with `entities.languages(id, attr)` and writes them all back as `{ languageMap }` in `create`/`update`; writing the row's string would drop the others.

## Errors (SDK-04)
Refusals from endpoints and policy checks arrive as `ProblemError` instances containing HTTP `status`, `title`, and `detail`. The `useSave` hook exposes `problem` directly without throwing exceptions, allowing `EntityForm` and `<Problem error={problem} />` to render error details inline without reloading the page.

## Access
Permissions are evaluated using `useAccess().can(operation, type, attr)`. Recognized operation names include:
- `queryEntity`: Query entities of a type.
- `retrieveEntity`: Fetch a single entity by ID.
- `queryTemporal`: Query temporal history.
- `createEntity`: Create a new entity instance.
- `updateAttrs`: Update selected attributes of an entity.
- `updateEntity`: Replace an existing entity.
- `deleteEntity`: Delete an entity.

---

## Example: Complete Page

```tsx
import { useState } from "react";
import { useAccess, useEntities, useFilters } from "@joinedcontext/sdk";
import { BarChartCard } from "../components/charts";
import { EntityDetail } from "../components/EntityDetail";
import { EntityForm } from "../components/EntityForm";
import { EntityMap } from "../components/EntityMap";
import { TypeGrid } from "../components/TypeGrid";
import { FilterBar, SearchBox, SelectFilter } from "../components/filters";
import { StatTiles } from "../components/StatTiles";

export function StationsPage() {
  const { rows, loading, error } = useEntities("BikeHireDockingStation");
  const { shown, bind, reset } = useFilters(rows, [
    { kind: "search", attrs: ["name"] },
    { kind: "select", attr: "status" },
  ]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [editing, setEditing] = useState(false);
  const access = useAccess();
  const selectedRow = shown.find((r) => r.id === selectedId) ?? null;

  return (
    <div>
      <FilterBar shown={shown.length} total={rows.length} onReset={reset}>
        <SearchBox binding={bind(0)} />
        <SelectFilter binding={bind(1)} />
      </FilterBar>
      <StatTiles
        rows={shown}
        loading={loading}
        tiles={[
          { label: "Stations", agg: "count" },
          { label: "Available Bikes", agg: "sum", attr: "availableBikeNumber" },
        ]}
      />
      <BarChartCard rows={shown} x="name" y="availableBikeNumber" top={10} title="Top Stations" />
      <EntityMap rows={shown} label="name" selected={selectedId} onSelect={(r) => setSelectedId(r.id)} />
      <TypeGrid type="BikeHireDockingStation" columns={[{ attr: "name" }, { attr: "occupancy", formula: "availableBikeNumber / totalSlotNumber * 100" }]} onSelect={setSelectedId} />
      <EntityDetail row={selectedRow} onClose={() => setSelectedId(null)} />
      {access.can("updateAttrs", "BikeHireDockingStation").ok && selectedRow && (
        <button type="button" onClick={() => setEditing(true)}>Edit</button>
      )}
      {editing && selectedRow && (
        <EntityForm
          type="BikeHireDockingStation"
          row={selectedRow}
          onSaved={() => setEditing(false)}
          onCancel={() => setEditing(false)}
        />
      )}
    </div>
  );
}
```

---

## Example: Backend Function & Test

```ts
// functions/rebalance.ts: the default export is the handler (SDK-21)
import type { FnHandler } from "@joinedcontext/sdk/server";

const rebalance: FnHandler = async (req, ctx) => {
  if (req.method !== "POST") return { status: 405, body: { title: "POST only" } };
  const { stationId, bikes } = req.body ?? {};
  if (typeof stationId !== "string" || typeof bikes !== "number") return { status: 400, body: { title: "stationId and bikes are required" } };
  ctx.log("Rebalancing station", stationId, bikes);
  await ctx.jc.entities.update(stationId, { availableBikeNumber: bikes });
  return { status: 200, body: { ok: true, stationId } };
};
export default rebalance;
```

```ts
// functions/rebalance.test.ts
import { describe, expect, it } from "vitest";
import { fakeContext } from "@joinedcontext/sdk/testing";
import rebalance from "./rebalance";

describe("rebalance", () => {
  it("updates the station's bike count and logs it", async () => {
    const ctx = fakeContext({
      entities: [{ id: "urn:ngsi-ld:Station:example.org:demo:1", type: "Station", availableBikeNumber: 2 }],
    });
    const res = await rebalance(
      { method: "POST", query: {}, body: { stationId: "urn:ngsi-ld:Station:example.org:demo:1", bikes: 8 }, user: null },
      ctx,
    );
    expect(res.status).toBe(200);
    expect(ctx.logs[0]).toEqual(["Rebalancing station", "urn:ngsi-ld:Station:example.org:demo:1", 8]);
    const updated = await ctx.jc.entities.get("urn:ngsi-ld:Station:example.org:demo:1");
    expect(updated.availableBikeNumber).toBe(8);
  });
});
```
