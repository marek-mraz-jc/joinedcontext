import { useEffect, useMemo, useRef, useState } from "react";
import { Card, Empty, Grid, Loading, Page, ProblemError, displayName, format, useClient, useEntities, useEntitySelection } from "@joinedcontext/sdk";
import type { Row } from "@joinedcontext/sdk";
import { useAnalysis } from "../analysis";
import type { AnalysisOutput, Place } from "../analysis";
import { ALERT, ATTRS, EMPTY, filterOf, readView, startOf, toInput, writeView } from "../alerts";
import type { ViewState } from "../alerts";
import { cellOf, weekOption } from "../charts";
import { Acks } from "../components/Acks";
import { ChartCard } from "../components/ChartCard";
import { Filters } from "../components/Filters";
import { HexMap } from "../components/HexMap";
import { Reports } from "../components/Reports";
import { Weeks } from "../components/Weeks";
import { day, hourOfWeek, moment, number, subCategory, t } from "../i18n";
import type { Lang } from "../i18n";
import { useServer } from "../server";

const QUERY = { attrs: ATTRS };

/** Where a repeat place is, in the words its alerts use: the name of the first, else its address. */
function placeName(place: Place, rows: Map<string, Row>): string {
  for (const id of place.ids) {
    const row = rows.get(id);
    if (row) return displayName(row) || format(row.address);
  }
  return "";
}

/**
 * The newest alerts by when they start, at most `most`, each named in words with its start, as
 * the feed repeats a name for every alert at one place.
 */
export function newest(rows: readonly Row[], lang: Lang, most = 10): Array<{ id: string; name: string }> {
  return rows
    .map((row) => ({ row, at: startOf(row) }))
    .sort((a, b) => (b.at ?? -Infinity) - (a.at ?? -Infinity))
    .slice(0, most)
    .map(({ row, at }) => {
      const name = displayName(row);
      return { id: row.id, name: at === null ? name : `${name} ${moment(lang, at)}` };
    });
}

/** The sentence that answers the page's question before anything is touched. */
export function summaryOf(lang: Lang, out: AnalysisOutput): string {
  const hot = number(lang, out.hexes[0]?.count ?? 0);
  if (out.first === null || out.last === null || out.busiest === null) return t(lang, "summaryNoTime", { kept: number(lang, out.kept), hot });
  return t(lang, "summary", {
    kept: number(lang, out.kept),
    first: day(lang, out.first),
    last: day(lang, out.last),
    hot,
    busiest: hourOfWeek(lang, out.busiest.weekday, out.busiest.hour),
  });
}

/**
 * Where and when Helsinki's alerts happen (T-3333): one sentence that answers it, the hexagon map
 * with the places alerts keep coming back to, the hours of the week they start in (a cell picks
 * that hour), and the filters, all in the address.
 */
