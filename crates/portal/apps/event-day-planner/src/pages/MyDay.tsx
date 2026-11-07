import { useEffect, useMemo, useState } from "react";
import { Card, currentTokens, download, Empty, Loading, Page, ProblemError, Split, useEntities, useEntitySelection } from "@joinedcontext/sdk";
import type { Row } from "@joinedcontext/sdk";
import { useAccess } from "../access";
import { ChartCard } from "../components/ChartCard";
import { FeaturedCard } from "../components/FeaturedCard";
import { MapView } from "../components/MapView";
import { problemText, ShareDay } from "../components/ShareDay";
import type { MapPoint } from "../components/MapView";
import {
  ATTRS,
  cancelled,
  dayBounds,
  dayOf,
  EVENT,
  eventsOfDay,
  localOf,
  nameOf,
  perHour,
  planEvents,
  sourceOf,
  textOf,
  upcomingQuery,
} from "../events";
import { localeOf, number, useLang, ZONE } from "../i18n";
import type { Lang } from "../i18n";
import { useFeatured } from "../featured";
import type { Featured } from "../featured";
import { computeDay } from "../planner";
import type { Day, Fit } from "../planner";
import { apiBase, isCode, shareApi } from "../share";
import type { ShareApi } from "../share";
import { t } from "../texts";
import { listOf, useParam } from "../url";

/** The most events the list shows at once; the search and the hour narrow it. */
const LISTED = 60;

/** "12.00" in Helsinki. */
function clock(ms: number, lang: Lang): string {
  return new Date(ms).toLocaleTimeString(localeOf(lang), { hour: "2-digit", minute: "2-digit", timeZone: ZONE });
}

function unreadable(error: Error, lang: Lang): string {
  return error instanceof ProblemError && error.status > 0 ? t(lang, "unreadable", { status: error.status }) : t(lang, "offline");
}

/** The hour in the address, when it is one of the day's 24; `null` else. */
export function hourOf(value: string): number | null {
  const hour = Number(value);
  return value !== "" && Number.isInteger(hour) && hour >= 0 && hour <= 23 ? hour : null;
}

function hourChart(counts: number[], lang: Lang): Record<string, unknown> | null {
  if (counts.every((count) => count === 0)) return null;
  const tokens = currentTokens();
  return {
    tooltip: { trigger: "axis", axisPointer: { type: "shadow" } },
    grid: { containLabel: true, left: 8, right: 16, top: 24, bottom: 8 },
    xAxis: { type: "category", data: counts.map((_, h) => String(h)) },
    yAxis: { type: "value", name: t(lang, "eventsAxis"), minInterval: 1 },
    series: [{ type: "bar", name: t(lang, "eventsAxis"), itemStyle: { color: tokens.color.accent }, data: counts }],
  };
}

/**
 * The visitor's question, answered on opening (T-3329): what can I see today, and in which order.
 * With nothing chosen the planner suggests a day of events that fit one after another on foot; a
 * visitor who picks events gets them in the order that fits most, with when to be where, the walk
 * between at 5 km/h, what clashes or is out of reach, and the day as a calendar file. The plan is
 * made by the WebAssembly planner in a worker; the day, the search, the hour and the picks are
 * kept in the address. An event in the plan, on the map or in the list opens in the shell's entity
 * panel (SDK-40), linked to the Portal. A steward features events every visitor sees, with a note
 * (T-3826); a signed-in visitor keeps the days they share under My days.
 */
