/**
 * The region's public registers as grids (T-2784, UI-64, UI-71, UI-84): hospitals, social
 * services, bridges, organizations and the territorial division from `bbsk-registre`, read through
 * the app's public endpoint, so nobody signs in and the bundle holds no token (AP-28).
 *
 * The table is the SDK's `EntityGrid`: paging, a filter per column that becomes the endpoint's own
 * `q`, the model's enums as pick lists, the name as the row's primary field opening the whole row.
 * What this application adds is one tab per register, the region's labels and the endpoint's own
 * downloads of each register. It sits in the SDK's shell (SDK-39), where the grid opens a row by
 * its name in the shell's entity panel (SDK-40): a public App, so the panel links to it in the
 * Portal and never offers Edit (AP-140).
 */
import { useMemo, useRef, useState } from "react";
import type { KeyboardEvent } from "react";
import { AppShell, Empty, endpointSource, EntityGrid, Page, transportFor, useClient } from "@joinedcontext/sdk";
import { DATASETS, ENUMS, exportUrl, gridConfig } from "./datasets";
import type { Dataset } from "./datasets";
import { stringsFor } from "./locales";

/** The space this application reads (`Development/10` §2). */
const SPACE = "bbsk-registre";

export default function App() {
  const { config } = useClient();
  const s = stringsFor(config.language);
  return <AppShell title={s.title} pages={[{ id: "registers", label: s.datasets, render: () => <Registers /> }]} language={config.language} />;
}

function Registers() {
  const { config } = useClient();
  const s = stringsFor(config.language);
  const language = config.language;
  const slug = config.endpoints?.find((candidate) => candidate.space === SPACE)?.slug ?? (config.space === SPACE ? config.slug : null);
  const [dataset, setDataset] = useState<Dataset>("hospitals");
  const tabs = useRef<Partial<Record<Dataset, HTMLButtonElement | null>>>({});

  const source = useMemo(
    () => (slug ? endpointSource(slug, transportFor(config), language) : null),
    // `config` is the document the Portal served once; the slug and the language are what change.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [slug, language],
  );
  const grid = useMemo(() => (slug ? gridConfig(slug, dataset, s.column) : null), [slug, dataset, s.column]);
  const enums = useMemo(
    () =>
      Object.fromEntries(
        Object.entries(ENUMS).map(([attr, values]) => [attr, values.map((value) => ({ value, title: s.values[value] }))]),
      ),
    [s.values],
  );

  // The tabs move with the arrow keys, Home and End, as a tablist does (WAI-ARIA APG).
  const onTabKey = (event: KeyboardEvent<HTMLButtonElement>) => {
    const at = DATASETS.indexOf(dataset);
    const next =
      event.key === "ArrowRight" ? DATASETS[(at + 1) % DATASETS.length]
      : event.key === "ArrowLeft" ? DATASETS[(at - 1 + DATASETS.length) % DATASETS.length]
      : event.key === "Home" ? DATASETS[0]
      : event.key === "End" ? DATASETS[DATASETS.length - 1]
      : null;
    if (next) {
      event.preventDefault();
      setDataset(next);
      tabs.current[next]?.focus();
    }
  };

  return (
    <Page>
      <p className="about">{s.subtitle}</p>
      {source && grid && slug ? (
        <>
          <div role="tablist" aria-label={s.datasets} className="tabs">
            {DATASETS.map((one) => (
              <button
                key={one}
                ref={(node) => {
                  tabs.current[one] = node;
                }}
                type="button"
                role="tab"
                id={`tab-${one}`}
                aria-selected={one === dataset}
                aria-controls={`panel-${one}`}
                tabIndex={one === dataset ? 0 : -1}
                onClick={() => setDataset(one)}
                onKeyDown={onTabKey}
              >
                {s.dataset[one]}
              </button>
            ))}
          </div>
          <section role="tabpanel" id={`panel-${dataset}`} aria-labelledby={`tab-${dataset}`} className="panel">
            <p className="about">{s.about[dataset]}</p>
            <div className="downloads">
              <span>{s.download}:</span>
              <a href={exportUrl(slug, dataset)} download>
                {s.csv}{" "}
                <span className="visually-hidden">{s.dataset[dataset]}</span>
              </a>
              <small>{s.downloadNote}</small>
            </div>
            <EntityGrid key={dataset} config={grid} source={source} labels={s.grid} enums={enums} />
          </section>
        </>
      ) : (
        <Empty>{s.noEndpoint}</Empty>
      )}
      <p className="source">{s.source}</p>
    </Page>
  );
}
