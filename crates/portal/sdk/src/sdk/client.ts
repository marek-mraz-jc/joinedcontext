import type { Cell, Geo, Row } from "../ngsi";
import { cell, isLanguageMap, isRelationshipObject, toRow } from "../ngsi";
import type { WriteValue } from "../ngsi";
import type { Schema } from "../write";
import type { AccessDocument } from "./access";
import { parseAccess } from "./access";
import type { JcConfig, JcEndpoint, JcUser } from "./config";
import { readConfig } from "./config";
import { queryString, randomId } from "./query";
import type { Transport } from "./transport";
import { transportFor } from "./transport";

export class ProblemError extends Error {
  readonly status: number;
  readonly title: string;
  readonly detail?: string;
  readonly type?: string;
  readonly file?: string;
  readonly line?: number;

  constructor(status: number, body: unknown) {
    const b = (typeof body === "object" && body !== null ? body : {}) as {
      title?: unknown;
      detail?: unknown;
      type?: unknown;
      error?: { message?: unknown; file?: unknown; line?: unknown };
    };

    const runtimeMsg = typeof b.error?.message === "string" ? b.error.message : undefined;
    const title = typeof b.title === "string" ? b.title : (runtimeMsg ?? (status ? `HTTP ${status}` : "Network Error"));
    const detail = typeof b.detail === "string" ? b.detail : runtimeMsg;
    const message = detail ?? title;

    super(message);
    this.name = "ProblemError";
    this.status = status;
    this.title = title;
    this.detail = detail;
    this.type = typeof b.type === "string" ? b.type : undefined;
    this.file = typeof b.error?.file === "string" ? b.error.file : undefined;
    this.line = typeof b.error?.line === "number" ? b.error.line : undefined;
  }
}

/** A platform service the App's layers switched off, or whose quota is used up (SDK-43, API/06 §3). */
export class ServiceRefusedError extends ProblemError {
  readonly service: "files" | "email" | "jobs" | "ai";
  readonly layer?: "organization" | "project" | "app";
  readonly quota?: string;
  readonly resetAt?: string;

  constructor(status: number, body: unknown, service: ServiceRefusedError["service"]) {
    super(status, body);
    const b = (typeof body === "object" && body !== null ? body : {}) as Record<string, unknown>;
    this.name = "ServiceRefusedError";
    this.service = service;
    const layer = b.layer;
    this.layer = layer === "organization" || layer === "project" || layer === "app" ? layer : undefined;
    this.quota = typeof b.quota === "string" ? b.quota : undefined;
    this.resetAt = typeof b.resetAt === "string" ? b.resetAt : undefined;
  }
}

/** The error of a service's refusal: a switched-off service or a used-up quota is a `ServiceRefusedError`. */
function serviceError(status: number, body: unknown, service: ServiceRefusedError["service"]): ProblemError {
  const type = (typeof body === "object" && body !== null ? (body as { type?: unknown }).type : undefined) ?? "";
  return typeof type === "string" && (type.endsWith("/service-off") || type.endsWith("/quota"))
    ? new ServiceRefusedError(status, body, service)
    : new ProblemError(status, body);
}

/** A message to people of the organization, by person id or `"me"` (AP-168). */
export interface Email {
  to: string[] | "me";
  subject: string;
  text: string;
  html?: string;
}

/** An object the App keeps under its own prefix (AP-170). */
export interface FileInfo {
  path: string;
  size: number;
  contentType: string;
  modifiedAt: string;
}

/** One turn of a conversation with the platform's model (AP-169). */
export interface AiMessage {
  role: "system" | "user" | "assistant";
  content: string;
}

/** A completion: the answer, the JSON value when a schema was asked for, and the tokens it took. */
export interface Completion {
  text: string;
  json?: unknown;
  tokens: { in: number; out: number };
}

/** A scheduled job of the App and its runs (AP-154, AP-162); `nextRun` is absent for a schedule that names no minute within a year. */
export interface JobStatus {
  name: string;
  schedule: string;
  nextRun?: string;
  lastRun?: { at: string; ok: boolean; message?: string };
}

export interface Query {
  /** The endpoint to read, by the name the served configuration lists; only needed for a type more than one endpoint serves. */
  endpoint?: string;
  attrs?: string[];
  q?: string;
  /** A regular expression the entity id matches (NGSI-LD `idPattern`). */
  idPattern?: string;
  georel?: string;
  geometry?: string;
  coordinates?: string;
  limit?: number;
  offset?: number;
}

export interface TemporalQuery {
  endpoint?: string;
  /** The entities whose history is asked, by id (NGSI-LD `id`): one station's week, not every station's. */
  id?: string[];
  attrs?: string[];
  q?: string;
  timerel: "before" | "after" | "between";
  timeAt: string;
  endTimeAt?: string;
  lastN?: number;
  limit?: number;
}

export interface TemporalPoint {
  value: Cell;
  observedAt: string;
}

export interface TemporalRow {
  id: string;
  type: string;
  series: Record<string, TemporalPoint[]>;
}