export function Heatmap({ lang }: { lang: Lang }) {
  const { rows, loading, error, reload } = useEntities(ALERT, QUERY);
  const server = useServer(useClient().config.appName);
  const snapshot = useRef<(() => Promise<Blob | null>) | null>(null);
  const [view, setView] = useState<ViewState>(() => readView(window.location.search));
  useEffect(() => {
    try {
      window.history.replaceState(null, "", `${window.location.pathname}${writeView(window.location.search, view)}${window.location.hash}`);
    } catch {
      // A sandboxed preview may refuse; the view still holds on the page.
    }
  }, [view]);

  const latest = useMemo(() => newest(rows, lang), [rows, lang]);
  const byId = useMemo(() => new Map(rows.map((row) => [row.id, row])), [rows]);
  const alerts = useMemo(() => rows.map(toInput), [rows]);
  const input = useMemo(() => (rows.length === 0 ? null : { alerts, filter: filterOf(view) }), [rows.length, alerts, view]);
  const { output, running, error: failed } = useAnalysis(input);
  const week = useMemo(() => (output ? weekOption(output.hourOfWeek, lang) : null), [output, lang]);
  const waiting = loading && rows.length === 0;
  const picked = view.weekday !== null && view.hour !== null ? hourOfWeek(lang, view.weekday, view.hour) : null;

  const unreadable =
    error instanceof ProblemError && error.status > 0 ? t(lang, "unreadableStatus", { status: error.status }) : t(lang, "unreadable");
  // A repeat place opens its first alert in the SDK's panel (SDK-40); a hexagon is a count, not an entity.
  const { select } = useEntitySelection();
  const openPlace = (place: Place) => select({ id: place.ids[0], type: ALERT });
  const toggle = (kind: string) =>
    setView((current) => ({ ...current, kinds: current.kinds.includes(kind) ? current.kinds.filter((k) => k !== kind) : [...current.kinds, kind] }));

  return (
    <Page label={t(lang, "page")}>
      {error && (
        <div className="jc-problem" role="alert">
          <strong>{unreadable}</strong>
          <button type="button" className="jc-button" onClick={reload}>
            {t(lang, "retry")}
          </button>
        </div>
      )}
      {failed && (
        <div className="jc-problem" role="alert">
          <strong>{t(lang, "analysisFailed", { reason: failed.message })}</strong>
        </div>
      )}
      {waiting ? (
        <Loading label={t(lang, "loading")} />
      ) : rows.length === 0 && !error ? (
        <Empty>{t(lang, "none")}</Empty>
      ) : output ? (
        <p className="app-summary" aria-live="polite">
          {output.kept === 0 ? t(lang, "noneMatch") : summaryOf(lang, output)}
          {output.unlocated > 0 && <span className="app-note"> {t(lang, "unlocated", { n: number(lang, output.unlocated) })}</span>}
        </p>
      ) : running ? (
        <Loading label={t(lang, "analysing")} />
      ) : null}

      <form className="app-filters" aria-label={t(lang, "filters")} onSubmit={(event) => event.preventDefault()}>
        <label>
          <span>{t(lang, "from")}</span>
          <input type="date" value={view.from} max={view.to || undefined} onChange={(event) => setView({ ...view, from: event.target.value })} />
        </label>
        <label>
          <span>{t(lang, "to")}</span>
          <input type="date" value={view.to} min={view.from || undefined} onChange={(event) => setView({ ...view, to: event.target.value })} />
        </label>
        <fieldset className="app-kinds">
          <legend>{t(lang, "kinds")}</legend>
          {(output?.subCategories ?? []).map((kind) => (
            <label key={kind.name} className="app-check">
              <input type="checkbox" checked={view.kinds.includes(kind.name)} onChange={() => toggle(kind.name)} />
              {subCategory(lang, kind.name)} ({number(lang, kind.count)})
            </label>
          ))}
        </fieldset>
        <button type="button" className="jc-button" onClick={() => setView(EMPTY)}>
          {t(lang, "clear")}
        </button>
      </form>
      {picked && (
        <p className="app-picked" aria-live="polite">
          <button type="button" className="app-pick" onClick={() => setView({ ...view, weekday: null, hour: null })} aria-label={t(lang, "unpick", { when: picked })}>
            {t(lang, "picked", { when: picked })} ×
          </button>
        </p>
      )}

      <Card title={t(lang, "map")}>
        <HexMap
          hexes={output?.hexes ?? []}
          places={output?.places ?? []}
          label={t(lang, "map")}
          onPlace={openPlace}
          snapshot={snapshot}
          describe={(what) =>
            what.kind === "hex"
              ? [t(lang, "hexLine", { count: number(lang, what.hex.count) })]
              : [placeName(what.place, byId), t(lang, "placeLine", { count: number(lang, what.place.count), radius: number(lang, what.place.radius) })].filter(Boolean)
          }
        />
        <p className="app-legend-text">{t(lang, "mapLegend")}</p>
        {output && <p className="app-note">{t(lang, "onMap", { n: number(lang, output.hexes.length) })}</p>}
      </Card>
      <Grid columns={2}>
        <ChartCard
          title={t(lang, "week")}
          option={error ? null : week}
          loading={waiting || (running && !output)}
          empty={t(lang, "weekEmpty")}
          height={300}
          onPick={(params) => {
            const cell = cellOf(params);
            if (cell) setView((current) => ({ ...current, ...(current.weekday === cell.weekday && current.hour === cell.hour ? { weekday: null, hour: null } : cell) }));
          }}
        />
        <Card title={t(lang, "places")}>
          {!output ? (
            waiting ? <Loading label={t(lang, "loading")} /> : null
          ) : output.places.length === 0 ? (
            <Empty>{t(lang, "placesEmpty")}</Empty>
          ) : (
            <ol className="app-places" aria-label={t(lang, "places")}>
              {output.places.slice(0, 10).map((place) => {
                const name = placeName(place, byId) || `${place.lat.toFixed(4)}, ${place.lon.toFixed(4)}`;
                return (
                  <li key={place.ids.join(",")}>
                    <button type="button" className="app-place" aria-label={t(lang, "open", { name })} onClick={() => openPlace(place)}>
                      {name}
                    </button>
                    <span>{t(lang, "placeLine", { count: number(lang, place.count), radius: number(lang, place.radius) })}</span>
                  </li>
                );
              })}
            </ol>
          )}
        </Card>
      </Grid>
      <Grid columns={2}>
        <Reports
          lang={lang}
          server={server}
          output={output}
          nameOf={(place) => placeName(place, byId)}
          snapshot={snapshot}
          onOpen={(saved) => setView(readView(`?${saved}`))}
        />
        <Weeks lang={lang} server={server} />
      </Grid>
      <Grid columns={2}>
        <Filters lang={lang} server={server} onApply={(saved) => setView(readView(`?${saved}`))} />
        <Acks lang={lang} server={server} alerts={latest} />
      </Grid>
    </Page>
  );
}
