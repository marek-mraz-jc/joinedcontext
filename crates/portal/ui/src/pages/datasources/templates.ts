/**
 * Data source templates (T-3249): the public sources a city starts from, each asking only what it
 * needs and turned into the DataSource form of an existing type. No new kind: a CSV file, a
 * spreadsheet, a zipped shapefile, a REST API, a CKAN resource and a WFS layer are all `http`, a GTFS
 * Realtime feed is `gtfs-rt`.
 */
import { PRESET_KINDS, RECORDS_PATH, SHEET_NAME } from "../../schemas/kinds";
import type { PresetKind, TypedDataSourceType } from "../../schemas/kinds";

export type TemplateId = "csv" | "xlsx" | "shapefile" | "json" | "ckan" | "wfs" | "gtfs-rt" | `api-${PresetKind}`;

export interface Template {
  id: TemplateId;
  type: TypedDataSourceType;
  /** The wizard's group (T-7094): an open data API the runner walks page by page, or one file or feed. */
  group: "api" | "file";
  /** The open data API kind (`spec.http.preset`), which sets the paging and the records itself. */
  preset?: PresetKind;
  /** The WFS template also names the layer; every template asks for an address. */
  asksLayer: boolean;
  /** The JSON template may name the array its records sit in (T-6572). */
  asksRecords?: boolean;
  /** The spreadsheet template may name the sheet to read (T-7086). */
  asksSheet?: boolean;
}

export const TEMPLATES: Template[] = [
  ...PRESET_KINDS.map((kind): Template => ({ id: `api-${kind}`, type: "http", group: "api", preset: kind, asksLayer: false })),
  { id: "csv", type: "http", group: "file", asksLayer: false },
  { id: "xlsx", type: "http", group: "file", asksLayer: false, asksSheet: true },
  { id: "shapefile", type: "http", group: "file", asksLayer: false },
  { id: "json", type: "http", group: "file", asksLayer: false, asksRecords: true },
  { id: "ckan", type: "http", group: "file", asksLayer: false },
  { id: "wfs", type: "http", group: "file", asksLayer: true },
  { id: "gtfs-rt", type: "gtfs-rt", group: "file", asksLayer: false },
];

/** A credential as the wizard picks it: the name of a secret the project uses and its key (PL-14). */
export interface PickedSecret {
  name: string;
  key: string;
}

/** The first answer's paging as `POST /datasource-paging` guesses it (`ops/datasource_paging.rs`). */
export interface PagingGuess {
  paging?: Record<string, unknown>;
  records?: string;
  onPage?: number;
  total?: number;
  pages?: number;
  more: boolean;
  skipped?: string;
}

/** What a person typed into a template. */
export interface TemplateValues {
  name: string;
  url: string;
  layer?: string;
  records?: string;
  sheet?: string;
  /** A CSV file split into one record per row (`spec.http.format: csv`, PL-114). */
  rows?: boolean;
  /** The walk the wizard's paging guess found, for a JSON API (`spec.http.paging`). */
  paging?: Record<string, unknown>;
  /** The credential the source sends, as a secretRef only. */
  secret?: PickedSecret;
  /** An open data API's `spec.http` after a discovery pick wrote into it. */
  http?: Record<string, unknown>;
}

/** Why an address does not fit the template, as a key under `datasources.template.problem`. */
export type TemplateProblem = "name" | "url" | "ckanDataset" | "layer" | "records" | "sheet" | "secretKey";

const DNS1123 = /^[a-z0-9]([-a-z0-9]*[a-z0-9])?$/;

/** The address checked for the template; a CKAN dataset page is not a file the runner can read. */
export function problemOf(template: Template, values: TemplateValues): TemplateProblem | undefined {
  // The address first: a name taken from a bad address is wrong only because the address is.
  let url: URL;
  try {
    url = new URL(values.url.trim());
  } catch {
    return "url";
  }
  if (url.protocol !== "https:" && url.protocol !== "http:") return "url";
  if (!DNS1123.test(values.name.trim()) || values.name.trim().length > 63) return "name";
  if (template.id === "ckan" && /\/dataset\/[^/]+\/?$/.test(url.pathname)) return "ckanDataset";
  if (template.asksLayer && !isGetFeature(url) && !(values.layer ?? "").trim()) return "layer";
  const records = (values.records ?? "").trim();
  if (template.asksRecords && records !== "" && !new RegExp(RECORDS_PATH).test(records)) return "records";
  const sheet = values.sheet ?? "";
  if (template.asksSheet && sheet.trim() !== "" && !new RegExp(SHEET_NAME, "u").test(sheet.trim())) return "sheet";
  if (values.secret && !values.secret.key.trim()) return "secretKey";
  return undefined;
}

const isGetFeature = (url: URL): boolean =>
  [...url.searchParams].some(([key, value]) => key.toLowerCase() === "request" && value.toLowerCase() === "getfeature") ||
  url.pathname.toLowerCase().endsWith(".geojson");