/** One read of a collection's Features (OGC API - Features Parts 1 and 3, EP-29). */
export interface FeatureQuery {
  /** The endpoint to read, by name; only needed for a type more than one endpoint serves. */
  endpoint?: string;
  /** `[minLon, minLat, maxLon, maxLat]` in WGS 84. */
  bbox?: [number, number, number, number];
  /** An RFC 3339 instant or interval (`a/b`, either end `..`). */
  datetime?: string;
  /** A CQL2 text filter (Part 3), e.g. `speed > 50 AND name LIKE 'A%'`. */
  filter?: string;
  /** The properties each Feature keeps; all when absent. */
  properties?: string[];
  /** Sort keys, `-` before a name for descending (Part 5 `sortby`). */
  sortby?: string[];
  /** 1 to 1000, 100 when absent. */
  limit?: number;
}

/** A GeoJSON Feature: the Entity's URN, its geometry, its other attributes as plain values. */
export interface Feature {
  type: "Feature";
  id: string;
  geometry: Geo | null;
  properties: Record<string, unknown>;
}

/** A page of Features as `/items` answers it, ready for a MapLibre GeoJSON source. */
export interface FeatureCollection {
  type: "FeatureCollection";
  features: Feature[];
  numberMatched?: number;
  numberReturned?: number;
  links?: { href: string; rel: string; type?: string; title?: string }[];
}

/** One collection the endpoint serves: an Entity type. */
export interface FeatureCollectionInfo {
  id: string;
  title?: string;
  description?: string;
  extent?: { spatial?: { bbox: number[][] }; temporal?: { interval: (string | null)[][] } };
}

export interface DataClient {
  readonly config: JcConfig;
  entities: {
    list<T extends Row = Row>(type: string, query?: Query): Promise<T[]>;
    all<T extends Row = Row>(type: string, query?: Query): Promise<T[]>;
    get<T extends Row = Row>(id: string, attrs?: string[], options?: EndpointOption): Promise<T>;
    /**
     * Creates one entity through the type's endpoint. `localId` is minted into
     * `urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}`; a whole `urn:ngsi-ld:{Type}:…` id is kept
     * as given (ADR-N-041). Answers the id.
     */
    create(type: string, attrs: Record<string, WriteValue>, localId?: string, options?: EndpointOption): Promise<string>;
    update(id: string, patch: Record<string, WriteValue>, options?: EndpointOption): Promise<void>;
    /** Every language of one LanguageProperty, which a row reduces to one; `{}` when it has none. */
    languages(id: string, attr: string, options?: EndpointOption): Promise<Record<string, string>>;
    remove(id: string, options?: EndpointOption): Promise<void>;
  };
  temporal: { list(type: string, query: TemporalQuery): Promise<TemporalRow[]> };
  /** The endpoint's OGC API - Features surface: the same Entities its view and Policy admit (EP-29). */
  features: {
    collections(options?: EndpointOption): Promise<FeatureCollectionInfo[]>;
    items(collection: string, query?: FeatureQuery): Promise<FeatureCollection>;
    get(collection: string, id: string, options?: EndpointOption): Promise<Feature>;
  };
  /** Every endpoint's schemas merged, or one endpoint's by name. */
  schema(endpoint?: string): Promise<Schema>;
  /** One endpoint's grant document, the primary's by default. */
  access(endpoint?: string): Promise<AccessDocument>;
  me(): JcUser | null;
  entityId(type: string, localId: string, options?: EndpointOption): string;
}

/** Names one endpoint of an application that reads several (SDK-02). */
export interface EndpointOption {
  endpoint?: string;
}

export interface Client extends DataClient {
  /** The client's own way to the endpoint, for a grid source: `endpointSource(slug, client.transport)`. */
  readonly transport: Transport;
  functions: { call<T = unknown>(name: string, body?: unknown): Promise<T> };
  /** The `email` service: the platform holds the relay, the App names people (SDK-41, API/06 §4). */
  email: { send(message: Email): Promise<{ id: string }> };
  /**
   * The `files` service: objects under the App's own prefix, each at most 25 MiB, all within its
   * `filesMiB` (SDK-41, API/06 §4). Over the browser's own host; a key is relative and has no
   * `.` or `..` segment.
   */
  files: {
    put(path: string, body: Blob | ArrayBuffer | string, opts?: { contentType?: string }): Promise<FileInfo>;
    get(path: string): Promise<Blob>;
    list(prefix?: string): Promise<FileInfo[]>;
    remove(path: string): Promise<void>;
    /** A PUT URL is signed for `size` bytes: the store refuses a body of any other length. */
    url(path: string, opts?: { method?: "GET" } | { method: "PUT"; size: number }): Promise<{ url: string; expiresAt: string }>;
  };
  /**
   * The `ai` service: a completion from the model the platform chooses, within the App's
   * `aiTokensPerDay` (SDK-41, API/06 §4). With `schema`, the answer is one JSON value of it.
   */
  ai: { complete(request: { messages: AiMessage[]; maxTokens?: number; schema?: object }): Promise<Completion> };
  /** The `jobs` service: the App's schedules with their next and last run (SDK-41, API/06 §4). */
  jobs: { list(): Promise<JobStatus[]> };
}

