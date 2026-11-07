import { useCallback, useEffect, useId, useMemo, useRef, useState } from "react";
import type { ResolvedGridConfig, GridColumn } from "./config";
import type { GeoArea } from "./geoarea";
import { areaQuery } from "./geoarea";
import type { EntitySource, GridQuery } from "./source";
import { SourceError } from "./source";
import type { RichRow, RichCell } from "./model";
import { attributesOf, cellText } from "./model";
import { formulasOf, withFormulas } from "./formula";
import { unitSymbol } from "../sdk/units";
import { andQ, opsForKind, queryFromFilters, typeOfFilter } from "./filters";
import type { ColumnFilter, FilterColumn, FilterKind, FilterOp } from "./filters";
import type { AttributeChange, EntityChange } from "./apply";
import type { EnumOption } from "../enums";
import type { RelationEnd } from "../relations";
import { readableName } from "./readableName";
export { readableName };

export type MetaKey = "observedAt" | "unit" | "datasetId" | "createdAt" | "modifiedAt";

export interface GridState {
  offset: number;
  activeCell: { row: number; col: number } | null;
  selected: string[];
  shown: Record<string, MetaKey[]>;
  sort: { attr: string; dir: "asc" | "desc" } | null;
  /** One filter per column key, as the filter row holds them (UI-66). */
  filters: Record<string, ColumnFilter>;
  /** The `q` typed by hand instead of the row; `null` while the row is what asks. */
  filterText: string | null;
  /** Cells a person changed and has not applied: entity id, then attribute (UI-67). */
  edits: Record<string, Record<string, unknown>>;
}

export interface GridLabels {
  id: string;
  type: string;
  /** The entity's scope column (NGSI-LD `scope`), last when a row carries one. */
  scope: string;
  createdAt: string;
  modifiedAt: string;
  observedAt: string;
  unit: string;
  datasetId: string;
  empty: string;
  loading: string;
  /** Said over the dimmed rows while a narrowed question is read again (T-5623). */
  filtering: string;
  previous: string;
  next: string;
  page: string;
  showMetadata: string;
  error: string;
  /** A private dataset read without a session (401), T-6376. */
  signedOutTitle: string;
  signedOutBody: string;
  signIn: string;
  /** A session whose grants do not reach this dataset (403). */
  noAccessTitle: string;
  noAccessBody: string;
  /** The filter row: the operators, by name, and what the query under the grid is called. */
  filter: string;
  ops: Record<FilterOp, string>;
  value: string;
  upperValue: string;
  query: string;
  copyQuery: string;
  editAsText: string;
  filterRow: string;
  /** The unset choice of a column's filter: it lets every row through (T-4222). */
  noFilter: string;
  /** The one-line hint of a column's filter, by what the column holds (T-4949, UI-16). */
  filterHintText: string;
  filterHintNumber: string;
  filterHintDate: string;
  filterHintEnum: string;
  filterHintOther: string;
  /** The placeholder of a filter's value box, an example of what to type (T-6328). */
  filterExampleText: string;
  filterExampleNumber: string;
  filterExampleId: string;
  /** The hint of the switch that hands the query to the person. */
  editAsTextHint: string;
  /** Before the example query the hint builds from the shown type (T-5415), e.g. "e.g.". */
  exampleLead: string;
  /** The query line when nothing narrows the rows. */
  queryAll: string;
  /** Said for a moment after the query was copied. */
  copied: string;
  /** Said on a sortable header, because the order is of the loaded page only (UI-66). */
  sortPage: string;
  /** Said on a sortable header whose source orders the whole answer. */
  sortAll: string;
  /**
   * Under the table when the source refused to order the whole answer (a view that also reads
   * other brokers, CIM 009 4.23.1): `{count}` is the number of loaded rows the order is of (T-5618).
   */
  sortedLoaded: string;
  /**
   * Under the toolbar when the person may read only some of the type's attributes (T-6210): those
   * are never rendered, the line says they exist. `{count}` is how many are not shown.
   */
  notReadable: string;
  /** Before the problem's title when the broker refused the grid's query (a 400). */
  queryRefused: string;
  /** The disclosure holding the broker's own sentence about a refused query. */
  errorDetail: string;
  /** What the footer calls the endpoint's own count of matching entities: a word after the number, or a
   *  text holding `{n}` where the number goes, for a language whose word agrees with it (sk "Vyhovuje: {n}"). */
  matching: string;
  /** Beside the loaded rows when the endpoint's answer carries no total (R22, T-4885). */
  noTotal: string;
  /** The entry that opens one attribute's history, and the panel's own labels (T-1431). */
  history: string;
  /** A geo column's filter cell: draw an area on the map, and take it away again. */
  areaPick: string;
  /** The pager's page-size picker: "Rows per page". */
  rowsPerPage: string;
  areaClear: string;
  historyLabels?: Partial<import("./EntityHistory").HistoryLabels>;
  /** The cell inspector's words (T-5528): its title also names each cell's "i" button. */
  inspectorLabels?: Partial<import("./CellInspector").CellInspectorLabels>;
  /** The row detail's Metadata tab: created, changed, by whom and through what (T-5529). */
  metadata: string;
  metadataLabels?: Partial<import("./AttributeMetadata").MetadataLabels>;
  /** The row detail's Changes tab: who changed what and when, from the kept versions (T-6333). */
  changesLabels?: Partial<import("./EntityChanges").ChangesLabels>;
  /** Edit mode: the cell editor, the pending list and what applying them is called (UI-67). */
  edit: string;
  pending: string;
  review: string;
  apply: string;
  discard: string;
  /** The review's column headers: which row, which attribute, its stored value and the one about to be sent. */
  entity: string;
  /** The button beside a reviewed row's name that copies its identifier (T-5530). */
  copyId: string;
  attribute: string;
  before: string;
  after: string;
  observedKeep: string;
  observedNow: string;
  applying: string;
  /**
   * Why the person writes (DM-113): the review's field, the text it starts with, what is said
   * while it is empty or too long, what one apply says once it landed, its Undo, and the reason
   * an Undo writes with, before the original one.
   */
  reason: string;
  reasonDefault: string;
  reasonNeeded: string;
  saved: string;
  undo: string;
  undoReason: string;
  undone: string;
  refusedHere: string;
  /** Said beside a stored value the attribute's enum does not list (UI-86). */
  notInList: string;
  /** A relationship cell's picker: "Search School", nothing found, remove a target (UI-84). */
  relationSearch: string;
  relationNone: string;
  relationRemove: string;
  relationFailed: string;
  /** Said after a computed end's list when the page's read reached its limit. */
  relationMore: string;
  /** The row's detail panel (T-3097): the identifier opens it, the panel's name, and closing it. */
  openRow: string;
  /** A group header's value where the rows have none (T-5188). */
  groupNoValue: string;
  rowDetail: string;
  /** The detail's first tab, with the row's values (T-4828). */
  rowFields: string;
  /** The detail's button that copies the row's identifier, kept off the screen as text (T-5599). */
  rowCopyId: string;
  /** What the detail shows for an attribute with no value (T-5599). */
  rowNotSet: string;
  close: string;
  /** What a paste of a range did (T-3097): cells it set, cells it left, and a range too large. */
  pasted: string;
  skipped: string;
  /** Pasted cells the person's access does not let them change (T-5530). */
  notAllowed: string;
  /** A read-only column's lock and its reasons beyond the inspector's own (T-5530). */
  locked: string;
  lockReadOnly: string;
  lockNotOffered: string;
  pasteTooLarge: string;
  /** A scrolling grid's footer (T-3097): "120 of 10 400 loaded". */
  loaded: string;
  of: string;
  /** What a typed value breaks of its model's rule (T-3097), and the count to correct first. */
  mustBeInteger: string;
  mustBeNumber: string;
  mustBeBoolean: string;
  mustBeDate: string;
  mustBeUri: string;
  atLeast: string;
  atMost: string;
  patternMismatch: string;
  required: string;
  toCorrect: string;
  /** A true/false attribute's two choices in its picker. */
  yes: string;
  no: string;
  /** The column header menu (T-4827). */
  columnMenu: string;
  sortAsc: string;
  sortDesc: string;
  filterColumn: string;
  hideColumn: string;
  /** The control that lists the fields hidden from the header menu (T-5620), with its count after it. */
  hiddenFields: string;
  showColumn: string;
  showAllColumns: string;
  renameColumn: string;
  renameReset: string;
  columnWidth: string;
  narrower: string;
  wider: string;
  moveLeft: string;
  moveRight: string;
  details: string;
  /** A column's header filter (T-6328): its values to tick, a condition, and the chips above the grid. */
  filterValues: string;
  searchValues: string;
  searchValuesHint: string;
  selectAll: string;
  /** The button beside a listed value that filters to it alone. */
  onlyValue: string;
  valuesLoading: string;
  valuesFailed: string;
  noValues: string;
  /** `{count}` entities read of `{total}`. */
  valuesCapped: string;
  filterCondition: string;
  applyFilter: string;
  clearFilter: string;
  /** The header's filter button, with the column's label after it. */
  openFilter: string;
  /** Said beside a header whose column is filtered. */
  filtered: string;
  activeFilters: string;
  /** `{name}`: the chip's text. */
  removeFilter: string;
  clearFilters: string;
  /** What a header's icon says the column holds. */
  kindText: string;
  kindNumber: string;
  kindBoolean: string;
  kindDatetime: string;
  kindGeo: string;
  kindRelationship: string;
  kindLanguage: string;
  kindStructured: string;
  kindList: string;
  /** After a geometry's vertex count: `Polygon, 12 points`. */
  points: string;
  /** Before a relationship chip's target: "Open: Organization · unit-12". */
  openTarget: string;
  /** The hover of "+N" on an attribute with several instances (datasetId). */
  moreInstances: string;
  /** Beside a value whose kind is not the model's: "The model expects: Number". */
  wrongKind: string;
  /** The name of the breadcrumb of opened relationship targets, for a screen reader. */
  drillTrail: string;
  /** The breadcrumb's step back to the rows a target was opened from. */
  drillBack: string;
  /** The row detail's name for the instance without a datasetId. */
  defaultInstance: string;
}