/**
 * The GeoJSON GetFeature address of a WFS layer: the service address with the layer and
 * `outputFormat=application/json`; an address that already asks for features is kept as it is.
 */
export function wfsUrl(service: string, layer: string): string {
  const url = new URL(service.trim());
  if (isGetFeature(url)) return url.toString();
  for (const key of [...url.searchParams.keys()]) {
    if (["service", "version", "request", "typename", "typenames", "outputformat"].includes(key.toLowerCase())) {
      url.searchParams.delete(key);
    }
  }
  url.searchParams.set("service", "WFS");
  url.searchParams.set("version", "2.0.0");
  url.searchParams.set("request", "GetFeature");
  url.searchParams.set("typeNames", layer.trim());
  url.searchParams.set("outputFormat", "application/json");
  return url.toString();
}

/** A source name from an address, as a DNS-1123 label: the file or the last path part. */
export function nameFrom(address: string): string {
  try {
    const url = new URL(address.trim());
    const part = url.pathname.split("/").filter(Boolean).pop();
    const base = part === undefined ? url.hostname : decodeURIComponent(part).replace(/\.[a-z0-9]+$/i, "");
    const label = base
      .normalize("NFD")
      .replace(/[̀-ͯ]/g, "")
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, 63)
      .replace(/-+$/, "");
    return DNS1123.test(label) ? label : "";
  } catch {
    return "";
  }
}

/** The type and the form the template fills; the person checks and proposes it as any source. */
export function fromTemplate(template: Template, values: TemplateValues): { type: TypedDataSourceType; form: Record<string, unknown> } {
  const name = values.name.trim();
  if (template.type === "gtfs-rt") {
    return { type: "gtfs-rt", form: { name, gtfsRt: { url: values.url.trim(), feed: "vehiclePositions" } } };
  }
  const authorization = values.secret
    ? { authorization: { headerRef: { name: values.secret.name, key: values.secret.key.trim() } } }
    : {};
  if (template.preset) {
    const http = values.http ?? { url: values.url.trim(), preset: { kind: template.preset } };
    return { type: "http", form: { name, http: { ...http, verb: "GET", ...authorization } } };
  }
  const url = template.asksLayer ? wfsUrl(values.url, values.layer ?? "") : values.url.trim();
  if (template.asksSheet) {
    // The runner splits the workbook into one record per row of the sheet (T-7086).
    const sheet = (values.sheet ?? "").trim();
    return { type: "http", form: { name, http: { url, verb: "GET", format: "xlsx", ...(sheet ? { sheet } : {}), ...authorization } } };
  }
  if (template.id === "shapefile") {
    // The runner reads the zipped layer as one WGS 84 GeoJSON feature per record (T-7089).
    return { type: "http", form: { name, http: { url, verb: "GET", format: "shapefile", ...authorization } } };
  }
  const records = template.asksRecords ? (values.records ?? "").trim() : "";
  const paging = template.asksRecords && values.paging ? { paging: values.paging } : {};
  const rows = template.id === "csv" && values.rows ? { format: "csv" } : {};
  return {
    type: "http",
    form: { name, http: { url, verb: "GET", ...(records ? { records } : {}), ...paging, ...rows, ...authorization } },
  };
}

/**
 * The address whose first answer tells how a source pages: an open data API's records resource
 * (a WFS GetFeature, a CKAN package search of the organization), or the address as typed.
 */
export function pagingUrl(template: Template, values: TemplateValues): string {
  const http = values.http ?? {};
  const url = typeof http.url === "string" ? http.url : values.url.trim();
  const preset = (http.preset ?? {}) as { typeNames?: string; organization?: string };
  if (template.preset === "wfs" && preset.typeNames) return wfsUrl(url, preset.typeNames);
  if (template.preset === "ckan") {
    const at = new URL(url);
    const root = at.pathname.split("/api/3/action")[0].replace(/\/$/, "");
    at.pathname = `${root}/api/3/action/package_search`;
    at.search = preset.organization ? `fq=${encodeURIComponent(`organization:${preset.organization}`)}` : "";
    return at.toString();
  }
  return url;
}

/** The guess in words, as a key under `datasources.wizard.paging` and its values. */
export function pagingWords(guess: PagingGuess): { key: string; values: Record<string, unknown> } {
  if (guess.skipped) return { key: "skipped", values: { reason: guess.skipped } };
  const values = { pages: guess.pages, onPage: guess.onPage, total: guess.total };
  if (guess.paging && guess.pages !== undefined) return { key: "pages", values };
  if (guess.paging && guess.more) return { key: "more", values };
  if (guess.paging) return { key: "walk", values };
  if (guess.total !== undefined) return { key: "totalOnly", values };
  return { key: "once", values };
}