export function MyDay({ shares }: { shares?: ShareApi } = {}) {
  const lang = useLang();
  const api = useMemo(() => shares ?? shareApi(apiBase()), [shares]);
  const [now] = useState(() => Date.now());
  const today = dayOf(new Date(now));
  const [dayText, setDay] = useParam("day", today);
  const [search, setSearch] = useParam("q");
  const [hourText, setHour] = useParam("hour");
  const [pickText, setPick] = useParam("pick");
  const bounds = useMemo(() => dayBounds(dayText) ?? (dayBounds(today) as [number, number]), [dayText, today]);
  const day = dayOf(new Date(bounds[0]));
  const hour = hourOf(hourText);
  const picks = useMemo(() => listOf(pickText), [pickText]);
  const { select } = useEntitySelection();
  const { steward } = useAccess();
  const featured = useFeatured(api);

  // A shared link, `?share=<code>`: its day and picks put into the address, then the code dropped,
  // so a reload keeps the day and the visitor's own changes stay theirs (T-3347).
  const [shareCode, setShareCode] = useParam("share");
  const [shareNote, setShareNote] = useState<{ text: string; error: boolean } | null>(null);
  useEffect(() => {
    if (!shareCode) return;
    if (!isCode(shareCode)) {
      setShareNote({ text: t(lang, "shareGone"), error: true });
      setShareCode("");
      return;
    }
    let current = true;
    setShareNote({ text: t(lang, "openingShare"), error: false });
    api
      .get(shareCode)
      .then((shared) => {
        if (!current) return;
        setDay(shared.day);
        setPick(shared.picks.join(","));
        setShareNote({ text: t(lang, "openedShare", { n: number(shared.picks.length) }), error: false });
      })
      .catch((error: unknown) => {
        if (current) setShareNote({ text: problemText(lang, error, "openShareFailed"), error: true });
      })
      .finally(() => {
        if (current) setShareCode("");
      });
    return () => {
      current = false;
    };
    // The code is dropped once read, so a change of language never opens the share again.
  }, [shareCode, api, lang, setDay, setPick, setShareCode]);

  // From the start of the day shown: an event that ended before it is never read.
  const query = useMemo(() => ({ attrs: ATTRS, q: upcomingQuery(Math.min(bounds[0], now)) }), [bounds, now]);
  const { rows, loading, error, reload } = useEntities(EVENT, query);
  const ofDay = useMemo(() => eventsOfDay(rows, bounds), [rows, bounds]);
  const byLocal = useMemo(() => new Map(rows.map((row) => [localOf(row.id), row])), [rows]);
  const byId = useMemo(() => new Map(rows.map((row) => [row.id, row])), [rows]);
  const ofDayById = useMemo(() => new Map(ofDay.map((row) => [row.id, row])), [ofDay]);
  const listed = useMemo(() => eventsOfDay(ofDay, bounds, search, hour), [ofDay, bounds, search, hour]);
  const counts = useMemo(() => perHour(eventsOfDay(ofDay, bounds, search), bounds), [ofDay, bounds, search]);
  const chart = useMemo(() => hourChart(counts, lang), [counts, lang]);

  const [plan, setPlan] = useState<Day | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  // Planned over the day's events; cancelled ones only when the visitor chose them anyway.
  const plannable = useMemo(() => ofDay.filter((row) => !cancelled(row) || picks.includes(localOf(row.id))), [ofDay, picks]);
  useEffect(() => {
    let current = true;
    const isToday = day === today;
    computeDay({
      events: planEvents(plannable, bounds),
      settings: { chosen: picks.filter((id) => byLocal.has(id)), dayStart: isToday ? now : null, now },
    })
      .then((answer) => {
        if (!current) return;
        setPlan(answer);
        setFailed(null);
      })
      .catch((why: unknown) => {
        if (current) setFailed(why instanceof Error ? why.message : String(why));
      });
    return () => {
      current = false;
    };
  }, [plannable, bounds, picks, byLocal, day, today, now]);

  const tokens = useMemo(() => currentTokens(), []);
  const items = useMemo(() => plan?.items ?? [], [plan]);
  const kept = useMemo(() => items.filter((item) => item.fit !== "missed"), [items]);
  const points: MapPoint[] = useMemo(() => {
    const colourOf = (fit: Fit): string => (fit === "ok" ? tokens.color.accent : fit === "late" ? tokens.color.warning : tokens.color.danger);
    return items.flatMap((item) => {
      const row = byLocal.get(item.id);
      const event = row ? planEvents([row], bounds)[0] : null;
      if (!event || event.lon === null || event.lat === null) return [];
      return [{ id: item.id, at: [event.lon, event.lat] as [number, number], color: colourOf(item.fit) }];
    });
  }, [items, byLocal, bounds, tokens]);
  const line = useMemo(
    () => points.filter((p) => kept.some((item) => item.id === p.id)).map((p) => p.at),
    [points, kept],
  );
  const waiting = loading && rows.length === 0;
  const shareIds = useMemo(() => picks.flatMap((id) => byLocal.get(id)?.id ?? []), [picks, byLocal]);

  const toggle = (id: string) => setPick((picks.includes(id) ? picks.filter((x) => x !== id) : [...picks, id]).join(","));
  const reset = () => {
    setDay(today);
    setSearch("");
    setHour("");
    setPick("");
  };
  const saveIcs = () => {
    if (!plan || kept.length === 0) return;
    download(new Blob([plan.ics], { type: "text/calendar;charset=utf-8" }), `${day}-helsinki.ics`);
  };
  /** Opens the event with the local id `id` in the shell's panel; one the page no longer holds opens nothing. */
  const open = (id: string) => {
    const row = byLocal.get(id);
    if (row) select({ id: row.id, type: EVENT });
  };
  const nameOfId = (id: string) => {
    const row = byLocal.get(id);
    return row ? nameOf(row) : id;
  };

  return (
    <Page label={t(lang, "page")}>
      {shareNote && (
        <p role={shareNote.error ? "alert" : "status"} className={shareNote.error ? "app-error" : "app-lead"}>
          {shareNote.text}
        </p>
      )}
      {error && (
        <div className="jc-problem" role="alert">
          <strong>{unreadable(error, lang)}</strong>
          <button type="button" className="jc-button" onClick={reload}>
            {t(lang, "retry")}
          </button>
        </div>
      )}
      {failed && (
        <div className="jc-problem" role="alert">
          <strong>{t(lang, "plannerFailed", { why: failed })}</strong>
        </div>
      )}
      <form className="app-filters" role="search" aria-label={t(lang, "events")} onSubmit={(event) => event.preventDefault()}>
        <label>
          <span>{t(lang, "day")}</span>
          <input type="date" value={day} min={today} onChange={(event) => setDay(event.target.value || today)} />
        </label>
        <label>
          <span>{t(lang, "search")}</span>
          <input type="search" value={search} placeholder={t(lang, "searchHint")} onChange={(event) => setSearch(event.target.value)} />
        </label>
        <button type="button" className="jc-button" onClick={reset}>
          {t(lang, "clear")}
        </button>
      </form>
      <Split ratio="1:1">
        <Card title={t(lang, "plan")}>
          {waiting ? (
            <Loading label={t(lang, "reading")} />
          ) : !plan ? (
            <Loading label={t(lang, "planning")} />
          ) : items.length === 0 ? (
            <Empty>{t(lang, "noEvents")}</Empty>
          ) : (
            <>
              <p className="app-lead">{plan.suggested ? t(lang, "suggested") : t(lang, "yours", { n: number(items.length) })}</p>
              <ol className="app-stops" aria-label={t(lang, "plan")}>
                {items.map((item) => (
                  <li key={item.id} className="app-stop" data-fit={item.fit}>
                    <p className="app-stop-time">
                      <time dateTime={new Date(item.begin).toISOString()}>{clock(item.begin, lang)}</time>–
                      <time dateTime={new Date(item.finish).toISOString()}>{clock(item.finish, lang)}</time>
                    </p>
                    <strong>
                      <button type="button" className="app-open" onClick={() => open(item.id)}>
                        {item.name}
                      </button>
                    </strong>
                    <p className="app-stop-facts">
                      {item.walkMinutes > 0 && t(lang, "walkFrom", { min: number(item.walkMinutes), km: number(item.walkKm, 1) })}
                      {!item.located && <span>{t(lang, "noPlace")}</span>}
                      {item.fit === "late" && (
                        <span className="app-chip" data-fit="late">
                          {t(lang, "late", { min: number(item.lateMinutes) })}
                        </span>
                      )}
                      {item.fit === "missed" && (
                        <span className="app-chip" data-fit="missed">
                          {t(lang, "missed")}
                        </span>
                      )}
                    </p>
                  </li>
                ))}
              </ol>
              <p className="app-lead">{t(lang, "walking", { min: number(plan.walkMinutes), km: number(plan.walkKm, 1) })}</p>
              {plan.conflicts.length > 0 && (
                <div className="app-conflicts" role="status">
                  <strong>{t(lang, "conflicts")}</strong>
                  <ul>
                    {plan.conflicts.map(([a, b]) => (
                      <li key={`${a}-${b}`}>{t(lang, "conflict", { a: nameOfId(a), b: nameOfId(b) })}</li>
                    ))}
                  </ul>
                </div>
              )}
              <button
                type="button"
                className="jc-button"
                onClick={saveIcs}
                aria-disabled={kept.length === 0}
                title={kept.length === 0 ? t(lang, "icsNone") : undefined}
              >
                {t(lang, "ics")}
              </button>
              <ShareDay lang={lang} api={api} day={day} ids={shareIds} />
            </>
          )}
        </Card>
        <Card title={t(lang, "map")}>
          <MapView points={points} line={line} label={t(lang, "mapLabel")} onPick={open} />
        </Card>
      </Split>
      <FeaturedCard lang={lang} featured={featured} ofDay={ofDayById} known={byId} picks={picks} onToggle={toggle} />
      <Split ratio="2:1">
        <Card title={`${t(lang, "events")} · ${t(lang, "eventsOf", { n: number(listed.length), all: number(ofDay.length) })}`}>
          {hour !== null && (
            <p className="app-picked">
              <button type="button" className="app-pick" onClick={() => setHour("")} aria-label={t(lang, "showAll", { h: t(lang, "hour", { h: hour }) })}>
                {t(lang, "hour", { h: hour })} ×
              </button>
            </p>
          )}
          {waiting ? (
            <Loading label={t(lang, "reading")} />
          ) : listed.length === 0 ? (
            <Empty>{ofDay.length === 0 ? t(lang, "noEvents") : t(lang, "noMatch")}</Empty>
          ) : (
            <ul className="app-events" aria-label={t(lang, "events")}>
              {listed.slice(0, LISTED).map((row) => (
                <EventEntry
                  key={row.id}
                  row={row}
                  lang={lang}
                  chosen={picks.includes(localOf(row.id))}
                  onToggle={() => toggle(localOf(row.id))}
                  onOpen={() => open(localOf(row.id))}
                  bounds={bounds}
                  featured={steward ? featured : null}
                />
              ))}
            </ul>
          )}
        </Card>
        <ChartCard
          title={t(lang, "perHour")}
          option={error ? null : chart}
          loading={waiting}
          loadingLabel={t(lang, "reading")}
          empty={t(lang, "noEvents")}
          onSelect={(picked) => setHour(String(picked) === hourText ? "" : String(picked))}
        />
      </Split>
    </Page>
  );
}

