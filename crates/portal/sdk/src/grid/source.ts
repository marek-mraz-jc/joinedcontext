/**
 * Where an entity grid reads and writes (UI-71, SDK-29): an interface, two built-in sources over
 * the SDK transport, and an in-memory one for tests and examples.
 *
 * A source holds no credential: the transport carries the person's session and CSRF header as
 * every other SDK call does, so an Endpoint's Policy decides every read and write (EP-55). A config
 * names a slug or a space, never a URL, so these two path shapes are the only ones a grid reaches.
 */
import type { Transport } from "../sdk/transport";
import { queryString } from "../sdk/query";
import type { GeoQuery } from "./geoarea";
import type { Geometry } from "../geo/validate";
import type { RichRow } from "./model";
import { toRichRow } from "./model";
import { changesOf } from "./changes";
import type { EntityChange } from "./changes";

export interface GridQuery {
  type: string;
  q?: string;
  attrs?: string[];
  idPattern?: string;
  scopeQ?: string;
  /** Exactly these entities, for a side-by-side read of one page (T-1435). */
  ids?: string[];
  /** The area drawn on the map, as a geo query the endpoint answers (UI-72). */
  area?: GeoQuery;
  /** The order of the whole answer, as CIM 009 4.23 writes it: `attr` or `attr;desc`, keys by commas. */
  orderBy?: string;
}

/** An entity as the map draws it: where it is and what it is called, nothing else (T-6345). */
export interface MapPoint {
  id: string;
  name?: string;
  geometry: Geometry;
}

export interface PointPage {
  points: MapPoint[];
  /** How many entities the page held, with a geometry or not: a full page means ask for the next. */
  read: number;
  total?: number;
}

/** A keyValues entity as a point: a name as text (or in the asked language) and a geometry. */
export function pointOf(entity: unknown, attr: string): MapPoint | null {
  if (typeof entity !== "object" || entity === null) return null;
  const raw = entity as Record<string, unknown>;
  const geometry = raw[attr] as { type?: unknown; coordinates?: unknown } | undefined;
  if (typeof raw.id !== "string" || !geometry || typeof geometry.type !== "string" || geometry.coordinates === undefined) return null;
  const name = typeof raw.name === "string" && raw.name !== "" ? raw.name : undefined;
  return { id: raw.id, name, geometry: geometry as Geometry };
}

export interface GridPage {
  rows: RichRow[];
  /** Known only where the source can count; the grid then pages by whether a page came back full. */
  total?: number;
}

export interface HistoryPoint {
  at: string;
  value: unknown;
  /** When a later version replaced this one; only in a `versions` answer (DM-114). */
  supersededAt?: string;
  /** Who wrote this version, through what and why (ADR-N-054 §2.3); only in a `versions` answer. */
  provenance?: Provenance;
}

/** The provenance the broker keeps per version: the fields a person reads, as the broker sent them. */
export interface Provenance {
  channel?: string;
  actor?: string;
  operation?: string;
  reason?: string;
  writtenAt?: string;
  /** The writer's organization, as the gateway stamps it from the verified token (T-5362). */
  organization?: string;
  /** What wrote it: `person`, `pipeline`, `serviceAccount` or `agent` (T-5362). */
  actorKind?: string;
}

export interface HistoryWindow {
  from?: string;
  to?: string;
  lastN?: number;
  /** Every version of each measurement (`versions=all`): the gateway answers 403 without `readVersions`. */
  versions?: boolean;
}

/** Why an attribute's decisions are what they are, as the Endpoint's `access/attributes` says (API/02 §7b). */
export type AccessReason = "grant" | "not_in_grant" | "prohibited" | "no_grant" | "formula";

/** The caller's own decisions on one attribute: the gateway's, never computed in the browser. */
export interface AttributeAccess {
  name: string;
  read: boolean;
  write: boolean;
  delete: boolean;
  reason: AccessReason;
}

const REASONS: readonly AccessReason[] = ["grant", "not_in_grant", "prohibited", "no_grant", "formula"];

/**
 * The rows of an `access/attributes` answer the grid can trust: a row without a name is dropped,
 * a flag that is not `true` is a refusal and an unknown reason reads as no grant (deny by default).
 */
export function accessOf(body: unknown): AttributeAccess[] {
  const rows = (body as { attributes?: unknown } | null)?.attributes;
  if (!Array.isArray(rows)) throw new SourceError(502, "the access answer is not a list of attributes");
  return rows.flatMap((raw) => {
    const row = (raw ?? {}) as Record<string, unknown>;
    if (typeof row.name !== "string" || row.name === "") return [];
    const reason = REASONS.find((known) => known === row.reason) ?? "no_grant";
    return [{ name: row.name, read: row.read === true, write: row.write === true, delete: row.delete === true, reason }];
  });
}

