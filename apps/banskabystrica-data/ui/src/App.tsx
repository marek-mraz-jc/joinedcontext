/**
 * The city's public datasets as grids (T-2782, UI-64, UI-71, UI-84): events, schools and air
 * quality from `banskabystrica-verejne`, read through the app's public endpoint, so nobody signs
 * in and the bundle holds no token (AP-28).
 *
 * The table is the SDK's `EntityGrid`: paging, a filter per column that becomes the endpoint's own
 * `q`, the model's enum as a pick list, id, type and name first as on the Portal's Endpoint page
 * (T-6210). The page sits in the SDK's `AppShell` (SDK-39), and a row's pinned id opens the entity
 * in the SDK's panel (SDK-40): a public App, so the panel reads it and links
 * to it in the Portal (AP-140). What this application adds is one tab per dataset, the city's
 * labels and the endpoint's own downloads of each dataset.
 */
import { useMemo, useRef, useState } from "react";
import type { KeyboardEvent } from "react";
import { AppShell, endpointSource, EntityGrid, Page, transportFor, useClient } from "@joinedcontext/sdk";
import { DATASETS, EVENT_CATEGORIES, exportUrl, gridConfig } from "./datasets";
import type { Dataset } from "./datasets";
import { stringsFor } from "./locales";

/** The space this application reads (`Development/10` §2). */
const SPACE = "banskabystrica-verejne";

export default function App() {
  const { config } = useClient();
  const s = stringsFor(config.language);
  return <AppShell title={s.title} language={s.locale} pages={[{ id: "data", label: s.title, render: () => <Datasets /> }]} />;
}

function Datasets() {
  const { config } = useClient();
  const s = stringsFor(config.language);
  const language = config.language;
  const slug = config.endpoints?.find((candidate) => candidate.space === SPACE)?.slug ?? (config.space === SPACE ? config.slug : null);
  const [dataset, setDataset] = useState<Dataset>("events");
  const tabs = useRef<Record<Dataset, HTMLButtonElement | null>>({ events: null, schools: null, air: null });

  const source = useMemo(
    () => (slug ? endpointSource(slug, transportFor(config), language) : null),
    // `config` is the document the Portal served once; the slug and the language are what change.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [slug, language],
  );
  const grid = useMemo(() => (slug ? gridConfig(slug, dataset, s.column) : null), [slug, dataset, s.column]);
  const enums = useMemo(
    () => ({ eventCategory: EVENT_CATEGORIES.map((value) => ({ value, title: s.category[value] ?? value })) }),
    [s.category],
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
      <p className="subtitle">{s.subtitle}</p>
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
        <p role="status">{s.noEndpoint}</p>
      )}
      <p className="source">{s.source}</p>
    </Page>
  );
}
