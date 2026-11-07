import { useId, useRef, useState } from "react";
import { displayName, Header, Page, Split, Tabs, useEntities } from "@joinedcontext/sdk";
import type { Row } from "@joinedcontext/sdk";
import { EntityDetail } from "./components/EntityDetail";
import { Empty, Loading, Problem } from "./components/states";
import { addDays, byStartDay, dayKey, monthGrid, onDay, spans, startOfDay, timeline, weekOf, type Span } from "./calendar";

const TYPE = "Event";
const MONTH = new Intl.DateTimeFormat(undefined, { month: "long", year: "numeric" });
const DAY_LONG = new Intl.DateTimeFormat(undefined, { weekday: "long", day: "numeric", month: "long" });
const DAY_SHORT = new Intl.DateTimeFormat(undefined, { weekday: "short", day: "numeric", month: "short" });
const WEEKDAY = new Intl.DateTimeFormat(undefined, { weekday: "short" });
const WEEKDAY_LONG = new Intl.DateTimeFormat(undefined, { weekday: "long" });
const TIME = new Intl.DateTimeFormat(undefined, { hour: "2-digit", minute: "2-digit" });

function when(span: Span, day?: Date): string {
  const sameDay = dayKey(span.start) === dayKey(span.end);
  if (sameDay) return span.start.getTime() === span.end.getTime() ? TIME.format(span.start) : `${TIME.format(span.start)}–${TIME.format(span.end)}`;
  if (day && dayKey(day) !== dayKey(span.start)) return dayKey(day) === dayKey(span.end) ? `until ${TIME.format(span.end)}` : "all day";
  return `${TIME.format(span.start)}, until ${DAY_SHORT.format(span.end)}`;
}

function count(n: number): string {
  return n === 0 ? "no events" : n === 1 ? "1 event" : `${n} events`;
}

/** One event as a button: its time first, then its name, then where. */
function EventButton({ span, day, onOpen }: { span: Span; day?: Date; onOpen: (row: Row, from: HTMLButtonElement) => void }): React.JSX.Element {
  return (
    <button type="button" className="app-event" data-category={String(span.row.category ?? "")} onClick={(event) => onOpen(span.row, event.currentTarget)}>
      <span className="app-event-time">{when(span, day)}</span>
      <span className="app-event-name">{displayName(span.row)}</span>
      {typeof span.row.venue === "string" && <span className="app-event-venue">{span.row.venue}</span>}
    </button>
  );
}

function EventList({ list, day, onOpen, empty }: { list: Span[]; day?: Date; onOpen: (row: Row, from: HTMLButtonElement) => void; empty: string }): React.JSX.Element {
  if (list.length === 0) return <p className="app-quiet">{empty}</p>;
  return (
    <ul className="app-events">
      {list.map((span) => (
        <li key={span.row.id}>
          <EventButton span={span} day={day} onOpen={onOpen} />
        </li>
      ))}
    </ul>
  );
}

/** Previous, today and next, with the period's name read out when it changes. */
function Stepper({ title, unit, onStep, onToday }: { title: string; unit: string; onStep: (by: -1 | 1) => void; onToday: () => void }): React.JSX.Element {
  return (
    <div className="app-stepper">
      <h2 aria-live="polite">{title}</h2>
      <div className="app-stepper-buttons">
        <button type="button" aria-label={`Previous ${unit}`} onClick={() => onStep(-1)}>
          <span aria-hidden="true">‹</span>
        </button>
        <button type="button" onClick={onToday}>
          Today
        </button>
        <button type="button" aria-label={`Next ${unit}`} onClick={() => onStep(1)}>
          <span aria-hidden="true">›</span>
        </button>
      </div>
    </div>
  );
}