/** What a grid can do follows what its source offers: an absent capability switches its feature off. */
export interface EntitySource {
  /** The source orders the whole answer by `GridQuery.orderBy`; without it the grid sorts a page. */
  orders?: boolean;
  /** The Endpoint base (`/api/endpoint/{slug}`) whose OGC vector tiles draw the map past a count (T-5195). */
  tilesAt?: string;
  /** `count: false` asks for rows alone: the broker then skips counting the whole match (T-6161). */
  query(q: GridQuery, page: { offset: number; limit: number; count?: boolean }): Promise<GridPage>;
  get(id: string): Promise<RichRow | null>;
  /**
   * One page of the map's points: id, name and the geometry of `attr` alone, as plain values
   * (`attrs` + keyValues), so a map of a whole type is not a read of every attribute (T-6345).
   */
  points?(q: GridQuery, attr: string, page: { offset: number; limit: number; count?: boolean }): Promise<PointPage>;
  history?(id: string, attr: string, window: HistoryWindow): Promise<HistoryPoint[]>;
  /** Who changed `attrs` of one entity and when, newest first (T-6333): a 403 without `readVersions`. */
  changes?(id: string, attrs: string[]): Promise<EntityChange[]>;
  /** `reason` is why the person writes, sent as `JC-Reason` (DM-113). */
  patch?(id: string, attrs: Record<string, unknown>, reason?: string): Promise<void>;
  remove?(id: string): Promise<void>;
  /** The caller's decisions per attribute of `type`, where the source can ask (an Endpoint). */
  access?(type: string): Promise<AttributeAccess[]>;
}

export class SourceError extends Error {
  readonly status: number;
  /** The attribute the refusal is about, where the problem names one (`slot`, DM-70). */
  readonly slot?: string;
  /** Seconds the server asked to wait before asking again (`Retry-After` on a 429 or 503). */
  readonly retryAfter?: number;
  /** The problem document's own `title` and `detail`, for a grid that words the refusal itself. */
  readonly problem?: { title?: string; detail?: string };
  /**
   * The source refused to order the answer (a 400 BadRequestData on a query with `orderBy`): a
   * broker that also reads other brokers never orders (CIM 009 5.7.2.4, 4.23.1).
   */
  readonly orderRefused?: boolean;
  constructor(
    status: number,
    message: string,
    slot?: string,
    extra?: { retryAfter?: number; problem?: { title?: string; detail?: string }; orderRefused?: boolean },
  ) {
    super(message);
    this.name = "SourceError";
    this.status = status;
    this.slot = slot;
    this.retryAfter = extra?.retryAfter;
    this.problem = extra?.problem;
    this.orderRefused = extra?.orderRefused;
  }
}

/** `Retry-After` in seconds: delta-seconds or an HTTP date (RFC 9110 10.2.3); none when absent or unreadable. */
function retryAfterOf(headers: Record<string, string> | undefined): number | undefined {
  const value = headers?.["retry-after"]?.trim();
  if (!value) return undefined;
  if (/^\d+$/.test(value)) return Number(value);
  const at = Date.parse(value);
  return Number.isNaN(at) ? undefined : Math.max(0, Math.ceil((at - Date.now()) / 1000));
}

/**
 * `NGSILD-Results-Count`, when the answer carries one. An answer the gateway narrowed carries
 * none on purpose (R22): the count would say how many entities were withheld, so the grid pages
 * by whether a page came back full instead of showing a total it was not told.
 */
function resultsCount(headers: Record<string, string> | undefined): number | undefined {
  const raw = headers?.["ngsild-results-count"]?.trim();
  if (!raw) {
    return undefined;
  }
  const count = Number(raw);
  return Number.isInteger(count) && count >= 0 ? count : undefined;
}

/**
 * The URL a query of this many ids would carry, split so no query string grows past `maxChars`.
 * A gateway, a proxy and nginx all have a line length of their own; 4 000 characters is under
 * every default, and a page of 25 urns fits in one read (T-1435).
 */
export function idChunks(ids: string[], maxChars = 3_500): string[][] {
  const chunks: string[][] = [];
  let current: string[] = [];
  let length = 0;
  for (const id of ids) {
    const cost = encodeURIComponent(id).length + 3;
    if (current.length > 0 && length + cost > maxChars) {
      chunks.push(current);
      current = [];
      length = 0;
    }
    current.push(id);
    length += cost;
  }
  if (current.length > 0) {
    chunks.push(current);
  }
  return chunks;
}

