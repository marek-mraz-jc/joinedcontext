import { useMemo, useState } from "react";
import { Card, Empty, Grid, Loading, Page, ProblemError, Split, useEntities, useEntitySelection } from "@joinedcontext/sdk";
import type { Row } from "@joinedcontext/sdk";
import { dayLabel, perDayOption, registerColours, registerOption } from "../charts";
import { ChartCard } from "../components/ChartCard";
import { EntityMap } from "../components/EntityMap";
import {
  byRegister,
  dateOf,
  dayOf,
  EVENT,
  filterEvents,
  inputDay,
  located,
  perDay,
  registerOf,
  sourceOf,
  textOf,
  upcomingQuery,
  when,
  ZONE,
} from "../events";

/** The start of the reader's day, fixed for the life of the page so the query stays one query. */
function today(): Date {
  const now = new Date();
  return new Date(now.getFullYear(), now.getMonth(), now.getDate());
}

/** What went wrong in words a reader can act on, with the endpoint's status (T-2597's error state). */
function unreadable(error: Error): string {
  return error instanceof ProblemError && error.status > 0
    ? `The events could not be read (HTTP ${error.status}). Try again later.`
    : "The events could not be read. Check the connection and try again.";
}

const UNREAD = "Nothing to chart until the events are read.";

/**
 * Helsinki's upcoming events (T-2923): a search and a date range, the events per day for the next
 * 30 days and per register (a bar picks them out), every located event on the map in its
 * register's colour, and the list, soonest first. An event on the map or in the list opens in the
 * shell's entity panel (SDK-40), linked to the Portal: a public App writes nothing.
 */
export function Events() {
  const [start] = useState(today);
  const query = useMemo(() => ({ q: upcomingQuery(start) }), [start]);
  const { rows, loading, error, reload } = useEntities(EVENT, query);
  const [search, setSearch] = useState("");
  const [from, setFrom] = useState(() => start.toLocaleDateString("sv-SE"));
  const [to, setTo] = useState("");
  const [day, setDay] = useState<string | null>(null);
  const [register, setRegister] = useState<string | null>(null);
  const { selected, select } = useEntitySelection();
  const open = (row: Row) => select({ id: row.id, type: EVENT });

  const ranked = useMemo(() => byRegister(rows), [rows]);
  const colours = useMemo(() => registerColours(ranked.map((entry) => entry.register)), [ranked]);
  const colourOf = (row: Row) => colours.get(registerOf(row)) ?? "currentColor";
  const ranged = useMemo(
    () => filterEvents(rows, { from: inputDay(from), to: inputDay(to, true), query: search }),
    [rows, from, to, search],
  );
  const shown = useMemo(() => filterEvents(ranged, { day, register }), [ranged, day, register]);
  const perDayChart = useMemo(() => perDayOption(perDay(ranged, inputDay(from) ?? start)), [ranged, from, start]);
  const registerChart = useMemo(() => registerOption(byRegister(ranged), colours), [ranged, colours]);
  const mapped = useMemo(() => shown.filter(located), [shown]);
  const waiting = loading && rows.length === 0;

  const reset = () => {
    setSearch("");
    setFrom(start.toLocaleDateString("sv-SE"));
    setTo("");
    setDay(null);
    setRegister(null);
  };

  return (
    <Page label="Events">
      {error && (
        <div className="jc-problem" role="alert">
          <strong>{unreadable(error)}</strong>
          <button type="button" className="jc-button" onClick={reload}>
            Retry
          </button>
        </div>
      )}
      <form className="app-filters" role="search" aria-label="Find events" onSubmit={(event) => event.preventDefault()}>
        <label>
          <span>Search</span>
          <input type="search" value={search} onChange={(event) => setSearch(event.target.value)} placeholder="Name, place or words" />
        </label>
        <label>
          <span>From</span>
          <input type="date" value={from} onChange={(event) => setFrom(event.target.value)} />
        </label>
        <label>
          <span>To</span>
          <input type="date" value={to} min={from} onChange={(event) => setTo(event.target.value)} />
        </label>
        <button type="button" className="jc-button" onClick={reset}>
          Clear filters
        </button>
      </form>
      {(day || register) && (
        <p className="app-picked" aria-live="polite">
          {day && (
            <button type="button" className="app-pick" onClick={() => setDay(null)} aria-label={`Show every day, not only ${dayLabel(day)}`}>
              {dayLabel(day)} ×
            </button>
          )}
          {register && (
            <button type="button" className="app-pick" onClick={() => setRegister(null)} aria-label={`Show every register, not only ${register}`}>
              {register} ×
            </button>
          )}
        </p>
      )}
      {/* The failure is said once, in the alert above; the charts only say they have nothing to draw. */}
      <Grid columns={2}>
        <ChartCard
          title="Events per day, next 30 days"
          option={error ? null : perDayChart}
          loading={waiting}
          empty={error ? UNREAD : "No event takes place on these 30 days."}
          onSelect={(picked) => setDay(picked === day ? null : picked)}
        />
        <ChartCard
          title="Events by register"
          option={error ? null : registerChart}
          loading={waiting}
          empty={error ? UNREAD : "No event matches."}
          onSelect={(picked) => setRegister(picked === register ? null : picked)}
        />
      </Grid>
      <Split ratio="1:1">
        <Card title="On the map">
          <EntityMap rows={mapped} location="location" label="name" colorOf={colourOf} cluster selected={selected?.id ?? null} onSelect={open} height={420} />
          <ul className="app-legend" aria-label="Map colour: the register that publishes the event">
            {ranked.map((entry) => (
              <li key={entry.register}>
                <span className="app-swatch" aria-hidden="true" style={{ background: colours.get(entry.register) }} />
                {entry.register}
              </li>
            ))}
          </ul>
        </Card>
        <Card title={`${shown.length} of ${rows.length} events`}>
          {waiting ? (
            <Loading label="Reading the events…" />
          ) : shown.length === 0 ? (
            <Empty>{rows.length === 0 ? "No upcoming events." : "No event matches the filters."}</Empty>
          ) : (
            <ul className="app-events" aria-label="Upcoming events">
              {shown.map((row) => (
                <EventCard key={row.id} row={row} colour={colourOf(row)} onOpen={() => open(row)} />
              ))}
            </ul>
          )}
        </Card>
      </Split>
    </Page>
  );
}

