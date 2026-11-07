import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useHeaderFilter } from "./AutoFilter";
import { columnKind } from "./cells";
import type { ColumnKind } from "./cells";
import { columnValues } from "./columnValues";
import type { FilterColumn } from "./filters";
import { readableName } from "./readableName";
import type { ValueRule } from "./rules";
import { MAX_COLUMN_WIDTH, MIN_COLUMN_WIDTH, NAME_COLUMN } from "./useEntityGrid";
import type { ColumnLayout, EntityGrid as EntityGridApi, MetaKey, UseEntityGridOptions, VisibleColumn } from "./useEntityGrid";

/** What a column's header does: its menu, order, width, label and kind, and the person's own hides. */
export function useColumnControls(
  options: UseEntityGridOptions,
  grid: EntityGridApi,
  {
    tableRef,
    layout,
    setLayout,
    rules,
    onHide,
    ownHidden,
    setOwnHidden,
  }: {
    tableRef: React.RefObject<HTMLTableElement | null>;
    layout: ColumnLayout;
    setLayout: (next: ColumnLayout) => void;
    rules?: Record<string, ValueRule>;
    onHide?: (attr: string) => void;
    ownHidden: string[];
    setOwnHidden: React.Dispatch<React.SetStateAction<string[]>>;
  },
) {
  const { rows, columns, toggleMeta } = grid;
  const relations = options.relations;
  const [openMenu, setOpenMenu] = useState<string | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  // Metadata menu toggle
  const toggleMenu = useCallback((attr: string) => {
    setOpenMenu((prev) => (prev === attr ? null : attr));
  }, []);

  const handleMetaCheck = useCallback((attr: string, meta: MetaKey) => {
    toggleMeta(attr, meta);
  }, [toggleMeta]);

  /** The attributes in the order the header shows them: what Move and a drop rearrange. */
  const order = useMemo(() => columns.filter((c) => c.attr !== null && c.meta === null).map((c) => c.attr!), [columns]);
  // The name stays first among the attributes (T-6210): nothing moves in front of it.
  const firstMovable = order[0] === NAME_COLUMN ? 1 : 0;
  const [dragOver, setDragOver] = useState<string | null>(null);
  const formatOf = useMemo(() => new Map(options.config.columns.map((column) => [column.attr, column.format])), [options.config.columns]);
  const kindOf = useMemo(() => {
    const kinds = new Map<string, ColumnKind>();
    const sample = rows.slice(0, 50);
    for (const col of columns) {
      if (col.meta === "observedAt" || col.meta === "createdAt" || col.meta === "modifiedAt" || col.key === "entity:createdAt" || col.key === "entity:modifiedAt") kinds.set(col.key, "datetime");
      else if (col.attr === null || col.meta !== null) kinds.set(col.key, "text");
      else kinds.set(col.key, columnKind(sample.map((row) => row.cells[col.attr!]), rules?.[col.attr]?.kind, Boolean(relations?.[col.attr])));
    }
    return kinds;
  }, [columns, rows, rules, relations]);
  /** A column's label without the unit the grid adds: what Rename starts from. */
  const baseLabel = (col: VisibleColumn): string => col.label.replace(/ \([^()]*\)$/, "");
  const closeMenu = useCallback(() => {
    const attr = openMenu;
    setOpenMenu(null);
    // Back to the button that opened the menu, so a keyboard carries on where it was.
    if (attr) tableRef.current?.querySelector<HTMLElement>(`th[data-col-attr="${attr.replace(/["\\]/g, "\\$&")}"] .jc-grid-meta-btn`)?.focus();
  }, [openMenu]);
  // A column's header filter (T-6328): one open at a time, under its header, like the menu.
  const { openFilter, open: openHeaderFilter, close: closeFilter } = useHeaderFilter(tableRef);
  const focusFilter = useCallback((key: string) => {
    setOpenMenu(null);
    openHeaderFilter(key);
  }, [openHeaderFilter]);
  const { source: gridSource } = options;
  const { valuesQuery } = grid;
  const loadValues = useCallback(
    (column: FilterColumn) => () => columnValues(gridSource, valuesQuery(column.key), column.attr ?? ""),
    [gridSource, valuesQuery],
  );
  const hideColumn = useCallback((attr: string) => {
    setOpenMenu(null);
    if (onHide) onHide(attr);
    else setOwnHidden((before) => [...before, attr]);
  }, [onHide]);
  // The way back from "Hide field" where the grid holds the hide itself (T-5620); a host that holds
  // `hidden` offers its own, and the host's own `hidden` (an Endpoint's projection) is not the person's.
  const [shownAgain, setShownAgain] = useState<string | null>(null);
  const showColumns = (attrs: readonly string[]) => {
    const left = ownHidden.filter((attr) => !attrs.includes(attr));
    setOwnHidden(left);
    // With none left the control goes: focus moves to the restored column's menu button once drawn.
    if (left.length === 0) setShownAgain(attrs[0] ?? null);
  };
  useEffect(() => {
    if (shownAgain === null) return;
    setShownAgain(null);
    tableRef.current?.querySelector<HTMLElement>(`th[data-col-attr="${shownAgain.replace(/["\\]/g, "\\$&")}"] .jc-grid-meta-btn`)?.focus();
  }, [shownAgain]);
  const hiddenLabel = (attr: string): string =>
    layout.labels?.[attr] ?? options.config.columns.find((column) => column.attr === attr)?.label ?? readableName(attr);
  const renameColumn = useCallback((attr: string, text: string | undefined) => {
    const { [attr]: _gone, ...rest } = layout.labels ?? {};
    setLayout({ ...layout, labels: text === undefined ? rest : { ...rest, [attr]: text } });
  }, [layout, setLayout]);
  const widthOf = (attr: string, fallback?: number): number =>
    fallback ?? tableRef.current?.querySelector<HTMLElement>(`th[data-col-attr="${attr.replace(/["\\]/g, "\\$&")}"]`)?.offsetWidth ?? 0;
  const widen = (attr: string, current: number | undefined, delta: number) => {
    const from = widthOf(attr, current) || 160;
    const next = Math.max(MIN_COLUMN_WIDTH, Math.min(MAX_COLUMN_WIDTH, Math.round(from + delta)));
    setLayout({ ...layout, width: { ...(layout.width ?? {}), [attr]: next } });
  };
  /** Dragging a header's edge: the column follows the pointer and keeps its width when let go. */
  const startResize = (e: React.PointerEvent<HTMLElement>, attr: string) => {
    e.preventDefault();
    e.stopPropagation();
    const th = e.currentTarget.closest("th");
    if (!th) return;
    const startX = e.clientX;
    const startWidth = th.offsetWidth;
    let width = startWidth;
    const move = (ev: PointerEvent) => {
      width = Math.max(MIN_COLUMN_WIDTH, Math.min(MAX_COLUMN_WIDTH, Math.round(startWidth + ev.clientX - startX)));
      th.style.width = th.style.minWidth = th.style.maxWidth = `${width}px`;
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      if (width !== startWidth) setLayout({ ...layout, width: { ...(layout.width ?? {}), [attr]: width } });
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  // Close menu on outside click
  React.useEffect(() => {
    if (!openMenu) return;
    const handler = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        setOpenMenu(null);
      }
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [openMenu]);
  return { openMenu, setOpenMenu, menuRef, toggleMenu, handleMetaCheck, order, firstMovable, dragOver, setDragOver, formatOf, kindOf, baseLabel, closeMenu, openFilter, closeFilter, focusFilter, loadValues, hideColumn, showColumns, hiddenLabel, renameColumn, widen, startResize };
}
