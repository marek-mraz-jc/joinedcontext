/**
 * The publisher's records, with the one attribute a person may write (T-2436, T-2437).
 *
 * The same bundle is both grids: which body's records it shows comes from the space the served
 * configuration names, and the endpoint comes from the same document (SDK-02). Nothing here is
 * built for one project, so the city's screen and the region's cannot drift apart.
 *
 * The table itself is the SDK's `EntityGrid` (UI-64, UI-71, SDK-29): paging, a filter per column
 * that becomes the endpoint's own `q`, one open column, and a refusal shown beside the row it
 * belongs to. What this application adds is the narrowing — one type, six columns, one writable
 * attribute — and the sentence that says why the figures are not writable.
 *
 * Above the table, the same records as charts with the statistics office's names (T-2966): the
 * table keeps the publisher's codes, because its filters ask the endpoint about those.
 *
 * It sits in the SDK's shell (SDK-39), and a record opens in the shell's entity panel (SDK-40),
 * which offers Edit to a reader whose rights allow the note, on the note alone (README).
 */
import { useMemo } from "react";
import { AppShell, endpointSource, EntityGrid, Page, Problem, transportFor, useAccess, useClient, useEntitySelection } from "@joinedcontext/sdk";
import { recordRenderers } from "./cells";
import { Overview } from "./Overview";
import { gridConfig, NOTE, notesOnly } from "./records";
import { bodyOf, noteWords, SPACE_OF, stringsFor } from "./locales";

export default function App() {
  const { config } = useClient();
  const s = stringsFor(config.language);
  const body = bodyOf(config.space);
  const title = body ? s.title[body] : s.records;
  return <AppShell title={title} pages={[{ id: "records", label: title, render: () => <Records /> }]} language={config.language} />;
}

function Records() {
  const { config } = useClient();
  const { select } = useEntitySelection();
  const s = stringsFor(config.language);
  const body = bodyOf(config.space);
  const language = config.language;

  // The endpoint of this application's own space, found by space and never by position: a
  // configuration that names another body's endpoint first must not decide whose rows are shown.
  const listed = body ? config.endpoints?.find((candidate) => candidate.space === SPACE_OF[body]) : undefined;
  const slug = body ? (listed?.slug ?? config.slug) : null;

  const source = useMemo(
    () => (slug ? notesOnly(endpointSource(slug, transportFor(config), language), noteWords(s)) : null),
    // `config` is the document the Portal served once; the slug and the language are what change.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [slug, language],
  );
  const endpoint = listed?.name;
  // The note opens only for a reader whose access document grants the write (T-5091: the steward
  // role); the gateway decides each save again. Until the document arrives the grid is a reader.
  const { access, error: accessError, can } = useAccess(endpoint);
  const mayNote = can("updateAttrs", "KeyPerformanceIndicator", NOTE).ok;
  const accessKnown = access !== null || accessError !== null;
  const grid = useMemo(() => (slug ? gridConfig(slug, s.column, mayNote) : null), [slug, s.column, mayNote]);
  const renderers = useMemo(
    () =>
      recordRenderers(s.locale.startsWith("sk") ? "sk" : "en", { locale: s.locale, showId: s.showId, openRecord: s.openRecord }, (row) =>
        select({ id: row.id, type: row.type, endpoint }),
      ),
    [s.locale, s.showId, s.openRecord, select, endpoint],
  );

  if (!body) {
    return (
      <Page>
        <Problem error={new Error(s.unknownSpace)} />
      </Page>
    );
  }

  return (
    <Page>
      <p className="subtitle">{s.subtitle[body]}</p>
      {source && grid ? (
        <>
          <Overview body={body} source={source} s={s} />
          <section className="records" aria-labelledby="records-title">
            <h2 id="records-title">{s.records}</h2>
            <div className="notes">
              <p className="note">{s.recordsWhy}</p>
              <p className="note">{s.readOnlyWhy}</p>
              <p className="note">{s.noteWhy}</p>
              {accessKnown && !mayNote ? (
                <p className="note" role="note">
                  {s.noteStewardsOnly[body]}
                </p>
              ) : null}
            </div>
            <EntityGrid config={grid} source={source} labels={s.grid} renderers={renderers} />
          </section>
        </>
      ) : (
        <Problem error={new Error(s.noEndpoint)} />
      )}
      <p className="source">{s.source[body]}</p>
    </Page>
  );
}
