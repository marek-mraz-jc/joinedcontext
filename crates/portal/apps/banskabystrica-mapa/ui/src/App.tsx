/**
 * The city's map for its residents (T-2782, AP-07, AP-14, UI-15, UI-30): the events the city
 * announces, every school of the national school map and the air-quality station, read from the
 * public space `banskabystrica-verejne` through its public endpoint, so the bundle holds no token
 * and never asks anybody to sign in (AP-28).
 *
 * A map is not readable by a screen reader or a keyboard, so the same places are a list beside it;
 * the list is the screen and the map is the picture of it. Each kind loads on its own: one the
 * endpoint refuses is a sentence saying why, and the others stay. The page sits in the SDK's
 * `AppShell` (SDK-39), and a place picked in the list or on the map opens in the SDK's entity panel
 * (SDK-40): a public App, so the panel reads it and links to it in the Portal (AP-140).
 */
import { useEffect, useMemo, useRef, useState } from "react";
import { Map as MapLibreMap } from "maplibre-gl";
import type { GeoJSONSource } from "maplibre-gl";
import "maplibre-gl/dist/maplibre-gl.css";
import { AppShell, endpointSource, Page, SourceError, styleFor, transportFor, useClient, useEntitySelection } from "@joinedcontext/sdk";
import type { EntitySource } from "@joinedcontext/sdk";
import { byOrder, featuresOf, KIND_COLOUR, KIND_SHAPE, KINDS, matches, placeOf, TYPE_OF, upcoming } from "./places";
import type { Kind, Place } from "./places";
import { stringsFor } from "./locales";
import type { Strings } from "./locales";

/** The space this application reads (`Development/10` §2). */
const SPACE = "banskabystrica-verejne";
/** One request's page, and the most of one kind the map holds: a list a person reads, not a dump. */
const PAGE = 200;
export const MOST = 1000;
/** The city centre, where the map opens when no place has a position. */
const CENTRE: [number, number] = [19.1462, 48.7359];
const SOURCE_ID = "places";

type Layer =
  | { status: "loading" }
  | { status: "ready"; places: Place[]; truncated: boolean }
  | { status: "failed"; reason: string };

function useEndpointSlug(): string | null {
  const { config } = useClient();
  const listed = config.endpoints?.find((candidate) => candidate.space === SPACE)?.slug;
  return listed ?? (config.space === SPACE ? config.slug : null) ?? null;
}

function reasonOf(cause: unknown): string {
  if (cause instanceof SourceError) return cause.message;
  return cause instanceof Error ? cause.message : String(cause);
}

/** Every place of one kind, page by page up to `MOST`. */
async function loadKind(source: EntitySource, kind: Kind, locale: string): Promise<Layer> {
  const places: Place[] = [];
  for (let offset = 0; offset < MOST; offset += PAGE) {
    const page = await source.query({ type: TYPE_OF[kind] }, { offset, limit: PAGE });
    places.push(...page.rows.map((row) => placeOf(row, kind, locale)));
    if (page.rows.length < PAGE) return { status: "ready", places, truncated: false };
  }
  return { status: "ready", places, truncated: true };
}

export function useLayers(): { layers: Record<Kind, Layer> | null } {
  const { config } = useClient();
  const slug = useEndpointSlug();
  const language = config.language ?? "sk";
  const [layers, setLayers] = useState<Record<Kind, Layer> | null>(null);
  const source = useMemo(
    () => (slug ? endpointSource(slug, transportFor(config), language) : null),
    // `config` is the document the Portal served once; the slug and the language are what change.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [slug, language],
  );

  useEffect(() => {
    if (!source) return;
    let live = true;
    setLayers({ event: { status: "loading" }, school: { status: "loading" }, air: { status: "loading" } });
    for (const kind of KINDS) {
      loadKind(source, kind, language.slice(0, 2))
        .catch((cause: unknown): Layer => ({ status: "failed", reason: reasonOf(cause) }))
        .then((layer) => {
          if (live) setLayers((before) => (before ? { ...before, [kind]: layer } : before));
        });
    }
    return () => {
      live = false;
    };
  }, [source, language]);

  return { layers: source ? layers : null };
}

export default function App({ today = new Date().toISOString().slice(0, 10) }: { today?: string }) {
  const { config } = useClient();
  const s = stringsFor(config.language);
  return <AppShell title={s.title} language={s.locale} pages={[{ id: "map", label: s.title, render: () => <PlacesPage today={today} s={s} /> }]} />;
}

