/**
 * The space on the left, an endpoint on the right (T-1435; UI-69, EP-07, EP-61).
 *
 * An endpoint's effect on real data is otherwise read by comparing two JSON documents. Here the
 * left side is read first, and the right side is asked for exactly the ids the left side answered,
 * so the two pages are the same entities in the same order. What the right side leaves out is
 * marked on the left — a row it does not return, an attribute it does not carry — because that is
 * the side that has something to show. Nothing about a hidden value is inferred or drawn on the
 * right; both sides are read with the person's own session through their own door, and both are
 * view mode.
 */
import React, { useCallback, useEffect, useMemo, useState } from "react";
import { EntityGrid } from "./EntityGrid";
import { idChunks, SourceError } from "./source";
import type { EntitySource, GridQuery } from "./source";
import type { ResolvedGridConfig } from "./config";
import type { GridLabelsInput } from "./useEntityGrid";
import type { RichRow } from "./model";
import "./grid.css";

export interface CompareLabels {
  /** The heading of each side. */
  left: string;
  right: string;
  /** What is marked, as the person reads it on the row or the cell. */
  missingRow: string;
  hiddenColumn: string;
  /** One line above the two sides. */
  lead: string;
  same: string;
  loading: string;
  error: string;
  /** Said instead of the left side when the space answers this person nothing. */
  leftRefused: string;
  previous: string;
  next: string;
  page: string;
}

export const DEFAULT_COMPARE_LABELS: CompareLabels = {
  left: "The space",
  right: "This endpoint",
  missingRow: "not in this endpoint (filter or policy)",
  hiddenColumn: "hidden by the projection",
  lead: "The same page of entities, read through both doors.",
  same: "This endpoint answers this page exactly as the space holds it.",
  loading: "Reading…",
  error: "The comparison could not be read",
  leftRefused: "You can read this endpoint but not the space behind it, so there is nothing to compare it with.",
  previous: "Previous",
  next: "Next",
  page: "Page",
};

export interface EntityCompareProps {
  /** The grid's config: its type, page size and columns rule both sides. */
  config: ResolvedGridConfig;
  /** The space surface. */
  left: EntitySource;
  /** The endpoint. */
  right: EntitySource;
  labels?: Partial<CompareLabels>;
  /** The strings of the two grids themselves, from the host's own catalogue. */
  gridLabels?: GridLabelsInput;
}

/** A page already read, as a source: both sides render through the grid without reading again. */
export function pageSource(rows: RichRow[]): EntitySource {
  return {
    async query() {
      return { rows, total: rows.length };
    },
    async get(id: string) {
      return rows.find((row) => row.id === id) ?? null;
    },
  };
}

/** The attributes a page of rows carries at all. */
function attributesOf(rows: RichRow[]): Set<string> {
  const names = new Set<string>();
  for (const row of rows) {
    for (const attr of Object.keys(row.cells)) {
      names.add(attr);
    }
  }
  return names;
}

interface Both {
  left: RichRow[];
  right: RichRow[];
}

export function EntityCompare({
  config,
  left,
  right,
  labels: partial,
  gridLabels,
}: EntityCompareProps): React.JSX.Element {
  const labels = useMemo(() => ({ ...DEFAULT_COMPARE_LABELS, ...partial }), [partial]);
  const [offset, setOffset] = useState(0);
  const [both, setBoth] = useState<Both | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [refusedLeft, setRefusedLeft] = useState(false);

  const read = useCallback(async () => {
    setLoading(true);
    setError(null);
    setRefusedLeft(false);
    const query: GridQuery = {
      type: config.type,
      q: config.filters.preset?.q,
      attrs: config.filters.preset?.attrs,
      idPattern: config.filters.preset?.idPattern,
      scopeQ: config.filters.preset?.scopeQ,
    };
    try {
      const page = await left.query(query, { offset, limit: config.pageSize });
      // Exactly those ids, in as few reads as a query string may carry (T-1435).
      const answered: RichRow[] = [];
      for (const chunk of idChunks(page.rows.map((row) => row.id))) {
        const part = await right.query({ ...query, ids: chunk }, { offset: 0, limit: chunk.length });
        answered.push(...part.rows);
      }
      setBoth({ left: page.rows, right: answered });
    } catch (err) {
      // A space this person may not read is not an error of the comparison: the endpoint half is
      // still theirs to see, and the page says why the other half is missing (SP-06).
      if (err instanceof SourceError && (err.status === 403 || err.status === 404)) {
        setRefusedLeft(true);
        try {
          const page = await right.query(query, { offset, limit: config.pageSize });
          setBoth({ left: [], right: page.rows });
        } catch (second) {
          setError(second instanceof Error ? second.message : String(second));
        }
      } else {
        setError(err instanceof Error ? err.message : String(err));
      }
    } finally {
      setLoading(false);
    }
  }, [left, right, config.type, config.pageSize, config.filters.preset, offset]);

  useEffect(() => {
    void read();
  }, [read]);

  const marks = useMemo(() => {
    if (!both) {
      return { rows: {}, columns: {} };
    }
    const answered = new Set(both.right.map((row) => row.id));
    const rows: Record<string, string> = {};
    for (const row of both.left) {
      if (!answered.has(row.id)) {
        rows[row.id] = labels.missingRow;
      }
    }
    const here = attributesOf(both.left);
    const there = attributesOf(both.right);
    const columns: Record<string, string> = {};
    for (const attr of here) {
      if (!there.has(attr)) {
        columns[attr] = labels.hiddenColumn;
      }
    }
    return { rows, columns };
  }, [both, labels.missingRow, labels.hiddenColumn]);

  const same =
    both !== null &&
    !refusedLeft &&
    Object.keys(marks.rows).length === 0 &&
    Object.keys(marks.columns).length === 0;
  // Both sides render a page already in hand, so neither grid reads anything of its own.
  const view = useMemo(() => ({ ...config, mode: "view" as const }), [config]);

  return (
    <section className="jc-compare-wrap" aria-label={`${labels.left} / ${labels.right}`}>
      <p>{labels.lead}</p>
      {loading ? <p role="status">{labels.loading}</p> : null}
      {error ? <p className="jc-grid-error">{`${labels.error}: ${error}`}</p> : null}
      {same ? <p role="status">{labels.same}</p> : null}

      {both ? (
        <div className="jc-compare">
          <div className="jc-compare-side">
            <h3>{labels.left}</h3>
            {refusedLeft ? (
              <p>{labels.leftRefused}</p>
            ) : (
              <EntityGrid
                config={view}
                source={pageSource(both.left)}
                labels={gridLabels}
                marks={marks}
              />
            )}
          </div>
          <div className="jc-compare-side">
            <h3>{labels.right}</h3>
            <EntityGrid config={view} source={pageSource(both.right)} labels={gridLabels} />
          </div>
        </div>
      ) : null}

      {/* One pager for both sides: the left page decides which entities the right side is asked
          for, so paging them apart would compare two different pages. */}
      <div className="jc-grid-pager">
        <button type="button" disabled={offset === 0} onClick={() => setOffset(Math.max(0, offset - config.pageSize))}>
          {labels.previous}
        </button>
        <span className="jc-grid-page-info">{`${labels.page} ${Math.floor(offset / config.pageSize) + 1}`}</span>
        <button
          type="button"
          disabled={(both?.left.length ?? 0) < config.pageSize && (both?.right.length ?? 0) < config.pageSize}
          onClick={() => setOffset(offset + config.pageSize)}
        >
          {labels.next}
        </button>
      </div>
    </section>
  );
}
