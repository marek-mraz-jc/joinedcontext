import { useEffect, useMemo, useRef, useState } from "react";
import type { Cell, FilterState, RelationshipObject, Row } from "./ngsi";
import { isRelationshipObject } from "./ngsi";
import { applyFilters, columnKind, distinct, extent, format, loadSource, toRow } from "./ngsi";
import type { Filter, Source, Spec, View } from "./spec";
import { DEFAULT_LIMIT, MAX_LIMIT, pagesOf, sourceOf } from "./spec";
import { download, toCsv, toGeoJson, toPdf, toPng } from "./artifact";
import { Chart } from "./views/Chart";
import { Detail } from "./views/Detail";
import { Form } from "./views/Form";
import { GridView } from "./views/GridView";
import { MapView } from "./views/MapView";
import { useTileTemplate } from "./geo/tiles";
import { Stats } from "./views/Stats";
import { Table } from "./views/Table";
import type { Schema, WriteResult } from "./write";
import { attrsOf, unitsOf, writeEntity } from "./write";
import { fixtureSource, sourceFor } from "./grid/source";
import { searchTargets } from "./relations";
import { originTransport } from "./sdk/transport";
import type { TargetSearch } from "./views/Form";

const DEFAULT_ACCENT = "#0f766e";

type Loaded = Record<string, Row[]>;

/** Every source read once; `error` names the first that failed. */
/** Entities the Portal read for the preview and inlined, keyed by source name. */
export type Inline = Record<string, unknown[]>;

/**
 * Rows per source: from the document when the Portal inlined them, from the endpoint
 * otherwise. The preview frame is sandboxed without an origin, so it can fetch nothing with a
 * session; the published app is same-origin and reads live.
 */