function EventEntry({
  row,
  lang,
  chosen,
  onToggle,
  onOpen,
  bounds,
  featured,
}: {
  row: Row;
  lang: Lang;
  chosen: boolean;
  onToggle: () => void;
  onOpen: () => void;
  bounds: [number, number];
  /** Given to a steward only: the event can be featured from here. */
  featured: Featured | null;
}) {
  const [event] = planEvents([row], bounds);
  const [note, setNote] = useState<{ text: string; error: boolean } | null>(null);
  const isCancelled = cancelled(row);
  const source = sourceOf(row);
  const description = textOf(row, "description");
  const name = nameOf(row);
  const whole = event.start !== null && event.end !== null && event.start <= bounds[0] && event.end >= bounds[1];
  return (
    <li className="app-event">
      <div className="app-event-pick">
        <label>
          <input type="checkbox" checked={chosen} onChange={onToggle} disabled={isCancelled && !chosen} aria-describedby={`when-${localOf(row.id)}`} />
          <span className="app-sr">
            {t(lang, "add")}: {name}
          </span>
        </label>
        <strong>
          <button type="button" className="app-open" onClick={onOpen}>
            {name}
          </button>
        </strong>
      </div>
      <p className="app-event-when" id={`when-${localOf(row.id)}`}>
        {whole || event.start === null || event.end === null ? t(lang, "allDay") : `${clock(event.start, lang)}–${clock(event.end, lang)}`}
        {event.address && ` · ${event.address}`}
      </p>
      <p className="app-event-tags">
        {isCancelled && (
          <span className="app-chip" data-fit="missed">
            {t(lang, "cancelled")}
          </span>
        )}
        {source && (
          <a href={source} rel="noopener noreferrer" target="_blank">
            {t(lang, "source")}
          </a>
        )}
        {featured && <FeatureButton lang={lang} featured={featured} row={row} onDone={setNote} />}
      </p>
      {note && (
        <p role={note.error ? "alert" : "status"} className={note.error ? "app-error" : "app-lead"}>
          {note.text}
        </p>
      )}
      {description && (
        <details>
          <summary>{t(lang, "about")}</summary>
          <p>{description}</p>
        </details>
      )}
    </li>
  );
}

/** A steward's "Feature" for one event; an event already featured says so instead. */
function FeatureButton({
  lang,
  featured,
  row,
  onDone,
}: {
  lang: Lang;
  featured: Featured;
  row: Row;
  onDone: (note: { text: string; error: boolean }) => void;
}) {
  const name = nameOf(row);
  if (featured.list?.some((one) => one.eventId === row.id))
    return (
      <span className="app-chip" data-fit="ok">
        {t(lang, "isFeatured")}
      </span>
    );
  return (
    <button
      type="button"
      className="jc-button"
      disabled={featured.list === null}
      aria-label={`${t(lang, "feature")}: ${name}`}
      onClick={() =>
        featured.add(row.id).then(
          () => onDone({ text: t(lang, "featured", { name }), error: false }),
          (error: unknown) => onDone({ text: problemText(lang, error, "featureFailed", "gone"), error: true }),
        )
      }
    >
      {t(lang, "feature")}
    </button>
  );
}
