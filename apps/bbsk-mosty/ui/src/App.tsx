/**
 * The bridge desk for the region's road management (T-2784, AP-07, AP-14, AP-118): every bridge of
 * the register with its road, year built, spans, length, material and heritage status, the
 * region's figures, and the oldest and longest tenth marked against the region's own bridges.
 * Staff sign in: the App is opened by its viewer role, held by its default group (T-2686). It
 * reads and never writes; the export is the table as shown, made in the browser. It sits in the
 * SDK's shell (SDK-39), and a bridge's name opens it in the shell's entity panel (SDK-40), read
 * fresh through the same endpoint. The App's grant reads only: the register is its pipeline's, so
 * the panel offers Edit to nobody here and links the bridge to the Portal instead (README).
 */
import { useEffect, useMemo, useState } from "react";
import { AppShell, Empty, endpointSource, Loading, Page, Problem, SourceError, transportFor, useClient, useEntitySelection } from "@joinedcontext/sdk";
import type { EntitySource } from "@joinedcontext/sdk";
import { bridgeOf, highestTenth, sorted, toCsv, totals } from "./bridges";
import type { Bridge, SortKey } from "./bridges";
import { stringsFor } from "./locales";
import type { Strings } from "./locales";

const SPACE = "bbsk-registre";
const PAGE = 200;
export const MOST = 5000;
const ROAD_CLASSES = ["motorway", "firstClass", "secondClass", "thirdClass", "local", "service"];

type Load = { status: "loading" } | { status: "ready"; bridges: Bridge[]; truncated: boolean } | { status: "failed"; reason: string };

function folded(text: string): string {
  return text.normalize("NFD").replace(/\p{Diacritic}/gu, "").toLowerCase();
}

/** Every bridge of the register, page by page up to `MOST`. */
export async function loadBridges(source: EntitySource, locale: string, year: number): Promise<{ bridges: Bridge[]; truncated: boolean }> {
  const bridges: Bridge[] = [];
  for (let offset = 0; offset < MOST; offset += PAGE) {
    const page = await source.query({ type: "Bridge" }, { offset, limit: PAGE });
    bridges.push(...page.rows.map((row) => bridgeOf(row, locale, year)));
    if (page.rows.length < PAGE) return { bridges, truncated: false };
  }
  return { bridges, truncated: true };
}

export default function App({ year = new Date().getFullYear() }: { year?: number }) {
  const { config } = useClient();
  const s = stringsFor(config.language);
  return <AppShell title={s.title} pages={[{ id: "bridges", label: s.title, render: () => <Desk year={year} /> }]} language={config.language} />;
}

