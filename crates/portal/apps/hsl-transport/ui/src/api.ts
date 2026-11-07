/**
 * The app's own backend, and nothing else. The Endpoint URL never reaches the browser: the
 * server holds it, follows its WebSocket once for every viewer and pushes what moved down this stream
 * (AP-04, AP-41). There is no login anywhere in this file, because the Endpoint behind the
 * app is public and the app never sees a user (AP-28).
 */
import { currentTokens, portalLinkOf, ProblemError, type DesignTokens, type PanelSource, type Row, type SelectedEntity } from "@joinedcontext/sdk";

const BASE = import.meta.env.BASE_URL;

export interface Vehicle {
  id: string;
  /** `[longitude, latitude]`, the GeoJSON order. */
  coordinates: [number, number];
  bearing?: number;
  speed?: number;
  refLine?: string;
  /** bus, tram, metro, train or ferry; a vehicle written before modes were (T-3893) is a bus. */
  mode?: string;
}

export interface VehicleFeature {
  type: "Feature";
  id: string;
  geometry: { type: "Point"; coordinates: [number, number] };
  properties: { id: string; color: string; bearing: number; refLine: string; speed?: number };
}

export interface VehicleCollection {
  type: "FeatureCollection";
  features: VehicleFeature[];
}

/** The fleet as the backend last saw it. A 503 means no read has produced one yet. */
export async function getVehicles(): Promise<Vehicle[]> {
  const response = await fetch(`${BASE}api/vehicles`);
  if (!response.ok) {
    return [];
  }
  return (await response.json()) as Vehicle[];
}

/**
 * Every bus that moves. The first message carries the whole fleet, so a browser that opens
 * the stream alone is already drawing. `onFeed` hears since when the backend's live feed is
 * lost (an ISO time), or `null` once it is live again (T-3581). Returns the function that
 * closes the stream.
 */
export function subscribe(
  onVehicles: (vehicles: Vehicle[]) => void,
  onFeed: (lostSince: string | null) => void = () => {},
): () => void {
  const source = new EventSource(`${BASE}api/stream`);
  source.addEventListener("vehicles", (event) => {
    try {
      onVehicles(JSON.parse((event as MessageEvent<string>).data) as Vehicle[]);
    } catch {
      // A malformed frame is one lost update, not a reason to tear the map down.
    }
  });
  source.addEventListener("feed", (event) => {
    try {
      const { lostSince } = JSON.parse((event as MessageEvent<string>).data) as { lostSince?: unknown };
      onFeed(typeof lostSince === "string" ? lostSince : null);
    } catch {
      // As above: one lost status, the next one comes with the next change.
    }
  });
  return () => source.close();
}

/**
 * One colour per line, from the design tokens' chart palette (AP-123), so two buses on the 550
 * look like the same route without anybody maintaining a colour table, and no entity value ever
 * becomes CSS. A line the Policy hides falls back to the tokens' muted colour.
 */
export function lineColor(refLine: string | undefined, tokens: DesignTokens = currentTokens()): string {
  if (!refLine) {
    return tokens.color.muted;
  }
  let hash = 0;
  for (const character of refLine) {
    hash = (hash * 31 + character.charCodeAt(0)) % 9973;
  }
  // A grey in the palette reads as "no line"; lines take the palette's colours only.
  const colours = tokens.chart.palette.filter(chromatic);
  const palette = colours.length > 0 ? colours : tokens.chart.palette;
  return palette[hash % palette.length];
}

function channels(hex: string): [number, number, number] | undefined {
  const match = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(hex);
  return match ? [parseInt(match[1], 16), parseInt(match[2], 16), parseInt(match[3], 16)] : undefined;
}

/** A colour, not a grey: its channels spread by more than a fifth of the range. */
function chromatic(hex: string): boolean {
  const rgb = channels(hex);
  return rgb !== undefined && Math.max(...rgb) - Math.min(...rgb) > 51;
}

/** `share` of the way from `from` to `to`, both `#rrggbb`; `from` when either is not. */
export function mix(from: string, to: string, share: number): string {
  const a = channels(from);
  const b = channels(to);
  if (!a || !b) return from;
  const t = Math.min(1, Math.max(0, share));
  return `#${a.map((value, index) => Math.round(value + (b[index] - value) * t).toString(16).padStart(2, "0")).join("")}`;
}

/** `[[west, south], [east, north]]`, as MapLibre's `getBounds().toArray()` answers. */
export type Bounds = [[number, number], [number, number]];