/** The attribute a problem document names as the one refused, where it names one. */
function slotOf(body: unknown): string | undefined {
  const slot = typeof body === "object" && body !== null ? (body as { slot?: unknown }).slot : undefined;
  return typeof slot === "string" && slot !== "" ? slot : undefined;
}

/** The problem document's own sentence, or `fallback`. */
function detailOf(body: unknown, fallback: string): string {
  if (typeof body === "object" && body !== null) {
    const problem = body as { detail?: unknown; title?: unknown };
    if (typeof problem.detail === "string" && problem.detail) return problem.detail;
    if (typeof problem.title === "string" && problem.title) return problem.title;
  }
  return fallback;
}

/** The text members of a problem document, where the answer is one. */
function problemOf(body: unknown): { type?: string; title?: string; detail?: string } {
  if (typeof body !== "object" || body === null) return {};
  const raw = body as Record<string, unknown>;
  const text = (key: string) => (typeof raw[key] === "string" && raw[key] !== "" ? (raw[key] as string) : undefined);
  return { type: text("type"), title: text("title"), detail: text("detail") };
}

function entitiesQuery(q: GridQuery, offset: number, limit: number, count = true): string {
  // id and type come with every entity; asking for them as attributes is a 400.
  const attrs = (q.attrs ?? []).filter((attr) => attr !== "id" && attr !== "type");
  return queryString({
    type: q.type,
    options: "sysAttrs",
    // How many entities the query matches, for the footer: the broker counts, the grid never does
    // (a page cannot be counted into a total). A count scans the whole match, so a caller that
    // already has it, or only asks whether the read is allowed, leaves it out (T-6161).
    count: count ? "true" : undefined,
    limit: String(limit),
    offset: offset > 0 ? String(offset) : undefined,
    q: q.q || undefined,
    attrs: attrs.length > 0 ? attrs.join(",") : undefined,
    idPattern: q.idPattern || undefined,
    scopeQ: q.scopeQ || undefined,
    id: q.ids && q.ids.length > 0 ? q.ids.join(",") : undefined,
    // The four of a geo query travel together or not at all: `georel` without `coordinates` is a
    // 400, and `coordinates` without `georel` is silently ignored, which reads as an empty area.
    georel: q.area?.georel,
    geometry: q.area?.geometry,
    coordinates: q.area?.coordinates,
    geoproperty: q.area?.geoproperty,
    orderBy: q.orderBy || undefined,
  });
}

/** The most versions one changes read asks for per attribute (EP-07, as history's MAX_POINTS). */
const MAX_VERSIONS = 1000;
/** NGSI-LD `limit` caps at 1000 on Antares (and most brokers). */
const BROKER_PAGE = 1000;

function historyQuery(attr: string, window: HistoryWindow): string {
  const { from, to, lastN, versions } = window;
  return queryString({
    attrs: attr,
    versions: versions ? "all" : undefined,
    timerel: from && to ? "between" : from ? "after" : to ? "before" : undefined,
    timeAt: from ?? to,
    endTimeAt: from && to ? to : undefined,
    lastN: lastN !== undefined ? String(lastN) : undefined,
    // Plain [value, time] pairs: 1.3-2.4 s against 4.8-10.8 s for a week of normalized instances on
    // dev (owner 2026-11-01, T-6346). Versions keep the full form, which carries the provenance.
    format: versions ? undefined : "temporalValues",
  });
}

/** The text fields of a version's provenance; anything else the broker adds is not shown. */
function provenanceOf(raw: unknown): Provenance | undefined {
  if (typeof raw !== "object" || raw === null) return undefined;
  const kept: Provenance = {};
  for (const key of ["channel", "actor", "operation", "reason", "writtenAt", "organization", "actorKind"] as const) {
    const value = (raw as Record<string, unknown>)[key];
    if (typeof value === "string") kept[key] = value;
  }
  return kept;
}

/** The instances of one attribute in a temporal answer: `[{value, observedAt}]`, or `{values: [[v, t]]}`. */
export function historyOf(body: unknown, attr: string): HistoryPoint[] {
  const attribute = typeof body === "object" && body !== null ? (body as Record<string, unknown>)[attr] : undefined;
  if (Array.isArray(attribute)) {
    return attribute.flatMap((instance) => {
      const { observedAt: at, value, supersededAt, provenance } = (instance ?? {}) as Record<string, unknown>;
      if (typeof at !== "string") return [];
      const point: HistoryPoint = { at, value };
      if (typeof supersededAt === "string") point.supersededAt = supersededAt;
      const kept = provenanceOf(provenance);
      if (kept) point.provenance = kept;
      return [point];
    });
  }
  const values = (attribute as { values?: unknown } | undefined)?.values;
  if (Array.isArray(values)) {
    return values.flatMap((pair) =>
      Array.isArray(pair) && typeof pair[1] === "string" ? [{ at: pair[1], value: pair[0] }] : [],
    );
  }
  return [];
}