function Desk({ year }: { year: number }) {
  const { config } = useClient();
  const s = stringsFor(config.language);
  const { selected, select } = useEntitySelection();
  const language = config.language ?? "sk";
  const listed = config.endpoints?.find((one) => one.space === SPACE);
  const slug = listed?.slug ?? (config.space === SPACE ? config.slug : null);
  const source = useMemo(
    () => (slug ? endpointSource(slug, transportFor(config), language) : null),
    // `config` is the document the Portal served once; the slug and the language are what change.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [slug, language],
  );
  const [load, setLoad] = useState<Load>({ status: "loading" });
  const [search, setSearch] = useState("");
  const [district, setDistrict] = useState("");
  const [roadClass, setRoadClass] = useState("");
  const [listedOnly, setListedOnly] = useState(false);
  const [sort, setSort] = useState<{ key: SortKey; ascending: boolean }>({ key: "age", ascending: false });

  useEffect(() => {
    if (!source) return;
    let live = true;
    loadBridges(source, language.slice(0, 2), year)
      .then((done) => {
        if (live) setLoad({ status: "ready", ...done });
      })
      .catch((cause: unknown) => {
        const reason = cause instanceof SourceError || cause instanceof Error ? cause.message : String(cause);
        if (live) setLoad({ status: "failed", reason });
      });
    return () => {
      live = false;
    };
  }, [source, language, year]);

  const all = load.status === "ready" ? load.bridges : [];
  const districts = useMemo(() => [...new Set(all.map((b) => b.district).filter((d): d is string => d !== null))].sort((a, b) => a.localeCompare(b, "sk")), [all]);
  const shown = useMemo(() => {
    const words = folded(search).split(/\s+/).filter(Boolean);
    const kept = all
      .filter((b) => !district || b.district === district)
      .filter((b) => !roadClass || b.roadClass === roadClass)
      .filter((b) => !listedOnly || b.heritage === "cultural" || b.heritage === "culturalAndTechnical")
      .filter((b) => words.every((word) => folded(`${b.name ?? ""} ${b.code ?? ""} ${b.manager ?? ""}`).includes(word)));
    return sorted(kept, sort.key, sort.ascending);
  }, [all, search, district, roadClass, listedOnly, sort]);
  // The tenths are the whole region's, whatever the filters narrow the table to.
  const oldestFrom = useMemo(() => highestTenth(all.map((b) => b.age)), [all]);
  const longestFrom = useMemo(() => highestTenth(all.map((b) => b.length)), [all]);
  const sum = useMemo(() => totals(all), [all]);

  const download = () => {
    const blob = new Blob([`﻿${toCsv(shown, s.csvHeader, s.values)}`], { type: "text/csv;charset=utf-8" });
    const url = URL.createObjectURL(blob);
    const link = document.createElement("a");
    link.href = url;
    link.download = s.csvName;
    link.click();
    URL.revokeObjectURL(url);
  };

  if (!source) {
    return (
      <Page>
        <p className="subtitle">{s.subtitle}</p>
        <Empty>{s.noEndpoint}</Empty>
      </Page>
    );
  }

  const number = (value: number | null, digits = 0) =>
    value === null ? s.noValue : new Intl.NumberFormat(s.locale, { maximumFractionDigits: digits }).format(value);

  return (
    <Page>
      <p className="subtitle">{s.subtitle}</p>
      {load.status === "loading" && <Loading label={s.loading} />}
      {load.status === "failed" && <Problem error={new Error(s.failed(load.reason))} />}
      {load.status === "ready" && (
        <>
          <dl className="tiles">
            <Tile label={s.tiles.bridges} value={number(sum.bridges)} />
            <Tile label={s.tiles.length} value={number(sum.length)} />
            <Tile label={s.tiles.medianAge} value={number(sum.medianAge, 1)} />
            <Tile label={s.tiles.listed} value={number(sum.listed)} />
            <Tile label={s.tiles.incomplete} value={number(sum.incomplete)} />
          </dl>
          {load.truncated && <p className="note">{s.truncated(MOST)}</p>}
          <div className="controls">
            <label className="search">
              {s.search}
              <input type="search" value={search} onChange={(event) => setSearch(event.target.value)} />
            </label>
            <label className="pick">
              {s.district}
              <select value={district} onChange={(event) => setDistrict(event.target.value)}>
                <option value="">{s.any}</option>
                {districts.map((one) => (
                  <option key={one} value={one}>
                    {one}
                  </option>
                ))}
              </select>
            </label>
            <label className="pick">
              {s.roadClass}
              <select value={roadClass} onChange={(event) => setRoadClass(event.target.value)}>
                <option value="">{s.any}</option>
                {ROAD_CLASSES.map((one) => (
                  <option key={one} value={one}>
                    {s.values[one]}
                  </option>
                ))}
              </select>
            </label>
            <label className="check">
              <input type="checkbox" checked={listedOnly} onChange={(event) => setListedOnly(event.target.checked)} />
              {s.listedOnly}
            </label>
            <button type="button" onClick={download} disabled={shown.length === 0}>
              {s.download}
            </button>
          </div>
          <p className="note">{oldestFrom === null && longestFrom === null ? s.tenthUnavailable : s.tenthNote}</p>
          <BridgeTable
            bridges={shown}
            sort={sort}
            onSort={setSort}
            oldestFrom={oldestFrom}
            longestFrom={longestFrom}
            number={number}
            s={s}
            selected={selected?.id ?? null}
            onOpen={(id) => select({ id, type: "Bridge", endpoint: listed?.name })}
          />
        </>
      )}
      <p className="source">{s.source}</p>
    </Page>
  );
}

