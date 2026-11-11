/**
 * A column's header menu (T-4827, T-6328, T-6738): the quick actions alone, Sort in the column's own
 * words ("Earliest first" for a date, "A to Z" for text), Filter, Hide and History, as Baserow and
 * Excel keep it. Width and order move by dragging the header and its edge; labels, order, hides and
 * the metadata columns live in the toolbar's Arrange columns panel. It is a menu: the arrow keys,
 * Home and End move between its items, Escape and Tab close it back to the header's button.
 */
import React, { useEffect, useLayoutEffect, useRef } from "react";
import type { ColumnKind } from "./cells";
import type { GridLabels } from "./useEntityGrid";

export interface ColumnMenuProps {
  /** What the column is called now: what the menu is named after. */
  label: string;
  /** What the column holds: the words its sort uses. */
  kind: ColumnKind;
  labels: GridLabels;
  /** The direction this column orders the rows in, if it does. */
  sorted: "asc" | "desc" | null;
  onSort: (dir: "asc" | "desc") => void;
  /** Absent where the column has no filter (the endpoint cannot be asked about it). */
  onFilter?: () => void;
  /** Absent for the primary column: it names the row and is never hidden. */
  onHide?: () => void;
  /** Absent where the source keeps no history. */
  onHistory?: () => void;
  onClose: () => void;
}

/** The two sort words a column of `kind` reads in. */
export function sortWords(kind: ColumnKind, labels: GridLabels): [asc: string, desc: string] {
  if (kind === "datetime") return [labels.sortAscDate, labels.sortDescDate];
  if (kind === "number") return [labels.sortAscNumber, labels.sortDescNumber];
  if (kind === "boolean") return [labels.sortAscBoolean, labels.sortDescBoolean];
  return [labels.sortAsc, labels.sortDesc];
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

/** Pixels one arrow key moves a header's edge. */
export const WIDTH_STEP = 40;

export const ColumnMenu = React.forwardRef<HTMLDivElement, ColumnMenuProps>(function ColumnMenu(
  { label, kind, labels, sorted, onSort, onFilter, onHide, onHistory, onClose },
  ref,
) {
  const box = usePlaceAtCell();
  const setBox = (el: HTMLDivElement | null) => {
    box.current = el;
    if (typeof ref === "function") ref(el);
    else if (ref) ref.current = el;
  };
  const items = () => [...(box.current?.querySelectorAll<HTMLElement>('[role^="menuitem"]') ?? [])];
  useEffect(() => {
    items()[0]?.focus();
  }, []);
  const [asc, desc] = sortWords(kind, labels);
  const item = (text: string, icon: React.ReactNode, onClick: () => void, checked?: boolean) => (
    <button
      type="button"
      className="jc-grid-meta-item"
      role={checked === undefined ? "menuitem" : "menuitemradio"}
      aria-checked={checked}
      tabIndex={-1}
      onClick={onClick}
    >
      <span className="jc-grid-menu-icon" aria-hidden="true">{icon}</span>
      {text}
    </button>
  );
  return (
    <div
      className="jc-grid-meta-menu jc-grid-col-menu"
      ref={setBox}
      role="menu"
      aria-label={`${labels.columnMenu}: ${label}`}
      onKeyDown={(e) => {
        // Inside the menu the grid's own arrow and Enter keys do not apply.
        e.stopPropagation();
        const all = items();
        const at = all.indexOf(document.activeElement as HTMLElement);
        const go = (to: number) => {
          e.preventDefault();
          all[(to + all.length) % all.length]?.focus();
        };
        if (e.key === "ArrowDown") go(at + 1);
        else if (e.key === "ArrowUp") go(at - 1);
        else if (e.key === "Home") go(0);
        else if (e.key === "End") go(all.length - 1);
        else if (e.key === "Escape" || e.key === "Tab") {
          e.preventDefault();
          onClose();
        }
      }}
    >
      {item(asc, "↑", () => onSort("asc"), sorted === "asc")}
      {item(desc, "↓", () => onSort("desc"), sorted === "desc")}
      {(onFilter || onHide || onHistory) && <hr className="jc-grid-menu-sep" role="separator" />}
      {onFilter && item(labels.filterColumn, <span className="jc-grid-funnel" />, onFilter)}
      {onHide && item(labels.hideColumn, "⊘", onHide)}
      {onHistory && item(labels.history, "◷", onHistory)}
    </div>
  );
});