/**
 * The strings a host supplies: any of them, operators included, so a host written before an
 * operator existed still compiles and shows the default for the one it does not name.
 */
export type GridLabelsInput = Partial<Omit<GridLabels, "ops">> & { ops?: Partial<GridLabels["ops"]> };

/**
 * The keys that put the first cell active when none is, rather than the one beyond it; Home, End
 * and the page keys go where they say from there.
 */
const MOVE_KEYS = new Set(["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight"]);

export const DEFAULT_LABELS: GridLabels = {
  id: "ID",
  type: "Type",
  scope: "Scope",
  createdAt: "Created",
  modifiedAt: "Modified",
  observedAt: "Observed",
  unit: "Unit",
  datasetId: "Dataset",
  empty: "No rows.",
  loading: "Loading…",
  filtering: "Filtering…",
  previous: "Previous",
  next: "Next",
  page: "Page",
  showMetadata: "Show metadata for",
  error: "Error",
  signedOutTitle: "Sign in to see this dataset",
  signedOutBody: "This dataset is not public. Sign in with an account that has access to it.",
  signIn: "Sign in",
  noAccessTitle: "You have no access to this dataset",
  noAccessBody: "Your account has no grant that covers this dataset. Ask its publisher for access.",
  filter: "Filter",
  ops: {
    contains: "contains",
    equals: "is",
    notEquals: "is not",
    gt: "is greater than",
    gte: "is at least",
    lt: "is less than",
    lte: "is at most",
    between: "between",
    empty: "is empty",
    present: "has a value",
    pattern: "contains",
    startsWith: "starts with",
    anyOf: "is one of",
  },
  value: "Value",
  upperValue: "Upper value",
  query: "The query this asks",
  copyQuery: "Copy the query",
  editAsText: "Edit as text",
  filterRow: "Filters",
  noFilter: "Any",
  filterHintText: "Choose a test, then type the words, e.g. contains Espoo.",
  filterExampleText: "Bratislava",
  filterExampleNumber: "1500",
  filterExampleId: "station-12",
  filterHintNumber: "Choose a test, then type a number, e.g. greater than 40.",
  filterHintDate: "Choose a test, then pick a date, e.g. after 2026-10-01.",
  filterHintEnum: "Choose a test; with any of, pick one or more allowed values:",
  filterHintOther: "Choose a test, e.g. has a value, or empty.",
  editAsTextHint: "Write the NGSI-LD query yourself",
  exampleLead: "e.g.",
  queryAll: "all records",
  copied: "Copied",
  sortPage: "Sort this page by",
  sortAll: "Sort all rows by",
  sortedLoaded: "Sorted within the {count} loaded rows: this view also reads other brokers, which do not sort.",
  notReadable: "{count} more fields are not shown: your access does not include them.",
  queryRefused: "The broker refused this query",
  errorDetail: "What the broker said",
  matching: "matching",
  noTotal: "the endpoint gives no total for this answer",
  history: "History",
  areaPick: "Pick area on map",
  rowsPerPage: "Rows per page",
  areaClear: "Clear area",
  metadata: "Metadata",
  edit: "Edit",
  pending: "not applied yet",
  review: "Review the changes",
  apply: "Apply",
  discard: "Discard the changes",
  entity: "Entity",
  copyId: "Copy the identifier",
  attribute: "Attribute",
  before: "Before",
  after: "After",
  observedKeep: "keep when each value was observed",
  observedNow: "these values were observed now",
  applying: "Applying…",
  reason: "Why you change these values",
  reasonDefault: "Corrected in the data grid",
  reasonNeeded: "Say why you change these values.",
  saved: "Values saved:",
  undo: "Undo",
  undoReason: "Undo",
  undone: "The change was undone.",
  refusedHere: "refused",
  notInList: "not in the list",
  relationSearch: "Search",
  relationNone: "Nothing you can read matches",
  relationRemove: "Remove",
  relationFailed: "The search failed",
  relationMore: "and more",
  openRow: "Open",
  groupNoValue: "(no value)",
  rowDetail: "Details",
  rowFields: "Fields",
  rowCopyId: "Copy identifier",
  rowNotSet: "not set",
  close: "Close",
  pasted: "cells pasted",
  skipped: "skipped (not editable, off the page or not a listed value)",
  notAllowed: "not allowed by your access",
  locked: "Read-only",
  lockReadOnly: "Your access lets you read it, not change it.",
  lockNotOffered: "This table does not offer it for editing.",
  pasteTooLarge: "The copied range is too large to paste at once; paste a smaller range, or import the rows from a file.",
  loaded: "loaded",
  of: "of",
  mustBeInteger: "must be a whole number",
  mustBeNumber: "must be a number",
  mustBeBoolean: "must be true or false",
  mustBeDate: "must be a date",
  mustBeUri: "must be a web address",
  atLeast: "must be at least",
  atMost: "must be at most",
  patternMismatch: "does not have the form this attribute requires",
  required: "is required",
  toCorrect: "to correct before applying",
  yes: "Yes",
  no: "No",
  columnMenu: "Column options",
  sortAsc: "Sort A → Z",
  sortDesc: "Sort Z → A",
  filterColumn: "Filter…",
  hideColumn: "Hide column",
  hiddenFields: "Hidden fields",
  showColumn: "Show field",
  showAllColumns: "Show all",
  renameColumn: "Label in this view",
  renameReset: "Use the model's label",
  columnWidth: "Width",
  narrower: "Narrower",
  wider: "Wider",
  moveLeft: "Move left",
  moveRight: "Move right",
  details: "Column settings",
  filterValues: "Values",
  searchValues: "Search values",
  searchValuesHint: "Type part of a value, then tick the ones to keep.",
  selectAll: "Select all",
  onlyValue: "Only",
  valuesLoading: "Reading the column's values…",
  valuesFailed: "The values could not be read: choose a condition below, or try again later.",
  noValues: "No values to list.",
  valuesCapped: "Values of the first {count} of {total} entities; a condition asks all of them.",
  filterCondition: "Condition",
  applyFilter: "Apply",
  clearFilter: "Clear filter",
  openFilter: "Filter column",
  filtered: "filtered",
  activeFilters: "Active filters",
  removeFilter: "Remove filter {name}",
  clearFilters: "Clear all filters",
  kindText: "Text",
  kindNumber: "Number",
  kindBoolean: "Yes or no",
  kindDatetime: "Date and time",
  kindGeo: "Geographic location",
  kindRelationship: "Link to an entity",
  kindLanguage: "Text in several languages",
  kindStructured: "Structured value",
  kindList: "List",
  points: "points",
  openTarget: "Open",
  moreInstances: "{count} more instances",
  wrongKind: "The model expects: {kind}",
  drillTrail: "Opened links",
  drillBack: "Back",
  defaultInstance: "default",
};

