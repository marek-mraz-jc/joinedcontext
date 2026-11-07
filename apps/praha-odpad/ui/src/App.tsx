/**
 * The waste-collection desk for Prague's staff (T-2786, AP-07, AP-14, AP-118): every sensor
 * container with its kind of waste, how full it was at its last reading and how long ago that was,
 * the isle it stands at, and the fullest and longest-unread tenth of the city marked in words.
 * Staff sign in: the App is opened by its viewer role, held by its default group (T-2686). It
 * reads and never writes; the export is the table as shown, made in the browser. A container's code
 * opens it in the SDK's entity panel, which links to it in the Portal: the viewer role grants no
 * write (SDK-39, SDK-40).
 */
import { useEffect, useMemo, useState } from "react";
import { AppShell, endpointSource, idChunks, Page, transportFor, useClient, useEntitySelection } from "@joinedcontext/sdk";
import type { EntitySource, RichCell, ShellPage } from "@joinedcontext/sdk";
import { containerOf, highestTenth, sorted, toCsv, totals } from "./containers";
import type { Container, SortKey } from "./containers";
import { stringsFor } from "./locales";
import type { Strings } from "./locales";

const SPACE = "praha-mesto";
const PAGE = 200;
export const MOST = 4000;
const KINDS = ["colouredGlass", "clearGlass", "paper", "plastic", "metal", "beverageCartons", "electronics", "edibleOil", "mixedRecyclables"];

type Load =
  | { status: "loading" }
  | { status: "ready"; containers: Container[]; isles: Map<string, string>; truncated: boolean }
  | { status: "failed"; reason: string };

function folded(text: string): string {
  return text.normalize("NFD").replace(/\p{Diacritic}/gu, "").toLowerCase();
}

/** The names of the isles the containers stand at, read by id in chunks: never every isle. */
async function isleNames(source: EntitySource, ids: string[], locale: string): Promise<Map<string, string>> {
  const names = new Map<string, string>();
  for (const chunk of idChunks(ids)) {
    const page = await source.query({ type: "WasteContainerIsle", ids: chunk }, { offset: 0, limit: chunk.length });
    for (const row of page.rows) {
      const cell = row.cells.name;
      const one = (Array.isArray(cell) ? cell[0] : cell) as RichCell | undefined;
      const map = one?.languageMap;
      const name = map ? (map[locale] ?? map.cs ?? Object.values(map)[0]) : one?.value;
      if (typeof name === "string" && name.trim() !== "") names.set(row.id, name.trim());
    }
  }
  return names;
}

/** The desk in the SDK's shell, which holds the entity panel (SDK-39). */
/**
 * `most` is how many containers the desk reads, page by page (default `MOST`); a test sets a small
 * one rather than render thousands of rows.
 */
export default function App({ now, most = MOST }: { now?: Date; most?: number }) {
  const { config } = useClient();
  const s = stringsFor(config.language);
  const pages: ShellPage[] = [{ id: "desk", label: s.page, render: () => <Desk now={now} most={most} s={s} /> }];
  return <AppShell title={s.title} pages={pages} language={s.locale} />;
}

