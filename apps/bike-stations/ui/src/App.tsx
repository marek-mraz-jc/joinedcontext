/**
 * The mobility project's city bike stations (T-3185, AP-04, AP-14, UI-15, UI-30): every HSL docking
 * station with its free bikes and free docks, read through the project's shared space reference
 * `city-bikes`, which names the helsinki project's public `helsinki-bikes` endpoint. The application
 * reads and never writes, and holds no token of its own (AP-28).
 *
 * A map is not readable by a screen reader or a keyboard, so the same stations are a list beside
 * it; the list is the screen and the map is the picture of it. It sits in the SDK's shell (SDK-39),
 * and a station picked in the list or on the map opens in the shell's entity panel (SDK-40), read
 * fresh through the shared endpoint; the App writes nothing, so the panel links it to the Portal.
 */
import { useEffect, useMemo, useRef, useState } from "react";
import { Map as MapLibreMap } from "maplibre-gl";
import type { GeoJSONSource } from "maplibre-gl";
import "maplibre-gl/dist/maplibre-gl.css";
import { AppShell, Empty, endpointSource, Loading, Page, Problem, styleFor, transportFor, useClient, useEntitySelection } from "@joinedcontext/sdk";
import type { EntitySource } from "@joinedcontext/sdk";
import { byName, featuresOf, matches, STANDING_COLOUR, STANDING_SHAPE, standingOf, stationOf, totals, TYPE } from "./stations";
import type { Station } from "./stations";
import { stringsFor } from "./locales";
import type { Strings } from "./locales";

/** The space the shared stations live in: the helsinki project's, through `city-bikes`. */
const SHARED_SPACE = "helsinki";
/** One request's page, and the most stations the map holds; Helsinki has about 460. */
const PAGE = 200;
export const MOST = 2000;
/** The centre of Helsinki, where the map opens. */
const CENTRE: [number, number] = [24.9384, 60.1699];
const SOURCE_ID = "stations";

export type Load =
  | { status: "loading" }
  | { status: "ready"; stations: Station[]; truncated: boolean }
  | { status: "failed"; reason: string };

/** The endpoint of the shared stations, found by its space and never by position (AP-04). */
function useStationsEndpoint(): { slug: string; name: string } | null {
  const { config } = useClient();
  return config.endpoints?.find((candidate) => candidate.space === SHARED_SPACE) ?? null;
}

