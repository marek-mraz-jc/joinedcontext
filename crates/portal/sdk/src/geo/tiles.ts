/**
 * The Endpoint's OGC vector tiles for a map past a count (T-5195, T-5189, EP-29).
 *
 * A map draws the rows a page loaded; past `TILE_THRESHOLD` it also draws the type's tileset from
 * the same Endpoint (`.../ogc/features/collections/{type}/tiles/WebMercatorQuad`), so every entity
 * is on the map without reading them all. A tile is read from the page's own host at the
 * Endpoint's path, with the session as every other read of the page (same-origin credentials), so
 * the view and the Policy narrow a tile as they narrow a page and no token leaves the host.
 *
 * The gateway names the tile template by its own public base; the template is accepted only on the
 * tileset's own path (`{z}/{y}/{x}` order, no query), and the map then reads that path on its own
 * host, never the host the answer named.
 */
import { useEffect, useState } from "react";
import { addProtocol } from "maplibre-gl";

export const MVT = "application/vnd.mapbox-vector-tile";
/** Past this many entities a map draws the type's tiles beside the loaded rows. */
export const TILE_THRESHOLD = 1000;
export const TILE_PROTOCOL = "jc-tile";
const TAIL = "{z}/{y}/{x}";

/** The tileset of `type` under an Endpoint's base (`/api/endpoint/{slug}`). */
export function tilesetPath(endpointBase: string, type: string): string {
  return `${endpointBase}/ogc/features/collections/${encodeURIComponent(type)}/tiles/WebMercatorQuad`;
}

/**
 * The tile template on the page's own host, `{path}/{z}/{y}/{x}`, when the tileset read at `path`
 * names its MVT item on that same path; `null` otherwise, and the map stays on the loaded rows.
 */
export function tileTemplate(tileset: unknown, path: string): string | null {
  const links = (tileset as { links?: unknown } | null)?.links;
  if (!Array.isArray(links)) return null;
  const item = links.find(
    (link): link is { href: string } =>
      typeof link === "object" && link !== null && link.rel === "item" && link.type === MVT && typeof link.href === "string",
  );
  if (!item) return null;
  const href = item.href.replace("{tileMatrix}", "{z}").replace("{tileRow}", "{y}").replace("{tileCol}", "{x}");
  if (!href.endsWith(TAIL) || /[{}]/.test(href.slice(0, -TAIL.length))) return null;
  let url: URL;
  try {
    url = new URL(href.slice(0, -TAIL.length), `https://host.invalid${path}`);
  } catch {
    return null;
  }
  if (url.pathname !== `${path}/` || url.search || url.hash) return null;
  return `${path}/${TAIL}`;
}

/** The URL MapLibre asks for: the template under the protocol whose loader reads the own host. */
export function tileUrl(template: string): string {
  return `${TILE_PROTOCOL}://${template}`;
}

/**
 * Reads one tile: only a path on the page's own host under `/api/endpoint/`, with the session, as
 * MVT. An empty tile (204, 404 for nothing there) draws nothing; any other answer is a map error.
 */
export async function loadTile(url: string, signal?: AbortSignal, fetchImpl: typeof fetch = fetch): Promise<{ data: ArrayBuffer }> {
  const path = url.slice(`${TILE_PROTOCOL}://`.length);
  // Judged as fetch will read it: the URL parser resolves `..` and its encoded `%2e%2e` alike.
  const resolved = path.startsWith("/") && !path.startsWith("//") ? new URL(path, "https://own.host").pathname : "";
  if (!url.startsWith(`${TILE_PROTOCOL}://`) || !resolved.startsWith("/api/endpoint/") || path.includes("..") || /%2e/i.test(path)) {
    throw new Error("kit: a tile outside the Endpoint is not read");
  }
  const response = await fetchImpl(path, { credentials: "same-origin", headers: { Accept: MVT }, signal });
  if (response.status === 204 || response.status === 404) return { data: new ArrayBuffer(0) };
  if (!response.ok) throw new Error(`kit: the Endpoint answered ${response.status} for a tile`);
  return { data: await response.arrayBuffer() };
}

let registered = false;

/** MapLibre's tile requests run in its worker; the protocol reads them on the page, once per page. */
export function registerTileProtocol(): void {
  if (registered) return;
  registered = true;
  addProtocol(TILE_PROTOCOL, (params, abort) => loadTile(params.url, abort.signal));
}

/**
 * The tile template of `type` on an Endpoint once `wanted` holds, or `null` while it is not wanted,
 * not read yet, refused, or names its tiles elsewhere.
 */
export function useTileTemplate(endpointBase: string | null, type: string, wanted: boolean, fetchImpl: typeof fetch = fetch): string | null {
  const [template, setTemplate] = useState<string | null>(null);
  useEffect(() => {
    setTemplate(null);
    if (!wanted || !endpointBase) return;
    let live = true;
    const path = tilesetPath(endpointBase, type);
    fetchImpl(path, { credentials: "same-origin", headers: { Accept: "application/json" } })
      .then((response) => (response.ok ? response.json() : null))
      .then((body: unknown) => {
        if (live) setTemplate(tileTemplate(body, path));
      })
      .catch(() => {
        if (live) setTemplate(null);
      });
    return () => {
      live = false;
    };
  }, [endpointBase, type, wanted, fetchImpl]);
  return template;
}
