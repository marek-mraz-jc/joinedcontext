/**
 * What one Endpoint answers (T-1433; UI-69, EP-07): the grid, read only, over the endpoint itself.
 *
 * The types and their attributes come from the endpoint's own published schema rather than from the
 * DataModel, because the schema is already projected to the policy: what it lists is what this
 * endpoint may publish at all (EP-46, EP-61). An attribute the steward hides is left out of the
 * columns here too, so the effect of a tick in the form is visible as data and not only as a name.
 */
import { useCallback, useEffect, useMemo, useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { andQ, EntityCompare, enumsOf, originTransport, parseGridConfig, parseQ, relationsOf, sourceFor } from "@joinedcontext/sdk";
import type { ColumnFilter, FilterColumn, FilterGroup, FilterKind, GridColumn, GridState } from "@joinedcontext/sdk";
import { fetchJson, publishedTypes } from "../../pages/endpoints/SchemaProjectionPanel";
import { classSlots, parseModel } from "../../pages/models/linkml";
import type { ClassView, LinkmlModel, LinkmlSlot } from "../../pages/models/linkml";
import { Button, EmptyState, PageFailed, safeHref } from "../ui";
import { ApiError } from "../../api/client";
import { useSignIn } from "../../auth/AuthProvider";
import { DataToolbar, sortKeysOf, useDataToolbarState } from "./DataToolbar";
import { conditionCount, EMPTY_GROUP, FilterChips } from "./FilterPanel";
import { GroupCount } from "./GridToolbar";
import { columnLabel, displayOrder } from "./displayOrder";
import { gridLabels, PortalEntityGrid } from "./PortalEntityGrid";
import { filtersFromQ } from "./presetFilters";
import { unreadableOf, useAttributeAccess } from "../../api/access";
import { useIsPhone } from "../../phone";

interface SchemaIndex {
  models?: { name?: string; version?: number }[];
}

/** What a link to this view holds (T-5082): the type, the query as typed, and the order. */
export interface DataLink {
  type?: string;
  q?: string;
  /** `attr` or `attr;desc`, as `orderBy` takes it (CIM 009 4.23). */
  sort?: string;
  /** The reader removed the model's default filter (DM-135): the link opens without it. */
  noDefault?: boolean;
}

/** One value of a row card (DM-135): its label, unit, kind and enum titles as the model gives them. */
export interface CardField {
  name: string;
  label: string;
  /** The unit's symbol, e.g. °C. */
  unit?: string;
  /** The schema's kind: `date`, `date-time`, `number`, `integer`, `uri`, `string`, or empty. */
  kind?: string;
  /** What a reader reads for an enum value, by value. */
  titles?: Record<string, string>;
  /** `view_hidden`: shown under the card's developer fold. */
  hidden?: boolean;
}

/** How a host draws one row of a type as a card for a reader (DM-135, T-5231). */
export interface RowCard {
  type: string;
  /** The class's label in the page's language, else its name made readable. */
  typeLabel: string;
  /** The attribute holding the row's title. */
  title: string;
  fields: CardField[];
  /** The publisher's map defaults for the type (API/11 EP-163), which the host checks against the model. */
  map?: MapDefaults;
}

/** What the widget's map starts from for one type, as `widget.json` lists it. */
export interface MapDefaults {
  geometry?: string;
  colour?: string;
  popup?: string[];
  cluster?: boolean;
}

/** The map defaults of `type` in a `widget.json` answer, members of the wrong shape left out. */
export function mapDefaultsOf(answer: unknown, type: string): MapDefaults | undefined {
  const map = (answer as { map?: unknown } | null)?.map;
  const one = map && typeof map === "object" && Object.hasOwn(map, type) ? (map as Record<string, unknown>)[type] : undefined;
  if (!one || typeof one !== "object") return undefined;
  const { geometry, colour, popup, cluster } = one as Record<string, unknown>;
  const out: MapDefaults = {
    ...(typeof geometry === "string" ? { geometry } : {}),
    ...(typeof colour === "string" ? { colour } : {}),
    ...(Array.isArray(popup) && popup.every((p) => typeof p === "string") ? { popup: popup as string[] } : {}),
    ...(typeof cluster === "boolean" ? { cluster } : {}),
  };
  return Object.keys(out).length > 0 ? out : undefined;
}

/**
 * Where the publisher starts the table of `type` (API/11 EP-164), as `widget.json` lists it under
 * `table`; members of the wrong shape are left out. It ranks above the model's `view_*` (DM-134).
 */
export function tableDefaultsOf(answer: unknown, type: string): ClassView | undefined {
  const table = (answer as { table?: unknown } | null)?.table;
  const one = table && typeof table === "object" && Object.hasOwn(table, type) ? (table as Record<string, unknown>)[type] : undefined;
  if (!one || typeof one !== "object") return undefined;
  const { filter, orderBy, pageLength, columns } = one as Record<string, unknown>;
  const out: ClassView = {
    ...(typeof filter === "string" ? { filter } : {}),
    ...(typeof orderBy === "string" ? { orderBy } : {}),
    ...(typeof pageLength === "number" && (PAGE_LENGTHS as readonly number[]).includes(pageLength) ? { pageLength } : {}),
    ...(Array.isArray(columns) && columns.length > 0 && columns.every((c) => typeof c === "string") ? { columns: columns as string[] } : {}),
  };
  return Object.keys(out).length > 0 ? out : undefined;
}

type Sort = { attr: string; dir: "asc" | "desc" } | null;

/** Where the grid stands: the typed query (or none), the column inputs and the order. */
export interface OpenedView {
  filterText: string | null;
  filters?: Record<string, ColumnFilter>;
  sort: Sort;
}

/** The page lengths a publisher may name; the model's `view_page_length` picks one (DM-134). */
const PAGE_LENGTHS = [10, 20, 50, 100] as const;

/** A link's order, kept only when it names a column of the type: nothing else reaches `orderBy`. */
export function sortOfLink(raw: string | undefined, attributes: string[]): Sort {
  const [attr, dir, ...rest] = (raw ?? "").split(";");
  if (rest.length > 0 || (dir !== undefined && dir !== "asc" && dir !== "desc")) return null;
  return attr === "id" || attributes.includes(attr) ? { attr, dir: dir === "desc" ? "desc" : "asc" } : null;
}

/** One row of the Fields table: an attribute, its type and its description in the schema. */
export interface FieldRow {
  name: string;
  kind: string;
  description: string;
  /** The model's label in the page's language; absent without a published model. */
  label?: string;
}

/**
 * The columns of a type as its published model orders and names them (T-5192, DM-98, DM-100): the
 * ranked slots in rank order are shown, everything else the endpoint answers stays one tick away;
 * a GeoProperty is left to the map. Without a model the schema's attributes keep their order, all
 * shown. Titles resolve model label > model title > field-order.json > the name made readable.
 */
export function columnModel(
  slots: readonly LinkmlSlot[],
  attributes: readonly string[],
  locale: string,
  columns?: readonly string[],
): { order: string[]; shown: string[]; labels: Record<string, string>; widths: Record<string, number>; title?: string } {
  const bySlot = new Map(slots.map((slot) => [slot.name, slot]));
  const titled = (names: readonly string[]) => Object.fromEntries(names.map((name) => [name, columnLabel(bySlot.get(name), name, locale)]));
  if (slots.length === 0) return { order: [...attributes], shown: [...attributes], labels: titled(attributes), widths: {} };
  const named = (columns ?? []).filter((name) => attributes.includes(name));
  // `view_columns` orders and picks the shown ones; the rest follow in the model's order.
  const order = [...new Set([...named, ...displayOrder(slots, attributes)])];
  const ranked = order.filter((name) => bySlot.get(name)?.rank !== undefined && bySlot.get(name)?.kind !== "GeoProperty");
  const shown = (named.length > 0 ? named : ranked.length > 0 ? ranked : order).filter((name) => !bySlot.get(name)?.hidden);
  // A width in characters as the cell's pixels at the grid's 14 px type, padding included.
  const widths = Object.fromEntries(
    order.flatMap((name) => {
      const width = bySlot.get(name)?.width;
      return width ? [[name, Math.round(width * 8 + 24)]] : [];
    }),
  );
  const title = order.find((name) => bySlot.get(name)?.role === "title");
  return { order, shown, labels: titled(order), widths, ...(title ? { title } : {}) };
}

/** The card of a type (DM-135): every attribute in the table's order, under its label. */
export function cardOf(
  type: string,
  klass: { label?: Record<string, string> } | undefined,
  slots: readonly LinkmlSlot[],
  shape: { order: string[]; labels: Record<string, string>; title?: string },
  kinds: ReadonlyMap<string, string>,
  enums: Record<string, readonly { value: string; title?: string }[]>,
  locale: string,
  map?: MapDefaults,
): RowCard {
  const bySlot = new Map(slots.map((slot) => [slot.name, slot]));
  return {
    ...(map ? { map } : {}),
    type,
    typeLabel: columnLabel(klass, type, locale),
    title: shape.title ?? "name",
    fields: shape.order.map((name) => {
      const slot = bySlot.get(name);
      // Every enum value, its title or itself: the host offers an enum as a map colour by its titles (T-5239).
      const titles = Object.fromEntries((enums[name] ?? []).map((one) => [one.value, one.title ?? one.value]));
      return {
        name,
        label: shape.labels[name] ?? columnLabel(undefined, name, locale),
        ...(slot?.unit?.symbol ? { unit: slot.unit.symbol } : {}),
        ...(kinds.get(name) ? { kind: kinds.get(name) } : {}),
        ...(Object.keys(titles).length > 0 ? { titles } : {}),
        ...(slot?.hidden ? { hidden: true } : {}),
      };
    }),
  };
}

/** A default filter in words (DM-135): column labels for names, the reader's word for `today`, ≥ for >=. */
export function filterWords(q: string, labels: Record<string, string>, today: string): string {
  return q
    .replace(/[A-Za-z_][A-Za-z0-9_]*/g, (word) => (word === "today" ? today : Object.hasOwn(labels, word) ? labels[word] : word))
    .replace(/>=/g, " ≥ ")
    .replace(/<=/g, " ≤ ")
    .replace(/==/g, " = ")
    .replace(/!=/g, " ≠ ")
    .replace(/(^|[^≥≤=≠ ])([<>])/g, "$1 $2 ")
    .replace(/;/g, ", ")
    .replace(/\|/g, " | ")
    .replace(/\s+/g, " ")
    .trim();
}

/** The same filter: the typed text and the Filter panel's conditions. */
function sameFilter(a: OpenedView, group: FilterGroup, b: OpenedView | null): boolean {
  if (!b) return false;
  return (a.filterText ?? "") === (b.filterText ?? "") && JSON.stringify(group) === JSON.stringify(groupOf(b.filters));
}

/** A default filter's column inputs as the Filter panel's conditions (DM-134), joined by AND. */
export function groupOf(filters: Record<string, ColumnFilter> | undefined): FilterGroup {
  return { join: "and", conditions: Object.entries(filters ?? {}).map(([key, filter]) => ({ key, filter })) };
}

/**
 * `today`, `today+7d`, `today-1d` in a default filter as dates of the reader's calendar (DM-134),
 * quoted or not; the rest of the `q` stays as written.
 */
export function relativeDates(q: string, today: Date): string {
  return q.replace(/\btoday(?:([+-])(\d{1,3})d)?\b/g, (_match, sign: string | undefined, days: string | undefined) => {
    const day = new Date(today.getFullYear(), today.getMonth(), today.getDate() + (sign === "-" ? -1 : 1) * Number(days ?? 0));
    return `${day.getFullYear()}-${String(day.getMonth() + 1).padStart(2, "0")}-${String(day.getDate()).padStart(2, "0")}`;
  });
}

/** What filters a column of this schema kind takes: chosen by the schema, never by the loaded rows (critic B5). */
export function formatOfKind(kind: string): GridColumn["format"] | undefined {
  if (kind === "date" || kind === "date-time") return "date";
  if (kind === "number" || kind === "integer") return "number";
  if (kind === "uri") return "link";
  return kind === "string" ? "text" : undefined;
}

const KIND_OF_FORMAT: Record<string, FilterKind> = { date: "date", number: "number", link: "text", text: "text" };

/**
 * Where a table opens without a link of its own (DM-133, DM-134): the model's default filter as
 * filled column inputs, or as text when one input per column cannot hold it; else `view_upcoming`.
 * A filter the `q` parser refuses or that names a column the endpoint does not answer is ignored.
 */
export function presetView(
  view: ClassView | undefined,
  columns: readonly GridColumn[],
  enums: Record<string, unknown[]>,
  today: Date,
): OpenedView | null {
  const attributes = columns.map((column) => column.attr);
  const day = relativeDates("today", today);
  const upcoming = modelView(view, attributes, day);
  if (!view?.filter) return upcoming;
  const q = relativeDates(view.filter, today);
  const sort = upcoming?.sort ?? null;
  const known: FilterColumn[] = columns.map((column) => ({
    key: column.attr,
    attr: column.attr,
    kind: enums[column.attr]?.length ? "enum" : (KIND_OF_FORMAT[column.format ?? "text"] ?? "text"),
  }));
  const filters = filtersFromQ(q, known);
  if (filters) return { filterText: null, filters, sort };
  // Otherwise text the reader can read and clear, but only a `q` the parser takes whose every
  // term names a column of this type: anything else is a publisher's mistake, and the table opens as without it.
  const parsed = parseQ(q);
  const fits = parsed !== null && parsed.every((all) => all.every((term) => attributes.includes(term.path[0] ?? "")));
  return fits ? { filterText: q, sort } : upcoming;
}

/**
 * Where a table opens when its link names no query or order (T-5192): as the class's model says
 * (`view_order_by`, `view_upcoming`), for any type; an events model opens at today, soonest first,
 * the undated last. An attribute the endpoint does not answer is ignored.
 */
export function modelView(
  view: { orderBy?: string; upcoming?: string } | undefined,
  attributes: readonly string[],
  today: string,
): { filterText: string | null; sort: Sort } | null {
  const upcoming = view?.upcoming && attributes.includes(view.upcoming) ? view.upcoming : undefined;
  const sort = sortOfLink(view?.orderBy, [...attributes]) ?? (upcoming ? { attr: upcoming, dir: "asc" as const } : null);
  if (!upcoming && !sort) return null;
  return { filterText: upcoming ? `!${upcoming}|${upcoming}>="${today}"` : null, sort };
}

/** An http(s) address a person opens in a new tab; anything else stays text. */
function webLink(value: unknown, text: string): JSX.Element | string {
  const safe = typeof value === "string" && /^https?:\/\//i.test(value.trim()) ? safeHref(value) : undefined;
  if (!safe) return typeof value === "string" ? value : "";
  return (
    <a className="text-primary-soft-fg underline hover:no-underline" href={safe} target="_blank" rel="noopener noreferrer" title={safe}>
      {text}
    </a>
  );
}

/** The Fields of one type from the endpoint's JSON Schema, in the column order. */
export function fieldsOf(definition: unknown, attributes: string[]): FieldRow[] {
  const properties = ((definition ?? {}) as { properties?: Record<string, unknown> }).properties ?? {};
  return attributes.map((name) => {
    const property = (properties[name] ?? {}) as { type?: unknown; format?: unknown; $ref?: unknown; description?: unknown; anyOf?: unknown };
    const branch = Array.isArray(property.anyOf) ? ((property.anyOf.find((each) => each && typeof each === "object") ?? {}) as typeof property) : property;
    const ref = typeof branch.$ref === "string" ? branch.$ref.split("/").pop() : undefined;
    const kind = typeof branch.format === "string" ? branch.format : typeof branch.type === "string" ? branch.type : ref ?? "";
    return { name, kind, description: typeof property.description === "string" ? property.description : "" };
  });
}

/** The definitions of an endpoint's JSON Schema, `$defs` and `definitions` alike. */
function definitionsOf(schema: unknown): Record<string, unknown> {
  const document = (schema ?? {}) as { definitions?: Record<string, unknown>; $defs?: Record<string, unknown> };
  return { ...document.$defs, ...document.definitions };
}

/** The download of exactly the rows the grid asked: the endpoint's `file.csv` with its type, query and order. */
function csvHref(origin: string | undefined, slug: string, type: string, q: string | undefined, orderBy: string | undefined): string {
  const query = new URLSearchParams({ type, ...(q ? { q } : {}), ...(orderBy ? { orderBy } : {}) });
  return `${origin ?? ""}/api/endpoint/${encodeURIComponent(slug)}/file.csv?${query.toString()}`;
}

/**
 * The view's actions: compare with the space (only when the endpoint serves one, T-1435), the full
 * window, and the CSV download of the current query.
 */
function DataActions({
  space,
  comparing,
  onCompare,
  full,
  onFull,
  csv,
  csvRefused,
}: {
  space?: string;
  comparing: boolean;
  onCompare: () => void;
  full: boolean;
  onFull: () => void;
  csv?: string;
  /** Why the CSV is not offered (T-5530): the person's decisions do not let them read what it holds. */
  csvRefused?: string;
}): JSX.Element {
  const { t } = useTranslation();
  return (
    <>
      <div className="flex flex-wrap items-center gap-2">
        {space ? (
          <Button size="sm" variant="secondary" onClick={onCompare}>
            {comparing ? t("endpoints.data.compareClose") : t("endpoints.data.compare")}
          </Button>
        ) : null}
        <Button size="sm" variant="secondary" aria-pressed={full} onClick={onFull}>
          {full ? t("endpoints.data.fullLeave") : t("endpoints.data.full")}
        </Button>
        {csvRefused ? (
          <Button size="sm" variant="secondary" disabled disabledReason={csvRefused}>
            {t("endpoints.data.csv")}
          </Button>
        ) : csv ? (
          <a className="focus-ring touch-target rounded-sm text-caption text-primary-soft-fg underline hover:no-underline" href={safeHref(csv)} download>
            {t("endpoints.data.csv")}
          </a>
        ) : null}
      </div>
      {csv && !csvRefused ? <p className="text-caption text-fg-muted">{t("endpoints.data.csvHelp")}</p> : null}
      {/* Seen as well as heard: the button already carries the reason as its description. */}
      {csvRefused ? (
        <p className="text-caption text-fg-muted" aria-hidden="true">
          {csvRefused}
        </p>
      ) : null}
    </>
  );
}

export interface EndpointDataViewProps {
  project: string;
  /** The endpoint's slug: the grid reads through it, never through a URL of its own (EP-55). */
  slug: string;
  /** `spec.projection.hiddenAttributes`, including what the open form has ticked but not saved. */
  hidden?: string[];
  /**
   * The space this endpoint serves (`spec.contextSpaceRef`), for the comparison of T-1435. Without
   * one there is nothing to compare against and the button is absent.
   */
  space?: string;
  /** Where the view opens: a shared link's type, query and order (T-5082). */
  link?: DataLink;
  /** Every change of type, query or order, for a host that writes them into its address. */
  onLink?: (next: DataLink) => void;
  /** The card of the chosen type, for a host that draws a row's detail itself (DM-135). */
  onCard?: (card: RowCard) => void;
  /** A row opened by the reader, for a host that shows it in its own card instead of the grid's detail. */
  onOpenRow?: (id: string) => void;
  /** The endpoint serves `file.csv`, so the current query can be downloaded as a table. */
  csv?: boolean;
  /**
   * Where `/api/endpoint/{slug}` answers: the Portal's own origin with the person's session
   * (default), or the catalogue's `https://api.{domain}`, read without a cookie or token (T-5082).
   */
  origin?: string;
}

/** Where the grid opens: the kept or preset view, else the link's query and order; the order as text. */
function gridState(kept: OpenedView | null, link: DataLink | undefined, columns: string[]) {
  const opened: OpenedView = kept ?? { filterText: link?.q ?? null, sort: sortOfLink(link?.sort, columns) };
  return {
    opened,
    sortText: opened.sort ? `${opened.sort.attr}${opened.sort.dir === "desc" ? ";desc" : ""}` : undefined,
  };
}

/** A `uri` column's cell as a link named in words (T-5192). */
function linkRenderers(links: Set<string>, text: string): Record<string, (cell: unknown) => JSX.Element | string> {
  return Object.fromEntries(
    [...links].map((name) => [
      name,
      (cell: unknown) => webLink(cell && !Array.isArray(cell) ? (cell as { value?: unknown }).value : undefined, text),
    ]),
  );
}

/** The full window is a modal dialog over the page; Escape leaves it. */
function windowProps(full: boolean, label: string): JSX.IntrinsicElements["div"] {
  return full
    ? { className: "fixed inset-0 z-50 flex flex-col gap-3 overflow-auto bg-bg p-4", role: "dialog", "aria-modal": true, "aria-label": label }
    : { className: "flex flex-col gap-3" };
}

/** The endpoint's published schema: its index, the JSON Schema of the newest version and its model. */
export function useEndpointSchema(base: string, slug: string) {
  const index = useQuery({
    queryKey: ["endpoint-schema-index", slug],
    retry: false,
    queryFn: () => fetchJson(`${base}/index.json`) as Promise<SchemaIndex>,
  });
  const version = index.data?.models?.[0]?.version ?? 1;
  const schema = useQuery({
    queryKey: ["endpoint-schema-artifact", slug, version, "json-schema"],
    enabled: index.isSuccess,
    retry: false,
    queryFn: () => fetchJson(`${base}/v${version}/json-schema`),
  });

  // The published model names and orders the columns (T-5192); an endpoint without one, or a read
  // that fails, keeps the schema's own attribute names.
  const model = useQuery({
    queryKey: ["endpoint-schema-artifact", slug, version, "linkml"],
    enabled: index.isSuccess,
    retry: false,
    queryFn: async () => {
      const answer = await fetch(new Request(`${base}/v${version}/model.linkml.yaml`, { headers: { Accept: "text/yaml" } }));
      return answer.ok ? parseModel(await answer.text()) : null;
    },
  });
  return { index, schema, model };
}

/** The chosen type of the schema and its columns as the published model presents them (DM-134). */
function useTypeColumns(
  schemaData: unknown,
  modelData: LinkmlModel | null | undefined,
  chosen: string,
  hidden: string[],
  readerHidden: string[] | undefined,
  language: string,
  widgetData: unknown,
) {
  const types = useMemo(() => publishedTypes(schemaData), [schemaData]);
  const type = types.find((each) => each.name === chosen) ?? types[0];
  const columns = useMemo(() => (type ? type.attributes.filter((attribute) => !hidden.includes(attribute)) : []), [type, hidden]);
  const klass = type && modelData ? modelData.classes.find((each) => each.name === type.name) : undefined;
  // The Endpoint's table start wins over the model's, member by member (EP-164, DM-134).
  const view = useMemo((): ClassView | undefined => {
    const table = type ? tableDefaultsOf(widgetData, type.name) : undefined;
    return klass?.view || table ? { ...klass?.view, ...table } : undefined;
  }, [klass, widgetData, type]);
  const slots = useMemo(() => (klass && modelData ? classSlots(modelData, klass) : []), [modelData, klass]);
  const shape = useMemo(() => columnModel(slots, columns, language, view?.columns), [slots, columns, language, view]);
  // What the reader hid with "Hide fields"; until then what the model leaves out of its table (DM-134).
  const hiddenNow = useMemo(() => readerHidden ?? shape.order.filter((name) => !shape.shown.includes(name)), [shape, readerHidden]);
  const shownColumns = useMemo(() => shape.order.filter((name) => !hiddenNow.includes(name)), [shape, hiddenNow]);
  // The endpoint's own schema names its enums: such a column is filtered by picking (UI-86).
  const defs = useMemo(() => definitionsOf(schemaData), [schemaData]);
  const enums = useMemo(() => (type ? enumsOf(defs[type.name], defs, language) : {}), [defs, type, language]);
  // Its relationship ends are columns picked from the target's entities (UI-84).
  const relations = useMemo(() => (type ? relationsOf(defs[type.name]) : {}), [defs, type]);
  const kinds = useMemo(
    () => new Map(type ? fieldsOf(defs[type.name], columns).map((field) => [field.name, field.kind]) : []),
    [defs, type, columns],
  );
  // A `uri` attribute is a link a person opens, named in words rather than printed whole (T-5192).
  const links = useMemo(() => new Set([...kinds].filter(([, kind]) => kind === "uri").map(([name]) => name)), [kinds]);
  // Every column as the model presents it (DM-134): its label, its width, the row's title pinned
  // first, and the filter its schema type takes.
  const gridColumns = useMemo(
    (): GridColumn[] =>
      shape.order.map((attribute) => {
        const format = formatOfKind(kinds.get(attribute) ?? "");
        return {
          attr: attribute,
          // Without a `view_role: title`, `name` is the label attribute of the Smart Data Models (T-3097).
          ...(attribute === (shape.title ?? "name") ? { pinned: true } : {}),
          ...(shape.labels[attribute] ? { label: shape.labels[attribute] } : {}),
          ...(shape.widths[attribute] ? { width: shape.widths[attribute] } : {}),
          ...(format ? { format } : {}),
        };
      }),
    [kinds, shape],
  );
  return { types, type, columns, klass, view, slots, shape, hiddenNow, shownColumns, defs, enums, relations, links, kinds, gridColumns };
}

/** The model's default view, and whether the reader removed it (DM-135): kept in the link. */
function useDefaultView(
  view: ClassView | undefined,
  gridColumns: readonly GridColumn[],
  enums: Record<string, unknown[]>,
  link: DataLink | undefined,
) {
  const [dropped, setDropped] = useState(link?.noDefault === true);
  const defaults = useMemo(() => presetView(view, gridColumns, enums, new Date()), [view, gridColumns, enums]);
  const preset = link?.q || link?.sort || dropped ? null : defaults;
  return { dropped, setDropped, defaults, preset };
}

/** Hands the host the card of the chosen type (DM-135) whenever it changes. */
function useRowCard(
  type: string | undefined,
  klass: { label?: Record<string, string> } | undefined,
  slots: readonly LinkmlSlot[],
  shape: { order: string[]; labels: Record<string, string>; title?: string },
  kinds: ReadonlyMap<string, string>,
  enums: Record<string, readonly { value: string; title?: string }[]>,
  locale: string,
  map: MapDefaults | undefined,
  onCard: ((card: RowCard) => void) | undefined,
) {
  const card = useMemo(
    () => (type ? cardOf(type, klass, slots, shape, kinds, enums, locale, map) : null),
    [type, klass, slots, shape, kinds, enums, locale, map],
  );
  useEffect(() => {
    if (card) onCard?.(card);
  }, [card, onCard]);
}

/** A class's label in the page's language, else its name made readable. */
function classLabel(model: LinkmlModel | null | undefined, name: string, locale: string): string {
  return columnLabel(model?.classes.find((one) => one.name === name), name, locale);
}

/** A host that opens rows itself takes them from the grid, which then opens none. */
function openRowProps(onOpenRow: ((id: string) => void) | undefined) {
  return onOpenRow ? { openRow: null, onOpenRow: (id: string | null) => id && onOpenRow(id) } : {};
}

/**
 * The model's default filter as a chip the reader removes (DM-135), shown as filled inputs too (DM-134);
 * a reset of every filter and a way back to the default.
 */
function DefaultFilter({
  filter,
  defaults,
  opened,
  group,
  filtered,
  labels,
  onClear,
  onRestore,
}: {
  filter: string | undefined;
  defaults: OpenedView | null;
  opened: OpenedView;
  group: FilterGroup;
  filtered: boolean;
  labels: Record<string, string>;
  onClear: () => void;
  onRestore: () => void;
}): JSX.Element {
  const { t } = useTranslation();
  const set = defaults !== null && (defaults.filterText !== null || Object.keys(defaults.filters ?? {}).length > 0);
  const words = filter && set ? filterWords(filter, labels, t("endpoints.data.today")) : null;
  const atDefault = words !== null && sameFilter(opened, group, defaults);
  return (
    <>
      {atDefault ? (
        <span className="inline-flex min-h-11 items-center gap-1 rounded-full border border-border bg-surface-muted pl-3 text-body">
          {t("endpoints.data.defaultFilter", { filter: words })}
          <Button
            size="sm"
            variant="ghost"
            className="min-h-11 min-w-11 rounded-full"
            aria-label={t("endpoints.data.removeDefault", { filter: words })}
            onClick={onClear}
          >
            <span aria-hidden="true">×</span>
          </Button>
        </span>
      ) : null}
      {filtered ? (
        <Button size="sm" variant="secondary" onClick={onClear}>
          {t("endpoints.data.resetFilters")}
        </Button>
      ) : null}
      {words !== null && !atDefault ? (
        <Button size="sm" variant="secondary" onClick={onRestore}>
          {t("endpoints.data.restoreDefault")}
        </Button>
      ) : null}
    </>
  );
}

/**
 * Why the CSV is not offered, or undefined when it is (T-5530). The CSV holds every shown column,
 * so it is offered only when the endpoint says the person reads each of them. Without a CSV, or
 * when another origin is read anonymously, there are no decisions to ask.
 */
function useCsvRefusal(
  csv: { offered: boolean; origin?: string; slug: string },
  type: string | undefined,
  shown: readonly string[],
  labels: Record<string, string>,
): string | undefined {
  const { t } = useTranslation();
  const slug = csv.offered && !csv.origin ? csv.slug : undefined;
  const decisions = useAttributeAccess(slug, type);
  const unreadable = useMemo(() => unreadableOf(decisions.data, shown), [decisions.data, shown]);
  if (!slug) return undefined;
  if (decisions.isPending) return t("entityGrid.inspector.loading");
  if (decisions.isError) return t("endpoints.data.csvUnchecked");
  if (unreadable.length === 0) return undefined;
  return t("endpoints.data.csvRefused", { attributes: unreadable.map((name) => labels[name] ?? name).join(", ") });
}

/**
 * What the Data section shows while its reads are not all in: a refused or failed read says why in
 * the surface's own sentence (a 403 names the grant), a pending one that it loads, a schema without
 * a type that there is nothing to show. Null once the grid can open.
 */
/**
 * The order and text the grid reports, kept as the held view when they did not change: a new
 * object for the same state would set the grid's view again and the two would chase each other.
 */
export function heldView(before: OpenedView | null, next: Pick<GridState, "filterText" | "sort">): OpenedView {
  const filterText = next.filterText ?? null;
  const sort = next.sort ?? null;
  const same = before !== null && (before.filterText ?? null) === filterText && before.sort?.attr === sort?.attr && before.sort?.dir === sort?.dir;
  return same ? before : { filterText, sort };
}

/** The view the grid is handed: an order only when both its column and its direction are known. */
function gridViewOf(filterText: string | null, attr: string | undefined, dir: "asc" | "desc" | undefined) {
  return { filterText, sort: attr && dir ? { attr, dir } : null };
}

/** The grid's config for one type of the endpoint, or null before a type is known. */
function gridConfigOf(slug: string, type: string | undefined, columns: GridColumn[], rowsPerPage: number | undefined) {
  if (!type) return null;
  return (
    parseGridConfig({
      source: { kind: "endpoint", slug },
      type,
      columns,
      pageSize: rowsPerPage ?? 20,
      // Read only here: a value is corrected in the explorer, where the person chose the endpoint
      // for that purpose.
      mode: "view",
      history: { enabled: true },
    }).config ?? null
  );
}

/** Escape leaves the full window, as it closes a dialog. */
function useEscapeLeaves(full: boolean, setFull: (full: boolean) => void): void {
  useEffect(() => {
    if (!full) return;
    const leave = (event: KeyboardEvent) => {
      if (event.key === "Escape") setFull(false);
    };
    window.addEventListener("keydown", leave);
    return () => window.removeEventListener("keydown", leave);
  }, [full, setFull]);
}

/** A filter condition's title: the column's label, with the system field it reads after a dot. */
function filterTitleOf(column: FilterColumn, labels: Record<string, string>, t: (key: string) => string): string {
  const title = labels[column.attr ?? ""] ?? column.attr ?? column.key;
  return column.meta ? `${title} · ${t(`entityGrid.${column.meta}`)}` : title;
}

/** A private dataset refused for who is asking: sign in (401) or no access (403), in the grid's words. */
export function DatasetRefused({
  signedOut,
  detail,
  onSignIn,
  t,
}: {
  signedOut: boolean;
  detail?: string;
  onSignIn?: () => void;
  t: (key: string) => string;
}): JSX.Element {
  return (
    <EmptyState
      icon="lock"
      title={t(signedOut ? "entityGrid.signedOutTitle" : "entityGrid.noAccessTitle")}
      description={
        <>
          {t(signedOut ? "entityGrid.signedOutBody" : "entityGrid.noAccessBody")}
          {detail ? <span className="mt-1 block text-caption">{detail}</span> : null}
        </>
      }
      action={
        signedOut && onSignIn ? (
          <Button size="sm" onClick={onSignIn}>
            {t("entityGrid.signIn")}
          </Button>
        ) : undefined
      }
    />
  );
}

export function schemaState(
  reads: Record<"index" | "schema" | "widget", { error: Error | null; isPending: boolean; refetch?: () => unknown }>,
  ready: boolean,
  t: (key: string) => string,
  signIn?: () => void,
): JSX.Element | null {
  // The widget's defaults are optional: its error leaves the model's, so only its wait shows.
  // A failed read says the surface's sentence, its reference and, where asking again can help,
  // Retry, as every page's failure does (T-6313).
  const failed = reads.index.error ?? reads.schema.error;
  // A private Endpoint refuses its schema before any row (T-6376): the same two states the grid
  // shows for its rows, never a failure to retry.
  if (failed instanceof ApiError && (failed.status === 401 || failed.status === 403)) {
    return <DatasetRefused signedOut={failed.status === 401} detail={failed.status === 403 ? failed.message : undefined} onSignIn={signIn} t={t} />;
  }
  if (failed) return <PageFailed error={failed} onRetry={() => void Promise.all([reads.index.refetch?.(), reads.schema.refetch?.()])} />;
  if (Object.values(reads).some((read) => read.isPending)) return <p role="status">{t("endpoints.projection.loading")}</p>;
  if (!ready) return <p className="text-caption text-fg-muted">{t("endpoints.data.noSchema")}</p>;
  return null;
}

export function EndpointDataView({
  project,
  slug,
  hidden = [],
  space,
  link,
  onLink,
  onCard,
  onOpenRow,
  csv = false,
  origin,
}: EndpointDataViewProps): JSX.Element {
  const { t, i18n } = useTranslation();
  const [chosen, setChosen] = useState<string>(link?.type ?? "");
  const [full, setFull] = useState(false);
  // The query the grid last asked, for the link and the download of exactly these rows.
  const [asked, setAsked] = useState<string | undefined>(link?.q);
  const [comparing, setComparing] = useState(false);
  // Refused (401/403, T-6376): the grid shows sign-in or no access, and no tool or CSV offers
  // rows the person cannot read.
  const [refused, setRefused] = useState(false);
  const root = origin ?? window.location.origin;
  const base = `${root}/api/endpoint/${slug}/schema`;
  // Another origin is read anonymously: a cross-origin fetch with `same-origin` credentials sends no cookie.
  const source = useMemo(
    () => (origin ? sourceFor({ kind: "endpoint", slug }, originTransport((path, init) => fetch(origin + String(path), init)), i18n.language) : undefined),
    [origin, slug, i18n.language],
  );
  // The group headers count through the same door the rows come from.
  const countSource = useMemo(() => source ?? sourceFor({ kind: "endpoint", slug }, originTransport(), i18n.language), [source, slug, i18n.language]);

  const { index, schema, model } = useEndpointSchema(base, slug);
  // The publisher's widget defaults (EP-163, EP-164); none, or an error, leaves the model's.
  const widget = useQuery({
    queryKey: ["endpoint-widget", slug],
    retry: false,
    queryFn: () => fetchJson(`${root}/api/endpoint/${slug}/widget.json`),
  });
  // The space's toolbar over this grid (T-6303), its state held by the toolbar's own hook.
  const tools = useDataToolbarState();
  const phone = useIsPhone();
  const { extras, group } = tools;
  const { types, type, columns, klass, view: classView, slots, shape, hiddenNow, shownColumns, enums, relations, links, kinds, gridColumns } =
    useTypeColumns(schema.data, model.data, chosen, hidden, extras.hidden, i18n.language, widget.data);
  const csvRefused = useCsvRefusal({ offered: csv, origin, slug }, type?.name, shownColumns, shape.labels);
  // The order and the typed query the grid opens with; the grid holds them from then on.
  const [view, setView] = useState<OpenedView | null>(null);
  // The address is read once, as the view opens: the link this view writes back carries the
  // toolbar's query too, and reading it again would hand that query to the grid a second time.
  const [opening] = useState(link);
  const { dropped, setDropped, defaults, preset } = useDefaultView(classView, gridColumns, enums, opening);
  const { opened, sortText } = gridState(view ?? preset, opening, columns);
  // The default filter's inputs open in the Filter panel; from the first default on, the panel is the reader's.
  tools.adoptDefault(groupOf(preset?.filters));
  const allAsked = andQ(asked, tools.q);
  const filtered = opened.filterText !== null || conditionCount(group) > 0;
  const sortKeys = useMemo(() => sortKeysOf(opened.sort, tools.thenBy), [opened.sort, tools.thenBy]);
  useEffect(() => {
    if (type) onLink?.({ type: type.name, q: allAsked || undefined, sort: sortText, ...(dropped ? { noDefault: true } : {}) });
  }, [type, allAsked, sortText, dropped, onLink]);
  const map = useMemo(() => (type ? mapDefaultsOf(widget.data, type.name) : undefined), [widget.data, type]);
  useRowCard(type?.name, klass, slots, shape, kinds, enums, i18n.language, map, onCard);
  const onQuery = useCallback((query: { q?: string }) => setAsked(query.q), []);
  // The grid reports its state on every change; the same text and order keep the held one, or the two would chase each other.
  const onGridState = useCallback((next: GridState) => setView((before) => heldView(before, next)), []);
  const sortAttr = opened.sort?.attr;
  const sortDir = opened.sort?.dir;
  const gridView = useMemo(() => gridViewOf(opened.filterText, sortAttr, sortDir), [opened.filterText, sortAttr, sortDir]);
  const { hide } = tools;
  const onHide = useCallback((attr: string) => hide(hiddenNow, attr), [hide, hiddenNow]);

  useEscapeLeaves(full, setFull);
  const rowsPerPage = classView?.pageLength;
  const config = useMemo(() => gridConfigOf(slug, type?.name, gridColumns, rowsPerPage), [slug, type, gridColumns, rowsPerPage]);

  const compareLabels = useMemo(
    () => ({
      left: t("endpoints.data.spaceSide"),
      right: t("endpoints.data.endpointSide"),
      missingRow: t("endpoints.data.missingRow"),
      hiddenColumn: t("endpoints.data.hiddenColumn"),
      lead: t("endpoints.data.compareLead"),
      same: t("endpoints.data.same"),
      loading: t("app.loading"),
      error: t("app.error.generic"),
      leftRefused: t("endpoints.data.spaceRefused"),
      previous: t("entityGrid.previous"),
      next: t("entityGrid.next"),
      page: t("entityGrid.page"),
    }),
    [t],
  );
  const labels = useMemo(() => gridLabels(t), [t]);
  const renderers = useMemo(() => linkRenderers(links, t("endpoints.data.openLink")), [links, t]);
  const panelFields = useMemo(() => shape.order.map((attr) => ({ value: attr, label: shape.labels[attr] ?? attr })), [shape]);
  const filterTitle = (column: FilterColumn) => filterTitleOf(column, shape.labels, t);

  const signIn = useSignIn();
  const notReady = schemaState({ index, schema, widget }, Boolean(type && config), t, signIn ? () => signIn() : undefined);
  if (notReady || !type || !config) {
    return notReady ?? <></>;
  }

  return (
    <div {...windowProps(full, t("endpoints.data.title"))}>
      {refused ? null : (
      <>
      <div className="flex flex-wrap items-center gap-1 border-b border-border pb-2" data-testid="grid-toolbar">
        <DataToolbar
          {...tools.props}
          filterTitle={filterTitle}
          attributes={shape.order}
          enums={enums}
          extras={{ ...extras, hidden: hiddenNow }}
          fields={panelFields}
          sortKeys={sortKeys}
          onSortKeys={(next) => {
            setView({ filterText: opened.filterText, sort: next[0] ?? null });
            tools.setThenBy(next.slice(1));
          }}
        />
        {/* Two doors on the same page of entities: what this endpoint's filter and projection leave
            out is marked on the space's side (T-1435). The CSV is the whole answer of this query as
            one table, built by the endpoint under its own limits and read with the person's own
            session, as the rows are (EP-08, EP-44). */}
        <div className="ml-auto">
          <DataActions
            space={space}
            comparing={comparing}
            onCompare={() => setComparing((each) => !each)}
            full={full}
            onFull={() => setFull((each) => !each)}
            csv={csv ? csvHref(origin, slug, type.name, allAsked, sortText) : undefined}
            csvRefused={csvRefused}
          />
        </div>
      </div>
      <div className="flex flex-wrap items-center gap-2 empty:hidden">
        {/* The conditions in force stay in sight, as on the space's grid (T-6161). */}
        <FilterChips columns={tools.props.filterColumns} titleOf={filterTitle} value={group} onChange={tools.props.onFilters} />
        <DefaultFilter
          filter={classView?.filter}
          defaults={defaults}
          opened={opened}
          group={group}
          filtered={filtered}
          labels={shape.labels}
          onClear={() => {
            setDropped(true);
            setView({ filterText: null, sort: opened.sort });
            tools.props.onFilters(EMPTY_GROUP);
          }}
          onRestore={() => {
            setDropped(false);
            setView(defaults && { filterText: defaults.filterText, sort: defaults.sort });
            tools.props.onFilters(groupOf(defaults?.filters));
          }}
        />
      </div>
      </>
      )}

      {comparing && space ? (
        <EntityCompare
          key={`${slug}-${type.name}-${space}`}
          config={config}
          left={sourceFor({ kind: "space", space }, originTransport(), i18n.language)}
          right={sourceFor({ kind: "endpoint", slug }, originTransport(), i18n.language)}
          labels={compareLabels}
          gridLabels={labels}
        />
      ) : (
        <PortalEntityGrid
          key={`${slug}-${type.name}`}
          project={project}
          config={config}
          // The type is picked in the grid's Type column, like on a space (owner 2026-11-01).
          typeChoice={{
            value: type.name,
            label: t("endpoints.data.type"),
            options: types.map((each) => ({ value: each.name, label: classLabel(model.data, each.name, i18n.language) })),
            onChange: (next) => {
              setChosen(next);
              // Another type has other columns: its query, order and tools start empty.
              setView({ filterText: null, sort: null });
              tools.reset();
              setAsked(undefined);
            },
          }}
          source={source}
          enums={enums}
          relations={relations}
          renderers={renderers}
          view={gridView}
          onGridState={onGridState}
          onQuery={onQuery}
          onRefused={setRefused}
          hidden={hiddenNow}
          onHide={onHide}
          thenBy={tools.thenBy}
          groupBy={tools.groupBy}
          groupHeader={(each) => <GroupCount scope={`endpoint/${slug}`} source={countSource} type={type.name} q={allAsked} path={each.path} />}
          rowTone={tools.rowTone}
          rowSize={tools.rowSize}
          query={tools.query}
         
          queryBox={tools.showQuery}
          filterRow={!phone}
          onFilterColumns={tools.onFilterColumns}
          {...openRowProps(onOpenRow)}
        />
      )}
    </div>
  );
}