/** The buses inside the map's view; every bus while the map has not said where it looks. */
export function inView(vehicles: Vehicle[], bounds: Bounds | undefined): Vehicle[] {
  if (!bounds) {
    return vehicles;
  }
  const [[west, south], [east, north]] = bounds;
  return vehicles.filter(({ coordinates: [longitude, latitude] }) =>
    longitude >= west && longitude <= east && latitude >= south && latitude <= north,
  );
}

export interface Bar {
  label: string;
  value: number;
  color: string;
}

/** What the per-line chart calls a bus whose line the Policy hides. */
export const NO_LINE = "Line not shown";

/** Buses per line, most first, a tie in line order, at most `top` lines (the rest are summed). */
export function perLine(vehicles: Vehicle[], top = 15, tokens: DesignTokens = currentTokens()): Bar[] {
  const counts = new Map<string, number>();
  for (const { refLine } of vehicles) {
    const line = refLine || NO_LINE;
    counts.set(line, (counts.get(line) ?? 0) + 1);
  }
  const sorted = [...counts].sort(([a, x], [b, y]) => y - x || a.localeCompare(b, "en", { numeric: true }));
  const bars = sorted.slice(0, top).map(([line, value]) => ({
    label: line,
    value,
    color: lineColor(line === NO_LINE ? undefined : line, tokens),
  }));
  const rest = sorted.slice(top).reduce((sum, [, value]) => sum + value, 0);
  return rest > 0 ? [...bars, { label: "Other lines", value: rest, color: tokens.color.muted }] : bars;
}

/** The speed chart's bands in km/h: the stream carries metres per second (HFP `spd`). */
export const SPEED_BANDS = [0, 10, 20, 30, 40, 50, 60] as const;

/**
 * Buses per speed band, every band present so the chart keeps its shape; a bus that reports no
 * speed is counted apart rather than as standing still.
 */
export function speedBands(vehicles: Vehicle[], tokens: DesignTokens = currentTokens()): { bars: Bar[]; unknown: number } {
  const counts: number[] = SPEED_BANDS.map(() => 0);
  let unknown = 0;
  for (const { speed } of vehicles) {
    if (speed === undefined || !Number.isFinite(speed) || speed < 0) {
      unknown += 1;
      continue;
    }
    const kmh = speed * 3.6;
    let band = SPEED_BANDS.length - 1;
    while (band > 0 && kmh < SPEED_BANDS[band]) band -= 1;
    counts[band] += 1;
  }
  // Slow to fast through the tokens' own colours: the map's low, success, warning, the map's high
  // and danger, each band on the way between two of them (AP-123).
  const ramp = [tokens.map.low, tokens.color.success, tokens.color.warning, tokens.map.high, tokens.color.danger];
  const at = (share: number): string => {
    const position = share * (ramp.length - 1);
    const stop = Math.min(ramp.length - 2, Math.floor(position));
    return mix(ramp[stop], ramp[stop + 1], position - stop);
  };
  const bars = SPEED_BANDS.map((from, index) => ({
    label: index === SPEED_BANDS.length - 1 ? `${from}+ km/h` : `${from}–${SPEED_BANDS[index + 1]} km/h`,
    value: counts[index],
    color: at(index / (SPEED_BANDS.length - 1)),
  }));
  return { bars, unknown };
}

/** The fleet as the map source reads it. */
export function featureCollection(vehicles: Vehicle[]): VehicleCollection {
  return {
    type: "FeatureCollection",
    features: vehicles.map((vehicle) => ({
      type: "Feature",
      id: vehicle.id,
      geometry: { type: "Point", coordinates: vehicle.coordinates },
      properties: {
        id: vehicle.id,
        color: lineColor(vehicle.refLine),
        // The arrow needs a number; a hidden bearing points north rather than disappearing.
        bearing: vehicle.bearing ?? 0,
        refLine: vehicle.refLine ?? "",
        ...(vehicle.speed === undefined ? {} : { speed: vehicle.speed }),
      },
    })),
  };
}

/**
 * Where the shell's entity panel reads a bus (SDK-40): from this app's own backend, which holds
 * the endpoint, the bus as the last notification left it. The app writes nothing, so the panel never offers
 * Edit; it links the bus to the Portal when the page names the project's page there.
 */