function Desk({ now, most, s }: { now?: Date; most: number; s: Strings }) {
  const { config } = useClient();
  const language = config.language ?? "cs";
  const slug = config.endpoints?.find((one) => one.space === SPACE)?.slug ?? (config.space === SPACE ? config.slug : null);
  const source = useMemo(
    () => (slug ? endpointSource(slug, transportFor(config), language) : null),
    // `config` is the document the Portal served once; the slug and the language are what change.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [slug, language],
  );
  const [load, setLoad] = useState<Load>({ status: "loading" });
  const [search, setSearch] = useState("");
  const [kind, setKind] = useState("");
  const [fullestOnly, setFullestOnly] = useState(false);
  const [sort, setSort] = useState<{ key: SortKey; ascending: boolean }>({ key: "fill", ascending: false });
  // The moment ages are counted from, fixed once: a new Date each render would reload forever.
  const [at] = useState(() => (now ?? new Date()).getTime());

  useEffect(() => {
    if (!source) return;
    let live = true;
    (async () => {
      const containers: Container[] = [];
      let truncated = true;
      for (let offset = 0; offset < most; offset += PAGE) {
        const page = await source.query({ type: "WasteContainer" }, { offset, limit: PAGE });
        containers.push(...page.rows.map((row) => containerOf(row, new Date(at))));
        if (page.rows.length < PAGE) {
          truncated = false;
          break;
        }
      }
      const ids = [...new Set(containers.map((c) => c.isle).filter((id): id is string => id !== null))];
      // An isle that cannot be named is said so; the containers are still the desk's.
      const isles = await isleNames(source, ids, language.slice(0, 2)).catch(() => new Map<string, string>());
      return { containers, isles, truncated };
    })()
      .then((done) => {
        if (live) setLoad({ status: "ready", ...done });
      })
      .catch((cause: unknown) => {
        if (live) setLoad({ status: "failed", reason: reasonOf(cause) });
      });
    return () => {
      live = false;
    };
  }, [source, language, at, most]);

  const all = useMemo(() => (load.status === "ready" ? load.containers : []), [load]);
  const isles = useMemo(() => (load.status === "ready" ? load.isles : new Map<string, string>()), [load]);
  const isleName = (id: string | null) => (id === null ? null : (isles.get(id) ?? null));
  // The tenths are the whole city's, whatever the filters narrow the table to.
  const fullestFrom = useMemo(() => highestTenth(all.map((c) => c.fill)), [all]);
  const unreadFrom = useMemo(() => highestTenth(all.map((c) => c.ageHours)), [all]);
  const shown = useMemo(() => {
    const words = folded(search).split(/\s+/).filter(Boolean);
    const kept = all
      .filter((c) => !kind || c.kind === kind)
      .filter((c) => !fullestOnly || (fullestFrom !== null && c.fill !== null && c.fill >= fullestFrom))
      .filter((c) => words.every((word) => folded(`${c.code ?? ""} ${isles.get(c.isle ?? "") ?? ""}`).includes(word)));
    return sorted(kept, sort.key, sort.ascending);
  }, [all, isles, search, kind, fullestOnly, fullestFrom, sort]);
  const sum = useMemo(() => totals(all), [all]);

  const download = () => {
    const blob = new Blob([`﻿${toCsv(shown, s.csvHeader, s.values, isleName)}`], { type: "text/csv;charset=utf-8" });
    const url = URL.createObjectURL(blob);
    const link = document.createElement("a");
    link.href = url;
    link.download = s.csvName;
    link.click();
    URL.revokeObjectURL(url);
  };

  if (!source) {
    return (
      <Page label={s.page}>
        <p className="subtitle">{s.subtitle}</p>
        <p role="status">{s.noEndpoint}</p>
      </Page>
    );
  }

  const percent = (value: number | null) =>
    value === null ? s.noValue : new Intl.NumberFormat(s.locale, { style: "percent", maximumFractionDigits: 0 }).format(value);

  return (
      <Page label={s.page}>
        <p className="subtitle">{s.subtitle}</p>
        {load.status === "loading" && <p role="status">{s.loading}</p>}
        {load.status === "failed" && <p role="alert" className="failed">{s.failed(load.reason)}</p>}
        {load.status === "ready" && (
          <>
            <dl className="tiles">
              <Tile label={s.tiles.containers} value={new Intl.NumberFormat(s.locale).format(sum.containers)} />
              <Tile label={s.tiles.meanFill} value={percent(sum.meanFill)} />
              <Tile label={s.tiles.unread} value={new Intl.NumberFormat(s.locale).format(sum.unread)} />
            </dl>
            {load.truncated && <p className="note">{s.truncated(most)}</p>}
            <div className="controls">
              <label className="search">
                {s.search}
                <input type="search" value={search} onChange={(event) => setSearch(event.target.value)} />
              </label>
              <label className="pick">
                {s.kind}
                <select value={kind} onChange={(event) => setKind(event.target.value)}>
                  <option value="">{s.any}</option>
                  {KINDS.map((one) => (
                    <option key={one} value={one}>
                      {s.values[one]}
                    </option>
                  ))}
                </select>
              </label>
              <label className="check">
                <input type="checkbox" checked={fullestOnly} disabled={fullestFrom === null} onChange={(event) => setFullestOnly(event.target.checked)} />
                {s.fullestOnly}
              </label>
              <button type="button" onClick={download} disabled={shown.length === 0}>
                {s.download}
              </button>
            </div>
            <p className="note">{fullestFrom === null && unreadFrom === null ? s.tenthUnavailable : s.tenthNote}</p>
            <ContainerTable
              containers={shown}
              sort={sort}
              onSort={setSort}
              fullestFrom={fullestFrom}
              unreadFrom={unreadFrom}
              percent={percent}
              isleName={isleName}
              s={s}
            />
          </>
        )}
        <p className="source">{s.source}</p>
      </Page>
  );
}