function PlacesPage({ today, s }: { today: string; s: Strings }) {
  const { layers } = useLayers();
  const [shown, setShown] = useState<Record<Kind, boolean>>({ event: true, school: true, air: true });
  const [search, setSearch] = useState("");
  const [upcomingOnly, setUpcomingOnly] = useState(true);
  // The place picked is the one open in the shell's panel; closing it gives the focus back.
  const { selected, select } = useEntitySelection();
  const pickedId = selected?.id ?? null;

  const all = useMemo(
    () => KINDS.flatMap((kind) => (layers?.[kind].status === "ready" ? layers[kind].places : [])),
    [layers],
  );
  const visible = useMemo(
    () =>
      all
        .filter((place) => shown[place.kind])
        .filter((place) => !upcomingOnly || upcoming(place, today))
        .filter((place) => matches(place, search))
        .sort(byOrder),
    [all, shown, upcomingOnly, today, search],
  );
  const pick = (id: string) => {
    const place = all.find((one) => one.id === id);
    if (place) select({ id, type: TYPE_OF[place.kind] });
  };

  const loading = layers !== null && KINDS.some((kind) => layers[kind].status === "loading");

  return (
    <Page>
      <p className="subtitle">{s.subtitle}</p>

      {layers === null && <p role="status">{s.noEndpoint}</p>}
      {layers !== null && (
        <>
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
            <fieldset className="layers">
              <legend>{s.show}</legend>
              {KINDS.map((kind) => {
                const layer = layers[kind];
                const count = layer.status === "ready" ? layer.places.length : null;
                return (
                  <label key={kind} className="layer">
                    <input
                      type="checkbox"
                      checked={shown[kind]}
                      onChange={(event) => setShown({ ...shown, [kind]: event.target.checked })}
                    />
                    <span aria-hidden="true" className="shape" style={{ color: KIND_COLOUR[kind] }}>
                      {KIND_SHAPE[kind]}
                    </span>
                    <span>
                      {s.kind[kind]}
                      {count !== null ? ` (${count})` : ""}
                    </span>
                  </label>
                );
              })}
              <label className="layer">
                <input type="checkbox" checked={upcomingOnly} onChange={(event) => setUpcomingOnly(event.target.checked)} />
                <span>{s.upcomingOnly}</span>
              </label>
            </fieldset>
          </div>

          {loading && <p role="status">{s.loading}</p>}
          {KINDS.map((kind) => {
            const layer = layers[kind];
            if (layer.status === "failed") {
              return (
                <p key={kind} role="alert" className="failed">
                  {s.refused(s.kind[kind], layer.reason)}
                </p>
              );
            }
            if (layer.status === "ready" && layer.truncated) {
              return (
                <p key={kind} className="note">
                  {s.truncated(s.kind[kind], MOST)}
                </p>
              );
            }
            return null;
          })}

          <div className="map-screen">
            <PlaceMap places={visible} picked={pickedId} onPick={pick} s={s} />
            <div className="side">
              <section className="results" aria-labelledby="results-heading">
                <h2 id="results-heading">{s.results(visible.length)}</h2>
                {!loading && visible.length === 0 ? <p>{s.noResults}</p> : null}
                <ul>
                  {visible.map((place) => (
                    <li key={place.id}>
                      <button
                        type="button"
                        data-id={place.id}
                        aria-pressed={place.id === pickedId}
                        onClick={() => pick(place.id)}
                      >
                        <span aria-hidden="true" className="shape" style={{ color: KIND_COLOUR[place.kind] }}>
                          {KIND_SHAPE[place.kind]}
                        </span>
                        <span className="name">{place.name ?? s.unnamed}</span>{" "}
                        <span className="sub">
                          {s.kind[place.kind]}
                          {place.kind === "event" && place.startDate ? ` · ${day(place.startDate, s)}` : ""}
                          {place.coordinates === null ? ` · ${s.notOnMap}` : ""}
                        </span>
                      </button>
                    </li>
                  ))}
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

function PlaceMap({
  places,
  picked,
  onPick,
  s,
}: {
  places: Place[];
  picked: string | null;
  onPick: (id: string) => void;
  s: Strings;
}) {
  // The basemap is the platform's, named in the document the Portal served; the app names no tile
  // host of its own, which the app's policy would refuse anyway (AP-67, AP-12).
  const { basemap } = useClient().config;
  const holder = useRef<HTMLDivElement | null>(null);
  const map = useRef<MapLibreMap | null>(null);
  const ready = useRef(false);
  const collection = useMemo(() => featuresOf(places, picked), [places, picked]);
  const latest = useRef(collection);
  latest.current = collection;
  // The map is built once: its click asks the latest handler, which knows the latest places.
  const pickRef = useRef(onPick);
  pickRef.current = onPick;

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
          "circle-radius": ["case", ["get", "picked"], 10, 7],
          "circle-color": ["get", "colour"],
          "circle-stroke-width": ["case", ["get", "picked"], 3, 1.5],
          "circle-stroke-color": "#ffffff",
        },
      });
    });
    drawn.on("click", SOURCE_ID, (event: { features?: Array<{ properties?: Record<string, unknown> }> }) => {
      const id = event.features?.[0]?.properties?.id;
      if (typeof id === "string") pickRef.current(id);
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

function day(iso: string, s: Strings): string {
  const at = new Date(`${iso.slice(0, 10)}T12:00:00`);
  return Number.isNaN(at.getTime()) ? iso : new Intl.DateTimeFormat(s.locale, { dateStyle: "medium" }).format(at);
}