/**
 * One NGSI-LD API under `base`. `hidden` is what a 404 becomes on a query or a write: a Context Space surface says the same whether the space is missing or not granted.
 */
function ngsiSource(base: string, transport: Transport, language: string, hidden?: string, tilesAt?: string): EntitySource {
  const refuse = (status: number, body: unknown, fallback: string, headers?: Record<string, string>): never => {
    if (status === 404 && hidden) throw new SourceError(404, hidden);
    throw new SourceError(status, detailOf(body, fallback), slotOf(body), { retryAfter: retryAfterOf(headers) });
  };
  const ok = (status: number) => status >= 200 && status < 300;
  const entity = (id: string) => `${base}/entities/${encodeURIComponent(id)}`;

  const src: EntitySource = {
    orders: true,
    tilesAt,
    async query(q, page) {
      // A broker answers at most BROKER_PAGE entities a request: a bigger page is read in turns.
      if (page.limit > BROKER_PAGE) {
        const first = await src.query(q, { ...page, limit: BROKER_PAGE });
        const rows = [...first.rows];
        let last = first.rows.length;
        while (rows.length < page.limit && last === BROKER_PAGE) {
          const next = await src.query(q, { offset: page.offset + rows.length, limit: Math.min(BROKER_PAGE, page.limit - rows.length), count: false });
          rows.push(...next.rows);
          last = next.rows.length;
        }
        return { rows, total: first.total };
      }
      const { status, body, headers } = await transport({
        method: "GET",
        path: `${base}/entities?${entitiesQuery(q, page.offset, page.limit, page.count ?? true)}`,
      });
      if (status === 400) {
        const { type, title, detail } = problemOf(body);
        throw new SourceError(400, detail ?? title ?? "the query was refused", slotOf(body), {
          retryAfter: retryAfterOf(headers),
          problem: { title, detail },
          orderRefused: Boolean(q.orderBy) && (type ?? title ?? "").endsWith("BadRequestData"),
        });
      }
      if (!ok(status)) refuse(status, body, "the read was refused", headers);
      if (!Array.isArray(body)) throw new SourceError(status, "the answer is not a list of entities");
      return {
        rows: body.map((item) => toRichRow(item as Record<string, unknown>, language)),
        total: resultsCount(headers),
      };
    },
    async points(q, attr, page) {
      const query = queryString({
        type: q.type,
        attrs: attr === "name" ? attr : `name,${attr}`,
        options: "keyValues",
        // A LanguageProperty name comes as one text in the person's language.
        lang: language,
        count: page.count ? "true" : undefined,
        limit: String(Math.min(page.limit, BROKER_PAGE)),
        offset: page.offset > 0 ? String(page.offset) : undefined,
        q: q.q || undefined,
        idPattern: q.idPattern || undefined,
        scopeQ: q.scopeQ || undefined,
        georel: q.area?.georel,
        geometry: q.area?.geometry,
        coordinates: q.area?.coordinates,
        geoproperty: q.area?.geoproperty,
      });
      const { status, body, headers } = await transport({ method: "GET", path: `${base}/entities?${query}` });
      if (!ok(status)) refuse(status, body, "the read was refused", headers);
      if (!Array.isArray(body)) throw new SourceError(status, "the answer is not a list of entities");
      return {
        points: body.flatMap((item) => pointOf(item, attr) ?? []),
        read: body.length,
        total: resultsCount(headers),
      };
    },
    async get(id) {
      const { status, body } = await transport({ method: "GET", path: `${entity(id)}?options=sysAttrs` });
      if (status === 404) return null;
      if (!ok(status)) refuse(status, body, "the read was refused");
      return toRichRow(body as Record<string, unknown>, language);
    },
    async history(id, attr, window) {
      const { status, body } = await transport({
        method: "GET",
        path: `${base}/temporal/entities/${encodeURIComponent(id)}?${historyQuery(attr, window)}`,
      });
      // A 404 here is "nothing recorded here": an Endpoint answers it both for an entity with no
      // instances and for one its grants do not reach (EP-07, T-2972), and a space answers it for
      // an attribute with no instance in the window of a row the person already read (T-5597).
      // Neither reveals a value; a refusal the server names (403) stays a refusal.
      if (status === 404) return [];
      if (!ok(status)) refuse(status, body, "the history was refused");
      return historyOf(body, attr);
    },
    async changes(id, attrs) {
      const query = queryString({ attrs: attrs.join(","), versions: "all", lastN: String(MAX_VERSIONS) });
      const { status, body } = await transport({ method: "GET", path: `${base}/temporal/entities/${encodeURIComponent(id)}?${query}` });
      // Nothing kept of any of `attrs` is the broker's 404, as for history above.
      if (status === 404) return [];
      if (!ok(status)) refuse(status, body, "the changes were refused");
      return changesOf(body);
    },
    async patch(id, attrs, reason) {
      const { status, body } = await transport({ method: "PATCH", path: `${entity(id)}/attrs`, body: attrs, reason });
      if (!ok(status)) refuse(status, body, "the write was refused");
    },
    async remove(id) {
      const { status, body } = await transport({ method: "DELETE", path: entity(id) });
      if (!ok(status)) refuse(status, body, "the removal was refused");
    },
  };
  return src;
}

