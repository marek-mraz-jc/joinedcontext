/**
 * The University of Žilina's open research (T-3140, AP-07, UI-30): the works of DREPO that carry
 * an open licence of their own, read from the public space `zilina-uniza` through its public
 * endpoint, so the bundle holds no token and never asks anybody to sign in (AP-28).
 *
 * Its own look (T-2779): a line of totals, the works per year as bars with the same numbers as a
 * table for a screen reader, the kinds as a switch and the busiest journals beside it, then the
 * list a person searches. No author is shown: the space carries none. A work's title opens it in the
 * SDK's entity panel, which links to it in the Portal: a public App writes nothing (SDK-39, SDK-40,
 * AP-140).
 */
import { useEffect, useMemo, useState } from "react";
import { AppShell, endpointSource, Page, transportFor, useClient, useEntitySelection } from "@joinedcontext/sdk";
import type { ShellPage } from "@joinedcontext/sdk";
import { byNewest, countBy, narrowed, perYear, workOf } from "./works";
import type { Work } from "./works";
import { stringsFor } from "./locales";
import type { Strings } from "./locales";

const SPACE = "zilina-uniza";
const PAGE = 200;
/** The most works the screen reads; DREPO held 787 open ones on 2026-10-06. */
export const MOST = 3000;
/** How many works the list shows before "show more". */
const STEP = 25;

type Loaded = { status: "loading" } | { status: "ready"; works: Work[]; truncated: boolean } | { status: "failed"; reason: string };