const FILES = "/api/services/files";

/**
 * A key's route, each segment encoded so a `?`, `#` or `%` in a name stays in the key. An empty,
 * `.` or `..` segment is refused here: fetch resolves dot segments before the request leaves, so
 * `../../endpoint/…` would reach another route of the App's host with the person's session.
 */
function filePath(path: string): string {
  const segments = path.split("/");
  if (segments.some((segment) => segment === "" || segment === "." || segment === "..")) {
    throw new ProblemError(0, {
      title: "Refused file key",
      detail: `The file key '${path}' has an empty, '.' or '..' segment: a key is relative, such as reports/2026-10.csv.`,
    });
  }
  return `${FILES}/${segments.map(encodeURIComponent).join("/")}`;
}

export const FUNCTION_NAME = /^[a-z][a-z0-9-]{0,39}$/;
const TYPE_RE = /^[A-Za-z][A-Za-z0-9_]*$/;
const LOCAL_ID_RE = /^[A-Za-z0-9._~-]{1,128}$/;
/** RFC 8141's namespace-specific string, at most 256 characters (ADR-N-041, PF-43). */
const URN_ID_RE = /^(?=.{1,256}$)(?:[A-Za-z0-9._~!$&'()*+,;=:@-]|%[0-9A-Fa-f]{2})(?:[A-Za-z0-9._~!$&'()*+,;=:@/-]|%[0-9A-Fa-f]{2})*$/;

/** The type an NGSI-LD URN names: `urn:ngsi-ld:{Type}:…`, or nothing. */
function typeOfUrn(id: string): string | undefined {
  return id.startsWith("urn:ngsi-ld:") ? id.split(":")[2] || undefined : undefined;
}

export function isEndpointPath(slug: string, path: string): boolean {
  if (path.includes("#")) {
    return false;
  }
  const prefix = `/api/endpoint/${encodeURIComponent(slug)}/`;
  if (!path.startsWith(prefix)) {
    return false;
  }
  const rest = path.slice(prefix.length);
  const [pathname, ...query] = rest.split("?");
  if (query.length > 1) return false;
  // Traversal is a matter of the path; a query value may hold `..`, as an open OGC interval does.
  if (pathname.includes("..") || pathname.includes("//") || pathname.includes("\\")) {
    return false;
  }

  const valid =
    /^(ngsi-ld\/v1\/entities(\/[^/]+(\/attrs)?)?|ngsi-ld\/v1\/temporal\/entities|ogc\/features\/collections(\/[^/]+\/items(\/[^/]+)?)?|schema\/index\.json|schema\/v\d+\/json-schema|access)$/;
  return valid.test(pathname);
}

function checkEndpointPath(slug: string, path: string): void {
  if (!isEndpointPath(slug, path)) {
    throw new ProblemError(0, { title: `Refused unauthorized endpoint path: ${path}` });
  }
}

function isGeoJsonGeometry(value: unknown): value is { type: string; coordinates: unknown } {
  return typeof value === "object" && value !== null && "type" in value && "coordinates" in value;
}

type Encoded =
  | { type: "Property" | "GeoProperty"; value: Cell }
  | { type: "LanguageProperty"; languageMap: Record<string, string> }
  | { type: "Relationship"; object: string | string[] };

function encodeAttrs(attrs: Record<string, WriteValue>): Record<string, Encoded> {
  const result: Record<string, Encoded> = {};
  for (const [key, val] of Object.entries(attrs)) {
    if (val === null) continue;
    if (isLanguageMap(val)) {
      result[key] = { type: "LanguageProperty", languageMap: val.languageMap };
    } else if (isRelationshipObject(val)) {
      result[key] = { type: "Relationship", object: val.object };
    } else if (isGeoJsonGeometry(val)) {
      result[key] = { type: "GeoProperty", value: val };
    } else {
      result[key] = { type: "Property", value: val };
    }
  }
  return result;
}

/** The endpoints a configuration names; one that names none reads its one endpoint. */
export function endpointsOf(config: JcConfig): JcEndpoint[] {
  if (config.endpoints && config.endpoints.length > 0) {
    return config.endpoints;
  }
  return [{ name: config.endpointName ?? config.slug, slug: config.slug, space: config.space, types: [] }];
}

/**
 * The endpoint a call reads (SDK-02): the named one; otherwise the one endpoint serving the
 * type; a type several serve is refused until the call names one; anything else the primary.
 */