export interface VisibleColumn {
  key: string;
  attr: string | null;
  meta: MetaKey | null;
  label: string;
  pinned: boolean;
  /** Pixels, as the config or the person's layout sets it; the table's own width without one. */
  width?: number;
}

/**
 * How a person arranged the columns (T-4827): the order of attributes, a width per attribute and
 * a label of this view's own. It changes what the grid shows, never the data or the model.
 */
export interface ColumnLayout {
  order?: readonly string[];
  width?: Readonly<Record<string, number>>;
  labels?: Readonly<Record<string, string>>;
}

/** Narrowest and widest a column is drawn, as the view store accepts them. */
export const MIN_COLUMN_WIDTH = 40;
export const MAX_COLUMN_WIDTH = 2000;


export interface UseEntityGridOptions {
  config: ResolvedGridConfig;
  source: EntitySource;
  labels?: GridLabelsInput;
  state?: Partial<GridState>;
  onStateChange?: (next: GridState) => void;
  query?: Omit<GridQuery, "type">;
  /**
   * The permissible values of each enum attribute, by attribute name, as the type's schema names
   * them (`enumOptions`): such a column is edited and filtered by picking (UI-86).
   */
  enums?: Record<string, EnumOption[]>;
  /**
   * The relationship ends of the type, by attribute name, as the model states them: a stored end
   * is edited by picking entities of its target, a computed end (`inverseOf`) is a read-only list
   * of the entities pointing back (UI-84, DM-67).
   */
  relations?: Record<string, RelationEnd>;
  /**
   * Scroll instead of pages (T-3097): the grid starts from the first page and appends the next as
   * the person nears the end, up to `MAX_VIRTUAL_ROWS`; the view draws only the rows in sight.
   */
  virtual?: boolean;
  /** A scrolling grid that still reads one page at a time, with a pager (owner 2026-11-01): no rows are appended past `pageSize`. */
  paged?: boolean;
  /**
   * Attributes left out of the grid, as a saved view hides them (API/01 §30): their columns are not
   * drawn and not filtered. The primary attribute is never hidden; it names the row. The keys
   * `id` and `entity:type` leave those two columns out, for a grid over one type with a pinned name.
   */
  hidden?: readonly string[];
  /** The person's own order, widths and labels of the columns (T-4827). */
  layout?: ColumnLayout;
  /** The keys that order rows the sort leaves equal, at most two more (T-5188, CIM 009 4.23). */
  thenBy?: readonly SortKey[];
  /**
   * The attributes the rows are grouped by, outermost first, at most three (T-5188): the rows are
   * ordered by them before the sort, so each group's rows come together under its header.
   */
  groupBy?: readonly string[];
}

/** One key of an order: the attribute and its direction. */
export interface SortKey {
  attr: string;
  dir: "asc" | "desc";
}

/**
 * The order the grid asks for: the group keys first, ascending unless the sort names the same
 * attribute, then the sort and its further keys, each attribute once.
 */
export function orderKeys(groupBy: readonly string[], sort: SortKey | null, thenBy: readonly SortKey[]): SortKey[] {
  const sorts = [...(sort ? [sort] : []), ...thenBy];
  const keys = [...groupBy.map((attr) => sorts.find((key) => key.attr === attr) ?? { attr, dir: "asc" as const }), ...sorts];
  return keys.filter((key, at) => keys.findIndex((other) => other.attr === key.attr) === at);
}

/** Rows one scrolling grid holds at most; past it the grid says to narrow the filter. */
export const MAX_VIRTUAL_ROWS = 50_000;

/**
 * Rows a scrolling grid asks for after its first page (T-5623): the first page stays the config's
 * size, so the first row shows soon, and the next ones are larger, so a scroll through 2 000 rows
 * takes ten reads rather than forty.
 */
export const VIRTUAL_PAGE_SIZE = 200;

export interface EntityGrid {
  /** In scrolling mode: whether the source holds more rows than are loaded, and asking for them. */
  more: boolean;
  loadingMore: boolean;
  loadMore(): void;
  rows: RichRow[];
  /** The source orders the whole answer, so a header sorts every row and not the page. */
  sortsAll: boolean;
  columns: VisibleColumn[];
  total?: number;
  loading: boolean;
  error: string | null;
  /**
   * The source refused the person, not the question (T-6376): `signedOut` for a 401, `noAccess`
   * for a 403. No row is kept then, so nothing read before the refusal stays on the screen.
   */
  refusal: GridRefusal | null;
  /** The Endpoint's own sentence about a `noAccess` refusal, shown under the state's words. */
  refusalDetail: string | null;
  /** The broker's own sentence about the refusal in `error`, for a disclosure under it. */
  errorDetail: string | null;
  /** A sort is asked for that the source refused to apply to the whole answer: the page is sorted. */
  sortedLoadedOnly: boolean;
  state: GridState;
  labels: GridLabels;
  cellOf(row: RichRow, column: VisibleColumn): { text: string; cell?: RichCell | RichCell[] };
  toggleMeta(attr: string, meta: MetaKey): void;
  /** The filter of one column (or several), or `undefined` to drop it; the answer starts again at page one. */
  setFilter(key: string | readonly string[], filter: ColumnFilter | undefined): void;
  /** Hands the query to the person as text, or gives the row back (`null`). */
  setFilterText(text: string | null): void;
  /** Every column the filter row can ask about, with what it holds. */
  filterColumns: FilterColumn[];
  /** One cell a person changed; `undefined` gives the endpoint's own value back. */
  setEdit(id: string, attr: string, value: unknown | undefined): void;
  /** Several pending values set or cleared in one step (a paste, an apply that clears what landed). */
  setEdits(changes: readonly { id: string; attr: string; value: unknown | undefined }[]): void;
  /** Forgets every edit that has not been applied. */
  clearEdits(): void;
  /** What applying would send: one entry per entity, with what each cell held before. */
  pendingChanges: EntityChange[];
  /** The query the filters ask for right now, as the endpoint receives it. */
  askedQuery: { q?: string; idPattern?: string };
  /** The grid's question without one column's own filter: what that column's value list reads (T-6328). */
  valuesQuery(key: string): GridQuery;
  /** The area drawn on the map, which the filter row cannot hold (UI-72); `null` for none. */
  area: GeoArea | null;
  /** Whether the source is asked for less than the whole type (a q, an id pattern, a scope, an area). */
  narrowed: boolean;
  /** Puts the active cell on a row and column outright, for a selection from the map (UI-72). */
  setActive(row: number, col: number): void;
  /** Draws the area or removes it; either way the answer starts again at page one. */
  setArea(area: GeoArea | null): void;
  setOffset(offset: number): void;
  setSort(attr: string, dir?: "asc" | "desc"): void;
  moveActive(key: string): boolean;
  getGridProps(): Record<string, unknown>;
  getHeaderProps(column: VisibleColumn, index: number): Record<string, unknown>;
  getRowProps(row: RichRow, index: number): Record<string, unknown>;
  getCellProps(row: RichRow, rowIndex: number, column: VisibleColumn, colIndex: number): Record<string, unknown>;
  reload(): void;
}

