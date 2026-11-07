/**
 * The city's air-quality screen (T-2435, AP-07, AP-14, AP-45, UI-15, UI-30).
 *
 * One space, one type, read only, no login: the city's public space `banskabystrica-verejne`,
 * where the EEA's hourly readings of station SK0263A land (T-2781, T-2949), is read through a
 * public endpoint, so the bundle holds no token and never asks anybody to sign in (AP-28).
 *
 * Two things this screen is arranged against. A station that stopped reporting must not read as
 * clean air, so staleness is a band of its own and is decided before the thresholds
 * (`stations.ts`). And a map is not readable by a screen reader or a keyboard, so the same
 * stations are a list beside it, with the same colours said in words — the list is the screen and
 * the map is the picture of it. It sits in the SDK's shell (SDK-39); a station picked on the map
 * and a card's Details button open the station in the shell's entity panel (SDK-40), which links
 * it to the Portal: a public App writes nothing (AP-140).
 */
import { useEffect, useMemo, useRef, useState } from "react";
import { Map as MapLibreMap } from "maplibre-gl";
import type { GeoJSONSource } from "maplibre-gl";
import "maplibre-gl/dist/maplibre-gl.css";
import {
  AppShell,
  Empty,
  endpointSource,
  EntityHistory,
  Grid,
  Loading,
  Page,
  Problem,
  Split,
  styleFor,
  transportFor,
  useClient,
  useEntitySelection,
} from "@joinedcontext/sdk";
import type { EntitySource, RichRow } from "@joinedcontext/sdk";
import { BAND_COLOUR, bandOf, stationsOf } from "./stations";
import type { Band, Station } from "./stations";
import { BAND_SHAPE, nameOf, stringsFor } from "./locales";
import type { Strings } from "./locales";

/** The space this application reads (`Development/10` §2). */
const SPACE = "banskabystrica-verejne";

/** One page holds the city's stations several times over, and caps a hostile answer. */
const LIMIT = 200;

/** The city centre, where the map opens when no station has a location yet. */
const BANSKA_BYSTRICA: [number, number] = [19.1462, 48.7359];

const SOURCE_ID = "stations";

type Load =
  | { status: "loading" }
  | { status: "ready"; stations: Station[] }
  | { status: "failed"; reason: string }
  | { status: "unreachable" };

/** The endpoint this application reads, found by space and never by position (SDK-02). */
function useEndpoint(): { slug: string; name?: string } | null {
  const { config } = useClient();
  const listed = config.endpoints?.find((candidate) => candidate.space === SPACE);
  if (listed) return { slug: listed.slug, name: listed.name };
  // An application with one data need is served without the list; its own slug is the endpoint.
  return config.space === SPACE && config.slug ? { slug: config.slug } : null;
}

