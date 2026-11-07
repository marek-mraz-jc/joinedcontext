import { useEffect, useMemo, useRef, useState } from "react";
import { Map as MapLibreMap } from "maplibre-gl";
import type { GeoJSONSource } from "maplibre-gl";
import "maplibre-gl/dist/maplibre-gl.css";
import { AppShell, Card, currentTokens, NO_BASEMAP, Page, Split, styleFor, useEntitySelection } from "@joinedcontext/sdk";
import {
  alertPlaces,
  alertsFor,
  featureCollection,
  getAlerts,
  getDepartures,
  getVehicles,
  MODES,
  ofModes,
  when,
  type AlertView,
  type StopBoard,
  inView,
  lineColor,
  panelSource,
  perLine,
  speedBands,
  subscribe,
  type Bar,
  type Bounds,
  type Vehicle,
  type VehicleCollection,
} from "./api";

const SOURCE = "vehicles";
const ALERTS = "alerts";
/** The departures and alerts are read again this often; the backend reads them every 30 s. */
const BOARD_MS = 30_000;
const ARROW = "heading-arrow";
const ARROW_SIZE = 24;
const HELSINKI: [number, number] = [24.94, 60.17];

/**
 * The project's basemap, as the page the App's server writes carries it: `#jc-config.basemap`,
 * the Portal's basemap route (AP-67, T-2928). The App names no tile host of its own; without a
 * configured basemap the map is the SDK's plain background and says so.
 */
export function basemapOf(doc: Document = document): string | undefined {
  const basemap = pageConfig(doc).basemap;
  return typeof basemap === "string" && basemap.startsWith("https://") ? basemap : undefined;
}

/** The `#jc-config` the App's server writes into its page (`JC_APP_CONFIG`), or nothing. */
function pageConfig(doc: Document): Record<string, unknown> {
  const text = doc.getElementById("jc-config")?.textContent;
  if (!text) return {};
  try {
    const parsed: unknown = JSON.parse(text);
    return parsed !== null && typeof parsed === "object" ? (parsed as Record<string, unknown>) : {};
  } catch {
    return {};
  }
}

/** The project's page in the Portal and the space, for the entity panel's link (SDK-40). */
export function portalOf(doc: Document = document): { portal?: string; space?: string } {
  const { portal, space } = pageConfig(doc);
  return {
    ...(typeof portal === "string" && portal.startsWith("https://") ? { portal } : {}),
    ...(typeof space === "string" ? { space } : {}),
  };
}

/**
 * A white arrow pointing north on a transparent square, drawn pixel by pixel so the map needs no
 * sprite sheet and no canvas; the symbol layer turns it to each bus's heading.
 */
export function arrowImage(size = ARROW_SIZE): { width: number; height: number; data: Uint8Array } {
  const data = new Uint8Array(size * size * 4);
  const middle = (size - 1) / 2;
  for (let y = 0; y < size; y += 1) {
    // Tip at the top, base at three quarters of the height, a third of the square wide there.
    const top = size * 0.2;
    const bottom = size * 0.72;
    if (y < top || y > bottom) continue;
    const half = ((y - top) / (bottom - top)) * (size * 0.26);
    for (let x = 0; x < size; x += 1) {
      if (Math.abs(x - middle) <= half) {
        data.set([255, 255, 255, 255], (y * size + x) * 4);
      }
    }
  }
  return { width: size, height: size, data };
}

/**
 * How live the map is: updating while the backend's feed is live, and since when it is lost when
 * it is not (T-3581), so a map that stopped moving says so instead of looking calm.
 */
export function feedState(live: boolean, lostSince: string | null): string {
  if (lostSince) {
    const at = new Date(lostSince);
    const time = Number.isNaN(at.getTime())
      ? lostSince
      : at.toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit", timeZone: "Europe/Helsinki" });
    return `, live feed lost since ${time}`;
  }
  return live ? ", updating live" : "";
}

/**
 * The fleet, seeded from the backend's snapshot and kept current by the stream. A message
 * carries only what moved, so it is merged into what is already on the map rather than
 * replacing it (AP-41).
 */
export function useVehicles(): { vehicles: Vehicle[]; live: boolean; lostSince: string | null } {
  const [byId, setById] = useState<Map<string, Vehicle>>(new Map());
  const [live, setLive] = useState(false);
  const [lostSince, setLostSince] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    const merge = (batch: Vehicle[]) => {
      if (cancelled || batch.length === 0) {
        return;
      }
      setById((current) => {
        const next = new Map(current);
        for (const vehicle of batch) {
          next.set(vehicle.id, vehicle);
        }
        return next;
      });
    };
    void getVehicles().then(merge);
    const close = subscribe(
      (batch) => {
        setLive(true);
        merge(batch);
      },
      (since) => !cancelled && setLostSince(since),
    );
    return () => {
      cancelled = true;
      close();
    };
  }, []);

  return { vehicles: [...byId.values()], live, lostSince };
}