function Tile({ label, value }: { label: string; value: string }) {
  return (
    <div className="tile">
      <dt>{label}</dt>
      <dd>{value}</dd>
    </div>
  );
}

function BridgeTable({
  bridges,
  sort,
  onSort,
  oldestFrom,
  longestFrom,
  number,
  s,
  selected,
  onOpen,
}: {
  bridges: Bridge[];
  sort: { key: SortKey; ascending: boolean };
  onSort: (next: { key: SortKey; ascending: boolean }) => void;
  oldestFrom: number | null;
  longestFrom: number | null;
  number: (value: number | null, digits?: number) => string;
  s: Strings;
  selected: string | null;
  onOpen: (id: string) => void;
}) {
  if (bridges.length === 0) return <Empty>{s.empty}</Empty>;
  const header = (key: SortKey) => {
    const active = sort.key === key;
    return (
      <th scope="col" aria-sort={active ? (sort.ascending ? "ascending" : "descending") : "none"}>
        <button type="button" aria-label={s.sortBy(s.column[key])} onClick={() => onSort({ key, ascending: active ? !sort.ascending : key === "name" })}>
          {s.column[key]}
          <span aria-hidden="true">{active ? (sort.ascending ? " ▲" : " ▼") : ""}</span>
        </button>
      </th>
    );
  };
  const word = (value: string | null) => (value === null ? s.noValue : (s.values[value] ?? value));
  return (
    <div className="table-scroll" tabIndex={0} role="region" aria-label={s.title}>
      <table>
        <thead>
          <tr>
            {header("name")}
            <th scope="col">{s.column.road}</th>
            <th scope="col">{s.column.yearBuilt}</th>
            {header("age")}
            {header("spans")}
            {header("length")}
            <th scope="col">{s.column.material}</th>
            <th scope="col">{s.column.heritage}</th>
            <th scope="col">{s.column.manager}</th>
            <th scope="col">{s.column.district}</th>
          </tr>
        </thead>
        <tbody>
          {bridges.map((b) => {
            const old = oldestFrom !== null && b.age !== null && b.age >= oldestFrom;
            const long = longestFrom !== null && b.length !== null && b.length >= longestFrom;
            return (
              <tr key={b.id} aria-selected={b.id === selected}>
                <th scope="row">
                  <button type="button" className="row-open" onClick={() => onOpen(b.id)}>
                    {b.name ?? s.noValue}
                  </button>
                  {b.code ? <span className="address">{b.code}</span> : null}
                </th>
                <td className="text">
                  {word(b.roadClass)}
                  {b.roadNumber ? ` ${b.roadNumber}` : ""}
                </td>
                <td>{b.yearBuilt === null ? s.noValue : String(b.yearBuilt)}</td>
                <td className={old ? "flag" : undefined}>
                  {b.age === null ? s.noValue : s.years(b.age)}
                  {old ? <span className="mark">▲ {s.oldest}</span> : null}
                </td>
                <td>{number(b.spans)}</td>
                <td className={long ? "flag" : undefined}>
                  {number(b.length, 1)}
                  {long ? <span className="mark">▲ {s.longest}</span> : null}
                </td>
                <td className="text">{b.material ?? s.noValue}</td>
                <td className="text">{word(b.heritage)}</td>
                <td className="text">{b.manager ?? s.noValue}</td>
                <td className="text">{b.district ?? s.noValue}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