/** Why a grid shows no rows because of who is asking, never because of what was asked. */
export type GridRefusal = "signedOut" | "noAccess";

/** The refusal a source error stands for: 401 asks for a session, 403 for a grant. */
export function refusalOf(err: unknown): GridRefusal | null {
  if (!(err instanceof SourceError)) return null;
  if (err.status === 401) return "signedOut";
  if (err.status === 403) return "noAccess";
  return null;
}

function mergeLabels(base: GridLabels, partial?: GridLabelsInput): GridLabels {
  if (!partial) return base;
  // The operator names are a group of their own: a caller that translates two of them keeps the
  // rest, instead of leaving the row with holes where a label used to be.
  return { ...base, ...partial, ops: { ...base.ops, ...partial.ops } };
}

const META_LABEL: Record<MetaKey, keyof GridLabels> = {
  observedAt: "observedAt",
  unit: "unit",
  datasetId: "datasetId",
  createdAt: "createdAt",
  modifiedAt: "modifiedAt",
};

/** The attribute that names an entity: always third, after id and type (T-6210). */
export const NAME_COLUMN = "name";

function buildColumns(
  config: ResolvedGridConfig,
  rows: RichRow[],
  shown: Record<string, MetaKey[]>,
  labels: GridLabels,
  relations: Record<string, RelationEnd> = {},
  hidden: readonly string[] = [],
  layout: ColumnLayout = {},
): VisibleColumn[] {
  const cols: VisibleColumn[] = [];

  // What every entity has comes first and stays (T-6210, owner 2026-10-31): its id, pinned, then its
  // type; the name follows when the type has one. Neither is moved. A grid over one named type can
  // leave them out, by the keys `id` and `entity:type` in `hidden`: its pinned attribute then leads.
  const left = new Set(hidden);
  if (!left.has("id")) cols.push({ key: "id", attr: null, meta: null, label: labels.id, pinned: true });
  if (!left.has("entity:type")) cols.push({ key: "entity:type", attr: null, meta: null, label: labels.type, pinned: false });

  // The model's relationship ends are columns even on a page where no entity holds one yet: an
  // empty end is what a person comes to fill, and a computed end is never in a row at all.
  const listed =
    config.columns.length > 0
      ? config.columns.map((c) => c.attr)
      : [...new Set([...attributesOf(rows), ...Object.keys(relations)])];
  // The field the config pins (T-3097) leads the attributes after the name; then the person's
  // order, then the rest as listed.
  const primary = config.columns.find((c) => c.pinned)?.attr;
  const ordered = [...new Set([...(layout.order ?? []).filter((attr) => listed.includes(attr)), ...listed])];
  const attrList = [
    ...(listed.includes(NAME_COLUMN) ? [NAME_COLUMN] : []),
    ...(primary !== undefined && primary !== NAME_COLUMN ? [primary] : []),
    ...ordered.filter((attr) => attr !== NAME_COLUMN && attr !== primary),
  ];
  const colMap = new Map<string, GridColumn>();
  for (const c of config.columns) {
    colMap.set(c.attr, c);
  }

  for (const attr of attrList) {
    // Hidden covers what the person may not read, so even the name leaves when it is not theirs.
    if (left.has(attr)) continue;
    const gridCol = colMap.get(attr);
    const label = layout.labels?.[attr] ?? gridCol?.label ?? readableName(attr);
    const width = layout.width?.[attr] ?? gridCol?.width;
    const symbol = unitSymbol(unitOfColumn(rows, attr));
    cols.push({ key: attr, attr, meta: null, label: symbol ? `${label} (${symbol})` : label, pinned: left.has("id") && attr === primary, ...(width ? { width } : {}) });

    const metaKeys = shown[attr] ?? [];
    for (const meta of metaKeys) {
      cols.push({ key: `${attr}:${meta}`, attr, meta, label: `${label} · ${labels[META_LABEL[meta]]}`, pinned: false });
    }
  }

  // The entity's own system members close the row: its timestamps, then its scope where one is set.
  if (config.entityTimestamps) {
    cols.push({ key: "entity:createdAt", attr: null, meta: null, label: labels.createdAt, pinned: false });
    cols.push({ key: "entity:modifiedAt", attr: null, meta: null, label: labels.modifiedAt, pinned: false });
  }
  if (rows.some((row) => row.scope !== undefined)) {
    cols.push({ key: "entity:scope", attr: null, meta: null, label: labels.scope, pinned: false });
  }

  return cols;
}

/**
 * Whether the source holds rows past the `loaded` ones: its total says so when it gives one,
 * otherwise a full last page does.
 */
function hasMore(loaded: number, total: number | undefined, lastPage: number, pageSize: number): boolean {
  return total !== undefined ? loaded < total : lastPage === pageSize;
}

/**
 * The one unit a column's values are in, when every value that states a unit states the same
 * (DM-06): the header then carries its symbol, `PM10 (µg/m³)`. Two units in one column name
 * none, and each cell keeps its own.
 */
function unitOfColumn(rows: RichRow[], attr: string): string | undefined {
  const codes = new Set<string>();
  for (const row of rows) {
    const cell = row.cells[attr];
    for (const one of Array.isArray(cell) ? cell : cell ? [cell] : []) {
      if (one.kind === "property" && one.unitCode) codes.add(one.unitCode);
    }
  }
  return codes.size === 1 ? [...codes][0] : undefined;
}

function metaOf(cell: RichCell, meta: MetaKey): string | undefined {
  return meta === "unit" ? cell.unitCode : cell[meta];
}

function cellTextWithUnit(cell: RichCell | RichCell[] | undefined, column: VisibleColumn): string {
  const text = cellText(cell);
  if (column.meta) return text;
  if (!Array.isArray(cell) && cell && cell.kind === "property" && cell.unitCode && !column.meta) {
    // The symbol the code list gives the unit, `µg/m³` for GQ (DM-06); the code stays in the
    // unit column and in the cell's title.
    const symbol = unitSymbol(cell.unitCode);
    return symbol ? `${text} ${symbol}` : text;
  }
  return text;
}

const META_KIND: Record<MetaKey, FilterKind> = {
  observedAt: "date",
  createdAt: "date",
  modifiedAt: "date",
  unit: "text",
  datasetId: "text",
};

const DATE_VALUE = /^\d{4}-\d{2}-\d{2}([T ]|$)/;

/**
 * What a column holds, as far as a filter cares: the configured format when the config names one,
 * else what the loaded rows show. Without a row and without a format a column is text, which is
 * the only guess that cannot refuse a value the endpoint would have accepted.
 */
export function filterKindOf(
  column: VisibleColumn,
  config: ResolvedGridConfig,
  rows: RichRow[],
  enums?: Record<string, EnumOption[]>,
): FilterKind {
  if (column.key === "id") {
    return "id";
  }
  // A Type filter only where the grid reads several types: with one, it would offer one box.
  if (column.key === "entity:type") {
    return config.type.includes(",") ? "type" : "none";
  }
  if (column.attr === null) {
    // The entity's own timestamps are not part of `q`; NGSI-LD filters them with its own
    // temporal parameters, which the grid does not send.
    return "none";
  }
  if (column.meta) {
    return META_KIND[column.meta];
  }
  if (enums?.[column.attr]?.length) {
    return "enum";
  }
  const format = config.columns.find((c) => c.attr === column.attr)?.format;
  if (format === "number") {
    return "number";
  }
  if (format === "date") {
    return "date";
  }
  if (format === "text" || format === "link") {
    return "text";
  }
  for (const row of rows) {
    const cell = row.cells[column.attr];
    const one = Array.isArray(cell) ? cell[0] : cell;
    if (!one) {
      continue;
    }
    if (one.kind === "relationship") {
      return "relationship";
    }
    if (one.kind === "geo") {
      return "geo";
    }
    if (typeof one.value === "number") {
      return "number";
    }
    if (typeof one.value === "boolean") {
      return "boolean";
    }
    if (typeof one.value === "string" && DATE_VALUE.test(one.value)) {
      return "date";
    }
    return "text";
  }
  return "text";
}

