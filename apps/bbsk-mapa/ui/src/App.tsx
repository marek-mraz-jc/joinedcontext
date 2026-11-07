/**
 * The region's map for its residents (T-2784, AP-07, AP-14, UI-15, UI-30): the hospitals, the
 * public social services and the organizations of the Banskobystrický samosprávny kraj, read from
 * its public register space `bbsk-registre` through a public endpoint, so the bundle holds no token
 * and never asks anybody to sign in (AP-28).
 *
 * A map is not readable by a screen reader or a keyboard, so the same places are a list beside it;
 * the list is the screen and the map is the picture of it. Each kind loads on its own: one the
 * endpoint refuses is a sentence saying why, and the others stay. It sits in the SDK's shell
 * (SDK-39), and a place picked in the list or on the map opens in the shell's entity panel (SDK-40),
 * read fresh through the same endpoint; a public App, so the panel links it to the Portal (AP-140).
 */
import { useEffect, useMemo, useRef, useState } from "react";
import { Map as MapLibreMap } from "maplibre-gl";
import type { GeoJSONSource } from "maplibre-gl";
import "maplibre-gl/dist/maplibre-gl.css";
import { AppShell, Empty, endpointSource, Loading, Page, Problem, SourceError, styleFor, transportFor, useClient, useEntitySelection } from "@joinedcontext/sdk";
import type { EntitySource } from "@joinedcontext/sdk";
import { byOrder, CATEGORY_Q, featuresOf, KIND_COLOUR, KIND_SHAPE, KINDS, matches, placeOf, TYPE_OF } from "./places";
import type { Kind, Place } from "./places";
import { stringsFor } from "./locales";
import type { Strings } from "./locales";

/** The space this application reads (`Development/10` §2). */
const SPACE = "bbsk-registre";
/** One request's page, and the most of one kind the map holds: a list a person reads, not a dump. */
const PAGE = 200;
export const MOST = 1000;
/** The middle of the region, where the map opens. */
const CENTRE: [number, number] = [19.45, 48.55];
const SOURCE_ID = "places";

export type Layer =
  | { status: "loading" }
  | { status: "ready"; places: Place[]; truncated: boolean }
  | { status: "failed"; reason: string; denied: boolean };

/** The endpoint of the register space: the one the Portal lists for it, else the App's own. */
function useEndpoint(): { slug: string; name?: string } | null {
  const { config } = useClient();
  const listed = config.endpoints?.find((candidate) => candidate.space === SPACE);
  if (listed) return { slug: listed.slug, name: listed.name };
  return config.space === SPACE && config.slug ? { slug: config.slug } : null;
}

function reasonOf(cause: unknown): string {
  if (cause instanceof SourceError) return cause.message;
  return cause instanceof Error ? cause.message : String(cause);
}

/** A refusal: asking again never grants a right, so it offers no retry (T-4912). */
function deniedBy(cause: unknown): boolean {
  return cause instanceof SourceError && (cause.status === 401 || cause.status === 403);
}

/** Every place of one kind, page by page up to `MOST`. */
export async function loadKind(source: EntitySource, kind: Kind, locale: string): Promise<Layer> {
  const places: Place[] = [];
  for (let offset = 0; offset < MOST; offset += PAGE) {
    const page = await source.query({ type: TYPE_OF[kind], q: CATEGORY_Q[kind] }, { offset, limit: PAGE });
    places.push(...page.rows.map((row) => placeOf(row, kind, locale)));
    if (page.rows.length < PAGE) return { status: "ready", places, truncated: false };
  }
  return { status: "ready", places, truncated: true };
}

