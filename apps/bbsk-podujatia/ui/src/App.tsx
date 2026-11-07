/**
 * The region's events dashboard (T-4897): every upcoming event the pipelines of T-4896 collect
 * from the towns, municipalities and bodies of the Banskobystrický samosprávny kraj, on a map, in a
 * list and in a month calendar, with filters by town, category, dates and text, and the calendar
 * link a person subscribes to (EP-125).
 *
 * It reads `bbsk-podujatia` through its own public endpoint, whose Policy keeps the region's scope
 * `/SK/BB/#` (AP-04, AP-28): no token, no sign-in, no write. The map is not readable by a screen
 * reader or a keyboard, so the list and the calendar carry the same events. An event picked opens
 * in the SDK's entity panel (SDK-40), read fresh through the same endpoint.
 */
import { useEffect, useMemo, useRef, useState } from "react";
import { Map as MapLibreMap } from "maplibre-gl";
import type { GeoJSONSource } from "maplibre-gl";
import "maplibre-gl/dist/maplibre-gl.css";
import { AppShell, Card, Empty, endpointSource, Loading, Page, Problem, SourceError, styleFor, Tabs, transportFor, useClient, useEntitySelection } from "@joinedcontext/sdk";
import type { EntitySource } from "@joinedcontext/sdk";
import {
  apiOrigin,
  bySoonest,
  calendarUrl,
  choices,
  dayOf,
  eventOf,
  featuresOf,
  matches,
  monthDays,
  monthOf,
  NO_FILTERS,
  onDay,
  rangeInvalid,
  shiftMonth,
  webcalUrl,
  ZONE,
} from "./events";
import type { EventItem, Filters } from "./events";
import { stringsFor } from "./locales";
import type { Strings } from "./locales";

/** The space this application reads (`Development/10` §2). */
const SPACE = "bbsk-podujatia";
const TYPE = "Event";
/** One request's page, the most the endpoint answers, and the most the dashboard holds. */
const PAGE = 1000;
export const MOST = 5000;
/** The middle of the region, where the map opens. */
const CENTRE: [number, number] = [19.45, 48.55];
const SOURCE_ID = "events";
/** Events a calendar day shows by name; the rest are counted. */
const PER_DAY = 3;

export type Loaded =
  | { status: "loading" }
  | { status: "ready"; events: EventItem[]; truncated: boolean }
  | { status: "failed"; reason: string; denied: boolean };

/** The endpoint of the events space: the one the Portal lists for it, else the App's own. */
export function useEndpoint(): { slug: string; name?: string } | null {
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

/** Every event, page by page up to `MOST`, soonest first; a row without a name or a start is left out. */
export async function loadEvents(source: EntitySource, locale: string): Promise<Loaded> {
  const events: EventItem[] = [];
  for (let offset = 0; offset < MOST; offset += PAGE) {
    const page = await source.query({ type: TYPE }, { offset, limit: PAGE });
    for (const row of page.rows) {
      const event = eventOf(row, locale);
      if (event) events.push(event);
    }
    if (page.rows.length < PAGE) return { status: "ready", events: events.sort(bySoonest), truncated: false };
  }
  return { status: "ready", events: events.sort(bySoonest), truncated: true };
}

function useEvents(): { loaded: Loaded | null; reload: () => void } {
  const { config } = useClient();
  const slug = useEndpoint()?.slug ?? null;
  const language = config.language ?? "sk";
  const [loaded, setLoaded] = useState<Loaded | null>(null);
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
    setLoaded({ status: "loading" });
    loadEvents(source, language.slice(0, 2))
      .catch((cause: unknown): Loaded => ({ status: "failed", reason: reasonOf(cause), denied: deniedBy(cause) }))
      .then((result) => {
        if (live) setLoaded(result);
      });
    return () => {
      live = false;
    };
  }, [source, language, attempt]);

  return { loaded: source ? loaded : null, reload: () => setAttempt((n) => n + 1) };
}

