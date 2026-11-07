/**
 * Data source templates (T-3249): the public sources a city starts from, each asking only what it
 * needs and turned into the DataSource form of an existing type. No new kind: a CSV file, a REST
 * API, a CKAN resource and a WFS layer are all `http`, a GTFS Realtime feed is `gtfs-rt`.
 */
import { RECORDS_PATH } from "../../schemas/kinds";
import type { TypedDataSourceType } from "../../schemas/kinds";

export type TemplateId = "csv" | "json" | "ckan" | "wfs" | "gtfs-rt";

export interface Template {
  id: TemplateId;
  type: TypedDataSourceType;
  /** The WFS template also names the layer; every template asks for an address. */
  asksLayer: boolean;
  /** The JSON template may name the array its records sit in (T-6572). */
  asksRecords?: boolean;
}

export const TEMPLATES: Template[] = [
  { id: "csv", type: "http", asksLayer: false },
  { id: "json", type: "http", asksLayer: false, asksRecords: true },
  { id: "ckan", type: "http", asksLayer: false },
  { id: "wfs", type: "http", asksLayer: true },
  { id: "gtfs-rt", type: "gtfs-rt", asksLayer: false },
];

/** What a person typed into a template. */
export interface TemplateValues {
  name: string;
  url: string;
  layer?: string;
  records?: string;
}

/** Why an address does not fit the template, as a key under `datasources.template.problem`. */
export type TemplateProblem = "name" | "url" | "ckanDataset" | "layer" | "records";

const DNS1123 = /^[a-z0-9]([-a-z0-9]*[a-z0-9])?$/;

/** The address checked for the template; a CKAN dataset page is not a file the runner can read. */
export function problemOf(template: Template, values: TemplateValues): TemplateProblem | undefined {
  if (!DNS1123.test(values.name.trim()) || values.name.trim().length > 63) return "name";
  let url: URL;
  try {
    url = new URL(values.url.trim());
  } catch {
    return "url";
  }
  if (url.protocol !== "https:" && url.protocol !== "http:") return "url";
  if (template.id === "ckan" && /\/dataset\/[^/]+\/?$/.test(url.pathname)) return "ckanDataset";
  if (template.asksLayer && !isGetFeature(url) && !(values.layer ?? "").trim()) return "layer";
  const records = (values.records ?? "").trim();
  if (template.asksRecords && records !== "" && !new RegExp(RECORDS_PATH).test(records)) return "records";
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
  const url = template.asksLayer ? wfsUrl(values.url, values.layer ?? "") : values.url.trim();
  const records = template.asksRecords ? (values.records ?? "").trim() : "";
  return { type: "http", form: { name, http: records ? { url, verb: "GET", records } : { url, verb: "GET" } } };
}