/** A refusal's words (a `SourceError` is an `Error`), else whatever was thrown. */
function reasonOf(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

/** Every station of the shared endpoint, page by page up to `MOST`. */
export async function loadStations(source: EntitySource, locale: string): Promise<Load> {
  const stations: Station[] = [];
  for (let offset = 0; offset < MOST; offset += PAGE) {
    const page = await source.query({ type: TYPE }, { offset, limit: PAGE });
    stations.push(...page.rows.map((row) => stationOf(row, locale)));
    if (page.rows.length < PAGE) return { status: "ready", stations, truncated: false };
  }
  return { status: "ready", stations, truncated: true };
}

export function useStations(): Load | null {
  const { config } = useClient();
  const slug = useStationsEndpoint()?.slug ?? null;
  const language = config.language ?? "en";
  const [load, setLoad] = useState<Load>({ status: "loading" });

  useEffect(() => {
    if (!slug) return;
    let live = true;
    setLoad({ status: "loading" });
    const source = endpointSource(slug, transportFor(config), language);
    loadStations(source, language.slice(0, 2))
      .catch((cause: unknown): Load => ({ status: "failed", reason: reasonOf(cause) }))
      .then((next) => {
        if (live) setLoad(next);
      });
    return () => {
      live = false;
    };
    // `config` is the document the Portal served once; the slug and the language are what change.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [slug, language]);

  return slug ? load : null;
}

export default function App() {
  const { config } = useClient();
  const s = stringsFor(config.language);
  return <AppShell title={s.title} pages={[{ id: "stations", label: s.title, render: () => <Stations /> }]} language={config.language} />;
}

function Stations() {
  const { config } = useClient();
  const s = stringsFor(config.language);
  const load = useStations();
  const endpoint = useStationsEndpoint()?.name;
  const { selected, select } = useEntitySelection();
  const [search, setSearch] = useState("");
  const pickedId = selected?.id ?? null;

  const all = useMemo(() => (load?.status === "ready" ? load.stations : []), [load]);
  const visible = useMemo(() => all.filter((station) => matches(station, search)).sort(byName), [all, search]);
  const sum = totals(all);
  // A station opens in the shell's panel, which gives the focus back to what opened it on close.
  const pick = (id: string) => select({ id, type: TYPE, endpoint });

  return (
    <Page>
      <p className="subtitle">{s.subtitle}</p>

      {load === null && <Empty>{s.noEndpoint}</Empty>}
      {load?.status === "loading" && <Loading label={s.loading} />}
      {load?.status === "failed" && <Problem error={new Error(s.refused(load.reason))} />}
      {load?.status === "ready" && (
        <>
          <p className="totals" role="status">
            {s.totals(all.length, sum.bikes, sum.docks)}
          </p>
          {load.truncated && <p className="note">{s.truncated(MOST)}</p>}
          <div className="controls">
            <div className="search">
              <label htmlFor="search">{s.search}</label>
              <input
                id="search"
                type="search"
                value={search}
                onChange={(event) => setSearch(event.target.value)}
                aria-describedby="search-help"
              />
              <small id="search-help">{s.searchHelp}</small>
            </div>
            <ul className="legend" aria-label={s.status}>
              {(["available", "empty", "full", "unknown"] as const).map((standing) => (
                <li key={standing}>
                  <span aria-hidden="true" className="shape" style={{ color: STANDING_COLOUR[standing] }}>
                    {STANDING_SHAPE[standing]}
                  </span>
                  {s.standing[standing]}
                </li>
              ))}
            </ul>
          </div>

          <div className="map-screen">
            <StationMap stations={visible} picked={pickedId} onPick={pick} s={s} />
            <div className="side">
              <section className="results" aria-labelledby="results-heading">
                <h2 id="results-heading">{s.results(visible.length)}</h2>
                {visible.length === 0 ? <p>{s.noResults}</p> : null}
                <ul>
                  {visible.map((station) => {
                    const standing = standingOf(station);
                    return (
                      <li key={station.id}>
                        <button
                          type="button"
                          data-id={station.id}
                          aria-pressed={station.id === pickedId}
                          onClick={() => pick(station.id)}
                        >
                          <span aria-hidden="true" className="shape" style={{ color: STANDING_COLOUR[standing] }}>
                            {STANDING_SHAPE[standing]}
                          </span>
                          <span className="name">{station.name ?? s.unnamed}</span>
                          <span className="sub">
                            {s.bikesAndDocks(station.bikes, station.docks)} · {s.standing[standing]}
                            {station.coordinates === null ? ` · ${s.notOnMap}` : ""}
                          </span>
                        </button>
                      </li>
                    );
                  })}
                </ul>
              </section>
            </div>
          </div>
        </>
      )}

      <p className="source">{s.attribution}</p>
    </Page>
  );
}

function StationMap({
  stations,
  picked,
  onPick,
  s,
}: {
  stations: Station[];
  picked: string | null;
  onPick: (id: string) => void;
  s: Strings;
}) {
  // The basemap is the platform's, named in the document the Portal served (AP-67, AP-12).
  const { basemap } = useClient().config;
  const holder = useRef<HTMLDivElement | null>(null);
  const map = useRef<MapLibreMap | null>(null);
  const ready = useRef(false);
  const collection = useMemo(() => featuresOf(stations, picked), [stations, picked]);
  const latest = useRef(collection);
  latest.current = collection;
  // The click handler is bound once with the map; it opens what the latest render knows.
  const latestPick = useRef(onPick);
  latestPick.current = onPick;

  useEffect(() => {
    if (!holder.current || map.current) return;
    const drawn = new MapLibreMap({ container: holder.current, style: styleFor(basemap), center: CENTRE, zoom: 12 });
    drawn.on("load", () => {
      ready.current = true;
      drawn.addSource(SOURCE_ID, { type: "geojson", data: latest.current });
      drawn.addLayer({
        id: SOURCE_ID,
        type: "circle",
        source: SOURCE_ID,
        paint: {
          "circle-radius": ["case", ["get", "picked"], 10, 6],
          "circle-color": ["get", "colour"],
          "circle-stroke-width": ["case", ["get", "picked"], 3, 1.5],
          "circle-stroke-color": "#ffffff",
        },
      });
    });
    drawn.on("click", SOURCE_ID, (event: { features?: Array<{ properties?: Record<string, unknown> }> }) => {
      const id = event.features?.[0]?.properties?.id;
      if (typeof id === "string") latestPick.current(id);
    });
    map.current = drawn;
    return () => {
      drawn.remove();
      map.current = null;
      ready.current = false;
    };
    // The map is built once; the effect below keeps its data current.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (!map.current || !ready.current) return;
    (map.current.getSource(SOURCE_ID) as GeoJSONSource | undefined)?.setData(collection);
  }, [collection]);

  return (
    <div className="jc-map">
      <div className="jc-map-canvas" ref={holder} data-testid="jc-map" role="application" aria-label={s.mapLabel} />
      {!basemap && <p className="jc-map-notice">{s.noBasemap}</p>}
    </div>
  );
}
