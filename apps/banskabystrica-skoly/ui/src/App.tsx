/**
 * The school coverage desk for city staff (T-2782, AP-07, AP-14, AP-118): every school of the
 * national school map in the city with its pupils, teachers and budget, the two ratios a desk
 * compares, and what stands out against the city's own schools. Staff sign in: the App's
 * visibility is its roles, and the viewer role is its default group's (T-2686). It reads and
 * never writes; the export is the table as shown, made in the browser. It sits in the SDK's shell
 * (SDK-39), and a school's name opens it in the shell's entity panel (SDK-40), read fresh through
 * the same endpoint. The App's grant reads only: the schools are the national school map's,
 * written by its pipeline, so the panel links a school to the Portal instead (README).
 */
import { useEffect, useMemo, useState } from "react";
import { AppShell, Empty, endpointSource, Loading, Page, Problem, transportFor, useClient, useEntitySelection } from "@joinedcontext/sdk";
import type { EntitySource } from "@joinedcontext/sdk";
import { schoolOf, sorted, tenth, toCsv, totals, withShared } from "./coverage";
import type { School, SortKey } from "./coverage";
import { stringsFor } from "./locales";
import type { Strings } from "./locales";

const SPACE = "banskabystrica-verejne";
const PAGE = 200;
export const MOST = 1000;

type Load = { status: "loading" } | { status: "ready"; schools: School[]; truncated: boolean } | { status: "failed"; reason: string };

function folded(text: string): string {
  return text.normalize("NFD").replace(/\p{Diacritic}/gu, "").toLowerCase();
}

/** Every school of the space, page by page up to `MOST`. */
export async function loadSchools(source: EntitySource, locale: string): Promise<{ schools: School[]; truncated: boolean }> {
  const schools: School[] = [];
  for (let offset = 0; offset < MOST; offset += PAGE) {
    const page = await source.query({ type: "Organization" }, { offset, limit: PAGE });
    schools.push(...page.rows.map((row) => schoolOf(row, locale)));
    if (page.rows.length < PAGE) return { schools: withShared(schools), truncated: false };
  }
  return { schools: withShared(schools), truncated: true };
}

export default function App() {
  const { config } = useClient();
  const s = stringsFor(config.language);
  return <AppShell title={s.title} pages={[{ id: "schools", label: s.title, render: () => <Desk /> }]} language={config.language} />;
}