/** Whether the rows hold the attribute as a timestamp, a date with a time of day. */
function holdsTime(rows: RichRow[], attr: string): boolean {
  return rows.some((row) => {
    const cell = row.cells[attr];
    const value = (Array.isArray(cell) ? cell[0] : cell)?.value;
    return typeof value === "string" && /^\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}/.test(value);
  });
}

/** Whether the rows hold the attribute as a LanguageProperty, a text per language. */
function holdsLanguage(rows: RichRow[], attr: string): boolean {
  return rows.some((row) => {
    const cell = row.cells[attr];
    return (Array.isArray(cell) ? cell[0] : cell)?.kind === "language";
  });
}

/**
 * Sources that refused `orderBy` (a view that also reads other brokers, CIM 009 4.23.1): asked
 * once, then their pages are sorted in the browser, so a sort never sends a refused read twice and
 * a remembered sort reopens with rows instead of the refusal (T-5618).
 */
const refusesOrder = new WeakSet<EntitySource>();

/** A narrowed read still brings what the computed columns read, or they would stay empty. */
function withReads(attrs: string[] | undefined, formulas: readonly { reads: string[] }[]): string[] | undefined {
  if (attrs === undefined) return undefined;
  const all = [...attrs];
  for (const name of formulas.flatMap((each) => each.reads)) if (!all.includes(name)) all.push(name);
  return all;
}