/**
 * The departures and alerts, read when the page opens and every half minute after; what was read
 * last stays on the page when a read fails.
 */
export function useBoard(): { stops: StopBoard[]; alerts: AlertView[] } {
  const [stops, setStops] = useState<StopBoard[]>([]);
  const [alerts, setAlerts] = useState<AlertView[]>([]);
  useEffect(() => {
    let cancelled = false;
    const read = () => {
      void getDepartures().then((next) => !cancelled && next.length > 0 && setStops(next));
      void getAlerts().then((next) => !cancelled && setAlerts(next));
    };
    read();
    const timer = setInterval(read, BOARD_MS);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, []);
  return { stops, alerts };
}

type AlertCollection = ReturnType<typeof alertPlaces>;
const NO_ALERTS: AlertCollection = alertPlaces([]);

/** The map, and the one source every update writes to. */
export function VehicleMap({
  collection,
  alerts = NO_ALERTS,
  onView,
  onPick,
  basemap,
}: {
  collection: VehicleCollection;
  /** The stops HSL's alerts name, marked on the map. */
  alerts?: AlertCollection;
  /** The basemap's style URL; the SDK's plain background without one. */
  basemap?: string;
  /** Where the map looks, once it has loaded and after every pan or zoom. */
  onView?: (bounds: Bounds) => void;
  /** Called with the id of the bus a person clicks. */
  onPick?: (id: string) => void;
}) {
  const reportView = useRef(onView);
  const pick = useRef(onPick);
  useEffect(() => {
    reportView.current = onView;
    pick.current = onPick;
  }, [onView, onPick]);
  const container = useRef<HTMLDivElement>(null);
  const map = useRef<MapLibreMap | null>(null);
  const [ready, setReady] = useState(false);

  useEffect(() => {
    if (!container.current || map.current) {
      return;
    }
    const instance = new MapLibreMap({
      container: container.current,
      style: styleFor(basemap),
      center: HELSINKI,
      zoom: 11,
    });
    map.current = instance;
    instance.on("load", () => {
      // The alerts' stops under the vehicles: a ring in the tokens' danger colour.
      instance.addSource(ALERTS, { type: "geojson", data: alerts });
      instance.addLayer({
        id: "alert-stops",
        type: "circle",
        source: ALERTS,
        paint: {
          "circle-radius": 7,
          "circle-color": "#ffffff",
          "circle-stroke-width": 3,
          "circle-stroke-color": currentTokens().color.danger,
        },
      });
      instance.addSource(SOURCE, { type: "geojson", data: collection });
      instance.addLayer({
        id: "vehicle-dots",
        type: "circle",
        source: SOURCE,
        paint: {
          "circle-radius": 9,
          "circle-color": ["get", "color"],
          "circle-stroke-width": 2,
          "circle-stroke-color": "#ffffff",
        },
      });
      if (!instance.hasImage(ARROW)) {
        instance.addImage(ARROW, arrowImage());
      }
      // The heading, on top of the line's colour: the arrow turns with the map, so north on the
      // bus is north on the ground.
      instance.addLayer({
        id: "vehicle-heading",
        type: "symbol",
        source: SOURCE,
        layout: {
          "icon-image": ARROW,
          "icon-size": 0.6,
          "icon-rotate": ["get", "bearing"],
          "icon-rotation-alignment": "map",
          "icon-allow-overlap": true,
          "icon-ignore-placement": true,
        },
      });
      setReady(true);
      // The container may have taken its final size after the map measured it (the page's grid
      // settles once the charts beside it render): measure again before saying where it looks.
      instance.resize();
      reportView.current?.(instance.getBounds().toArray() as Bounds);
    });
    // A bus clicked opens in the shell's entity panel; the map is built once, so the handler
    // calls whichever pick the page holds now.
    instance.on("click", "vehicle-dots", (event: { features?: Array<{ properties?: Record<string, unknown> }> }) => {
      const id = event.features?.[0]?.properties?.id;
      if (typeof id === "string") pick.current?.(id);
    });
    const report = () => reportView.current?.(instance.getBounds().toArray() as Bounds);
    instance.on("moveend", report);
    instance.on("resize", report);
    return () => {
      instance.remove();
      map.current = null;
    };
    // The collection is deliberately not a dependency: the map is built once and every later
    // fleet reaches it through setData below, which is what keeps a moving bus from
    // rebuilding the layer on every frame.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (!ready) {
      return;
    }
    const source = map.current?.getSource(SOURCE) as GeoJSONSource | undefined;
    source?.setData(collection);
  }, [collection, ready]);

  useEffect(() => {
    if (!ready) {
      return;
    }
    const source = map.current?.getSource(ALERTS) as GeoJSONSource | undefined;
    source?.setData(alerts);
  }, [alerts, ready]);

  return (
    <>
      <div className="map" ref={container} data-testid="map" role="application" aria-label="Vehicle map" />
      {!basemap && <p className="note">{NO_BASEMAP}</p>}
    </>
  );
}