export function useSources(slug: string, spec: Spec, inline?: Inline): { data: Loaded; loading: boolean; error: string | null } {
  const [data, setData] = useState<Loaded>({});
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    const load = async (source: Source): Promise<Row[]> => {
      const given = inline?.[source.name];
      if (Array.isArray(given)) {
        return given.filter((e): e is Record<string, unknown> => typeof e === "object" && e !== null).map((e) => toRow(e));
      }
      return loadSource(slug, source);
    };
    Promise.all(spec.sources.map(async (source) => [source.name, await load(source)] as const))
      .then((pairs) => {
        if (!cancelled) setData(Object.fromEntries(pairs));
      })
      .catch((err: unknown) => {
        if (!cancelled) setError(err instanceof Error ? err.message : String(err));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [slug, spec, inline]);
  return { data, loading, error };
}

function FilterControl({ filter, index, rows, value, onChange }: { filter: Filter; index: number; rows: Row[]; value: FilterState[number]; onChange: (value: FilterState[number]) => void }) {
  const id = `filter-${index}`;
  if (filter.kind === "search") {
    return (
      <label className="filter" htmlFor={id}>
        <span>{filter.label ?? "Search"}</span>
        <input id={id} type="search" value={typeof value === "string" ? value : ""} placeholder={filter.attrs.join(", ")} onChange={(e) => onChange(e.target.value)} />
      </label>
    );
  }
  if (filter.kind === "select") {
    return (
      <label className="filter" htmlFor={id}>
        <span>{filter.label ?? filter.attr}</span>
        <select id={id} value={typeof value === "string" ? value : ""} onChange={(e) => onChange(e.target.value)}>
          <option value="">All</option>
          {distinct(rows, filter.attr).map((option) => (
            <option key={option} value={option}>{option}</option>
          ))}
        </select>
      </label>
    );
  }
  const range = extent(rows, filter.attr);
  if (!range || columnKind(rows, filter.attr) !== "number") {
    return null;
  }
  const current = Array.isArray(value) ? value : range;
  return (
    <div className="filter range" role="group" aria-label={filter.label ?? filter.attr}>
      <span>{filter.label ?? filter.attr}: {current[0]} – {current[1]}</span>
      <input type="range" min={range[0]} max={range[1]} step="any" value={current[0]} aria-label={`${filter.label ?? filter.attr} from`} onChange={(e) => onChange([Math.min(Number(e.target.value), current[1]), current[1]])} />
      <input type="range" min={range[0]} max={range[1]} step="any" value={current[1]} aria-label={`${filter.label ?? filter.attr} to`} onChange={(e) => onChange([current[0], Math.max(Number(e.target.value), current[0])])} />
    </div>
  );
}

type Save = (source: Source, id: string | null, patch: Record<string, Cell | RelationshipObject>) => Promise<WriteResult>;

/** A written value as the rows on screen hold it: a relationship end is its URN, or URNs joined. */
function asCell(value: Cell | RelationshipObject): Cell {
  return isRelationshipObject(value) ? (Array.isArray(value.object) ? value.object.join(", ") : value.object) : value;
}

function summarizeFilters(filters: Filter[], state: FilterState): string {
  const parts: string[] = [];
  filters.forEach((filter, index) => {
    const val = state[index];
    if (val !== undefined && val !== "") {
      const label = filter.label ?? ("attr" in filter ? filter.attr : filter.attrs.join(", "));
      if (Array.isArray(val)) {
        parts.push(`${label}: ${val[0]} – ${val[1]}`);
      } else {
        parts.push(`${label}: ${val}`);
      }
    }
  });
  return parts.join(", ");
}

async function svgToPng(svg: SVGSVGElement): Promise<Blob> {
  const xml = new XMLSerializer().serializeToString(svg);
  const svgBlob = new Blob([xml], { type: "image/svg+xml;charset=utf-8" });
  const url = URL.createObjectURL(svgBlob);
  const img = new Image();
  return new Promise((resolve, reject) => {
    img.onload = () => {
      const canvas = document.createElement("canvas");
      const bbox = svg.getBoundingClientRect();
      canvas.width = bbox.width > 0 ? bbox.width : 640;
      canvas.height = bbox.height > 0 ? bbox.height : 280;
      const ctx = canvas.getContext("2d");
      if (!ctx) {
        URL.revokeObjectURL(url);
        reject(new Error("no 2d context"));
        return;
      }
      ctx.drawImage(img, 0, 0);
      URL.revokeObjectURL(url);
      toPng(canvas).then(resolve, reject);
    };
    img.onerror = () => {
      URL.revokeObjectURL(url);
      reject(new Error("failed to load svg"));
    };
    img.src = url;
  });
}

function ViewCard({
  view,
  spec,
  rows,
  accent,
  selected,
  onSelect,
  onSave,
  schema,
  creating,
  onCreate,
  slug,
  basemap,
  filters,
  filterState,
  inlineRows,
  search,
}: {
  view: View;
  spec: Spec;
  rows: Row[];
  accent: string;
  selected: string | null;
  onSelect: (id: string | null) => void;
  onSave: Save;
  schema?: Schema;
  creating: boolean;
  onCreate: (on: boolean) => void;
  slug: string;
  basemap?: string;
  filters: Filter[];
  filterState: FilterState;
  /** The entities the Portal inlined for a preview, for the one view that would otherwise fetch. */
  inlineRows?: Record<string, unknown>[];
  /** Where a form's relationship picker finds its targets. */
  search?: TargetSearch;
}) {
  const cardRef = useRef<HTMLElement>(null);
  const source = sourceOf(spec, view);
  const units = unitsOf(schema?.[source.type]);
  // A map whose source read its whole limit holds more than it drew: past that, and with nothing
  // narrowing it (a q, a filter on screen, a preview's inlined rows), it also draws the Endpoint's
  // tiles of the type (T-5195).
  const limit = Math.min(source.limit ?? DEFAULT_LIMIT, MAX_LIMIT);
  const narrowed = Boolean(source.q) || filters.some((filter, index) => (filter.source ?? source.name) === source.name && filterState[index] !== undefined && filterState[index] !== "");
  const tileTemplate = useTileTemplate(
    `/api/endpoint/${encodeURIComponent(slug)}`,
    source.type,
    view.kind === "map" && (view.location ?? "location") === "location" && !inlineRows && !narrowed && rows.length >= limit,
  );
  const body = (() => {
    switch (view.kind) {
      case "stats":
        return <Stats rows={rows} items={view.items} units={units} />;
      case "map":
        return (
          <MapView
            rows={rows}
            location={view.location ?? "location"}
            label={view.label}
            color={view.color}
            accent={accent}
            selected={selected}
            onSelect={onSelect}
            basemap={basemap}
            tiles={tileTemplate ? { template: tileTemplate, layer: source.type } : null}
          />
        );
      case "table":
        return <Table rows={rows} columns={view.columns} sort={view.sort} selected={selected} onSelect={onSelect} units={units} />;
      case "chart":
        return <Chart rows={rows} x={view.x} y={view.y} agg={view.agg} top={view.top} type={view.type} accent={accent} />;
      case "detail":
        return <Detail row={rows.find((r) => r.id === selected) ?? null} attrs={source.attrs} units={units} />;
      case "grid":
        // The grid reads the endpoint for itself, so it takes the slug and the type rather than
        // the rows this app loaded; in a preview it renders the entities the Portal inlined.
        return <GridView slug={slug} type={source.type} config={view.grid} inline={inlineRows} schema={schema} />;
      case "form":
        return (
          <>
            <Form row={rows.find((r) => r.id === selected) ?? null} rows={rows} fields={view.fields ?? source.attrs} title={view.title} schema={schema?.[source.type]} defs={schema} creating={creating} search={search} onSave={(id, patch) => onSave(source, id, patch)} onClose={() => { onSelect(null); onCreate(false); }} />
            <button type="button" className="new" onClick={() => { onSelect(null); onCreate(true); }}>New {source.type}</button>
          </>
        );
      default:
        return null;
    }
  })();

  const exportControls = (() => {
    if (rows.length === 0) return null;
    const filterDesc = summarizeFilters(filters, filterState);
    if (view.kind === "table") {
      return (
        <div className="export-bar">
          <span className="export-label">Export:</span>
          <button
            type="button"
            className="export-btn"
            onClick={() => {
              const blob = toCsv(view.columns, rows);
              download(blob, `${slug}-${view.title ?? "table"}.csv`);
            }}
          >
            CSV
          </button>
          <button
            type="button"
            className="export-btn"
            onClick={() => {
              const lines = [
                view.columns.join(" | "),
                ...rows.map((r) => view.columns.map((c) => format(r[c])).join(" | ")),
              ];
              const blob = toPdf({
                title: view.title ?? spec.title,
                endpoint: slug,
                filters: filterDesc,
                takenAt: new Date().toISOString(),
                attribution: basemap ? "Basemap: Platform Basemap" : undefined,
                lines,
              });
              download(blob, `${slug}-${view.title ?? "table"}.pdf`);
            }}
          >
            PDF
          </button>
        </div>
      );
    }
    if (view.kind === "map") {
      return (
        <div className="export-bar">
          <span className="export-label">Export:</span>
          <button
            type="button"
            className="export-btn"
            onClick={() => {
              const geojson = toGeoJson(rows, view.location ?? "location");
              const blob = new Blob([JSON.stringify(geojson, null, 2)], {
                type: "application/geo+json",
              });
              download(blob, `${slug}-${view.title ?? "map"}.geojson`);
            }}
          >
            GeoJSON
          </button>
          <button
            type="button"
            className="export-btn"
            onClick={() => {
              const canvas = cardRef.current?.querySelector("canvas");
              if (canvas) {
                void toPng(canvas)
                  .then((blob) => {
                    download(blob, `${slug}-${view.title ?? "map"}.png`);
                  })
                  .catch((err) => console.error("Export PNG failed", err));
              }
            }}
          >
            PNG
          </button>
        </div>
      );
    }
    if (view.kind === "chart") {
      return (
        <div className="export-bar">
          <span className="export-label">Export:</span>
          <button
            type="button"
            className="export-btn"
            onClick={() => {
              const svg = cardRef.current?.querySelector("svg");
              if (svg) {
                void svgToPng(svg)
                  .then((blob) => {
                    download(blob, `${slug}-${view.title ?? "chart"}.png`);
                  })
                  .catch((err) => console.error("Export PNG failed", err));
              }
            }}
          >
            PNG
          </button>
        </div>
      );
    }
    return null;
  })();

  return (
    <section ref={cardRef} className={`card card-${view.kind}`} aria-label={view.title ?? view.kind}>
      <div className="card-header">
        {view.title ? <h2>{view.title}</h2> : <div />}
        {exportControls}
      </div>
      {body}
    </section>
  );
}

export function App({
  slug,
  spec,
  inline,
  schema,
  bridge = false,
  basemap,
}: {
  slug: string;
  spec: Spec;
  inline?: Inline;
  schema?: Schema;
  bridge?: boolean;
  basemap?: string;
}) {
  const { data, loading, error } = useSources(slug, spec, inline);
  const [state, setState] = useState<FilterState>({});
  const [selected, setSelected] = useState<string | null>(null);
  // What a form saved, by entity id, laid over the rows read: on screen only.
  const [edits, setEdits] = useState<Record<string, Record<string, Cell>>>({});
  // What a form created, by source name, after the endpoint accepted it.
  const [created, setCreated] = useState<Record<string, Row[]>>({});
  const [creating, setCreating] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const pages = pagesOf(spec);
  const [page, setPage] = useState<string>(pages[0] ?? "");
  const accent = spec.theme?.accent ?? DEFAULT_ACCENT;
  const filters = spec.filters ?? [];
  const edited = useMemo(
    () => Object.fromEntries(spec.sources.map((s) => [s.name, [...(data[s.name] ?? []), ...(created[s.name] ?? [])].map((row) => (edits[row.id] ? { ...row, ...edits[row.id] } : row))])) as Loaded,
    [data, edits, created, spec.sources],
  );
  const filtered = useMemo(
    () => Object.fromEntries(spec.sources.map((s) => [s.name, applyFilters(edited[s.name] ?? [], filters, state, s.name)])) as Loaded,
    [edited, filters, state, spec.sources],
  );
  // A form's relationship end searches the app's own endpoint with the person's session, which
  // offers only what they may read; a preview has no session and searches what the Portal inlined.
  const targetSearch = useMemo<TargetSearch>(() => {
    const source = inline
      ? fixtureSource(
          Object.values(inline)
            .filter(Array.isArray)
            .flat()
            .filter((e): e is Record<string, unknown> => typeof e === "object" && e !== null && !Array.isArray(e)),
        )
      : sourceFor({ kind: "endpoint", slug }, originTransport());
    return (target, text) => searchTargets(source, target, text);
  }, [inline, slug]);
  // A save is one write through the endpoint (AP-62); the screen follows only what it accepted.
  const save: Save = async (source, id, patch) => {
    const result = id && !creating
      ? await writeEntity(slug, { id, type: source.type, patch }, bridge)
      : await writeEntity(slug, { type: source.type, entity: { id: id ?? "", ...attrsOf(patch) } }, bridge);
    if (result.ok) {
      const cells = Object.fromEntries(Object.entries(patch).map(([k, v]) => [k, asCell(v)]));
      if (id && !creating) {
        setEdits((e) => ({ ...e, [id]: { ...e[id], ...cells } }));
      } else if (id) {
        setCreated((c) => ({ ...c, [source.name]: [...(c[source.name] ?? []), { id, type: source.type, ...cells }] }));
      }
      setNotice(`Saved ${id ?? ""}`);
    }
    return result;
  };
  const shown = spec.views.filter((view) => view.page === undefined || view.page === page);
  const total = spec.sources.reduce((n, s) => n + (edited[s.name]?.length ?? 0), 0);
  const kept = spec.sources.reduce((n, s) => n + (filtered[s.name]?.length ?? 0), 0);

  return (
    <div className="app" style={{ ["--accent" as string]: accent }}>
      <header>
        <div>
          <h1>{spec.title}</h1>
          {spec.subtitle && <p className="subtitle">{spec.subtitle}</p>}
        </div>
        <span className="status" role="status">
          {loading ? "Loading…" : error ? error : kept === total ? `${total} entities` : `${kept} of ${total} entities`}
        </span>
      </header>
      {filters.length > 0 && (
        <div className="filters">
          {filters.map((filter, index) => (
            <FilterControl key={index} filter={filter} index={index} rows={data[sourceOf(spec, filter).name] ?? []} value={state[index]} onChange={(value) => setState((s) => ({ ...s, [index]: value }))} />
          ))}
        </div>
      )}
      {error && <p role="alert" className="error">{error}</p>}
      {notice && <p className="notice" aria-live="polite">{notice}</p>}
      {pages.length > 0 && (
        <nav className="pages" aria-label="Pages">
          {pages.map((name) => (
            <button key={name} type="button" aria-current={name === page ? "page" : undefined} onClick={() => setPage(name)}>{name}</button>
          ))}
        </nav>
      )}
      <main>
        {shown.map((view) => (
          <ViewCard
            key={spec.views.indexOf(view)}
            view={view}
            spec={spec}
            rows={filtered[sourceOf(spec, view).name] ?? []}
            accent={accent}
            selected={selected}
            onSelect={setSelected}
            onSave={save}
            schema={schema}
            creating={creating}
            onCreate={setCreating}
            slug={slug}
            basemap={basemap}
            filters={filters}
            filterState={state}
            inlineRows={inline?.[sourceOf(spec, view).name] as Record<string, unknown>[] | undefined}
            search={targetSearch}
          />
        ))}
      </main>
    </div>
  );
}
