/**
 * HSL's stops and lines as the page reads them (T-3356): the GtfsStop and PublicTransportRoute entities of
 * the space, turned into the network the module routes over. A row carries a list as one text
 * joined by commas, so a line's stops are split back out of it.
 */
import { useCallback, useEffect, useState } from "react";
import { ProblemError, useClient } from "@joinedcontext/sdk";
import type { DataClient, Row } from "@joinedcontext/sdk";
import type { NetworkInput } from "./analysis";

export const STOP = "GtfsStop";
export const ROUTE = "PublicTransportRoute";
/** What the stops carry: where they are, their name and sign code. */
export const STOP_ATTRS = ["name", "stopCode", "location"];
/** What the lines carry: their number, what runs them, their stops in order. */
export const ROUTE_ATTRS = ["routeShortName", "transportMode", "stopSequence"];

/** The GTFS stop id that ends a GtfsStop's id, the id a line names its stops by. */
export function stopIdOf(id: string): string {
  return id.slice(id.lastIndexOf(":") + 1);
}

function text(value: unknown): string {
  return typeof value === "string" ? value.trim() : typeof value === "number" ? String(value) : "";
}

/** The point a row's `location` names, or `null`. */
function pointOf(value: unknown): { lon: number; lat: number } | null {
  if (value === null || typeof value !== "object") return null;
  const geo = value as { type?: unknown; coordinates?: unknown };
  if (geo.type !== "Point" || !Array.isArray(geo.coordinates)) return null;
  const [lon, lat] = geo.coordinates as unknown[];
  return typeof lon === "number" && typeof lat === "number" ? { lon, lat } : null;
}

/** The network of the rows: every placed stop, every line with a number and at least two stops. */
export function toNetwork(stops: Row[], routes: Row[]): NetworkInput {
  return {
    stops: stops.flatMap((row) => {
      const at = pointOf(row.location);
      if (!at) return [];
      const name = text(row.name);
      const code = text(row.stopCode);
      return [{ id: stopIdOf(row.id), ...at, ...(name ? { name } : {}), ...(code ? { code } : {}) }];
    }),
    routes: routes.flatMap((row) => {
      const name = text(row.routeShortName);
      const sequence = text(row.stopSequence)
        .split(",")
        .map((id) => id.trim())
        .filter(Boolean);
      if (!name || sequence.length < 2) return [];
      const mode = text(row.transportMode);
      return [{ name, ...(mode ? { mode } : {}), stops: sequence }];
    }),
  };
}

/** HSL has some 8400 stops, past what the SDK's `all` gathers (5000), so the pages are read here. */
const PAGE = 1000;
/** A bound on the pages of one type, so an endpoint that never ends a list cannot hold the page. */
const MAX_PAGES = 50;

async function readAll(client: DataClient, type: string, attrs: string[]): Promise<Row[]> {
  const rows: Row[] = [];
  for (let page = 0; page < MAX_PAGES; page++) {
    const batch = await client.entities.list(type, { attrs, limit: PAGE, offset: page * PAGE });
    rows.push(...batch);
    if (batch.length < PAGE) break;
  }
  return rows;
}

/** The key of a point, the stop's coordinates as the module hands them back unchanged. */
export function pointKey(at: { lon: number; lat: number }): string {
  return `${at.lon},${at.lat}`;
}

/** Each placed stop's entity id by its point, so a stop the module reached opens as its entity (SDK-40). */
export function entitiesOf(stops: Row[]): Map<string, string> {
  const ids = new Map<string, string>();
  for (const row of stops) {
    const at = pointOf(row.location);
    if (at) ids.set(pointKey(at), row.id);
  }
  return ids;
}

type NetworkState = { network: NetworkInput; entities: Map<string, string>; loading: boolean; error: ProblemError | null };

/** HSL's stops and lines of the space, every page of both; an error beside an empty network. */
export function useNetwork(): NetworkState & { reload: () => void } {
  const client = useClient();
  const [state, setState] = useState<NetworkState>({
    network: { stops: [], routes: [] },
    entities: new Map(),
    loading: true,
    error: null,
  });
  const [nonce, setNonce] = useState(0);
  const reload = useCallback(() => setNonce((n) => n + 1), []);
  useEffect(() => {
    let current = true;
    setState((previous) => ({ ...previous, loading: true, error: null }));
    Promise.all([readAll(client, STOP, STOP_ATTRS), readAll(client, ROUTE, ROUTE_ATTRS)]).then(
      ([stops, routes]) => current && setState({ network: toNetwork(stops, routes), entities: entitiesOf(stops), loading: false, error: null }),
      (error: unknown) =>
        current &&
        setState({
          network: { stops: [], routes: [] },
          entities: new Map(),
          loading: false,
          error: error instanceof ProblemError ? error : new ProblemError(0, { title: error instanceof Error ? error.message : String(error) }),
        }),
    );
    return () => {
      current = false;
    };
  }, [client, nonce]);
  return { ...state, reload };
}