export function panelSource(page: { portal?: string; space?: string }): PanelSource {
  return {
    async get(entity: SelectedEntity): Promise<Row> {
      const vehicle = (await getVehicles()).find((candidate) => candidate.id === entity.id);
      if (!vehicle) throw new ProblemError(404, { title: "This bus is no longer reported." });
      return {
        id: vehicle.id,
        type: "Vehicle",
        location: { type: "Point", coordinates: vehicle.coordinates },
        ...(vehicle.refLine ? { route: vehicle.refLine } : {}),
        ...(vehicle.speed !== undefined ? { speed: vehicle.speed } : {}),
        ...(vehicle.bearing !== undefined ? { heading: vehicle.bearing } : {}),
      };
    },
    async update() {
      throw new ProblemError(403, { title: "This app writes nothing." });
    },
    mayEdit: () => false,
    portalLink: (entity) => (page.portal && page.space ? portalLinkOf(page.portal, page.space, entity.id) : null),
    language: "en",
  };
}

/** HSL's modes, in the order the filter shows them (T-3893). */
export const MODES = ["bus", "tram", "metro", "train", "ferry"] as const;

/** The vehicles of the chosen modes; one without a mode counts as a bus. */
export function ofModes(vehicles: Vehicle[], modes: ReadonlySet<string>): Vehicle[] {
  return vehicles.filter((vehicle) => modes.has(vehicle.mode ?? "bus"));
}

export interface Departure {
  line: string;
  headsign?: string;
  /** ISO 8601, UTC. */
  expected: string;
  /** scheduled, skipped or canceled. */
  status: string;
  /** Seconds behind the timetable, when HSL sends it. */
  delay?: number;
}

export interface StopBoard {
  id: string;
  name: string;
  coordinates?: [number, number];
  departures: Departure[];
}

export interface AlertView {
  id: string;
  /** HSL's text; rendered as text, never as HTML. */
  title: string;
  body?: string;
  severity?: string;
  validFrom?: string;
  validTo?: string;
  lines: string[];
  stops: string[];
  places: Array<{ stop: string; coordinates: [number, number] }>;
}

/** A JSON array from the backend, or nothing: a 503 before its first read, or no backend. */
async function listOf(path: string): Promise<unknown[]> {
  try {
    const response = await fetch(`${BASE}${path}`);
    if (!response.ok) return [];
    const body: unknown = await response.json();
    return Array.isArray(body) ? body : [];
  } catch {
    return [];
  }
}

/** The stops with departures, by name (T-3891). */
export async function getDepartures(): Promise<StopBoard[]> {
  return (await listOf("api/departures")).filter(
    (stop): stop is StopBoard =>
      typeof (stop as StopBoard)?.id === "string" && Array.isArray((stop as StopBoard).departures),
  );
}

/** HSL's service alerts, most severe first (T-3892). */
export async function getAlerts(): Promise<AlertView[]> {
  return (await listOf("api/alerts")).filter(
    (alert): alert is AlertView =>
      typeof (alert as AlertView)?.title === "string" && Array.isArray((alert as AlertView).lines),
  );
}

/** The alerts on a line, or every alert without one. */
export function alertsFor(alerts: AlertView[], line: string | undefined): AlertView[] {
  return line ? alerts.filter((alert) => alert.lines.includes(line)) : alerts;
}

/** Whole minutes from `now` to `expected`, never below zero; undefined for a time it cannot read. */
export function minutesUntil(expected: string, now: number = Date.now()): number | undefined {
  const at = Date.parse(expected);
  if (Number.isNaN(at)) return undefined;
  return Math.max(0, Math.floor((at - now) / 60000));
}

/** "now", "1 min", "12 min", or the clock time an hour or more ahead. */
export function when(expected: string, now: number = Date.now()): string {
  const minutes = minutesUntil(expected, now);
  if (minutes === undefined) return "";
  if (minutes === 0) return "now";
  if (minutes < 60) return `${minutes} min`;
  return new Date(expected).toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit", timeZone: "Europe/Helsinki" });
}

export interface AlertFeature {
  type: "Feature";
  id: string;
  geometry: { type: "Point"; coordinates: [number, number] };
  properties: { alert: string; stop: string; title: string };
}

/** One point per stop an alert names and the space places, for the map's alert layer. */
export function alertPlaces(alerts: AlertView[]): { type: "FeatureCollection"; features: AlertFeature[] } {
  return {
    type: "FeatureCollection",
    features: alerts.flatMap((alert) =>
      alert.places.map((place) => ({
        type: "Feature" as const,
        id: `${alert.id}#${place.stop}`,
        geometry: { type: "Point" as const, coordinates: place.coordinates },
        properties: { alert: alert.id, stop: place.stop, title: alert.title },
      })),
    ),
  };
}