export function resolveEndpoint(endpoints: JcEndpoint[], type?: string, name?: string): JcEndpoint {
  const names = endpoints.map((e) => e.name).join(", ");
  if (name !== undefined) {
    const named = endpoints.find((e) => e.name === name);
    if (!named) {
      throw new ProblemError(0, { title: `Unknown endpoint '${name}'`, detail: `Unknown endpoint '${name}': the application reads ${names}.` });
    }
    return named;
  }
  if (type !== undefined) {
    const serving = endpoints.filter((e) => e.types.includes(type));
    if (serving.length === 1) {
      return serving[0];
    }
    if (serving.length > 1) {
      throw new ProblemError(0, {
        title: `Type '${type}' is served by more than one endpoint`,
        detail: `Type '${type}' is served by ${serving.map((e) => e.name).join(" and ")}; pass { endpoint: "${serving[0].name}" } to say which one to read.`,
      });
    }
  }
  return endpoints[0];
}

export function createClient(config: JcConfig, transport: Transport): Client {
  const schemas = new Map<string, Schema>();
  const accesses = new Map<string, AccessDocument>();
  const known = endpointsOf(config);

  /**
   * The endpoint of an entity id: the named one, else the one serving its type. An entity is its
   * space and its URN (ADR-N-041): the URN never says which space, so a type several endpoints
   * serve is refused until the call names one, whatever the URN's segments read.
   */
  const endpointOfId = (id: string, name?: string): JcEndpoint => {
    if (name !== undefined || known.length === 1) {
      return resolveEndpoint(known, undefined, name);
    }
    return resolveEndpoint(known, typeOfUrn(id));
  };

  /** The prefixed id of `localId` in the space of the endpoint `create` writes to (SDK-05). */
  const entityId = (type: string, localId: string, options?: EndpointOption): string => {
    const space = known.length === 1 ? config.space : resolveEndpoint(known, type, options?.endpoint).space || config.space;
    return `urn:ngsi-ld:${type}:${config.orgDomain}:${space}:${localId}`;
  };

  const entities = {
    async list<T extends Row = Row>(type: string, query?: Query): Promise<T[]> {
      if (!TYPE_RE.test(type)) {
        throw new ProblemError(0, { title: `Invalid entity type: '${type}'` });
      }
      const limit = query?.limit ?? 100;
      if (typeof limit !== "number" || limit < 1 || limit > 1000) {
        throw new ProblemError(0, { title: `limit must be between 1 and 1000, got ${limit}` });
      }
      if (query?.offset !== undefined && (typeof query.offset !== "number" || query.offset < 0)) {
        throw new ProblemError(0, { title: `offset must be >= 0, got ${query.offset}` });
      }

      const geo = [query?.georel, query?.geometry, query?.coordinates];
      const geoCount = geo.filter((g) => g !== undefined).length;
      if (geoCount > 0 && geoCount < 3) {
        throw new ProblemError(0, { title: "georel, geometry, coordinates must all be provided together" });
      }

      const params: Record<string, string | undefined> = { type, options: "keyValues", limit: String(limit) };
      if (query?.offset !== undefined && query.offset > 0) {
        params.offset = String(query.offset);
      }
      if (query?.attrs) {
        const filtered = query.attrs.filter((a) => a !== "id" && a !== "type" && a !== "@context");
        if (filtered.length > 0) {
          params.attrs = filtered.join(",");
        }
      }
      if (query?.q) {
        params.q = query.q;
      }
      if (query?.idPattern) {
        params.idPattern = query.idPattern;
      }
      if (query?.georel && query?.geometry && query?.coordinates) {
        params.georel = query.georel;
        params.geometry = query.geometry;
        params.coordinates = query.coordinates;
      }

      const { slug } = resolveEndpoint(known, type, query?.endpoint);
      const path = `/api/endpoint/${encodeURIComponent(slug)}/ngsi-ld/v1/entities?${queryString(params)}`;
      checkEndpointPath(slug, path);

      const resp = await transport({ method: "GET", path });
      if (resp.status < 200 || resp.status >= 300) {
        throw new ProblemError(resp.status, resp.body);
      }
      if (!Array.isArray(resp.body)) {
        throw new ProblemError(resp.status, { title: "The endpoint did not answer a list." });
      }

      return resp.body.map((item) => toRow(item as Record<string, unknown>, config.language)) as T[];
    },

    async all<T extends Row = Row>(type: string, query?: Query): Promise<T[]> {
      const maxTotal = Math.min(query?.limit ?? 1000, 5000);
      const rows: T[] = [];
      let offset = query?.offset ?? 0;

      while (rows.length < maxTotal) {
        const pageLimit = Math.min(1000, maxTotal - rows.length);
        const page = await entities.list<T>(type, { ...query, limit: pageLimit, offset });
        rows.push(...page);
        if (page.length < pageLimit) {
          break;
        }
        offset += page.length;
      }

      return rows;
    },

    async get<T extends Row = Row>(id: string, attrs?: string[], options?: EndpointOption): Promise<T> {
      const params: Record<string, string | undefined> = { options: "keyValues" };
      if (attrs) {
        const filtered = attrs.filter((a) => a !== "id" && a !== "type" && a !== "@context");
        if (filtered.length > 0) params.attrs = filtered.join(",");
      }
      const qs = queryString(params);
      const { slug } = endpointOfId(id, options?.endpoint);
      const path = `/api/endpoint/${encodeURIComponent(slug)}/ngsi-ld/v1/entities/${encodeURIComponent(id)}${qs ? `?${qs}` : ""}`;
      checkEndpointPath(slug, path);

      const resp = await transport({ method: "GET", path });
      if (resp.status < 200 || resp.status >= 300) {
        throw new ProblemError(resp.status, resp.body);
      }
      if (typeof resp.body !== "object" || resp.body === null) {
        throw new ProblemError(resp.status, { title: "The endpoint did not answer an entity." });
      }
      return toRow(resp.body as Record<string, unknown>, config.language) as T;
    },

    async create(type: string, attrs: Record<string, WriteValue>, localId?: string, options?: EndpointOption): Promise<string> {
      if (!TYPE_RE.test(type)) {
        throw new ProblemError(0, { title: `Invalid entity type: '${type}'` });
      }
      const lid = localId ?? randomId();
      // A whole NGSI-LD URN is kept as given (ADR-N-041 §3.4: the source's own id, or one the app
      // built from a template); a local id is minted into the prefixed shape.
      const given = lid.startsWith("urn:ngsi-ld:");
      if (given && typeOfUrn(lid) !== type) {
        throw new ProblemError(0, {
          title: `The id '${lid}' is not a ${type}`,
          detail: `An id given whole names the type it creates: urn:ngsi-ld:${type}:…`,
        });
      }
      if (given ? !URN_ID_RE.test(lid.slice(`urn:ngsi-ld:${type}:`.length)) : !LOCAL_ID_RE.test(lid)) {
        throw new ProblemError(0, {
          title: `Invalid ${given ? "id" : "localId"}: '${lid}'`,
          detail: given
            ? "After urn:ngsi-ld:{Type}: an id holds 1 to 256 letters, digits or -._~!$&'()*+,;=:@/ and %-escapes (RFC 8141)."
            : "A local id holds 1 to 128 letters, digits or -._~; pass a whole urn:ngsi-ld: id to keep another shape.",
        });
      }
      const { slug } = resolveEndpoint(known, type, options?.endpoint);
      const id = given ? lid : entityId(type, lid, options);
      const body = {
        id,
        type,
        ...encodeAttrs(attrs),
      };

      const path = `/api/endpoint/${encodeURIComponent(slug)}/ngsi-ld/v1/entities`;
      checkEndpointPath(slug, path);

      const resp = await transport({ method: "POST", path, body });
      if (resp.status < 200 || resp.status >= 300) {
        throw new ProblemError(resp.status, resp.body);
      }
      return id;
    },

    async update(id: string, patch: Record<string, WriteValue>, options?: EndpointOption): Promise<void> {
      const encoded = encodeAttrs(patch);
      if (Object.keys(encoded).length === 0) {
        return;
      }
      const { slug } = endpointOfId(id, options?.endpoint);
      const path = `/api/endpoint/${encodeURIComponent(slug)}/ngsi-ld/v1/entities/${encodeURIComponent(id)}/attrs`;
      checkEndpointPath(slug, path);

      const resp = await transport({ method: "PATCH", path, body: encoded });
      if (resp.status < 200 || resp.status >= 300) {
        throw new ProblemError(resp.status, resp.body);
      }
    },

    async languages(id: string, attr: string, options?: EndpointOption): Promise<Record<string, string>> {
      const { slug } = endpointOfId(id, options?.endpoint);
      const path = `/api/endpoint/${encodeURIComponent(slug)}/ngsi-ld/v1/entities/${encodeURIComponent(id)}?${queryString({ options: "keyValues", attrs: attr })}`;
      checkEndpointPath(slug, path);

      const resp = await transport({ method: "GET", path });
      if (resp.status < 200 || resp.status >= 300) {
        throw new ProblemError(resp.status, resp.body);
      }
      const value = typeof resp.body === "object" && resp.body !== null ? (resp.body as Record<string, unknown>)[attr] : undefined;
      // keyValues carries a LanguageProperty as `{languageMap}`, some brokers as the bare map; a
      // plain string is a Property the form turns into one language of a map.
      const map = isLanguageMap(value) ? value.languageMap : value;
      if (typeof map === "string") return { [config.language ?? "en"]: map };
      if (typeof map !== "object" || map === null) return {};
      return Object.fromEntries(Object.entries(map).filter((entry): entry is [string, string] => typeof entry[1] === "string"));
    },

    async remove(id: string, options?: EndpointOption): Promise<void> {
      const { slug } = endpointOfId(id, options?.endpoint);
      const path = `/api/endpoint/${encodeURIComponent(slug)}/ngsi-ld/v1/entities/${encodeURIComponent(id)}`;
      checkEndpointPath(slug, path);

      const resp = await transport({ method: "DELETE", path });
      if (resp.status < 200 || resp.status >= 300) {
        throw new ProblemError(resp.status, resp.body);
      }
    },
  };

  const temporal = {
    async list(type: string, query: TemporalQuery): Promise<TemporalRow[]> {
      if (!TYPE_RE.test(type)) {
        throw new ProblemError(0, { title: `Invalid entity type: '${type}'` });
      }
      if (query.timerel === "between" && !query.endTimeAt) {
        throw new ProblemError(0, { title: "endTimeAt is required for timerel 'between'" });
      }
      // The ids go into one comma-separated parameter: an id that is no URN, or holds a comma or a
      // space, would ask for something else than the caller named.
      const badId = query.id?.find((value) => !/^urn:[^\s,]+$/.test(value));
      if (badId !== undefined) {
        throw new ProblemError(0, { title: `Invalid entity id: '${badId}'` });
      }

      const params: Record<string, string | undefined> = {
        type,
        options: "temporalValues",
        timerel: query.timerel,
        timeAt: query.timeAt,
      };
      if (query.endTimeAt) params.endTimeAt = query.endTimeAt;
      if (query.id && query.id.length > 0) params.id = query.id.join(",");
      if (query.lastN !== undefined) params.lastN = String(query.lastN);
      if (query.limit !== undefined) params.limit = String(query.limit);
      if (query.q) params.q = query.q;
      if (query.attrs) {
        const filtered = query.attrs.filter((a) => a !== "id" && a !== "type" && a !== "@context");
        if (filtered.length > 0) params.attrs = filtered.join(",");
      }

      const { slug } = resolveEndpoint(known, type, query.endpoint);
      const path = `/api/endpoint/${encodeURIComponent(slug)}/ngsi-ld/v1/temporal/entities?${queryString(params)}`;
      checkEndpointPath(slug, path);

      const resp = await transport({ method: "GET", path });
      if (resp.status < 200 || resp.status >= 300) {
        throw new ProblemError(resp.status, resp.body);
      }
      if (!Array.isArray(resp.body)) {
        throw new ProblemError(resp.status, { title: "The endpoint did not answer a list." });
      }

      return resp.body.map((item: unknown) => {
        const obj = (typeof item === "object" && item !== null ? item : {}) as Record<string, unknown>;
        const series: Record<string, TemporalPoint[]> = {};

        for (const [key, val] of Object.entries(obj)) {
          if (key === "id" || key === "type" || key === "@context") continue;
          if (typeof val === "object" && val !== null) {
            const rawProp = val as { values?: unknown[] };
            const values = Array.isArray(rawProp.values) ? rawProp.values : Array.isArray(val) ? val : [];
            series[key] = values
              .map((entry): TemporalPoint | null => {
                if (Array.isArray(entry) && entry.length >= 2) {
                  return { value: cell(entry[0], config.language), observedAt: String(entry[1]) };
                }
                if (typeof entry === "object" && entry !== null && "value" in entry && "observedAt" in entry) {
                  const e = entry as { value: unknown; observedAt: unknown };
                  return { value: cell(e.value, config.language), observedAt: String(e.observedAt) };
                }
                return null;
              })
              .filter((p): p is TemporalPoint => p !== null);
          }
        }

        return {
          id: String(obj.id ?? ""),
          type: String(obj.type ?? ""),
          series,
        };
      });
    },
  };

  /** `/api/endpoint/{slug}/ogc/features/{rest}`, checked like every endpoint path. */
  const ogcPath = (slug: string, rest: string): string => {
    const path = `/api/endpoint/${encodeURIComponent(slug)}/ogc/features/${rest}`;
    checkEndpointPath(slug, path);
    return path;
  };

  const ogcGet = async (path: string, shape: "FeatureCollection" | "Feature" | "collections"): Promise<Record<string, unknown>> => {
    const resp = await transport({ method: "GET", path });
    if (resp.status < 200 || resp.status >= 300) {
      throw new ProblemError(resp.status, resp.body);
    }
    const body = (typeof resp.body === "object" && resp.body !== null ? resp.body : {}) as Record<string, unknown>;
    const fine =
      shape === "collections" ? Array.isArray(body.collections) : body.type === shape && (shape === "Feature" || Array.isArray(body.features));
    if (!fine) {
      throw new ProblemError(resp.status, { title: `The endpoint did not answer ${shape === "collections" ? "a collection list" : `a ${shape}`}.` });
    }
    return body;
  };

  const collectionOf = (collection: string): string => {
    if (!TYPE_RE.test(collection)) {
      throw new ProblemError(0, { title: `Invalid collection: '${collection}'` });
    }
    return collection;
  };

  const features = {
    async collections(options?: EndpointOption): Promise<FeatureCollectionInfo[]> {
      const { slug } = resolveEndpoint(known, undefined, options?.endpoint);
      const body = await ogcGet(ogcPath(slug, "collections"), "collections");
      return body.collections as FeatureCollectionInfo[];
    },

    async items(collection: string, query?: FeatureQuery): Promise<FeatureCollection> {
      collectionOf(collection);
      const limit = query?.limit ?? 100;
      if (!Number.isInteger(limit) || limit < 1 || limit > 1000) {
        throw new ProblemError(0, { title: `limit must be between 1 and 1000, got ${limit}` });
      }
      const params: Record<string, string | undefined> = { limit: String(limit) };
      if (query?.bbox) {
        const [minLon, minLat, maxLon, maxLat] = query.bbox;
        const finite = query.bbox.length === 4 && query.bbox.every((n) => Number.isFinite(n));
        if (!finite || minLon < -180 || maxLon > 180 || minLat < -90 || maxLat > 90 || minLat > maxLat) {
          throw new ProblemError(0, { title: `bbox must be [minLon, minLat, maxLon, maxLat] in WGS 84, got ${JSON.stringify(query.bbox)}` });
        }
        params.bbox = query.bbox.join(",");
      }
      if (query?.datetime) params.datetime = query.datetime;
      if (query?.filter) {
        params.filter = query.filter;
        params["filter-lang"] = "cql2-text";
      }
      if (query?.properties) {
        for (const name of query.properties) {
          if (!TYPE_RE.test(name)) throw new ProblemError(0, { title: `Invalid property: '${name}'` });
        }
        params.properties = query.properties.join(",");
      }
      if (query?.sortby && query.sortby.length > 0) {
        for (const key of query.sortby) {
          if (!TYPE_RE.test(key.replace(/^[+-]/, ""))) throw new ProblemError(0, { title: `Invalid sort key: '${key}'` });
        }
        params.sortby = query.sortby.join(",");
      }
      const { slug } = resolveEndpoint(known, collection, query?.endpoint);
      const path = ogcPath(slug, `collections/${encodeURIComponent(collection)}/items?${queryString(params)}`);
      return (await ogcGet(path, "FeatureCollection")) as unknown as FeatureCollection;
    },

    async get(collection: string, id: string, options?: EndpointOption): Promise<Feature> {
      collectionOf(collection);
      const { slug } = resolveEndpoint(known, collection, options?.endpoint);
      const path = ogcPath(slug, `collections/${encodeURIComponent(collection)}/items/${encodeURIComponent(id)}`);
      return (await ogcGet(path, "Feature")) as unknown as Feature;
    },
  };

  const schema = async (endpoint?: string): Promise<Schema> => {
    if (endpoint === undefined && known.length > 1) {
      // Every endpoint's schema the caller may read, merged; a type two endpoints serve keeps the
      // first one's. An endpoint that refuses the caller (401/403: a public App's visitor beside
      // the steward's write endpoint, T-3579) is left out; when none may be read, or one fails
      // otherwise, the merge fails with that endpoint's problem.
      const merged: Schema = {};
      let refused: ProblemError | undefined;
      let read = 0;
      for (const e of [...known].reverse()) {
        try {
          Object.assign(merged, await schema(e.name));
          read += 1;
        } catch (err) {
          if (!(err instanceof ProblemError) || (err.status !== 401 && err.status !== 403)) throw err;
          refused = err;
        }
      }
      if (read === 0 && refused) throw refused;
      return merged;
    }
    const { slug, name } = resolveEndpoint(known, undefined, endpoint);
    const cachedSchema = schemas.get(name);
    if (cachedSchema) return cachedSchema;

    const indexPath = `/api/endpoint/${encodeURIComponent(slug)}/schema/index.json`;
    checkEndpointPath(slug, indexPath);
    const indexResp = await transport({ method: "GET", path: indexPath });
    if (indexResp.status < 200 || indexResp.status >= 300) {
      throw new ProblemError(indexResp.status, indexResp.body);
    }
    const indexBody = (typeof indexResp.body === "object" && indexResp.body !== null ? indexResp.body : {}) as {
      models?: Array<{ version?: number; types?: unknown }>;
    };
    // An endpoint may expose several models; their types never overlap, so the schemas merge.
    const versions = [...new Set((indexBody.models ?? []).map((m) => m.version).filter((v): v is number => Number.isInteger(v)))];
    const merged: Schema = {};
    for (const version of versions.length > 0 ? versions : [1]) {
      const schemaPath = `/api/endpoint/${encodeURIComponent(slug)}/schema/v${version}/json-schema`;
      checkEndpointPath(slug, schemaPath);
      const schemaResp = await transport({ method: "GET", path: schemaPath });
      if (schemaResp.status < 200 || schemaResp.status >= 300) {
        throw new ProblemError(schemaResp.status, schemaResp.body);
      }
      const schemaDoc = (typeof schemaResp.body === "object" && schemaResp.body !== null ? schemaResp.body : {}) as {
        definitions?: Schema;
        $defs?: Schema;
      };
      // Model Tools renders draft-07 (`definitions`); a derived schema may use 2019-09 (`$defs`).
      Object.assign(merged, schemaDoc.$defs, schemaDoc.definitions);
    }
    // A schema document also defines the abstract base class and whatever the endpoint does not
    // serve; the index names the types there are rows of, so only those are the app's types.
    const served = (indexBody.models ?? []).flatMap((m) =>
      Array.isArray(m.types) ? m.types.filter((t): t is string => typeof t === "string") : [],
    );
    for (const name of Object.keys(merged)) {
      if (name === "Entity" || (served.length > 0 && !served.includes(name))) {
        delete merged[name];
      }
    }
    schemas.set(name, merged);
    return merged;
  };

  const access = async (endpoint?: string): Promise<AccessDocument> => {
    const { slug, name } = resolveEndpoint(known, undefined, endpoint);
    const cachedAccess = accesses.get(name);
    if (cachedAccess) return cachedAccess;

    const path = `/api/endpoint/${encodeURIComponent(slug)}/access`;
    checkEndpointPath(slug, path);
    const resp = await transport({ method: "GET", path });
    if (resp.status < 200 || resp.status >= 300) {
      throw new ProblemError(resp.status, resp.body);
    }
    const parsed = parseAccess(resp.body);
    accesses.set(name, parsed);
    return parsed;
  };

  const functions = {
    async call<T = unknown>(name: string, body?: unknown): Promise<T> {
      if (!FUNCTION_NAME.test(name)) {
        throw new ProblemError(0, { title: `Invalid function name: '${name}'` });
      }

      const path =
        config.transport === "bridge"
          ? `/functions/${name}`
          : `/api/functions/${name}`;

      const resp = await transport({ method: "POST", path, body });
      if (resp.status < 200 || resp.status >= 300) {
        throw new ProblemError(resp.status, resp.body);
      }
      return resp.body as T;
    },
  };

  const email = {
    async send(message: Email): Promise<{ id: string }> {
      const path = config.transport === "bridge" ? "/services/email/send" : "/api/services/email/send";
      const resp = await transport({ method: "POST", path, body: message });
      if (resp.status < 200 || resp.status >= 300) {
        throw serviceError(resp.status, resp.body, "email");
      }
      return resp.body as { id: string };
    },
  };

  const files = {
    async put(path: string, body: Blob | ArrayBuffer | string, opts?: { contentType?: string }): Promise<FileInfo> {
      const contentType =
        opts?.contentType ??
        (typeof body === "string"
          ? "text/plain;charset=utf-8"
          : body instanceof Blob && body.type !== ""
            ? body.type
            : "application/octet-stream");
      const resp = await transport({ method: "PUT", path: filePath(path), raw: { data: body, contentType } });
      if (resp.status !== 201) throw serviceError(resp.status, resp.body, "files");
      return resp.body as FileInfo;
    },
    async get(path: string): Promise<Blob> {
      const resp = await transport({ method: "GET", path: filePath(path), as: "blob" });
      if (resp.status !== 200) throw serviceError(resp.status, resp.body, "files");
      return resp.body as Blob;
    },
    async list(prefix = ""): Promise<FileInfo[]> {
      const resp = await transport({ method: "GET", path: `${FILES}?prefix=${encodeURIComponent(prefix)}` });
      if (resp.status !== 200) throw serviceError(resp.status, resp.body, "files");
      return resp.body as FileInfo[];
    },
    async remove(path: string): Promise<void> {
      const resp = await transport({ method: "DELETE", path: filePath(path) });
      if (resp.status !== 204) throw serviceError(resp.status, resp.body, "files");
    },
    async url(path: string, opts?: { method?: "GET" } | { method: "PUT"; size: number }): Promise<{ url: string; expiresAt: string }> {
      const body = opts?.method === "PUT" ? { method: "PUT", size: opts.size } : { method: "GET" };
      const resp = await transport({ method: "POST", path: `${filePath(path)}:url`, body });
      if (resp.status !== 200) throw serviceError(resp.status, resp.body, "files");
      return resp.body as { url: string; expiresAt: string };
    },
  };

  const ai = {
    async complete(request: { messages: AiMessage[]; maxTokens?: number; schema?: object }): Promise<Completion> {
      const path = config.transport === "bridge" ? "/services/ai/complete" : "/api/services/ai/complete";
      const resp = await transport({ method: "POST", path, body: request });
      if (resp.status !== 200) throw serviceError(resp.status, resp.body, "ai");
      return resp.body as Completion;
    },
  };

  const jobs = {
    async list(): Promise<JobStatus[]> {
      const path = config.transport === "bridge" ? "/services/jobs" : "/api/services/jobs";
      const resp = await transport({ method: "GET", path });
      if (resp.status < 200 || resp.status >= 300) {
        throw serviceError(resp.status, resp.body, "jobs");
      }
      return resp.body as JobStatus[];
    },
  };

  return {
    config,
    transport,
    entities,
    temporal,
    features,
    schema,
    access,
    me: () => config.user ?? null,
    entityId,
    functions,
    email,
    files,
    jobs,
    ai,
  };
}

let activeClient: Client | undefined;

export function jc(): Client {
  if (!activeClient) {
    const config = readConfig();
    activeClient = createClient(config, transportFor(config));
  }
  return activeClient;
}

export function setClient(client?: Client): void {
  activeClient = client;
}