/** When an event takes place, in Slovak time, as a person reads it. */
export function whenOf(event: EventItem, s: Strings): string {
  const date = (day: string) =>
    new Intl.DateTimeFormat(s.locale, { timeZone: "UTC", weekday: "short", day: "numeric", month: "long" }).format(new Date(`${day}T12:00:00Z`));
  const time = (instant: Date) => new Intl.DateTimeFormat(s.locale, { timeZone: ZONE, hour: "2-digit", minute: "2-digit" }).format(instant);
  if (event.allDay) return event.endDay === event.startDay ? `${date(event.startDay)}, ${s.allDay}` : `${date(event.startDay)} – ${date(event.endDay)}`;
  if (event.endDay !== event.startDay) return `${date(event.startDay)} ${time(event.start)} – ${date(event.endDay)}`;
  return event.end ? `${date(event.startDay)}, ${time(event.start)} – ${time(event.end)}` : `${date(event.startDay)}, ${time(event.start)}`;
}

export default function App() {
  const { config } = useClient();
  const s = stringsFor(config.language);
  return <AppShell title={s.title} pages={[{ id: "events", label: s.title, render: () => <Dashboard /> }]} language={config.language} />;
}

function Dashboard() {
  const { config } = useClient();
  const s = stringsFor(config.language);
  const { loaded, reload } = useEvents();
  const endpoint = useEndpoint();
  const { selected, select } = useEntitySelection();
  const [filters, setFilters] = useState<Filters>(NO_FILTERS);
  const pickedId = selected?.id ?? null;

  const all = useMemo(() => (loaded?.status === "ready" ? loaded.events : []), [loaded]);
  const visible = useMemo(() => all.filter((event) => matches(event, filters)), [all, filters]);
  const pick = (id: string) => select({ id, type: TYPE, endpoint: endpoint?.name });

  if (loaded === null || endpoint === null) {
    return (
      <Page>
        <p className="subtitle">{s.subtitle}</p>
        <Empty>{s.noEndpoint}</Empty>
      </Page>
    );
  }

  const body =
    loaded.status === "loading" ? (
      <Loading label={s.loading} />
    ) : loaded.status === "failed" ? (
      <Problem error={new Error(s.refused(loaded.reason))} onRetry={loaded.denied ? undefined : reload} />
    ) : all.length === 0 ? (
      <Empty>{s.noEvents}</Empty>
    ) : (
      <>
        {loaded.truncated && <p className="note">{s.truncated(MOST)}</p>}
        <p className="count" role="status">
          {s.results(visible.length)}
        </p>
        {visible.length === 0 && <Empty>{s.noResults}</Empty>}
        <Tabs
          label={s.views}
          tabs={[
            { id: "map", label: s.view.map, render: () => <MapView events={visible} picked={pickedId} onPick={pick} s={s} /> },
            { id: "list", label: s.view.list, render: () => <EventList events={visible} picked={pickedId} onPick={pick} s={s} /> },
            { id: "calendar", label: s.view.calendar, render: () => <MonthView events={visible} picked={pickedId} onPick={pick} s={s} /> },
          ]}
        />
      </>
    );

  return (
    <Page>
      <p className="subtitle">{s.subtitle}</p>
      <div className="dashboard">
        <div className="main">
          <FilterBar filters={filters} onChange={setFilters} towns={choices(all, "town")} categories={choices(all, "category")} s={s} />
          {body}
        </div>
        <aside className="side">
          <Subscribe slug={endpoint.slug} filters={filters} s={s} />
        </aside>
      </div>
      <p className="source">{s.attribution}</p>
    </Page>
  );
}

