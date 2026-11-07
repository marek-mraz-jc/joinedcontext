import React, { useRef, useState } from "react";
import { FilterChips } from "./AutoFilter";
import { TypeChoiceCell } from "./GridFilterRow";
import { GridPending } from "./GridReview";
import type { GridLabels } from "./useEntityGrid";
import type { GridView } from "./useGridView";

/** What stands above the rows: the last paste, Undo, the pending list, the folded filters, the chips and the hidden fields. */
export function GridNotices({ view }: { view: GridView }): React.JSX.Element {
  const { props, grid, labels, editing, columns, filterable, asText, where, pasteNote, lastApplied, undoNote, applying, undo, filtersOpen, setFiltersOpen, activeFilters, ownHidden, hiddenLabel, showColumns, unreadable } = view;
  const { pendingChanges, state, setFilter, rows } = grid;
  const { filterRow = true, onHide } = props;
  return (
    <>
      {pasteNote && (
        <p className="jc-grid-paste-note" role="status">
          {pasteNote}
        </p>
      )}

      {editing && (lastApplied || undoNote) && pendingChanges.length === 0 && (
        <div className="jc-grid-pending" role="status">
          {lastApplied ? (
            <>
              <span>{`${labels.saved} ${lastApplied.entities.reduce((sum, entity) => sum + entity.changes.length, 0)}`}</span>
              <button type="button" disabled={applying} onClick={() => void undo()}>
                {labels.undo}
              </button>
            </>
          ) : (
            <span>{undoNote}</span>
          )}
        </div>
      )}

      <GridPending view={view} />

      {filterRow && filterable.size > 0 && !asText && (
        <button type="button" className="jc-grid-filter-toggle" aria-expanded={filtersOpen} onClick={() => setFiltersOpen((open) => !open)}>
          {activeFilters > 0 ? `${labels.filterRow} (${activeFilters})` : labels.filterRow}
        </button>
      )}
      {!asText && <FilterChips columns={columns} filterable={filterable} filters={state.filters} labels={labels} locale={where} onRemove={(keys) => setFilter(keys, undefined)} />}
      {!onHide && ownHidden.length > 0 && <HiddenFields attrs={ownHidden} labels={labels} labelOf={hiddenLabel} onShow={showColumns} />}
      {unreadable.length > 0 && <p className="jc-grid-sort-note jc-grid-not-readable">{labels.notReadable.replace("{count}", String(unreadable.length))}</p>}
      {grid.sortedLoadedOnly && rows.length > 0 && (
        <p className="jc-grid-sort-note" role="status">{labels.sortedLoaded.replace("{count}", String(rows.length))}</p>
      )}
    </>
  );
}

/** The grid with nothing to show yet: no rows, still reading, or the read failed. */
export function GridStatus({ view }: { view: GridView }): React.JSX.Element {
  const { props, grid, labels, filterable } = view;
  const { rows, loading, error } = grid;
  const { typeChoice, empty: emptySlot } = props;
  return (
    <>
      {!loading && rows.length === 0 && !error && (
        <div className="jc-grid-empty">
          {/* A type with no rows and no model has no filter row: the way to another type stays. */}
          {typeChoice && filterable.size === 0 && <TypeChoiceCell choice={typeChoice} />}
          {emptySlot ?? labels.empty}
        </div>
      )}

      {loading && (
        <div className="jc-grid-loading" role="status">
          {rows.length > 0 && grid.narrowed ? labels.filtering : labels.loading}
        </div>
      )}

      {error && (
        <div className="jc-grid-error">
          {labels.error}: {error}
          {grid.errorDetail && (
            <details className="jc-grid-error-detail">
              <summary>{labels.errorDetail}</summary>
              <p>{grid.errorDetail}</p>
            </details>
          )}
        </div>
      )}
    </>
  );
}

/**
 * "Hidden fields (2)" above the rows (T-5620): the fields hidden from the header menu, each with
 * "Show field", and "Show all" for more than one. Focus stays on the count while fields remain.
 */
export function HiddenFields({ attrs, labels, labelOf, onShow }: { attrs: readonly string[]; labels: GridLabels; labelOf: (attr: string) => string; onShow: (attrs: readonly string[]) => void }) {
  const [open, setOpen] = useState(false);
  const listId = React.useId();
  const toggleRef = useRef<HTMLButtonElement>(null);
  const show = (which: readonly string[]) => {
    onShow(which);
    toggleRef.current?.focus();
  };
  return (
    <div className="jc-grid-hidden">
      <button ref={toggleRef} type="button" className="jc-grid-filter-toggle jc-grid-hidden-toggle" aria-expanded={open} aria-controls={listId} onClick={() => setOpen((was) => !was)}>
        {`${labels.hiddenFields} (${attrs.length})`}
      </button>
      {open && (
        <ul id={listId} className="jc-grid-hidden-list" aria-label={labels.hiddenFields}>
          {attrs.map((attr) => (
            <li key={attr}>
              <span>{labelOf(attr)}</span>
              <button type="button" aria-label={`${labels.showColumn}: ${labelOf(attr)}`} onClick={() => show([attr])}>
                {labels.showColumn}
              </button>
            </li>
          ))}
          {attrs.length > 1 && (
            <li>
              <button type="button" onClick={() => show(attrs)}>
                {labels.showAllColumns}
              </button>
            </li>
          )}
        </ul>
      )}
    </div>
  );
}