/** What one Endpoint answers, by its slug (EP-55). */
export function endpointSource(slug: string, transport: Transport, language = "en"): EntitySource {
  const endpoint = `/api/endpoint/${encodeURIComponent(slug)}`;
  return {
    ...ngsiSource(`${endpoint}/ngsi-ld/v1`, transport, language, undefined, endpoint),
    // Asked with the person's own session, so each answer is the decision their request gets.
    async access(type) {
      const { status, body } = await transport({ method: "GET", path: `${endpoint}/access/attributes?type=${encodeURIComponent(type)}` });
      if (status < 200 || status >= 300) throw new SourceError(status, detailOf(body, "the access was not read"));
      return accessOf(body);
    },
  };
}

/** A whole Context Space surface, by its segment (SP-06); a 404 never says whether the space exists. */
export function spaceSource(space: string, transport: Transport, language = "en"): EntitySource {
  return ngsiSource(`/cs/${encodeURIComponent(space)}/ngsi-ld/v1`, transport, language, "no grant on this space");
}

/** Entities in memory, for tests and examples: type and idPattern filter, paging, a total. */
export function fixtureSource(entities: Record<string, unknown>[], language = "en"): EntitySource {
  const rows = entities.map((entity) => toRichRow(entity, language));
  return {
    orders: true,
    async query(q, page) {
      const pattern = q.idPattern ? new RegExp(q.idPattern) : null;
      const matching = rows.filter(
        (row) =>
          row.type === q.type &&
          (!pattern || pattern.test(row.id)) &&
          (!q.ids || q.ids.includes(row.id)),
      );
      const ordered = q.orderBy ? orderRows(matching, q.orderBy) : matching;
      return { rows: ordered.slice(page.offset, page.offset + page.limit), total: matching.length };
    },
    async get(id) {
      return rows.find((row) => row.id === id) ?? null;
    },
  };
}

/**
 * `rows` in the order `orderBy` names (`attr`, `attr;desc`, several keys by commas), as the broker
 * orders them (CIM 009 4.23.1): numbers by value, other values by their text, an entity without
 * the attribute after every one that has it, either way; a later key orders what an earlier one
 * leaves equal.
 */
export function orderRows(rows: RichRow[], orderBy: string): RichRow[] {
  const keys = orderBy.split(",").map((part) => {
    const [attr, dir] = part.split(";");
    return { attr, factor: dir === "desc" ? -1 : 1 };
  });
  const valueOf = (row: RichRow, attr: string): unknown => {
    if (attr === "id") return row.id;
    const cell = row.cells[attr];
    const one = Array.isArray(cell) ? cell[0] : cell;
    return one?.kind === "relationship" ? one.object : one?.value;
  };
  const compare = (attr: string, factor: number, a: RichRow, b: RichRow): number => {
    const x = valueOf(a, attr);
    const y = valueOf(b, attr);
    const missingX = x === undefined || x === null;
    const missingY = y === undefined || y === null;
    if (missingX || missingY) return Number(missingX) - Number(missingY);
    if (typeof x === "number" && typeof y === "number") return factor * (x - y);
    return factor * String(x).localeCompare(String(y));
  };
  return [...rows].sort((a, b) => {
    for (const { attr, factor } of keys) {
      const by = compare(attr, factor, a, b);
      if (by !== 0) return by;
    }
    return 0;
  });
}

/** The built-in source a config names. */
export function sourceFor(
  config: { kind: "endpoint"; slug: string } | { kind: "space"; space: string },
  transport: Transport,
  language?: string,
): EntitySource {
  return config.kind === "endpoint"
    ? endpointSource(config.slug, transport, language)
    : spaceSource(config.space, transport, language);
}