/** Events on a week, a month and a timeline, soonest first, and the chosen one beside them. */
export default function App(): React.JSX.Element {
  const { rows, loading, error, reload } = useEntities(TYPE, undefined, { all: true });
  const [anchor, setAnchor] = useState(() => startOfDay(new Date()));
  const [day, setDay] = useState(() => startOfDay(new Date()));
  const [openId, setOpenId] = useState<string | null>(null);
  const cameFrom = useRef<HTMLButtonElement | null>(null);
  const dayHeading = useId();

  const all = spans(rows);
  const undated = rows.length - all.length;
  const today = startOfDay(new Date());
  const open = rows.find((row) => row.id === openId) ?? null;

  const openEvent = (row: Row, from: HTMLButtonElement) => {
    cameFrom.current = from;
    setOpenId(row.id);
  };
  const close = () => {
    setOpenId(null);
    cameFrom.current?.focus();
  };
  const goToday = () => {
    setAnchor(today);
    setDay(today);
  };

  const month = () => {
    const weeks = monthGrid(anchor);
    return (
      <>
        <Stepper
          title={MONTH.format(anchor)}
          unit="month"
          onToday={goToday}
          onStep={(by) => setAnchor(new Date(anchor.getFullYear(), anchor.getMonth() + by, 1))}
        />
        <div className="app-month-box">
          <table className="app-month" aria-label={MONTH.format(anchor)}>
            <thead>
              <tr>
                {weeks[0].map((date) => (
                  <th key={date.getDay()} scope="col" abbr={WEEKDAY_LONG.format(date)}>
                    {WEEKDAY.format(date)}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {weeks.map((week) => (
                <tr key={dayKey(week[0])}>
                  {week.map((date) => {
                    const here = onDay(all, date);
                    const chosen = dayKey(date) === dayKey(day);
                    return (
                      <td key={dayKey(date)} data-outside={date.getMonth() !== anchor.getMonth() || undefined} data-chosen={chosen || undefined}>
                        <button
                          type="button"
                          className="app-day"
                          aria-label={`${DAY_LONG.format(date)}, ${count(here.length)}`}
                          aria-pressed={chosen}
                          aria-current={dayKey(date) === dayKey(today) ? "date" : undefined}
                          onClick={() => setDay(date)}
                        >
                          <span aria-hidden="true">{date.getDate()}</span>
                          {here.length > 0 && (
                            <span className="app-day-count" aria-hidden="true">
                              {here.length}
                            </span>
                          )}
                        </button>
                        {here.length > 0 && (
                          <ul className="app-day-events">
                            {here.slice(0, 2).map((span) => (
                              <li key={span.row.id}>{displayName(span.row)}</li>
                            ))}
                            {here.length > 2 && <li className="app-quiet">{here.length - 2} more</li>}
                          </ul>
                        )}
                      </td>
                    );
                  })}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
        <section className="app-day-detail" aria-labelledby={dayHeading}>
          <h3 id={dayHeading}>{DAY_LONG.format(day)}</h3>
          <EventList list={onDay(all, day)} day={day} onOpen={openEvent} empty="Nothing planned on this day." />
        </section>
      </>
    );
  };

  const week = () => (
    <>
      <Stepper title={`${DAY_SHORT.format(weekOf(anchor)[0])} – ${DAY_SHORT.format(weekOf(anchor)[6])}`} unit="week" onToday={goToday} onStep={(by) => setAnchor(addDays(anchor, 7 * by))} />
      <ol className="app-week">
        {weekOf(anchor).map((date) => (
          <li key={dayKey(date)} aria-current={dayKey(date) === dayKey(today) ? "date" : undefined}>
            <h3>{DAY_LONG.format(date)}</h3>
            <EventList list={onDay(all, date)} day={date} onOpen={openEvent} empty="Nothing planned." />
          </li>
        ))}
      </ol>
    </>
  );

  const line = () => {
    const { upcoming, earlier } = timeline(all, new Date());
    const groups = (list: Span[]) => (
      <ol className="app-timeline">
        {byStartDay(list).map((group) => (
          <li key={dayKey(group.day)}>
            <h4>{dayKey(group.day) === dayKey(today) ? `Today, ${DAY_LONG.format(group.day)}` : DAY_LONG.format(group.day)}</h4>
            <EventList list={group.spans} onOpen={openEvent} empty="" />
          </li>
        ))}
      </ol>
    );
    return (
      <>
        <h2>Coming up</h2>
        {upcoming.length === 0 ? <p className="app-quiet">Nothing is planned yet.</p> : groups(upcoming)}
        {undated > 0 && <p className="app-quiet">{undated === 1 ? "1 event has no date yet." : `${undated} events have no date yet.`}</p>}
        {earlier.length > 0 && (
          <details className="app-earlier">
            <summary>Earlier ({earlier.length})</summary>
            {groups(earlier)}
          </details>
        )}
      </>
    );
  };

  return (
    <div className="jc-shell">
      <main className="jc-main">
        <Page label="Events">
          <Header level={1} title="What's on" subtitle="Events in the city by month, by week and as a timeline" />
          <Problem error={error} onRetry={reload} />
          {loading && rows.length === 0 ? (
            <Loading label="Loading the events…" />
          ) : rows.length === 0 ? (
            !error && <Empty>No events yet.</Empty>
          ) : (
            <Split ratio="2:1">
              <Tabs
                label="Views of the events"
                tabs={[
                  { id: "month", label: "Month", render: month },
                  { id: "week", label: "Week", render: week },
                  { id: "timeline", label: "Timeline", render: line },
                ]}
              />
              {open ? (
                <EntityDetail row={open} title={displayName(open)} attrs={["startDate", "endDate", "venue", "category", "description"]} onClose={close} />
              ) : (
                <p className="app-quiet">Choose an event to read it here.</p>
              )}
            </Split>
          )}
        </Page>
      </main>
    </div>
  );
}