export function useLayers(): { layers: Record<Kind, Layer> | null; reload: () => void } {
  const { config } = useClient();
  const slug = useEndpoint()?.slug ?? null;
  const language = config.language ?? "sk";
  const [layers, setLayers] = useState<Record<Kind, Layer> | null>(null);
  const [attempt, setAttempt] = useState(0);
  const source = useMemo(
    () => (slug ? endpointSource(slug, transportFor(config), language) : null),
    // `config` is the document the Portal served once; the slug and the language are what change.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [slug, language],
  );

  useEffect(() => {
    if (!source) return;
    let live = true;
    setLayers({ hospital: { status: "loading" }, social: { status: "loading" }, organization: { status: "loading" } });
    for (const kind of KINDS) {
      loadKind(source, kind, language.slice(0, 2))
        .catch((cause: unknown): Layer => ({ status: "failed", reason: reasonOf(cause), denied: deniedBy(cause) }))
        .then((layer) => {
          if (live) setLayers((before) => (before ? { ...before, [kind]: layer } : before));
        });
    }
    return () => {
      live = false;
    };
  }, [source, language, attempt]);

  return { layers: source ? layers : null, reload: () => setAttempt((n) => n + 1) };
}

export default function App() {
  const { config } = useClient();
  const s = stringsFor(config.language);
  return <AppShell title={s.title} pages={[{ id: "map", label: s.title, render: () => <RegionMap /> }]} language={config.language} />;
}

function RegionMap() {
  const { config } = useClient();
  const s = stringsFor(config.language);
  const { layers, reload } = useLayers();
  const endpoint = useEndpoint()?.name;
  const { selected, select } = useEntitySelection();
  const [shown, setShown] = useState<Record<Kind, boolean>>({ hospital: true, social: true, organization: true });
  const [search, setSearch] = useState("");
  const pickedId = selected?.id ?? null;

  const all = useMemo(
    () => KINDS.flatMap((kind) => (layers?.[kind].status === "ready" ? layers[kind].places : [])),
    [layers],
  );
  const visible = useMemo(
    () =>
      all
        .filter((place) => shown[place.kind])
        .filter((place) => matches(place, search))
        .sort(byOrder),
    [all, shown, search],
  );
  // A place opens in the shell's panel, which gives the focus back to what opened it on close.
  const pick = (id: string) => {
    const place = all.find((candidate) => candidate.id === id);
    if (place) select({ id, type: TYPE_OF[place.kind], endpoint });
  };

  const loading = layers !== null && KINDS.some((kind) => layers[kind].status === "loading");
  // A layer that failed is unknown, not empty: its alert says so, "nothing found" would contradict it.
  const failed = layers !== null && KINDS.some((kind) => layers[kind].status === "failed");

  return (
    <Page>
      <p className="subtitle">{s.subtitle}</p>

      {layers === null && <Empty>{s.noEndpoint}</Empty>}
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
            </fieldset>
          </div>

          {loading && <Loading label={s.loading} />}
          <LayerNotes layers={layers} s={s} onRetry={reload} />

          <div className="map-screen">
            <PlaceMap places={visible} picked={pickedId} onPick={pick} s={s} />
            <div className="side">
              <section className="results" aria-labelledby="results-heading">
                <h2 id="results-heading">{s.results(visible.length)}</h2>
                {!loading && !failed && visible.length === 0 ? <p>{s.noResults}</p> : null}
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
                        <span className="name">{place.name ?? s.unnamed}</span>
                        <span className="sub">
                          {s.kind[place.kind]}
                          {place.district ? ` · ${place.district}` : ""}
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

/** Why a kind is missing, or that it was cut off at `MOST`: said beside the map, once per kind. */
export function LayerNotes({ layers, s, onRetry }: { layers: Record<Kind, Layer>; s: Strings; onRetry: () => void }) {
  return KINDS.map((kind) => {
    const layer = layers[kind];
    if (layer.status === "failed") {
      return (
        <Problem key={kind} error={new Error(s.refused(s.kind[kind], layer.reason))} onRetry={layer.denied ? undefined : onRetry} />
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
  });
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
  // The click handler is bound once with the map; it opens what the latest render knows.
  const latestPick = useRef(onPick);
  latestPick.current = onPick;

  useEffect(() => {
    if (!holder.current || map.current) return;
    const drawn = new MapLibreMap({ container: holder.current, style: styleFor(basemap), center: CENTRE, zoom: 8 });
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