function useWorks(): Loaded | null {
  const { config } = useClient();
  const slug = config.endpoints?.find((candidate) => candidate.space === SPACE)?.slug ?? (config.space === SPACE ? config.slug : null);
  const language = config.language ?? "sk";
  const [loaded, setLoaded] = useState<Loaded>({ status: "loading" });

  useEffect(() => {
    if (!slug) return;
    let live = true;
    const source = endpointSource(slug, transportFor(config), language);
    (async (): Promise<Loaded> => {
      const works: Work[] = [];
      for (let offset = 0; offset < MOST; offset += PAGE) {
        const page = await source.query({ type: "Article" }, { offset, limit: PAGE });
        works.push(...page.rows.map(workOf));
        if (page.rows.length < PAGE) return { status: "ready", works, truncated: false };
      }
      return { status: "ready", works, truncated: true };
    })()
      .catch((cause: unknown): Loaded => ({
        status: "failed",
        // A `SourceError` is an `Error`: the endpoint's own words, else what was thrown.
        reason: cause instanceof Error ? cause.message : String(cause),
      }))
      .then((next) => {
        if (live) setLoaded(next);
      });
    return () => {
      live = false;
    };
    // `config` is the document the Portal served once; the slug and the language are what change.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [slug, language]);

  return slug ? loaded : null;
}

/** The research in the SDK's shell, which holds the entity panel (SDK-39). */
export default function App() {
  const { config } = useClient();
  const s = stringsFor(config.language);
  const pages: ShellPage[] = [{ id: "research", label: s.page, render: () => <ResearchPage /> }];
  return <AppShell title={s.title} pages={pages} language={s.locale} />;
}

function ResearchPage() {
  const { config } = useClient();
  const s = stringsFor(config.language);
  const loaded = useWorks();

  return (
    <Page label={s.page}>
        <p className="subtitle">{s.subtitle}</p>
        {loaded === null && <p role="status">{s.noEndpoint}</p>}
        {loaded?.status === "loading" && <p role="status">{s.loading}</p>}
        {loaded?.status === "failed" && <p role="alert" className="failed">{s.refused(loaded.reason)}</p>}
        {loaded?.status === "ready" && <Research works={loaded.works} truncated={loaded.truncated} s={s} />}
        <p className="source">{s.noAuthors}</p>
        <p className="source">{s.attribution}</p>
    </Page>
  );
}

function Research({ works, truncated, s }: { works: Work[]; truncated: boolean; s: Strings }) {
  const { config } = useClient();
  const { selected, select } = useEntitySelection();
  const endpoint = config.endpoints?.find((candidate) => candidate.space === SPACE)?.name;
  const [search, setSearch] = useState("");
  const [kind, setKind] = useState<string | null>(null);
  const [year, setYear] = useState<number | null>(null);
  const [limit, setLimit] = useState(STEP);

  const years = useMemo(() => perYear(works), [works]);
  const kinds = useMemo(() => countBy(works, (work) => work.kind), [works]);
  const series = useMemo(() => countBy(works, (work) => work.series), [works]);
  const shown = useMemo(() => narrowed(works, { search, kind, year }).sort(byNewest), [works, search, kind, year]);
  const busiest = Math.max(1, ...years.map((entry) => entry.count));

  // A new narrowing starts the list from its top.
  useEffect(() => setLimit(STEP), [search, kind, year]);

  return (
    <>
      {truncated && <p className="note">{s.truncated(works.length)}</p>}
      <p className="totals">
        <strong>{s.works(works.length)}</strong>
        {years.length > 0 && <> {s.span(years[0].year, years[years.length - 1].year)}</>} {s.series(series.length)}
      </p>

      <div className="overview">
        <section className="panel years" aria-labelledby="years-title">
          <h2 id="years-title">{s.perYear}</h2>
          <div className="bars" aria-hidden="true">
            {years.map((entry) => (
              <div key={entry.year} className="bar" title={`${entry.year}: ${entry.count}`}>
                <span className="fill" style={{ height: `${(entry.count / busiest) * 100}%` }} />
                <span className="label">{String(entry.year).slice(2)}</span>
              </div>
            ))}
          </div>
          <table className="visually-hidden">
            <caption>{s.perYearTable}</caption>
            <thead>
              <tr>
                <th scope="col">{s.year}</th>
                <th scope="col">{s.count}</th>
              </tr>
            </thead>
            <tbody>
              {years.map((entry) => (
                <tr key={entry.year}>
                  <td>{entry.year}</td>
                  <td>{entry.count}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </section>

        <section className="panel" aria-labelledby="series-title">
          <h2 id="series-title">{s.topSeries}</h2>
          <ol className="series">
            {series.slice(0, 6).map((entry) => (
              <li key={entry.name}>
                <span>{entry.name}</span> <span className="count">{entry.count}</span>
              </li>
            ))}
          </ol>
        </section>
      </div>

      <section aria-labelledby="list-title" className="list">
        <h2 id="list-title">{s.list}</h2>
        <div className="controls">
          <div className="field grow">
            <label htmlFor="search">{s.search}</label>
            <input id="search" type="search" value={search} onChange={(event) => setSearch(event.target.value)} aria-describedby="search-help" />
            <small id="search-help">{s.searchHelp}</small>
          </div>
          <div className="field">
            <label htmlFor="year">{s.pickYear}</label>
            <select id="year" value={year ?? ""} onChange={(event) => setYear(event.target.value === "" ? null : Number(event.target.value))}>
              <option value="">{s.allYears}</option>
              {[...years].reverse().filter((entry) => entry.count > 0).map((entry) => (
                <option key={entry.year} value={entry.year}>
                  {entry.year} ({entry.count})
                </option>
              ))}
            </select>
          </div>
        </div>
        <fieldset className="kinds">
          <legend>{s.kinds}</legend>
          {[{ name: null as string | null, count: works.length }, ...kinds].map((entry) => (
            <label key={entry.name ?? "all"} className="kind" data-checked={kind === entry.name}>
              <input type="radio" name="kind" checked={kind === entry.name} onChange={() => setKind(entry.name)} />
              <span>
                {entry.name === null ? s.allKinds : (s.kind[entry.name] ?? entry.name)} ({entry.count})
              </span>
            </label>
          ))}
        </fieldset>

        <p role="status" className="shown">{s.shown(Math.min(limit, shown.length), shown.length)}</p>
        {shown.length === 0 ? (
          <p>{s.none}</p>
        ) : (
          <ul className="works">
            {shown.slice(0, limit).map((work) => (
              <li key={work.id}>
                <h3 lang={work.language ?? undefined}>
                  <button
                    type="button"
                    className="opens"
                    aria-pressed={selected?.id === work.id}
                    onClick={() => select({ id: work.id, type: "Article", endpoint })}
                  >
                    {work.title ?? s.noTitle}
                  </button>
                </h3>
                <p className="meta">
                  {[
                    work.kind ? (s.kind[work.kind] ?? work.kind) : null,
                    work.year ?? s.noYear,
                    work.language ? (s.language[work.language] ?? work.language) : null,
                  ]
                    .filter((part) => part !== null)
                    .join(" · ")}
                </p>
                {work.collection && <p className="collection">{work.collection}</p>}
                {work.url && (
                  <a href={work.url} target="_blank" rel="noopener noreferrer">
                    {s.open}
                    <span className="visually-hidden">: {work.title ?? s.noTitle}</span>
                  </a>
                )}
              </li>
            ))}
          </ul>
        )}
        {shown.length > limit && (
          <button type="button" className="more" onClick={() => setLimit(limit + STEP)}>
            {s.more}
          </button>
        )}
      </section>
    </>
  );
}
