import React from "react";
import type { GridView } from "./useGridView";

export function pageNumbers(current: number, total: number): (number | null)[] {
  const kept = [...new Set([1, current - 1, current, current + 1, total])].filter((page) => page >= 1 && page <= total).sort((a, b) => a - b);
  return kept.flatMap((page, at) => (at > 0 && page - (kept[at - 1] ?? page) > 1 ? [null, page] : [page]));
}


/**
 * Where a grid says how much it holds. `status` is the scrolling grid's line (rows loaded of how
 * many); `paged` the numbered pager with its page sizes. Without `mode` the grid's own setting decides.
 */
export function GridPager({ view, mode }: { view: GridView; mode?: "status" | "paged" }): React.JSX.Element {
  const { grid, options, labels, props } = view;
  const { rows, state, setOffset } = grid;
  const { classNames, pageSizes, onPageSize } = props;
  const pageSize = options.config.pageSize;
  // Without a total the pager cannot know the last page: it shows only next and previous.
  const totalPages = grid.total !== undefined ? Math.max(1, Math.ceil(grid.total / pageSize)) : undefined;
  const currentPage = Math.floor(state.offset / pageSize) + 1;
  if ((mode ?? (options.virtual === true && !options.paged ? "status" : "paged")) === "status") {
    return (
      <div className={`jc-grid-pager${classNames?.pager ? ` ${classNames.pager}` : ""}`} role="status">
        {/* The total is the endpoint's own count, from the same answer as the rows (T-4885): an
            answer the gateway narrowed carries none (R22), and the footer says so in words. */}
        <span className="jc-grid-total">
          {grid.loading && rows.length === 0
            ? labels.loading
            : grid.total !== undefined
              ? `${rows.length} ${labels.of} ${grid.total} ${labels.loaded}`
              : `${rows.length} ${labels.loaded} · ${labels.noTotal}`}
        </span>
        {grid.loadingMore && <span className="jc-grid-loading-more">{labels.loading}</span>}
      </div>
    );
  }
  return (
    <div className={`jc-grid-pager${classNames?.pager ? ` ${classNames.pager}` : ""}`}>
      <button
        type="button"
        disabled={state.offset === 0}
        className="jc-grid-pager-step"
        onClick={() => setOffset(Math.max(0, state.offset - pageSize))}
      >
        {/* On a phone only the arrow shows; the word stays for a screen reader (T-5415). */}
        <span aria-hidden="true">‹</span> <span className="jc-grid-pager-word">{labels.previous}</span>
      </button>
      <span className="jc-grid-page-info">
        {`${labels.page} ${currentPage}`}
        {totalPages !== undefined && <span className="jc-grid-page-count">{` / ${totalPages}`}</span>}
      </span>
      {/* Numbered pages as a DataTables pager (T-5196): the first, the last and the two around this one. */}
      {totalPages !== undefined && totalPages > 1 && (
        <span className="jc-grid-pages">
          {pageNumbers(currentPage, totalPages).map((page, at) =>
            page === null ? (
              <span key={`gap-${at}`} aria-hidden="true">…</span>
            ) : (
              <button
                key={page}
                type="button"
                aria-label={`${labels.page} ${page}`}
                aria-current={page === currentPage ? "page" : undefined}
                onClick={() => setOffset((page - 1) * pageSize)}
              >
                {page}
              </button>
            ),
          )}
        </span>
      )}
      {/* The endpoint's own count, not the page's: an answer the gateway narrowed carries none,
          and then the footer says nothing rather than a number it guessed (R22). */}
      {grid.total !== undefined && (
        <span className="jc-grid-total">{labels.matching.includes("{n}") ? labels.matching.replace("{n}", String(grid.total)) : `${grid.total} ${labels.matching}`}</span>
      )}
      <button
        type="button"
        disabled={totalPages !== undefined ? currentPage >= totalPages : rows.length < pageSize}
        className="jc-grid-pager-step"
        onClick={() => setOffset(state.offset + pageSize)}
      >
        <span className="jc-grid-pager-word">{labels.next}</span> <span aria-hidden="true">›</span>
      </button>
      {pageSizes && onPageSize ? (
        <label className="jc-grid-page-size">
          {labels.rowsPerPage}
          <select
            value={pageSize}
            onChange={(e) => {
              onPageSize(Number(e.target.value));
              setOffset(0);
            }}
          >
            {pageSizes.map((size) => (
              <option key={size} value={size}>
                {size}
              </option>
            ))}
          </select>
        </label>
      ) : null}
    </div>
  );
}