export function FilterBar({
  filters,
  onChange,
  towns,
  categories,
  s,
}: {
  filters: Filters;
  onChange: (filters: Filters) => void;
  towns: string[];
  categories: string[];
  s: Strings;
}) {
  const set = (field: keyof Filters) => (event: { target: { value: string } }) => onChange({ ...filters, [field]: event.target.value });
  const invalid = rangeInvalid(filters);
  const any = Object.values(filters).some((value) => value !== "");
  return (
    <form className="filters" aria-label={s.filters} onSubmit={(event) => event.preventDefault()}>
      <div className="field grow">
        <label htmlFor="f-text">{s.search}</label>
        <input id="f-text" type="search" value={filters.text} onChange={set("text")} aria-describedby="f-text-help" />
        <small id="f-text-help">{s.searchHelp}</small>
      </div>
      <div className="field">
        <label htmlFor="f-town">{s.town}</label>
        <select id="f-town" value={filters.town} onChange={set("town")}>
          <option value="">{s.anyTown}</option>
          {towns.map((town) => (
            <option key={town} value={town}>
              {town}
            </option>
          ))}
        </select>
      </div>
      <div className="field">
        <label htmlFor="f-category">{s.category}</label>
        <select id="f-category" value={filters.category} onChange={set("category")}>
          <option value="">{s.anyCategory}</option>
          {categories.map((category) => (
            <option key={category} value={category}>
              {category}
            </option>
          ))}
        </select>
      </div>
      <div className="field">
        <label htmlFor="f-from">{s.from}</label>
        <input id="f-from" type="date" value={filters.from} onChange={set("from")} aria-invalid={invalid} aria-describedby={invalid ? "f-range" : undefined} />
      </div>
      <div className="field">
        <label htmlFor="f-to">{s.to}</label>
        <input id="f-to" type="date" value={filters.to} onChange={set("to")} aria-invalid={invalid} aria-describedby={invalid ? "f-range" : undefined} />
      </div>
      <div className="field actions">
        <button type="button" className="button" onClick={() => onChange(NO_FILTERS)} disabled={!any}>
          {s.clear}
        </button>
      </div>
      {invalid && (
        <p id="f-range" role="alert" className="failed wide">
          {s.rangeInvalid}
        </p>
      )}
    </form>
  );
}

export function EventList({ events, picked, onPick, s }: { events: EventItem[]; picked: string | null; onPick: (id: string) => void; s: Strings }) {
  if (events.length === 0) return null;
  return (
    <ul className="events">
      {events.map((event) => (
        <li key={event.id} className={event.id === picked ? "picked" : undefined}>
          <button type="button" data-id={event.id} aria-pressed={event.id === picked} onClick={() => onPick(event.id)}>
            <span className="when">{whenOf(event, s)}</span>
            <span className="name">{event.name}</span>
            <span className="sub">{[event.venue, event.town, event.category].filter(Boolean).join(" · ")}</span>
          </button>
          {event.url && (
            <a className="out" href={event.url} target="_blank" rel="noopener noreferrer">
              {s.sourcePage}
              <span className="sr-only"> {event.name}</span>
            </a>
          )}
        </li>
      ))}
    </ul>
  );
}