/**
 * A bar per row as plain HTML: a screen reader reads it as the list it is, it wraps on a phone,
 * and the bar's length is the only style computed from the data (its colour is a token).
 */
export function BarChart({ title, bars, note }: { title: string; bars: Bar[]; note?: string }) {
  const most = Math.max(1, ...bars.map((bar) => bar.value));
  return (
    <Card title={title}>
      {bars.every((bar) => bar.value === 0) ? (
        <p className="empty">No vehicles in view.</p>
      ) : (
        <ol className="bars" aria-label={title}>
          {bars.map((bar) => (
            <li className="bar" key={bar.label}>
              <span className="bar-label">{bar.label}</span>
              <span className="bar-track" aria-hidden="true">
                <span
                  className="bar-fill"
                  data-testid="bar-fill"
                  style={{ width: `${(bar.value / most) * 100}%`, background: bar.color }}
                />
              </span>
              <span className="bar-value">{bar.value}</span>
            </li>
          ))}
        </ol>
      )}
      {note && <p className="note">{note}</p>}
    </Card>
  );
}

/**
 * The live buses in the SDK's shell (SDK-39): a bus clicked on the map, or chosen from the list of
 * those in view, opens in the shell's entity panel (SDK-40), read from this app's own backend.
 */
export function App() {
  const source = useMemo(() => panelSource(portalOf()), []);
  return <AppShell title="HSL live" pages={[{ id: "buses", label: "HSL live", render: () => <BusesLive /> }]} source={source} language="en" />;
}

/** The modes to draw: every one at first; a person ticks off what they do not want to see. */
export function ModeFilter({ vehicles, modes, onChange }: { vehicles: Vehicle[]; modes: ReadonlySet<string>; onChange: (modes: Set<string>) => void }) {
  const count = (mode: string) => vehicles.filter((vehicle) => (vehicle.mode ?? "bus") === mode).length;
  return (
    <fieldset className="modes">
      <legend>Show</legend>
      {MODES.map((mode) => (
        <label key={mode}>
          <input
            type="checkbox"
            checked={modes.has(mode)}
            onChange={(event) => {
              const next = new Set(modes);
              if (event.target.checked) next.add(mode);
              else next.delete(mode);
              onChange(next);
            }}
          />
          {`${mode} (${count(mode)})`}
        </label>
      ))}
    </fieldset>
  );
}

/** The next departures from one stop, chosen from the stops that have them. */
export function Departures({ stops, now = Date.now() }: { stops: StopBoard[]; now?: number }) {
  const [chosen, setChosen] = useState("");
  const stop = stops.find((candidate) => candidate.id === chosen) ?? stops[0];
  return (
    <Card title="Departures">
      {stops.length === 0 ? (
        <p className="empty">No departures yet.</p>
      ) : (
        <>
          <label className="pick">
            <span>Stop</span>
            <select value={stop?.id ?? ""} onChange={(event) => setChosen(event.target.value)}>
              {stops.map((candidate) => (
                <option key={candidate.id} value={candidate.id}>
                  {candidate.name}
                </option>
              ))}
            </select>
          </label>
          <ol className="departures" aria-label={`Departures from ${stop.name}`}>
            {stop.departures.map((departure, index) => (
              <li key={`${departure.line}-${departure.expected}-${index}`} className={departure.status === "scheduled" ? "departure" : "departure off"}>
                <span className="departure-line">{departure.line}</span>
                <span className="departure-to">{departure.headsign ?? ""}</span>
                <span className="departure-when">
                  {departure.status === "canceled" ? "cancelled" : departure.status === "skipped" ? "skips this stop" : when(departure.expected, now)}
                </span>
              </li>
            ))}
          </ol>
        </>
      )}
    </Card>
  );
}