function EventCard({ row, colour, onOpen }: { row: Row; colour: string; onOpen: () => void }) {
  const start = dateOf(row, "startDate");
  const description = textOf(row, "description");
  const source = sourceOf(row);
  const cancelled = textOf(row, "eventStatus") === "EventCancelled";
  return (
    <li className="app-event" style={{ borderLeftColor: colour }}>
      <div className="app-event-date" aria-hidden="true">
        {start ? (
          <>
            <strong>{start.toLocaleDateString("en-GB", { day: "numeric", timeZone: ZONE })}</strong>
            <span>{start.toLocaleDateString("en-GB", { month: "short", timeZone: ZONE })}</span>
          </>
        ) : (
          <span>–</span>
        )}
      </div>
      <div className="app-event-body">
        <h3>
          <button type="button" className="app-open" onClick={onOpen}>
            {textOf(row, "name") || row.id.slice(row.id.lastIndexOf(":") + 1)}
          </button>
        </h3>
        <p className="app-event-when">
          <time dateTime={start ? dayOf(start) : undefined}>{when(row)}</time>
        </p>
        {textOf(row, "address") && <p className="app-event-where">{textOf(row, "address")}</p>}
        <p className="app-event-tags">
          <span className="app-tag">
            <span className="app-swatch" aria-hidden="true" style={{ background: colour }} />
            {registerOf(row)}
          </span>
          {cancelled && <span className="app-chip" data-status="cancelled">Cancelled</span>}
          {source && (
            <a href={source} rel="noopener noreferrer" target="_blank">
              Source
            </a>
          )}
        </p>
        {description && (
          <details>
            <summary>About</summary>
            <p>{description}</p>
          </details>
        )}
      </div>
    </li>
  );
}
