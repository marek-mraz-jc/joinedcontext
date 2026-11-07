/**
 * A column's header menu (T-4827, T-6328): Sort A to Z, Z to A, Filter and Hide, each with its
 * icon, as Baserow and Excel keep it. A label of this view's own, the width and the place sit
 * folded under the column's settings with the attribute's metadata: dragging the header or its
 * edge moves and widens it, and these buttons are the keyboard's way to the same.
 */
import React, { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import type { GridLabels } from "./useEntityGrid";

export interface ColumnMenuProps {
  /** What the column is called now, without its unit: what Rename starts from. */
  label: string;
  labels: GridLabels;
  /** The direction this column orders the rows in, if it does. */
  sorted: "asc" | "desc" | null;
  onSort: (dir: "asc" | "desc") => void;
  /** Absent where the column has no filter (the endpoint cannot be asked about it). */
  onFilter?: () => void;
  /** Absent for the primary column: it names the row and is never hidden. */
  onHide?: () => void;
  /** A label of this view's own; `undefined` goes back to the model's. */
  onRename: (label: string | undefined) => void;
  renamed: boolean;
  onWiden: (delta: number) => void;
  onMove?: (delta: -1 | 1) => void;
  canMoveLeft: boolean;
  canMoveRight: boolean;
  /** History and the metadata columns, as the grid offered them before. */
  details?: React.ReactNode;
  onClose: () => void;
}

/**
 * A popup placed fixed at its header cell, inside the window, following it as the page scrolls.
 * The grid clips what overflows it, so under a short grid a menu lost everything below its first
 * items (T-4924). It stays under the host's sticky header, whose height the host gives as
 * --jc-grid-sticky-top (the menu's scroll-margin-top in grid.css, resolved to pixels; T-5181).
 */
export function usePlaceAtCell(): React.MutableRefObject<HTMLDivElement | null> {
  const box = useRef<HTMLDivElement | null>(null);
  useLayoutEffect(() => {
    const menu = box.current;
    const cell = menu?.parentElement;
    if (!menu || !cell) return;
    const place = () => {
      const at = cell.getBoundingClientRect();
      const width = menu.offsetWidth;
      const ceiling = parseFloat(getComputedStyle(menu).scrollMarginTop) || 0;
      const under = Math.max(at.bottom, ceiling);
      const below = window.innerHeight - under;
      const above = at.top - ceiling;
      // Above the cell when it does not fit below and there is more room above; it scrolls inside.
      const up = menu.scrollHeight > below && above > below;
      const room = up ? above : below;
      menu.style.maxHeight = `${room}px`;
      menu.style.top = `${up ? at.top - Math.min(menu.scrollHeight, room) : under}px`;
      // An 8 px gutter from both screen edges, as the page keeps (UI judge, T-6344). Never left of
      // the grid: a host's sidebar stands above the menu and would hide its first column's (T-6454).
      const gutter = 8;
      const edge = Math.max(gutter, cell.closest(".jc-grid")?.getBoundingClientRect().left ?? 0);
      menu.style.left = `${Math.max(edge, Math.min(at.right - width, window.innerWidth - width - gutter))}px`;
    };
    place();
    window.addEventListener("scroll", place, true);
    window.addEventListener("resize", place);
    // Opening Details makes it taller: placed again, it may go above the cell.
    menu.addEventListener("toggle", place, true);
    // A filter's value list arrives after it opened, and makes it taller too.
    const watch = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(place);
    watch?.observe(menu);
    return () => {
      watch?.disconnect();
      window.removeEventListener("scroll", place, true);
      window.removeEventListener("resize", place);
      menu.removeEventListener("toggle", place, true);
    };
  }, []);
  return box;
}

/** Pixels one Narrower or Wider press moves the edge. */
export const WIDTH_STEP = 40;

export const ColumnMenu = React.forwardRef<HTMLDivElement, ColumnMenuProps>(function ColumnMenu(
  { label, labels, sorted, onSort, onFilter, onHide, onRename, renamed, onWiden, onMove, canMoveLeft, canMoveRight, details, onClose },
  ref,
) {
  const id = useId();
  const [draft, setDraft] = useState(label);
  const first = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    first.current?.focus();
  }, []);
  const box = usePlaceAtCell();
  const setBox = (el: HTMLDivElement | null) => {
    box.current = el;
    if (typeof ref === "function") ref(el);
    else if (ref) ref.current = el;
  };
  const commit = () => {
    const text = draft.trim();
    if (text !== "" && text !== label) onRename(text);
  };
  return (
    <div
      className="jc-grid-meta-menu jc-grid-col-menu"
      ref={setBox}
      role="dialog"
      aria-label={`${labels.columnMenu}: ${label}`}
      onKeyDown={(e) => {
        // Inside the menu the grid's own arrow and Enter keys do not apply.
        e.stopPropagation();
        if (e.key === "Escape") onClose();
      }}
    >
      <button ref={first} type="button" className="jc-grid-meta-item" aria-pressed={sorted === "asc"} onClick={() => onSort("asc")}>
        <span className="jc-grid-menu-icon" aria-hidden="true">↑</span>
        {labels.sortAsc}
      </button>
      <button type="button" className="jc-grid-meta-item" aria-pressed={sorted === "desc"} onClick={() => onSort("desc")}>
        <span className="jc-grid-menu-icon" aria-hidden="true">↓</span>
        {labels.sortDesc}
      </button>
      {(onFilter || onHide) && <hr className="jc-grid-menu-sep" />}
      {onFilter && (
        <button type="button" className="jc-grid-meta-item" onClick={onFilter}>
          <span className="jc-grid-menu-icon jc-grid-funnel" aria-hidden="true" />
          {labels.filterColumn}
        </button>
      )}
      {onHide && (
        <button type="button" className="jc-grid-meta-item" onClick={onHide}>
          <span className="jc-grid-menu-icon" aria-hidden="true">⊘</span>
          {labels.hideColumn}
        </button>
      )}
      <details className="jc-grid-col-menu-details">
        <summary>
          <span className="jc-grid-menu-icon" aria-hidden="true">⚙</span>
          {labels.details}
        </summary>
        <label className="jc-grid-col-menu-field" htmlFor={`${id}-label`}>
          {labels.renameColumn}
        </label>
        <input
          id={`${id}-label`}
          className="jc-grid-col-menu-input"
          value={draft}
          maxLength={80}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={commit}
          onKeyDown={(e) => {
            if (e.key === "Enter") commit();
          }}
        />
        {renamed && (
          <button type="button" className="jc-grid-meta-item" onClick={() => onRename(undefined)}>
            {labels.renameReset}
          </button>
        )}
        <div className="jc-grid-col-menu-row" role="group" aria-label={labels.columnWidth}>
          <button type="button" className="jc-grid-meta-item" onClick={() => onWiden(-WIDTH_STEP)}>
            {labels.narrower}
          </button>
          <button type="button" className="jc-grid-meta-item" onClick={() => onWiden(WIDTH_STEP)}>
            {labels.wider}
          </button>
        </div>
        {onMove && (
          <div className="jc-grid-col-menu-row">
            <button type="button" className="jc-grid-meta-item" disabled={!canMoveLeft} onClick={() => onMove(-1)}>
              {labels.moveLeft}
            </button>
            <button type="button" className="jc-grid-meta-item" disabled={!canMoveRight} onClick={() => onMove(1)}>
              {labels.moveRight}
            </button>
          </div>
        )}
        {details}
      </details>
    </div>
  );
});
