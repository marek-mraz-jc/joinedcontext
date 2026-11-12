/**
 * The rows of a `vega-lite` widget (T-6786): one source of its Endpoint, read through the
 * Portal's Endpoint door with the reader's session, the way the viz renderer of components/viz
 * reads a published chart (host.js `firstRequest`). The spec never names where its rows come
 * from; the platform fills its dataset `source` with these.
 */

/** The source a widget names, as the Dashboard kind declares it (jc-core `VisualizationData`). */
export type WidgetSource =
  | { source: "csv"; type: string }
  | { source: "ogc"; collection: string; filter?: string; properties?: string }
  | { source: "odata"; entitySet: string; $filter?: string; $select?: string; $apply?: string; $orderby?: string }
  | { source: "temporal"; type: string; attrs: string[]; last: string; aggrMethod?: string; aggrPeriod?: string };

/** The most rows one widget draws; past it the chart says it shows the first ones. */
export const SOURCE_MAX_ROWS = 5_000;

const NAME = /^[A-Za-z][A-Za-z0-9_.:-]{0,255}$/;
const PAGE = 1000;

function query(params: Record<string, string | undefined>): string {
  const out = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) if (value !== undefined && value !== "") out.set(key, value);
  return out.toString();
}

/** Seconds of an ISO 8601 duration of weeks to seconds (`P7D`, `PT6H`), as jc-core reads it. */
export function seconds(duration: string): number | null {
  const match = /^P(?:(\d+)W)?(?:(\d+)D)?(?:T(?:(\d+)H)?(?:(\d+)M)?(?:(\d+)S)?)?$/.exec(duration);
  if (!match || duration === "P" || duration.endsWith("T")) return null;
  const [w, d, h, m, s] = match.slice(1).map((part) => Number(part ?? 0));
  return w * 604_800 + d * 86_400 + h * 3_600 + m * 60 + s;
}

/** The first address a source is read at, under the Endpoint's own path; null for a bad source. */
export function sourceUrl(slug: string, source: WidgetSource, now: number): string | null {
  const base = `/api/endpoint/${encodeURIComponent(slug)}/`;
  switch (source.source) {
    case "csv":
      return NAME.test(source.type) ? `${base}file.csv?${query({ type: source.type })}` : null;
    case "ogc":
      return NAME.test(source.collection)
        ? `${base}ogc/features/collections/${source.collection}/items?${query({ limit: String(PAGE), filter: source.filter, properties: source.properties })}`
        : null;
    case "odata":
      return NAME.test(source.entitySet)
        ? `${base}odata/v4/${source.entitySet}?${query({ $filter: source.$filter, $select: source.$select, $apply: source.$apply, $orderby: source.$orderby })}`
        : null;
    case "temporal": {
      const window = seconds(source.last);
      if (!NAME.test(source.type) || source.attrs.length === 0 || !source.attrs.every((a) => NAME.test(a)) || !window) return null;
      const aggregated = Boolean(source.aggrMethod);
      return `${base}ngsi-ld/v1/temporal/entities?${query({
        type: source.type,
        attrs: source.attrs.join(","),
        timerel: "after",
        timeAt: new Date(now - window * 1000).toISOString().replace(/\.\d{3}Z$/, "Z"),
        format: aggregated ? "aggregatedValues" : "temporalValues",
        aggrMethods: source.aggrMethod,
        aggrPeriodDuration: source.aggrPeriod,
        limit: String(PAGE),
      })}`;
    }
  }
}

export interface SourceRows {
  /** JSON rows, or the CSV text the chart parses. */
  rows: Record<string, unknown>[];
  csv?: string;
  /** More rows were there than {@link SOURCE_MAX_ROWS}. */
  cut: boolean;
}

/**
 * A next link is followed only while it stays under the first request's path; one that leaves it
 * stops the chart rather than drawing part of the rows as if they were all (as host.js does).
 */
function sameEndpoint(next: unknown, first: string): string | null {
  if (next === undefined || next === null) return null;
  const url = typeof next === "string" ? new URL(next, window.location.origin) : null;
  const prefix = first.slice(0, first.indexOf("?")).replace(/\/[^/]*$/, "/");
  if (!url || url.origin !== window.location.origin || !url.pathname.startsWith(prefix)) throw new Error("the next page is not on the Endpoint");
  return url.pathname + url.search;
}

/** A temporal answer flattened to one row per instance: id, attribute, observedAt, value. */
export function temporalRows(body: unknown, attrs: string[], aggregated: boolean): Record<string, unknown>[] {
  const rows: Record<string, unknown>[] = [];
  for (const entity of Array.isArray(body) ? body : []) {
    if (!entity || typeof entity.id !== "string") continue;
    for (const attr of attrs) {
      const parts: unknown[] = Array.isArray(entity[attr]) ? entity[attr] : [entity[attr]];
      for (const part of parts) {
        if (!part || typeof part !== "object") continue;
        const record = part as Record<string, unknown>;
        if (aggregated) {
          for (const [method, list] of Object.entries(record)) {
            if (!Array.isArray(list)) continue;
            for (const pair of list) if (Array.isArray(pair)) rows.push({ id: entity.id, attribute: attr, method, value: pair[0], observedAt: pair[1] });
          }
        } else if (Array.isArray(record.values)) {
          for (const pair of record.values) if (Array.isArray(pair)) rows.push({ id: entity.id, attribute: attr, value: pair[0], observedAt: pair[1] });
        }
      }
    }
  }
  return rows;
}

async function json(url: string, fetcher: typeof fetch): Promise<unknown> {
  const response = await fetcher(url, { credentials: "same-origin", headers: { Accept: "application/json" } });
  if (!response.ok) throw new Error(`HTTP ${response.status}`);
  return response.json();
}

/** Reads the source, following next links on the Endpoint up to {@link SOURCE_MAX_ROWS} rows. */
export async function readSource(slug: string, source: WidgetSource, fetcher: typeof fetch = fetch, now = Date.now()): Promise<SourceRows> {
  const first = sourceUrl(slug, source, now);
  if (!first) throw new Error("the widget's data names no readable source");
  if (source.source === "csv") {
    const response = await fetcher(first, { credentials: "same-origin", headers: { Accept: "text/csv" } });
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    const csv = await response.text();
    return { rows: [], csv, cut: false };
  }
  if (source.source === "temporal") {
    const rows = temporalRows(await json(first, fetcher), source.attrs, Boolean(source.aggrMethod));
    return { rows: rows.slice(0, SOURCE_MAX_ROWS), cut: rows.length > SOURCE_MAX_ROWS };
  }
  let rows: Record<string, unknown>[] = [];
  let url: string | null = first;
  while (url && rows.length < SOURCE_MAX_ROWS) {
    const body = (await json(url, fetcher)) as Record<string, unknown>;
    if (source.source === "ogc") {
      const features = Array.isArray(body.features) ? (body.features as Record<string, unknown>[]) : [];
      rows = rows.concat(features.map((f) => ({ id: f.id, ...((f.properties as Record<string, unknown>) ?? {}) })));
      const links = Array.isArray(body.links) ? (body.links as { rel?: string; href?: string }[]) : [];
      url = sameEndpoint(links.find((l) => l.rel === "next")?.href, first);
    } else {
      rows = rows.concat(Array.isArray(body.value) ? (body.value as Record<string, unknown>[]) : []);
      url = sameEndpoint(body["@odata.nextLink"], first);
    }
  }
  return { rows: rows.slice(0, SOURCE_MAX_ROWS), cut: rows.length > SOURCE_MAX_ROWS || url !== null };
}