/** HSL's service alerts, all or those on one line; the text is HSL's and rendered as text. */
export function Alerts({ alerts }: { alerts: AlertView[] }) {
  const [line, setLine] = useState("");
  const lines = useMemo(
    () => [...new Set(alerts.flatMap((alert) => alert.lines))].sort((a, b) => a.localeCompare(b, "en", { numeric: true })),
    [alerts],
  );
  const shown = alertsFor(alerts, line || undefined);
  return (
    <Card title="Service alerts">
      {alerts.length === 0 ? (
        <p className="empty">No service alerts.</p>
      ) : (
        <>
          {lines.length > 0 && (
            <label className="pick">
              <span>Line</span>
              <select value={line} onChange={(event) => setLine(event.target.value)}>
                <option value="">All lines</option>
                {lines.map((candidate) => (
                  <option key={candidate} value={candidate}>
                    {candidate}
                  </option>
                ))}
              </select>
            </label>
          )}
          {shown.length === 0 ? (
            <p className="empty">{`No alerts on line ${line}.`}</p>
          ) : (
            <ul className="alerts" aria-label="Service alerts">
              {shown.map((alert) => (
                <li key={alert.id} className={`alert ${alert.severity ?? ""}`}>
                  <strong>{alert.title}</strong>
                  {alert.body && <p>{alert.body}</p>}
                  {alert.lines.length > 0 && <p className="note">{`Lines ${alert.lines.join(", ")}`}</p>}
                </li>
              ))}
            </ul>
          )}
        </>
      )}
    </Card>
  );
}

function BusesLive() {
  const { vehicles: fleet, live, lostSince } = useVehicles();
  const { stops, alerts } = useBoard();
  const [modes, setModes] = useState<Set<string>>(() => new Set(MODES));
  const vehicles = useMemo(() => ofModes(fleet, modes), [fleet, modes]);
  const places = useMemo(() => alertPlaces(alerts), [alerts]);
  const { selected, select } = useEntitySelection();
  const [bounds, setBounds] = useState<Bounds | undefined>(undefined);
  const [basemap] = useState(() => basemapOf());
  const collection = useMemo(() => featureCollection(vehicles), [vehicles]);
  // The charts and the legend follow the map: what they count is what the person looks at.
  const shown = useMemo(() => inView(vehicles, bounds), [vehicles, bounds]);
  const lines = useMemo(() => perLine(shown), [shown]);
  const speeds = useMemo(() => speedBands(shown), [shown]);
  const legend = useMemo(
    () => [...new Set(shown.map((vehicle) => vehicle.refLine).filter(Boolean))].sort((a, b) =>
      (a as string).localeCompare(b as string, "en", { numeric: true }),
    ) as string[],
    [shown],
  );

  return (
    <div className="app">
      <Page width="full">
        <p className="status" role="status">
          {vehicles.length === 0
            ? fleet.length === 0
              ? "Waiting for the first positions"
              : "No vehicles of the chosen modes"
            : `${vehicles.length} vehicles${feedState(live, lostSince)}${bounds && shown.length !== vehicles.length ? `, ${shown.length} in view` : ""}`}
        </p>
        <Split ratio="2:1">
          <div className="map-pane">
            <ModeFilter vehicles={fleet} modes={modes} onChange={setModes} />
            <VehicleMap collection={collection} alerts={places} onView={setBounds} onPick={(id) => select({ id, type: "Vehicle" })} basemap={basemap} />
            {shown.length > 0 && (
              <label className="pick">
                <span>Open a vehicle</span>
                <select value={selected?.id ?? ""} onChange={(event) => event.target.value && select({ id: event.target.value, type: "Vehicle" })}>
                  <option value="">—</option>
                  {[...shown]
                    .sort((a, b) => a.id.localeCompare(b.id, "en", { numeric: true }))
                    .map((vehicle) => (
                      <option key={vehicle.id} value={vehicle.id}>
                        {`${vehicle.id.slice(vehicle.id.lastIndexOf(":") + 1)}${vehicle.refLine ? `, line ${vehicle.refLine}` : ""}`}
                      </option>
                    ))}
                </select>
              </label>
            )}
            {legend.length > 0 && (
              <ul className="lines" aria-label="Lines on the map">
                {legend.map((line) => (
                  <li className="line" key={line}>
                    <span className="swatch" style={{ background: lineColor(line) }} />
                    {line}
                  </li>
                ))}
              </ul>
            )}
          </div>
          <div className="charts">
            <Departures stops={stops} />
            <Alerts alerts={alerts} />
            <BarChart title="Vehicles per line" bars={lines} />
            <BarChart
              title="Speed"
              bars={speeds.bars}
              note={speeds.unknown > 0 ? `${speeds.unknown} without a speed reading` : undefined}
            />
          </div>
        </Split>
      </Page>
    </div>
  );
}