export function useEntityGrid(options: UseEntityGridOptions): EntityGrid {
  const { config, source, labels: labelsPartial, state: controlledState, onStateChange, query: queryPartial, enums, relations, virtual = false, paged = false, hidden, layout } = options;
  // By content, as `hidden`: a host building new arrays each render does not ask again.
  const thenByKey = JSON.stringify(options.thenBy ?? []);
  const groupByKey = JSON.stringify(options.groupBy ?? []);
  // The list by content, so a host that builds a new array each render does not rebuild columns.
  const hiddenKey = (hidden ?? []).join("\u0000");
  const labels = useMemo(() => mergeLabels(DEFAULT_LABELS, labelsPartial), [labelsPartial]);

  // Internal state (uncontrolled)
  const [internalOffset, setInternalOffset] = useState(0);
  const [internalActiveCell, setInternalActiveCell] = useState<{ row: number; col: number } | null>(null);
  // The drawn area is the map's own filter and lives beside the row's: a `GridState` a host stores
  // holds the typed filters, and an area is a shape a person drew in this session.
  const [area, setAreaState] = useState<GeoArea | null>(null);
  const [internalSelected] = useState<string[]>([]);
  const [internalShown, setInternalShown] = useState<Record<string, MetaKey[]>>(() => {
    const init: Record<string, MetaKey[]> = {};
    for (const col of config.columns) {
      if (col.show) {
        const keys: MetaKey[] = [];
        if (col.show.observedAt) keys.push("observedAt");
        if (col.show.unit) keys.push("unit");
        if (col.show.datasetId) keys.push("datasetId");
        if (col.show.createdAt) keys.push("createdAt");
        if (col.show.modifiedAt) keys.push("modifiedAt");
        if (keys.length > 0) init[col.attr] = keys;
      }
    }
    return init;
  });
  const [internalSort, setInternalSort] = useState<{ attr: string; dir: "asc" | "desc" } | null>(null);
  const [internalFilters, setInternalFilters] = useState<Record<string, ColumnFilter>>({});
  const [internalFilterText, setInternalFilterText] = useState<string | null>(null);
  const [internalEdits, setInternalEdits] = useState<Record<string, Record<string, unknown>>>({});

  // Controlled state: use controlled values when key present
  const offset = controlledState?.offset !== undefined ? controlledState.offset : internalOffset;
  const activeCell = controlledState?.activeCell !== undefined ? controlledState.activeCell : internalActiveCell;
  const selected = controlledState?.selected !== undefined ? controlledState.selected : internalSelected;
  const shown = controlledState?.shown !== undefined ? controlledState.shown : internalShown;
  const sort = controlledState?.sort !== undefined ? controlledState.sort : internalSort;
  const filters = controlledState?.filters !== undefined ? controlledState.filters : internalFilters;
  const filterText =
    controlledState?.filterText !== undefined ? controlledState.filterText : internalFilterText;
  const edits = controlledState?.edits !== undefined ? controlledState.edits : internalEdits;

  const [loadedRows, setRows] = useState<RichRow[]>([]);
  // A computed column (T-6384) is a cell of every loaded row, worked out here from the row's own
  // attributes: it sorts and shows as any other, and is never asked of or written to the endpoint.
  const formulas = useMemo(() => formulasOf(config.columns), [config.columns]);
  const rows = useMemo(() => withFormulas(loadedRows, formulas), [loadedRows, formulas]);
  const [total, setTotal] = useState<number | undefined>(undefined);
  const [more, setMore] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [errorDetail, setErrorDetail] = useState<string | null>(null);
  const [refusal, setRefusal] = useState<GridRefusal | null>(null);
  const [refusalDetail, setRefusalDetail] = useState<string | null>(null);
  // Bumped when the source refuses an order, so the question is asked again without it.
  const [, setOrderRefusals] = useState(0);
  const cancelledRef = useRef(false);
  const nonceRef = useRef(0);

  // The rows the columns and the filters' kinds are read off: the last answer that held any, for
  // this type. An empty answer named no attribute, so a filter that matched nothing lost its column,
  // the grid asked again without it, got rows, found the column and asked with it, and on, every
  // half second, never showing the empty state (T-5623).
  const shapeRef = useRef<{ type: string; rows: RichRow[] }>({ type: config.type, rows: [] });
  const shapeRows = useMemo(() => {
    if (rows.length > 0 || shapeRef.current.type !== config.type) shapeRef.current = { type: config.type, rows };
    return shapeRef.current.rows;
  }, [rows, config.type]);
  const columns = useMemo(
    () => buildColumns(config, shapeRows, shown, labels, relations, hiddenKey === "" ? [] : hiddenKey.split("\u0000"), layout),
    [config, shapeRows, shown, labels, relations, hiddenKey, layout],
  );
  // A source that orders the whole answer is asked to (`orderBy`, CIM 009 4.23), so the first page
  // holds the first rows of the set; elsewhere the order is of the loaded page only (UI-66).
  // The entity's own timestamps are no attribute `orderBy` names here: they sort the page.
  const keys = useMemo(
    () => orderKeys(JSON.parse(groupByKey) as string[], sort, JSON.parse(thenByKey) as SortKey[]),
    [groupByKey, sort, thenByKey],
  );
  const ordersAll = source.orders === true && !refusesOrder.has(source);
  // The entity's timestamps and a computed column are no stored attribute `orderBy` could name.
  const unstored = (attr: string) => attr.startsWith("entity:") || formulas.some((each) => each.attr === attr);
  const sortedLoadedOnly = source.orders === true && !ordersAll && keys.length > 0 && !keys.some((key) => unstored(key.attr));
  const serverOrder = ordersAll && keys.length > 0 && !keys.some((key) => unstored(key.attr))
    ? keys.map((key) => (key.dir === "desc" ? `${key.attr};desc` : key.attr)).join(",")
    : undefined;
  const sortedRows = useMemo(() => {
    if (keys.length === 0 || serverOrder) return rows;
    const compare = (key: SortKey, a: RichRow, b: RichRow) => {
      const x = cellText(a.cells[key.attr]);
      const y = cellText(b.cells[key.attr]);
      const nx = Number(x);
      const ny = Number(y);
      const byNumber = x !== "" && y !== "" && !Number.isNaN(nx) && !Number.isNaN(ny);
      return (key.dir === "asc" ? 1 : -1) * (byNumber ? nx - ny : x.localeCompare(y));
    };
    return [...rows].sort((a, b) => {
      for (const key of keys) {
        const by = compare(key, a, b);
        if (by !== 0) return by;
      }
      return 0;
    });
  }, [rows, keys, serverOrder]);

  // A search or filter that matches nothing leaves no row to read a column's kind from: the kinds
  // stay those of the last page that had rows, so a number never turns into text (T-5600).
  const filterColumns = useMemo(
    (): FilterColumn[] =>
      columns
        .map((column) => ({
          key: column.key,
          attr: column.attr,
          meta: column.meta,
          kind: filterKindOf(column, config, shapeRows, enums),
          options:
            column.key === "entity:type"
              ? config.type.split(",").map((each) => ({ value: each.trim() }))
              : column.attr !== null && column.meta === null ? enums?.[column.attr] : undefined,
          ...(column.attr !== null && column.meta === null && holdsTime(shapeRows, column.attr) ? { datetime: true } : {}),
          unit: column.attr !== null && column.meta === null ? unitSymbol(unitOfColumn(shapeRows, column.attr)) : undefined,
          ...(column.attr !== null && column.meta === null && holdsLanguage(shapeRows, column.attr) ? { language: true } : {}),
        }))
        // A computed end is not stored, so no `q` can ask about it.
        .filter(
          (column) =>
            opsForKind(column.kind).length > 0 &&
            !(column.attr !== null && relations?.[column.attr]?.inverseOf) &&
            !formulas.some((each) => each.attr === column.attr),
        ),
    [columns, config, shapeRows, enums, relations, formulas],
  );

  // What the endpoint is asked for: the typed query when the person took it over, else the row's.
  const askedQuery = useMemo(() => {
    const built = queryFromFilters(filterColumns, filters);
    return filterText === null ? built : { q: filterText.trim() || undefined, idPattern: built.idPattern };
  }, [filterColumns, filters, filterText]);

  /**
   * What applying the pending cells would send: the value before, the unit it was measured in and
   * whether the attribute is a Relationship all come from the row the person edited, so the
   * review reads what they saw and the write keeps what it must not lose. An edit outlives its
   * page, so the row it was made on is kept until the edit is applied or cleared: read from the
   * page shown now, a geometry edited on page 1 went out from page 2 as a plain Property (T-6230).
   */
  const editedRows = useRef(new Map<string, RichRow>());
  const pendingChanges = useMemo((): EntityChange[] => {
    const byId = editedRows.current;
    for (const id of byId.keys()) if (!(id in edits)) byId.delete(id);
    for (const row of rows) byId.set(row.id, row);
    return Object.entries(edits).map(([id, attrs]) => {
      const row = byId.get(id);
      const changes: AttributeChange[] = Object.entries(attrs).map(([attribute, after]) => {
        const cell = row?.cells[attribute];
        const one = Array.isArray(cell) ? cell[0] : cell;
        return {
          attribute,
          after,
          before: one?.kind === "relationship" ? one.object : one?.value,
          unitCode: one?.unitCode,
          // The cell's own kind decides how it is written: a geometry as a GeoProperty, so the
          // attribute keeps the type the model gave it (UI-72).
          // A model's relationship end is one even where the entity has no target on it yet.
          kind: one?.kind === "relationship" || relations?.[attribute] ? "relationship" : one?.kind === "geo" ? "geo" : "property",
        };
      });
      return { id, changes };
    });
  }, [edits, rows, relations]);

  // The question the source is asked, written out as a string, because the fetch must depend on the
  // question and not on the identity of the objects it is built from. `askedQuery` comes from the
  // columns, whose kinds are read off the rows, so every answer produced a new one: the effect asked
  // again, and again, 20 000 times in a second and a half, and a trusted click that started it froze
  // the page because React flushes such an update synchronously (T-2276). A host passing
  // `query={{ q: "…" }}` inline had the same effect.
  const gridQueryKey = JSON.stringify({
    type: typeOfFilter(config.type, filters["entity:type"]),
    // The caller's query and the filter row are both true: `;` is how NGSI-LD says "and", so a
    // preset that narrows the grid to one district cannot be widened by a filter (EP-07).
    q: andQ(queryPartial?.q ?? config.filters.preset?.q, askedQuery.q),
    attrs: withReads(queryPartial?.attrs ?? config.filters.preset?.attrs, formulas),
    idPattern: askedQuery.idPattern ?? queryPartial?.idPattern ?? config.filters.preset?.idPattern,
    scopeQ: queryPartial?.scopeQ ?? config.filters.preset?.scopeQ,
    // Exactly these entities: an opened relationship target (G-R2GRID2-017).
    ids: queryPartial?.ids,
    // `areaQuery` refuses a shape that is not an area, so a half-drawn one asks for nothing
    // rather than for everything.
    area: areaQuery(area) ?? undefined,
    orderBy: serverOrder,
  });

  const valuesQuery = useCallback(
    (key: string): GridQuery => {
      const asked = JSON.parse(gridQueryKey) as GridQuery;
      if (filterText !== null) return { ...asked, orderBy: undefined };
      const { [key]: _own, ...others } = filters;
      const built = queryFromFilters(filterColumns, others);
      return {
        ...asked,
        q: andQ(queryPartial?.q ?? config.filters.preset?.q, built.q),
        idPattern: built.idPattern ?? queryPartial?.idPattern ?? config.filters.preset?.idPattern,
        orderBy: undefined,
      };
    },
    [gridQueryKey, filterText, filters, filterColumns, queryPartial?.q, queryPartial?.idPattern, config.filters.preset],
  );

  const narrowed = useMemo(() => {
    const asked = JSON.parse(gridQueryKey) as GridQuery;
    return Boolean(asked.q || asked.idPattern || asked.scopeQ || asked.area || asked.ids);
  }, [gridQueryKey]);

  const fetchData = useCallback(() => {
    const nonce = ++nonceRef.current;
    cancelledRef.current = false;
    setLoading(true);
    // A next page of the last question still on its way is dropped when it lands, and leaves its
    // flag behind unless it is cleared here: the new question could then never load more (T-5623).
    setLoadingMore(false);
    setError(null);
    setErrorDetail(null);
    setRefusal(null);

    const gridQuery = JSON.parse(gridQueryKey) as GridQuery;
    // A scrolling grid starts over from the first row whenever the question changes.
    const from = virtual && !paged ? 0 : offset;
    if (from === 0) {
      setTotal(undefined);
      // The total has a read of its own beside the rows (T-6541): a count walks the whole match and
      // can outlast the gateway's 10 s cut (Praha Tree: 503 after 10 s, the rows alone in 0.2 s), so
      // a failed or slow count costs the footer its total, never the page its rows.
      source
        .query({ ...gridQuery, orderBy: undefined }, { offset: 0, limit: 0, count: true })
        .then((counted) => {
          if (cancelledRef.current || nonceRef.current !== nonce || counted.total === undefined) return;
          const total = counted.total;
          setTotal(total);
          setRows((loaded) => {
            setMore(hasMore(loaded.length, total, loaded.length, config.pageSize));
            return loaded;
          });
        })
        .catch(() => undefined);
    }

    source
      .query(gridQuery, { offset: from, limit: config.pageSize, count: false })
      .then((page) => {
        if (cancelledRef.current || nonceRef.current !== nonce) return;
        setRows(page.rows);
        if (page.total !== undefined) setTotal(page.total);
        setTotal((known) => {
          setMore(hasMore(page.rows.length, known, page.rows.length, config.pageSize));
          return known;
        });
        setLoading(false);
      })
      .catch((err) => {
        if (cancelledRef.current || nonceRef.current !== nonce) return;
        if (err instanceof SourceError && err.orderRefused && gridQuery.orderBy) {
          // The same question without the order: the rows come back and the page is sorted here.
          refusesOrder.add(source);
          setOrderRefusals((n) => n + 1);
          return;
        }
        const refused = refusalOf(err);
        if (refused) {
          // Who asks, not what: the rows of an earlier answer go too, and nothing is asked again
          // until the question or the session changes.
          setRows([]);
          setTotal(undefined);
          setMore(false);
          setRefusal(refused);
          setRefusalDetail(refused === "noAccess" && err instanceof Error ? err.message : null);
          setLoading(false);
          return;
        }
        if (err instanceof SourceError && err.status === 400 && err.problem) {
          // The broker's sentence quotes clauses and parameters: plain words first, its own under a disclosure.
          setError(err.problem.title ? `${labels.queryRefused}: ${err.problem.title}` : labels.queryRefused);
          setErrorDetail(err.problem.detail ?? null);
        } else {
          setError(err instanceof Error ? err.message : String(err));
        }
        setLoading(false);
      });
  }, [source, config.pageSize, offset, gridQueryKey, virtual, paged, labels.queryRefused]);

  /**
   * The next page of a scrolling grid, appended to what is loaded. An answer to an older question
   * is dropped, and an entity already loaded is not listed twice when the set moved under the read.
   */
  const loadMore = useCallback(() => {
    if (!virtual || paged || !more || loadingMore || loading) return;
    const nonce = nonceRef.current;
    const from = rows.length;
    const limit = Math.max(config.pageSize, VIRTUAL_PAGE_SIZE);
    setLoadingMore(true);
    source
      // The first page brought the total: a further page asks for rows alone (T-6161).
      .query(JSON.parse(gridQueryKey) as GridQuery, { offset: from, limit, count: false })
      .then((page) => {
        if (nonceRef.current !== nonce) return;
        setRows((loaded) => {
          const seen = new Set(loaded.map((row) => row.id));
          const next = [...loaded, ...page.rows.filter((row) => !seen.has(row.id))].slice(0, MAX_VIRTUAL_ROWS);
          setMore(next.length < MAX_VIRTUAL_ROWS && hasMore(next.length, page.total, page.rows.length, limit));
          return next;
        });
        if (page.total !== undefined) setTotal(page.total);
      })
      .catch((err) => {
        if (nonceRef.current !== nonce) return;
        const refused = refusalOf(err);
        if (refused) {
          setRows([]);
          setMore(false);
          setRefusal(refused);
          setRefusalDetail(refused === "noAccess" && err instanceof Error ? err.message : null);
          return;
        }
        setError(err instanceof Error ? err.message : String(err));
      })
      .finally(() => {
        if (nonceRef.current === nonce) setLoadingMore(false);
      });
  }, [virtual, paged, more, loadingMore, loading, rows.length, source, gridQueryKey, config.pageSize]);

  useEffect(() => {
    fetchData();
    return () => {
      cancelledRef.current = true;
    };
  }, [fetchData]);

  const reload = useCallback(() => {
    fetchData();
  }, [fetchData]);

  const cellOf = useCallback(
    (row: RichRow, column: VisibleColumn): { text: string; cell?: RichCell | RichCell[] } => {
      if (column.attr === null) {
        // id or entity timestamp
        if (column.key === "id") return { text: row.id };
        if (column.key === "entity:createdAt") return { text: row.createdAt ?? "" };
        if (column.key === "entity:modifiedAt") return { text: row.modifiedAt ?? "" };
        if (column.key === "entity:type") return { text: row.type };
        if (column.key === "entity:scope") return { text: [row.scope ?? []].flat().join(", ") };
        return { text: "" };
      }
      const cell = row.cells[column.attr];
      if (column.meta) {
        // metadata sub-column
        if (Array.isArray(cell)) {
          const texts = cell.map((c) => {
            const val = metaOf(c, column.meta!);
            return val == null ? "" : String(val);
          });
          return { text: texts.join(" | "), cell };
        }
        if (cell) {
          const val = metaOf(cell, column.meta);
          return { text: val == null ? "" : String(val), cell };
        }
        return { text: "" };
      }
      return { text: cellTextWithUnit(cell, column), cell };
    },
    [],
  );

  const toggleMeta = useCallback(
    (attr: string, meta: MetaKey) => {
      const next = { ...shown };
      const current = next[attr] ?? [];
      if (current.includes(meta)) {
        next[attr] = current.filter((m) => m !== meta);
        if (next[attr].length === 0) delete next[attr];
      } else {
        next[attr] = [...current, meta];
      }
      if (onStateChange) {
        onStateChange({ offset, activeCell, selected, shown: next, sort, filters, filterText, edits });
      }
      if (controlledState?.shown === undefined) {
        setInternalShown(next);
      }
    },
    [shown, offset, activeCell, selected, sort, filters, filterText, edits, onStateChange, controlledState],
  );

  const setOffset = useCallback(
    (newOffset: number) => {
      if (onStateChange) {
        onStateChange({ offset: newOffset, activeCell, selected, shown, sort, filters, filterText, edits });
      }
      if (controlledState?.offset === undefined) {
        setInternalOffset(newOffset);
      }
    },
    [activeCell, selected, shown, sort, filters, filterText, edits, onStateChange, controlledState],
  );

  const setSort = useCallback(
    (attr: string, dir?: "asc" | "desc") => {
      // A direction asked for is taken as it is; without one a second click turns the order round.
      const next = { attr, dir: dir ?? (sort?.attr === attr && sort.dir === "asc" ? ("desc" as const) : ("asc" as const)) };
      // A new order of the whole set starts at its first page, as a new filter does.
      const from = ordersAll ? 0 : offset;
      if (onStateChange) {
        onStateChange({ offset: from, activeCell, selected, shown, sort: next, filters, filterText, edits });
      }
      if (controlledState?.sort === undefined) {
        setInternalSort(next);
      }
      if (controlledState?.offset === undefined) {
        setInternalOffset(from);
      }
    },
    [sort, offset, activeCell, selected, shown, onStateChange, controlledState, ordersAll],
  );

  /**
   * One column's filter. The answer starts again at the first page: a filter applied on page
   * three would otherwise show an empty page of a set that does have matches.
   */
  const setFilter = useCallback(
    (key: string | readonly string[], filter: ColumnFilter | undefined) => {
      const next = { ...filters };
      for (const each of typeof key === "string" ? [key] : key) {
        if (filter === undefined) {
          delete next[each];
        } else {
          next[each] = filter;
        }
      }
      if (onStateChange) {
        onStateChange({ offset: 0, activeCell, selected, shown, sort, filters: next, filterText, edits });
      }
      if (controlledState?.filters === undefined) {
        setInternalFilters(next);
      }
      if (controlledState?.offset === undefined) {
        setInternalOffset(0);
      }
    },
    [filters, filterText, edits, activeCell, selected, shown, sort, onStateChange, controlledState],
  );

  /**
   * Moves the active cell outright, for a selection that came from somewhere other than the keyboard
   * or a cell click — the map's shapes (UI-72). `moveActive` walks by one key; this puts it where a
   * caller says, clamped to the page so a stale row index cannot point off it.
   */
  const setActive = useCallback(
    (row: number, col: number) => {
      const next = {
        row: Math.max(0, Math.min(row, Math.max(0, sortedRows.length - 1))),
        col: Math.max(0, Math.min(col, Math.max(0, columns.length - 1))),
      };
      if (onStateChange) {
        onStateChange({ offset, activeCell: next, selected, shown, sort, filters, filterText, edits });
      }
      if (controlledState?.activeCell === undefined) {
        setInternalActiveCell(next);
      }
    },
    [sortedRows.length, columns.length, offset, selected, shown, sort, filters, filterText, edits, onStateChange, controlledState],
  );

  /**
   * The drawn area, or `null` to remove it. Like a filter, a new area starts the answer at page one:
   * the page the person was on is a page of a different question.
   */
  const setArea = useCallback(
    (next: GeoArea | null) => {
      setAreaState(next);
      if (onStateChange) {
        onStateChange({ offset: 0, activeCell, selected, shown, sort, filters, filterText, edits });
      }
      if (controlledState?.offset === undefined) {
        setInternalOffset(0);
      }
    },
    [filters, filterText, edits, activeCell, selected, shown, sort, onStateChange, controlledState],
  );

  /**
   * One cell a person changed. A value equal to what the endpoint answered is not a change, so
   * typing a value back removes it from the pending list rather than sending it again.
   */
  /**
   * Sets or clears several pending values in one step. Each call builds on the edits as they are
   * now, so two calls in one event would keep only the second: a paste or an apply that clears
   * what landed goes through here with every change at once (T-3097).
   */
  const setEdits = useCallback(
    (changes: readonly { id: string; attr: string; value: unknown | undefined }[]) => {
      if (changes.length === 0) return;
      const next: Record<string, Record<string, unknown>> = { ...edits };
      for (const { id, attr, value } of changes) {
        const forEntity = { ...(next[id] ?? {}) };
        if (value === undefined) {
          delete forEntity[attr];
        } else {
          forEntity[attr] = value;
        }
        if (Object.keys(forEntity).length === 0) {
          delete next[id];
        } else {
          next[id] = forEntity;
        }
      }
      if (onStateChange) {
        onStateChange({ offset, activeCell, selected, shown, sort, filters, filterText, edits: next });
      }
      if (controlledState?.edits === undefined) {
        setInternalEdits(next);
      }
    },
    [edits, offset, activeCell, selected, shown, sort, filters, filterText, onStateChange, controlledState],
  );

  const setEdit = useCallback(
    (id: string, attr: string, value: unknown | undefined) => setEdits([{ id, attr, value }]),
    [setEdits],
  );

  const clearEdits = useCallback(() => {
    if (onStateChange) {
      onStateChange({ offset, activeCell, selected, shown, sort, filters, filterText, edits: {} });
    }
    if (controlledState?.edits === undefined) {
      setInternalEdits({});
    }
  }, [offset, activeCell, selected, shown, sort, filters, filterText, onStateChange, controlledState]);

  /** The query as text: the rows compose the first version, and the person owns it from then on. */
  const setFilterText = useCallback(
    (text: string | null) => {
      if (onStateChange) {
        onStateChange({ offset: 0, activeCell, selected, shown, sort, filters, filterText: text, edits });
      }
      if (controlledState?.filterText === undefined) {
        setInternalFilterText(text);
      }
      if (controlledState?.offset === undefined) {
        setInternalOffset(0);
      }
    },
    [filters, edits, activeCell, selected, shown, sort, onStateChange, controlledState],
  );

  // The grid's own id prefix, so every cell has an id the grid can name as its active descendant.
  const gridId = useId();
  const cellId = useCallback((row: number, col: number) => `${gridId}-r${row}-c${col}`, [gridId]);

  const moveActive = useCallback(
    (key: string, toEdge = false): boolean => {
      const maxRow = sortedRows.length - 1;
      const maxCol = columns.length - 1;
      let { row, col } = activeCell ?? { row: 0, col: 0 };
      // Nothing active yet: the first key that moves lands on the first cell, not one beyond it.
      const landing = activeCell === null && MOVE_KEYS.has(key);
      if (landing && maxRow < 0) return false;
      if (!landing) switch (key) {
        case "ArrowUp":
          if (row > 0) row--;
          else return false;
          break;
        case "ArrowDown":
          if (row < maxRow) row++;
          else return false;
          break;
        case "ArrowLeft":
          if (col > 0) col--;
          else return false;
          break;
        case "ArrowRight":
          if (col < maxCol) col++;
          else return false;
          break;
        case "Home":
          // Ctrl+Home is the grid's first cell, Home the row's (WAI-ARIA grid pattern).
          col = 0;
          if (toEdge) row = 0;
          break;
        case "End":
          col = maxCol;
          if (toEdge) row = maxRow;
          break;
        case "PageUp":
          row = Math.max(0, row - Math.max(1, sortedRows.length));
          break;
        case "PageDown":
          row = Math.min(maxRow, row + Math.max(1, sortedRows.length));
          break;
        default:
          return false;
      }

      const next = { row, col };
      if (onStateChange) {
        onStateChange({ offset, activeCell: next, selected, shown, sort, filters, filterText, edits });
      }
      if (controlledState?.activeCell === undefined) {
        setInternalActiveCell(next);
      }
      return true;
    },
    [activeCell, sortedRows.length, columns.length, offset, selected, shown, sort, filters, filterText, edits, onStateChange, controlledState],
  );

  const getGridProps = useCallback((): Record<string, unknown> => {
    return {
      role: "grid",
      // Every row of the set the grid pages through, and the header: a screen reader then says
      // "row 12 of 340" on page two, not "row 2 of 11". Unknown is -1 (WAI-ARIA).
      "aria-rowcount": total !== undefined ? total + 1 : -1,
      "aria-colcount": columns.length,
      // Focus stays on the grid and the active cell is its active descendant, so the keys work
      // from one tab stop and the cell is announced as it moves (UI-70).
      "aria-activedescendant": activeCell && sortedRows.length > 0 ? cellId(activeCell.row, activeCell.col) : undefined,
      tabIndex: 0,
      onKeyDown: (e: React.KeyboardEvent) => {
        // Inside a cell's own input or picker the keys are that control's: an arrow moves the
        // caret or the picked value, not the active cell (UI-70, UI-86).
        const target = e.target as HTMLElement;
        if (target !== e.currentTarget && /^(INPUT|SELECT|TEXTAREA)$/.test(target.tagName)) {
          return;
        }
        if (moveActive(e.key, e.ctrlKey || e.metaKey)) {
          e.preventDefault();
        }
      },
    };
  }, [total, columns.length, moveActive, activeCell, sortedRows.length, cellId]);

  const getHeaderProps = useCallback(
    (column: VisibleColumn, index: number): Record<string, unknown> => {
      // The sorted column says which way, for a screen reader; the arrow in its button is for the eye.
      const sorted = sort?.attr === (column.attr ?? column.key);
      return {
        role: "columnheader",
        "aria-colindex": index + 1,
        ...(sorted ? { "aria-sort": sort?.dir === "asc" ? "ascending" : "descending" } : {}),
      };
    },
    [sort],
  );

  const getRowProps = useCallback(
    (_row: RichRow, index: number): Record<string, unknown> => {
      return {
        role: "row",
        // The row's place in the whole set: the page's offset, then the header row.
        "aria-rowindex": offset + index + 2,
      };
    },
    [offset],
  );

  const getCellProps = useCallback(
    (_row: RichRow, rowIndex: number, _column: VisibleColumn, colIndex: number): Record<string, unknown> => {
      const isActive = activeCell?.row === rowIndex && activeCell?.col === colIndex;
      return {
        id: cellId(rowIndex, colIndex),
        role: "gridcell",
        "aria-colindex": colIndex + 1,
        "aria-selected": isActive,
        "data-active": isActive ? "" : undefined,
        // The cell a person clicks is the active one, so the keys go on from there (A04, UI-70).
        onMouseDown: () => setActive(rowIndex, colIndex),
      };
    },
    [activeCell, cellId, setActive],
  );

  return {
    more: virtual && !paged && more,
    loadingMore,
    loadMore,
    rows: sortedRows,
    sortsAll: ordersAll,
    columns,
    total,
    loading,
    error,
    refusal,
    refusalDetail,
    errorDetail,
    sortedLoadedOnly,
    state: { offset, activeCell, selected, shown, sort, filters, filterText, edits },
    labels,
    cellOf,
    toggleMeta,
    setFilter,
    setFilterText,
    filterColumns,
    askedQuery,
    valuesQuery,
    area,
    narrowed,
    setArea,
    setActive,
    setEdit,
    setEdits,
    clearEdits,
    pendingChanges,
    setOffset,
    setSort,
    moveActive,
    getGridProps,
    getHeaderProps,
    getRowProps,
    getCellProps,
    reload,
  };
}