/** A failure in words: the endpoint's own (a `SourceError` is an `Error`), else what was thrown. */
export function reasonOf(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

function Tile({ label, value }: { label: string; value: string }) {
  return (
    <div className="tile">
      <dt>{label}</dt>
      <dd>{value}</dd>
    </div>
  );
}

function ContainerTable({
  containers,
  sort,
  onSort,
  fullestFrom,
  unreadFrom,
  percent,
  isleName,
  s,
}: {
  containers: Container[];
  sort: { key: SortKey; ascending: boolean };
  onSort: (next: { key: SortKey; ascending: boolean }) => void;
  fullestFrom: number | null;
  unreadFrom: number | null;
  percent: (value: number | null) => string;
  isleName: (id: string | null) => string | null;
  s: Strings;
}) {
  if (containers.length === 0) return <p>{s.empty}</p>;
  const header = (key: SortKey) => {
    const active = sort.key === key;
    return (
      <th scope="col" aria-sort={active ? (sort.ascending ? "ascending" : "descending") : "none"}>
        <button type="button" aria-label={s.sortBy(s.column[key])} onClick={() => onSort({ key, ascending: active ? !sort.ascending : key === "code" })}>
          {s.column[key]}
          <span aria-hidden="true">{active ? (sort.ascending ? " ▲" : " ▼") : ""}</span>
        </button>
      </th>
    );
  };
  return (
    <div className="table-scroll" tabIndex={0} role="region" aria-label={s.title}>
      <table>
        <thead>
          <tr>
            {header("code")}
            <th scope="col">{s.column.kind}</th>
            {header("fill")}
            {header("ageHours")}
            <th scope="col">{s.column.isle}</th>
          </tr>
        </thead>
        <tbody>
          {containers.map((c) => {
            const full = fullestFrom !== null && c.fill !== null && c.fill >= fullestFrom;
            const unread = unreadFrom !== null && c.ageHours !== null && c.ageHours >= unreadFrom;
            return (
              <tr key={c.id}>
                <th scope="row">
                  <Opens id={c.id} name={c.code ?? s.noValue} />
                </th>
                <td className="text">{c.kind === null ? s.noValue : (s.values[c.kind] ?? c.kind)}</td>
                <td className={full ? "flag" : undefined}>
                  {percent(c.fill)}
                  {full ? <span className="mark">▲ {s.fullest}</span> : null}
                </td>
                <td className={unread ? "flag" : undefined}>
                  {c.measuredAt && c.ageHours !== null ? <time dateTime={c.measuredAt}>{s.ago(c.ageHours)}</time> : s.noValue}
                  {unread ? <span className="mark">▲ {s.longestUnread}</span> : null}
                </td>
                <td className="text">{isleName(c.isle) ?? s.isleUnknown}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

/** A container's code as a button that opens it in the entity panel, read through this App's endpoint (SDK-40). */
function Opens({ id, name }: { id: string; name: string }) {
  const { config } = useClient();
  const { select } = useEntitySelection();
  const endpoint = config.endpoints?.find((one) => one.space === SPACE)?.name;
  return (
    <button type="button" className="opens" onClick={() => select({ id, type: "WasteContainer", endpoint })}>
      {name}
    </button>
  );
}