function Desk() {
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
  const [completeOnly, setCompleteOnly] = useState(false);
  const [sort, setSort] = useState<{ key: SortKey; ascending: boolean }>({ key: "name", ascending: true });

  useEffect(() => {
    if (!source) return;
    let live = true;
    loadSchools(source, language.slice(0, 2))
      .then((done) => live && setLoad({ status: "ready", ...done }))
      .catch((cause: unknown) => {
        // A `SourceError` is an `Error`: the refusal's own words, else whatever was thrown.
        const reason = cause instanceof Error ? cause.message : String(cause);
        if (live) setLoad({ status: "failed", reason });
      });
    return () => {
      live = false;
    };
  }, [source, language]);

  const all = load.status === "ready" ? load.schools : [];
  const shown = useMemo(() => {
    const words = folded(search).split(/\s+/).filter(Boolean);
    const kept = all
      .filter((one) => !completeOnly || (one.pupilsPerTeacher !== null && one.budgetPerPupil !== null))
      .filter((one) => words.every((word) => folded(`${one.name ?? ""} ${one.address ?? ""}`).includes(word)));
    return sorted(kept, sort.key, sort.ascending);
  }, [all, search, completeOnly, sort]);
  // The tenths are the whole city's, whatever the search narrows the table to.
  const highRatio = useMemo(() => tenth(all.map((one) => one.pupilsPerTeacher), "high"), [all]);
  const lowBudget = useMemo(() => tenth(all.map((one) => one.budgetPerPupil), "low"), [all]);
  const sum = useMemo(() => totals(all), [all]);

  const download = () => {
    const blob = new Blob([`\uFEFF${toCsv(shown, s.csvHeader)}`], { type: "text/csv;charset=utf-8" });
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
            <Tile label={s.tiles.schools} value={number(sum.schools)} />
            <Tile label={s.tiles.pupils} value={number(sum.pupils)} />
            <Tile label={s.tiles.teachers} value={number(sum.teachers)} />
            <Tile label={s.tiles.ratio} value={number(sum.pupilsPerTeacher, 1)} />
            <Tile label={s.tiles.incomplete} value={number(sum.incomplete)} />
          </dl>
          {load.truncated && <p className="note">{s.truncated(MOST)}</p>}
          <div className="controls">
            <label className="search">
              {s.search}
              <input type="search" value={search} onChange={(event) => setSearch(event.target.value)} />
            </label>
            <label className="check">
              <input type="checkbox" checked={completeOnly} onChange={(event) => setCompleteOnly(event.target.checked)} />
              {s.completeOnly}
            </label>
            <button type="button" onClick={download} disabled={shown.length === 0}>
              {s.download}
            </button>
          </div>
          <p className="note">{highRatio === null && lowBudget === null ? s.tenthUnavailable : s.tenthNote}</p>
          <SchoolTable
            schools={shown}
            sort={sort}
            onSort={setSort}
            highRatio={highRatio}
            lowBudget={lowBudget}
            number={number}
            s={s}
            selected={selected?.id ?? null}
            onOpen={(id) => select({ id, type: "Organization", endpoint: listed?.name })}
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

function SchoolTable({
  schools,
  sort,
  onSort,
  highRatio,
  lowBudget,
  number,
  s,
  selected,
  onOpen,
}: {
  schools: School[];
  sort: { key: SortKey; ascending: boolean };
  onSort: (next: { key: SortKey; ascending: boolean }) => void;
  highRatio: number | null;
  lowBudget: number | null;
  number: (value: number | null, digits?: number) => string;
  s: Strings;
  selected: string | null;
  onOpen: (id: string) => void;
}) {
  if (schools.length === 0) return <Empty>{s.empty}</Empty>;
  const header = (key: SortKey) => {
    const active = sort.key === key;
    return (
      <th scope="col" aria-sort={active ? (sort.ascending ? "ascending" : "descending") : "none"}>
        <button
          type="button"
          aria-label={s.sortBy(s.column[key])}
          onClick={() => onSort({ key, ascending: active ? !sort.ascending : key === "name" })}
        >
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
            {header("name")}
            {header("pupils")}
            {header("teachers")}
            {header("pupilsPerTeacher")}
            <th scope="col">{s.column.otherStaff}</th>
            <th scope="col">{s.column.budget}</th>
            {header("budgetPerPupil")}
          </tr>
        </thead>
        <tbody>
          {schools.map((one) => {
            const high = highRatio !== null && one.pupilsPerTeacher !== null && one.pupilsPerTeacher >= highRatio;
            const low = lowBudget !== null && one.budgetPerPupil !== null && one.budgetPerPupil <= lowBudget;
            return (
              <tr key={one.id} aria-selected={one.id === selected}>
                <th scope="row">
                  <button type="button" className="row-open" onClick={() => onOpen(one.id)}>
                    {one.name ?? s.noValue}
                  </button>
                  {one.address ? <span className="address">{one.address}</span> : null}
                </th>
                <td>{number(one.pupils)}</td>
                <td>
                  {number(one.teachers)}
                  {one.sharedWith > 1 ? <span className="address">{s.shared(one.sharedWith)}</span> : null}
                </td>
                <td className={high ? "flag" : undefined}>
                  {number(one.pupilsPerTeacher, 1)}
                  {high ? <span className="mark">▲ {s.highest}</span> : null}
                </td>
                <td>{number(one.otherStaff)}</td>
                <td>
                  {number(one.budget)}
                  {one.budgetYear !== null ? <span className="address">{one.budgetYear}</span> : null}
                </td>
                <td className={low ? "flag" : undefined}>
                  {number(one.budgetPerPupil)}
                  {low ? <span className="mark">▼ {s.lowest}</span> : null}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
