import React, { useEffect, useMemo, useState } from "react";
import { areaQuery } from "./geoarea";
import type { GeoArea } from "./geoarea";
import { queryExample } from "./queryExample";
import type { GridView } from "./useGridView";

/** A long query wraps after its commas and ampersands, never inside a coordinate (T-6345). */
function breakable(text: string): React.ReactNode[] {
  return text.split(/(?<=[,&])/).flatMap((part, at) => (at === 0 ? [part] : [<wbr key={at} />, part]));
}

/** The query as the endpoint receives it, the drawn area's geo query included (T-6345). */
function queryText(asked: { q?: string; idPattern?: string }, area?: GeoArea | null): string {
  const parts: string[] = [];
  if (asked.q) {
    parts.push(`q=${asked.q}`);
  }
  if (asked.idPattern) {
    parts.push(`idPattern=${asked.idPattern}`);
  }
  const geo = areaQuery(area);
  if (geo) {
    parts.push(`georel=${geo.georel}&geometry=${geo.geometry}&coordinates=${geo.coordinates}&geoproperty=${geo.geoproperty}`);
  }
  return parts.join("&");
}

/** The query the filters ask, as the Endpoint receives it: copied, or taken over as text. */
export function GridQueryBox({ view }: { view: GridView }): React.JSX.Element | null {
  const { props, filterable, labels, grid, columns, asText } = view;
  const { queryBox = true } = props;
  const { state, rows, askedQuery, setFilterText } = grid;
  const queryHintId = React.useId();
  const example = useMemo(
    () => queryExample(columns.filter((c) => c.attr !== null && c.meta === null).map((c) => c.attr!), rows[0]),
    [columns, rows],
  );
  // "Copied" for two seconds after the query reached the clipboard.
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(false), 2000);
    return () => clearTimeout(timer);
  }, [copied]);
  if (!queryBox || filterable.size === 0) return null;
  return (
    <div className="jc-grid-query">
      <label className="jc-grid-query-text">
        <span className="jc-grid-query-label">{labels.query}</span>
        {asText ? (
          // The person owns the query from here: the row cannot show every `q` NGSI-LD allows
          // (a `|`, a bracket), so taking it over is how those are written at all.
          <input
            className="jc-grid-query-input"
            aria-describedby={queryHintId}
            value={state.filterText ?? ""}
            onChange={(e) => setFilterText(e.target.value)}
          />
        ) : (
          <output className="jc-grid-query-value">{breakable(queryText(askedQuery, grid.area) || labels.queryAll)}</output>
        )}
      </label>
      <span className="jc-grid-query-actions">
        {/* Nothing to copy while every row is asked for: the line says "all records" instead. */}
        {queryText(askedQuery, grid.area) !== "" && (
          <button
            type="button"
            className="jc-grid-query-copy"
            onClick={() => {
              void navigator.clipboard?.writeText(queryText(askedQuery, grid.area)).then(() => setCopied(true), () => setCopied(false));
            }}
          >
            {copied ? labels.copied : labels.copyQuery}
          </button>
        )}
        <label className="jc-grid-query-switch">
          <input
            type="checkbox"
            aria-describedby={queryHintId}
            checked={asText}
            onChange={(e) => setFilterText(e.target.checked ? (askedQuery.q ?? "") : null)}
          />
          <span>{labels.editAsText}</span>
        </label>
      </span>
      <span className="jc-grid-sr-only" role="status">{copied ? labels.copied : ""}</span>
      {/* The example names this type's own attribute and the first row's value (T-5415). */}
      <span id={queryHintId} className="jc-grid-query-hint">
        {labels.editAsTextHint}
        {example ? <>{`, ${labels.exampleLead} `}<code>{example}</code></> : null}.
      </span>
    </div>
  );
}