export function useStations(now: () => Date = () => new Date()): {
  load: Load;
  source: EntitySource | null;
} {
  const { config } = useClient();
  const slug = useEndpoint()?.slug ?? null;
  const language = config.language;
  const [load, setLoad] = useState<Load>({ status: "loading" });

  const source = useMemo(
    () => (slug ? endpointSource(slug, transportFor(config), language) : null),
    // `config` is the document the Portal served once; the slug and the language are what change.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [slug, language],
  );

  useEffect(() => {
    if (!source) {
      setLoad({ status: "unreachable" });
      return;
    }
    let live = true;
    setLoad({ status: "loading" });
    source
      .query({ type: "AirQualityObserved" }, { offset: 0, limit: LIMIT })
      .then((page) => {
        if (live) setLoad({ status: "ready", stations: stationsOf(page.rows as RichRow[], now()) });
      })
      .catch((cause: unknown) => {
        if (live) setLoad({ status: "failed", reason: reasonOf(cause) });
      });
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [source]);

  return { load, source };
}

/** A refusal's words (a `SourceError` is an `Error`), else whatever was thrown. */
function reasonOf(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

export default function App() {
  const { config } = useClient();
  const s = stringsFor(config.language);
  return <AppShell title={s.title} pages={[{ id: "air", label: s.title, render: () => <Air /> }]} language={config.language} />;
}

function Air() {
  const { config } = useClient();
  const s = stringsFor(config.language);
  const now = useMemo(() => new Date(), []);
  const { load, source } = useStations(() => now);
  const endpoint = useEndpoint()?.name;
  const { select } = useEntitySelection();
  const [pickedId, setPickedId] = useState<string | null>(null);

  const stations = load.status === "ready" ? load.stations : [];
  const picked = stations.find((station) => station.id === pickedId) ?? stations[0] ?? null;
  const open = (id: string) => select({ id, type: "AirQualityObserved", endpoint });

  return (
    <Page>
      <p className="subtitle">{s.subtitle}</p>

      {load.status === "loading" && <Loading label={s.loading} />}
      {load.status === "unreachable" && <Empty>{s.noEndpoint}</Empty>}
      {load.status === "failed" && <Problem error={new Error(`${s.failed} ${s.failedWhy}: ${load.reason}`)} />}
      {load.status === "ready" && stations.length === 0 && (
        <Empty>
          {s.empty} {s.emptyWhy}
        </Empty>
      )}

      {stations.length > 0 && (
        <>
          <Split ratio="2:1">
            <StationMap
              stations={stations}
              now={now}
              picked={picked}
              onPick={(id) => {
                // A station on the map is picked for its history and opened in the panel.
                setPickedId(id);
                open(id);
              }}
              s={s}
            />
            {picked && source ? <StationDetail station={picked} source={source} now={now} s={s} /> : null}
          </Split>
          <StationList
            stations={stations}
            now={now}
            pickedId={picked?.id ?? null}
            onPick={setPickedId}
            onOpen={open}
            s={s}
          />
          <p className="note">{s.limitNote}</p>
          <p className="note">{s.staleNote}</p>
        </>
      )}

      <p className="source">{s.source}</p>
    </Page>
  );
}

/** The stations as GeoJSON, which is the one thing the map is given (AP-41). */
export function featuresOf(stations: Station[], now: Date) {
  return {
    type: "FeatureCollection" as const,
    features: stations
      .filter((station) => station.coordinates !== null)
      .map((station) => ({
        type: "Feature" as const,
        id: station.id,
        geometry: { type: "Point" as const, coordinates: station.coordinates as [number, number] },
        properties: { id: station.id, colour: BAND_COLOUR[bandOf(station, now)] },
      })),
  };
}

function StationMap({
  stations,
  now,
  picked,
  onPick,
  s,
}: {
  stations: Station[];
  now: Date;
  picked: Station | null;
  onPick: (id: string) => void;
  s: Strings;
}) {
  // The basemap is the platform's, named in the document the Portal served; the app names no tile
  // host of its own, which the app's policy would refuse anyway (AP-67, AP-12).
  const { basemap } = useClient().config;
  const holder = useRef<HTMLDivElement | null>(null);
  const map = useRef<MapLibreMap | null>(null);
  const ready = useRef(false);
  const collection = featuresOf(stations, now);
  // The click handler is bound once with the map; it calls what the latest render passed.
  const latestPick = useRef(onPick);
  latestPick.current = onPick;
  const centre = picked?.coordinates ?? stations.find((one) => one.coordinates)?.coordinates ?? BANSKA_BYSTRICA;

  useEffect(() => {
    if (!holder.current || map.current) return;
    const drawn = new MapLibreMap({ container: holder.current, style: styleFor(basemap), center: centre, zoom: 11 });
    drawn.on("load", () => {
      ready.current = true;
      drawn.addSource(SOURCE_ID, { type: "geojson", data: collection });
      drawn.addLayer({
        id: SOURCE_ID,
        type: "circle",
        source: SOURCE_ID,
        paint: {
          "circle-radius": 8,
          "circle-color": ["get", "colour"],
          "circle-stroke-width": 2,
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
    // The map is built once; the data below keeps it current.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (!map.current || !ready.current) return;
    const source = map.current.getSource(SOURCE_ID) as GeoJSONSource | undefined;
    source?.setData(collection);
  }, [collection]);

  return (
    <div className="jc-map">
      {/* The map is a picture of the list below it, which carries the same stations in text and
          is what a screen reader reads (UI-15); the map itself pans and zooms by keyboard. */}
      <div className="jc-map-canvas" ref={holder} data-testid="jc-map" role="application" aria-label={s.mapLabel} />
      {!basemap && <p className="jc-map-notice">{s.noBasemap}</p>}
    </div>
  );
}

function StationList({
  stations,
  now,
  pickedId,
  onPick,
  onOpen,
  s,
}: {
  stations: Station[];
  now: Date;
  pickedId: string | null;
  onPick: (id: string) => void;
  onOpen: (id: string) => void;
  s: Strings;
}) {
  return (
    <section className="stations" aria-labelledby="stations-heading">
      <h2 id="stations-heading">{s.stationsLabel}</h2>
      <Grid columns={4}>
        {stations.map((station) => (
          <StationCard
            key={station.id}
            station={station}
            band={bandOf(station, now)}
            picked={station.id === pickedId}
            onPick={onPick}
            onOpen={onOpen}
            s={s}
          />
        ))}
      </Grid>
    </section>
  );
}

function StationCard({
  station,
  band,
  picked,
  onPick,
  onOpen,
  s,
}: {
  station: Station;
  band: Band;
  picked: boolean;
  onPick: (id: string) => void;
  onOpen: (id: string) => void;
  s: Strings;
}) {
  const name = nameOf(station, s);
  return (
    <article className={`station station-${band}${picked ? " picked" : ""}`}>
      <h3>{name}</h3>
      <p className={`band band-${band}`}>
        <span aria-hidden="true" className="shape" style={{ color: BAND_COLOUR[band] }}>
          {BAND_SHAPE[band]}
        </span>{" "}
        <span>
          {s.bandOf}: {s.band[band]}
        </span>
      </p>
      <dl>
        <dt>{s.pm10}</dt>
        <dd>{reading(station.pm10, s)}</dd>
        <dt>{s.pm25}</dt>
        <dd>{reading(station.pm25, s)}</dd>
        <dt>{s.measuredAt}</dt>
        <dd>
          {station.at ? (
            <time dateTime={station.at}>{moment(station.at, s)}</time>
          ) : (
            s.noReading
          )}
        </dd>
      </dl>
      {station.coordinates === null && <p className="note">{s.noLocation}</p>}
      <button type="button" onClick={() => onPick(station.id)} aria-pressed={picked}>
        {picked ? s.picked : s.pick}
      </button>
      <button type="button" onClick={() => onOpen(station.id)}>
        {s.details(name)}
      </button>
    </article>
  );
}

/** The latest values of the picked station, and one day of it from `queryTemporal`. */
function StationDetail({
  station,
  source,
  now,
  s,
}: {
  station: Station;
  source: EntitySource;
  now: Date;
  s: Strings;
}) {
  const name = nameOf(station, s);
  return (
    <section className="detail" aria-labelledby="detail-heading">
      <h2 id="detail-heading">
        {s.historyTitle}: {name}
      </h2>
      <EntityHistory
        source={source}
        id={station.id}
        attr="pm10"
        unit={s.unit}
        heading={`${s.historyOf} ${s.pm10}`}
        locale={s.locale}
        fractionDigits={1}
        labels={s.history}
        now={() => now}
      />
    </section>
  );
}

function reading(value: number | null, s: Strings): string {
  if (value === null) return s.noReading;
  return `${new Intl.NumberFormat(s.locale, { maximumFractionDigits: 1 }).format(value)} ${s.unit}`;
}

function moment(iso: string, s: Strings): string {
  const at = new Date(iso);
  return Number.isNaN(at.getTime())
    ? iso
    : new Intl.DateTimeFormat(s.locale, { dateStyle: "medium", timeStyle: "short" }).format(at);
}