export function MonthView({ events, picked, onPick, s }: { events: EventItem[]; picked: string | null; onPick: (id: string) => void; s: Strings }) {
  const today = dayOf(new Date());
  const [month, setMonth] = useState(() => monthOf(events[0]?.startDay && events[0].startDay > today ? events[0].startDay : today));
  const [day, setDay] = useState<string | null>(null);
  const days = monthDays(month);
  const heading = new Intl.DateTimeFormat(s.locale, { timeZone: "UTC", month: "long", year: "numeric" }).format(new Date(`${month}-15T12:00:00Z`));
  const dayLabel = (value: string) =>
    new Intl.DateTimeFormat(s.locale, { timeZone: "UTC", weekday: "long", day: "numeric", month: "long" }).format(new Date(`${value}T12:00:00Z`));
  const chosen = day ? onDay(events, day) : [];
  const move = (by: number) => {
    setMonth(shiftMonth(month, by));
    setDay(null);
  };

  return (
    <div className="month">
      <div className="month-nav">
        <button type="button" className="button" onClick={() => move(-1)} aria-label={s.previousMonth}>
          ‹
        </button>
        <h2 id="month-heading" aria-live="polite">
          {heading}
        </h2>
        <button type="button" className="button" onClick={() => move(1)} aria-label={s.nextMonth}>
          ›
        </button>
        <button type="button" className="button" onClick={() => setMonth(monthOf(today))} disabled={month === monthOf(today)}>
          {s.thisMonth}
        </button>
      </div>
      <table className="month-grid" aria-labelledby="month-heading">
        <thead>
          <tr>
            {s.weekdays.map((weekday) => (
              <th key={weekday} scope="col">
                {weekday}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {Array.from({ length: days.length / 7 }, (_, week) => (
            <tr key={days[week * 7]}>
              {days.slice(week * 7, week * 7 + 7).map((value) => {
                const those = onDay(events, value);
                const outside = monthOf(value) !== month;
                return (
                  <td key={value} className={[outside ? "outside" : "", value === today ? "today" : "", value === day ? "chosen" : ""].join(" ").trim() || undefined}>
                    <button type="button" className="day" aria-pressed={value === day} aria-label={s.dayEvents(dayLabel(value), those.length)} onClick={() => setDay(value)}>
                      <span className="number">{Number(value.slice(8))}</span>
                      {those.length > 0 && <span className="dot" aria-hidden="true">{those.length}</span>}
                      <span className="chips" aria-hidden="true">
                        {those.slice(0, PER_DAY).map((event) => (
                          <span key={event.id} className="chip">
                            {event.name}
                          </span>
                        ))}
                        {those.length > PER_DAY && <span className="chip more">{s.more(those.length - PER_DAY)}</span>}
                      </span>
                    </button>
                  </td>
                );
              })}
            </tr>
          ))}
        </tbody>
      </table>
      <section className="day-events" aria-labelledby="day-heading">
        <h3 id="day-heading">{day ? s.dayEvents(dayLabel(day), chosen.length) : s.pickDay}</h3>
        <EventList events={chosen} picked={picked} onPick={onPick} s={s} />
      </section>
    </div>
  );
}

export function Subscribe({ slug, filters, s }: { slug: string; filters: Filters; s: Strings }) {
  const origin = apiOrigin(window.location);
  const whole = calendarUrl(origin, slug);
  const ranged = filters.from !== "" || filters.to !== "";
  const narrowed = filters.town !== "" || filters.category !== "" || filters.text !== "";
  return (
    <Card title={s.subscribe}>
      <p className="note">{s.subscribeIntro}</p>
      <CalendarLink id="cal-region" label={s.wholeRegion} help={s.wholeRegionHelp} url={whole} s={s} />
      {ranged && !rangeInvalid(filters) && (
        <CalendarLink id="cal-range" label={s.dateRange} help={s.dateRangeHelp(filters.from, filters.to)} url={calendarUrl(origin, slug, filters)} s={s} />
      )}
      {(ranged || narrowed) && <p className="note">{s.onlyDates}</p>}
    </Card>
  );
}

function CalendarLink({ id, label, help, url, s }: { id: string; label: string; help: string; url: string; s: Strings }) {
  const [said, setSaid] = useState<string | null>(null);
  const copy = () => {
    navigator.clipboard
      .writeText(url)
      .then(() => setSaid(s.copied))
      .catch(() => setSaid(s.copyFailed));
  };
  return (
    <div className="cal-link">
      <h3 id={`${id}-label`}>{label}</h3>
      <p className="note">{help}</p>
      <label className="sr-only" htmlFor={id}>
        {s.calendarLink}: {label}
      </label>
      <input id={id} type="url" readOnly value={url} onFocus={(event) => event.target.select()} />
      <div className="cal-actions">
        <button type="button" className="button" onClick={copy} aria-describedby={`${id}-label`}>
          {s.copy}
        </button>
        <a className="button" href={webcalUrl(url)} aria-describedby={`${id}-label`}>
          {s.openCalendar}
        </a>
      </div>
      <p role="status" className="note">
        {said ?? ""}
      </p>
    </div>
  );
}

function MapView({ events, picked, onPick, s }: { events: EventItem[]; picked: string | null; onPick: (id: string) => void; s: Strings }) {
  const missing = events.filter((event) => event.coordinates === null).length;
  return (
    <div className="map-screen">
      <EventMap events={events} picked={picked} onPick={onPick} s={s} />
      {missing > 0 && <p className="note">{s.notOnMap(missing)}</p>}
      <EventList events={events} picked={picked} onPick={onPick} s={s} />
    </div>
  );
}

function EventMap({ events, picked, onPick, s }: { events: EventItem[]; picked: string | null; onPick: (id: string) => void; s: Strings }) {
  // The basemap is the platform's, named in the document the Portal served; the app names no tile
  // host of its own, which the app's policy would refuse anyway (AP-67, AP-12).
  const { basemap } = useClient().config;
  const holder = useRef<HTMLDivElement | null>(null);
  const map = useRef<MapLibreMap | null>(null);
  const ready = useRef(false);
  const collection = useMemo(() => featuresOf(events, picked), [events, picked]);
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
          "circle-color": "#b54708",
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
