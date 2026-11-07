import { useEffect, useRef, useState } from "react";
import { displayName, format, Header, useEntities, useFilters } from "@joinedcontext/sdk";
import type { FilterDef, Row } from "@joinedcontext/sdk";
import { EntityMap } from "./components/EntityMap";
import { SearchBox } from "./components/filters";
import { Loading, Problem } from "./components/states";

const TYPE = "BikeHireDockingStation";
const SEARCH: FilterDef[] = [{ kind: "search", attrs: ["name"], label: "Find a station" }];

/** A count a station reported, or a dash when it reported none: an unknown is not a zero. */
function count(value: unknown): string {
  return typeof value === "number" ? format(value, "number") : "—";
}

function status(row: Row): string {
  return row.status === "outOfService" ? "Out of service" : "Open";
}

/** One place, read out: what a resident came for first, then the rest. */
function Sheet({ row, onClose }: { row: Row; onClose: () => void }): React.JSX.Element {
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => heading.current?.focus(), [row.id]);
  return (
    <section
      className="app-sheet"
      aria-labelledby="app-sheet-title"
      onKeyDown={(event) => {
        if (event.key === "Escape") onClose();
      }}
    >
      <header className="app-sheet-header">
        <h2 id="app-sheet-title" ref={heading} tabIndex={-1}>
          {displayName(row)}
        </h2>
        <button type="button" onClick={onClose}>
          Close
        </button>
      </header>
      <dl className="app-facts">
        <div>
          <dt>Bikes now</dt>
          <dd className="app-fact-big">{count(row.availableBikeNumber)}</dd>
        </div>
        <div>
          <dt>Free docks</dt>
          <dd className="app-fact-big">{count(row.freeSlotNumber)}</dd>
        </div>
        <div>
          <dt>Station</dt>
          <dd>{status(row)}</dd>
        </div>
      </dl>
    </section>
  );
}

/** A full-screen map of the stations, a search over it, and the chosen one in a sheet. */
export default function App(): React.JSX.Element {
  const { rows, loading, error, reload } = useEntities(TYPE);
  const { shown, bind, values } = useFilters(rows, SEARCH);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const cameFrom = useRef<HTMLButtonElement | null>(null);
  const selected = rows.find((row) => row.id === selectedId) ?? null;
  const searching = typeof values[0] === "string" && values[0].trim() !== "";

  const close = () => {
    setSelectedId(null);
    cameFrom.current?.focus();
  };

  return (
    <div className="jc-shell app-map-app">
      <div className="app-bar">
        <Header level={1} title="City bikes near you" subtitle="Bikes and free docks at every station, now" />
        <div className="app-search">
          <SearchBox binding={bind(0)} placeholder="Station name" />
        </div>
      </div>
      <main className="app-map-screen">
        {error ? (
          <Problem error={error} onRetry={reload} />
        ) : loading && rows.length === 0 ? (
          <Loading label="Loading the stations…" />
        ) : (
          <>
            <EntityMap
              rows={shown}
              location="location"
              label="name"
              color="availableBikeNumber"
              selected={selectedId}
              onSelect={(row) => setSelectedId(row.id)}
            />
            {searching && (
              <nav className="app-results" aria-label="Stations found">
                {shown.length === 0 ? (
                  <p role="status">No station is called that. Try part of the name.</p>
                ) : (
                  <ul>
                    {shown.slice(0, 8).map((row) => (
                      <li key={row.id}>
                        <button
                          type="button"
                          aria-current={row.id === selectedId || undefined}
                          onClick={(event) => {
                            cameFrom.current = event.currentTarget;
                            setSelectedId(row.id);
                          }}
                        >
                          <span>{displayName(row)}</span>
                          <span className="app-result-count">{count(row.availableBikeNumber)} bikes</span>
                        </button>
                      </li>
                    ))}
                  </ul>
                )}
              </nav>
            )}
            {selected && <Sheet row={selected} onClose={close} />}
          </>
        )}
      </main>
    </div>
  );
}
