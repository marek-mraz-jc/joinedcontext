import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ResolvedGridConfig } from "./config";
import { complete } from "./filters";
import { CellView } from "./cells";
import type { CellLabels } from "./cells";
import { DEFAULT_INSPECTOR_LABELS } from "./CellInspector";
import type { EntityGridProps } from "./EntityGrid";
import { useChangesFeed } from "./EntityChanges";
import type { FilterColumn } from "./filters";
import { useGridDetail } from "./GridDetail";
import { useGridMap } from "./GridMapPanel";
import { cellText } from "./model";
import type { RichRow } from "./model";
import { useEntitySelection } from "../sdk/panel";
import { useCellContent } from "./useCellContent";
import { useColumnControls } from "./useColumnControls";
import { useEntityGrid } from "./useEntityGrid";
import type { ColumnLayout, VisibleColumn } from "./useEntityGrid";
import { lockReason, useGridAccess } from "./useGridAccess";
import { useGridEdits } from "./useGridEdits";
import { useGridKeys } from "./useGridKeys";
import { useGridTargets } from "./useGridTargets";
import { useRowWindow } from "./useRowWindow";

const EMPTY_IDS: readonly string[] = [];

function withoutAttrs(config: ResolvedGridConfig, gone: readonly string[]): ResolvedGridConfig {
  if (gone.length === 0) return config;
  const preset = config.filters.preset;
  return {
    ...config,
    columns: config.columns.filter((column) => !gone.includes(column.attr)),
    filters: preset?.attrs ? { ...config.filters, preset: { ...preset, attrs: preset.attrs.filter((attr) => !gone.includes(attr)) } } : config.filters,
  };
}

/** Everything a grid's parts share, for a host composing them itself: pass it to each part as `view`. */
export type GridView = ReturnType<typeof useGridView>;

/**
 * The grid without markup above `useEntityGrid`: access, edits, targets, the keyboard, the map and
 * the open row, one object every part (`GridTable`, `GridPager`, `GridMapPanel`, ...) draws from.
 * The options are `EntityGrid`'s props, so a host that composes the parts asks exactly what it asked.
 */
export function useGridView(props: EntityGridProps & { onDrill?: (urn: string, type: string) => void }) {
  const {
    renderers,
    onOpenRelationship,
    onDrill,
    termHref,
    onRows,
    onQuery,
    marks,
    toolbar,
    detailExtra,
    detailTabs,
    openRow,
    onOpenRow,
    detailPlace,
    onDetailPlace,
    mapEngine,
    mapAllowed,
    mapLabels,
    geoLabels,
    basemap,
    mapBounds,
    empty: emptySlot,
    onSignIn,
    rules,
    rowTone,
    reloadKey,
    className,
    classNames,
    locale,
    onLayout,
    onHide,
    filterRow = true,
    queryBox = true,
    onFilterColumns,
    rowNumbers,
    typeChoice,
    pageSizes,
    onPageSize,
    selected = EMPTY_IDS,
    onSelectedChange,
    rowSize,
    footer,
    groupHeader,
    access: hostAccess,
    explainAccess,
    accessRequestHref,
    ...options
  } = props;

  // The header menu's order, widths and labels: the host's when it keeps them, else the grid's own.
  const [ownLayout, setOwnLayout] = useState<ColumnLayout>(() => options.layout ?? {});
  const [ownHidden, setOwnHidden] = useState<string[]>([]);
  const layout = onLayout ? (options.layout ?? {}) : ownLayout;
  const setLayout = useCallback((next: ColumnLayout) => (onLayout ? onLayout(next) : setOwnLayout(next)), [onLayout]);
  const hostHidden = options.hidden;
  const hiddenNow = useMemo(
    () => (onHide || ownHidden.length === 0 ? hostHidden : [...(hostHidden ?? []), ...ownHidden]),
    [onHide, hostHidden, ownHidden],
  );
  const accessType = options.config.type;
  const askAccess = options.source.access;
  const { accessRows, unreadable, ownFailed } = useGridAccess(hostAccess, askAccess, accessType);
  const gridHidden = useMemo(
    () => (unreadable.length === 0 ? hiddenNow : [...(hiddenNow ?? []), ...unreadable]),
    [hiddenNow, unreadable],
  );
  const gridConfig = useMemo(() => withoutAttrs(options.config, unreadable), [options.config, unreadable]);
  const gridQuery = useMemo(
    () => (options.query?.attrs ? { ...options.query, attrs: options.query.attrs.filter((attr) => !unreadable.includes(attr)) } : options.query),
    [options.query, unreadable],
  );
  const grid = useEntityGrid({ ...options, config: gridConfig, query: gridQuery, layout, hidden: gridHidden });
  const reloadGrid = grid.reload;
  const reloadedFor = useRef(reloadKey);
  useEffect(() => {
    if (reloadedFor.current === reloadKey) return;
    reloadedFor.current = reloadKey;
    reloadGrid();
  }, [reloadKey, reloadGrid]);
  const where = locale ?? (typeof navigator !== "undefined" ? navigator.language : "en");
  const selection = useEntitySelection();
  const { rows, columns, labels, state, filterColumns } = grid;
  // The entity's own timestamps sort the page even where the source orders the answer (useEntityGrid).
  const sortLabel = (col: { key: string }): string =>
    grid.sortsAll && !col.key.startsWith("entity:") ? labels.sortAll : labels.sortPage;

  // A column offers a filter when it holds something `q` can ask about and the config allows it:
  // `filters.allowed` is the list a dashboard narrows its grid to.
  const allowed = options.config.filters?.allowed;
  const filterable = useMemo(() => {
    const byKey = new Map<string, FilterColumn>();
    for (const column of filterColumns) {
      if (allowed && column.attr !== null && !allowed.includes(column.attr)) {
        continue;
      }
      byKey.set(column.key, column);
    }
    return byKey;
  }, [filterColumns, allowed]);
  const asText = state.filterText !== null;
  // On a phone the filter row folds under one button (T-5415); wider screens always show it.
  const [filtersOpen, setFiltersOpen] = useState(false);
  // The drawn area is a filter as much as a typed one: the folded filter row counts it (T-6345).
  const activeFilters = Object.values(state.filters).filter(complete).length + (grid.area ? 1 : 0);
  useEffect(() => {
    onFilterColumns?.([...filterable.values()]);
  }, [filterable, onFilterColumns]);
  // Shift-click checks the range from the row clicked before (T-4883).
  const lastChecked = useRef<number | null>(null);
  const checkRow = (rowIndex: number, id: string, range: boolean) => {
    if (!onSelectedChange) return;
    const on = !selected.includes(id);
    const from = range && lastChecked.current !== null ? Math.min(lastChecked.current, rowIndex) : rowIndex;
    const to = range && lastChecked.current !== null ? Math.max(lastChecked.current, rowIndex) : rowIndex;
    const ids = rows.slice(from, to + 1).map((row) => row.id);
    lastChecked.current = rowIndex;
    onSelectedChange(on ? [...new Set([...selected, ...ids])] : selected.filter((each) => !ids.includes(each)));
  };
  const allChecked = rows.length > 0 && rows.every((row) => selected.includes(row.id));
  const lead = rowNumbers ? 1 : 0;
  // Edit mode needs a place to write: without one the grid is a reader, whatever the config says.
  // A source that takes no writes switches the feature off, whatever the config says (SDK-29).
  const editing = options.config.mode === "edit" && Boolean(options.source.patch);
  // Which columns open: the config's own list, which `parseGridConfig` requires in edit mode, so a
  // grid never opens a column nobody named.
  const accessFailed = hostAccess === null ? labels.error : ownFailed;
  const asked = hostAccess !== undefined || Boolean(askAccess);
  const accessByName = useMemo(() => new Map((accessRows ?? []).map((row) => [row.name, row])), [accessRows]);
  const accessOf = useCallback((attr: string) => accessByName.get(attr), [accessByName]);
  const editable = useMemo(() => {
    const allowed = options.config.editableAttrs;
    // A computed column is worked out from the row (T-6384): nothing to write back.
    const computed = new Set(options.config.columns.filter((c) => c.formula !== undefined).map((c) => c.attr));
    return (column: VisibleColumn): boolean =>
      // The attribute's own column only: a metadata column shows when the value was observed or
      // what it is measured in, and neither is corrected by typing over it.
      editing &&
      column.attr !== null &&
      column.meta === null &&
      allowed.includes(column.attr) &&
      !computed.has(column.attr) &&
      (!asked || accessOf(column.attr)?.write === true);
  }, [editing, options.config.editableAttrs, options.config.columns, asked, accessOf]);
  const [inspected, setInspected] = useState<{ attr: string; label: string; row?: RichRow } | null>(null);
  // A host that translated only the grid's labels still gets its own word for closing.
  const inspectorLabels = { ...DEFAULT_INSPECTOR_LABELS, close: labels.close, ...options.labels?.inspectorLabels };
  const inspects = (column: VisibleColumn): column is VisibleColumn & { attr: string } =>
    asked && column.attr !== null && column.meta === null && column.key !== "id";
  // In edit mode a column the person cannot change once their decisions are read wears a lock with
  // the reason (T-5530), so a value without an editor is never a puzzle; a read-only grid wears none.
  const lockOf = (column: VisibleColumn): string | undefined =>
    editing && accessRows && inspects(column) && !editable(column) ? lockReason(accessOf(column.attr), labels, inspectorLabels) : undefined;
  // Once per column per render, not per cell: ten thousand rows scroll through the same few reasons.
  const locks = new Map(columns.map((column) => [column.key, lockOf(column)]));
  const [history, setHistory] = useState<{ attr: string; id: string; heading: string } | null>(null);
  // A history opened from a cell shows under the grid: bring it into sight (owner 2026-11-01).
  const historyBox = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (history) historyBox.current?.scrollIntoView?.({ block: "start", behavior: "smooth" });
  }, [history]);
  const detail = useGridDetail(options, { openRow, onOpenRow, inShell: selection.inShell, select: selection.select, rows, loading: grid.loading });
  const { openDetail, detailRow } = detail;
  // The primary column is the first one when it is an attribute: the config pinned it (T-3097).
  const primaryKey = columns[0]?.attr !== null && columns[0]?.pinned ? columns[0].key : null;
  const isPrimary = useCallback(
    (column: VisibleColumn): boolean => column.key === primaryKey && column.pinned && column.meta === null,
    [primaryKey],
  );
  /** What a row is called: its primary field, else its name or title, else its id. */
  const rowName = useCallback(
    (row: RichRow): string => {
      const primary = primaryKey ? cellText(row.cells[primaryKey]) : "";
      return primary || cellText(row.cells.name) || cellText(row.cells.title) || row.id;
    },
    [primaryKey],
  );
  const tableRef = useRef<HTMLTableElement>(null);
  const virtual = options.virtual === true;
  const rowWindow = useRowWindow(virtual, grid);
  const edits = useGridEdits(options, grid, rules);
  const related = useGridTargets(options, rows, labels, onOpenRelationship, onDrill);
  const { targets } = related;
  const cellLabels = useMemo<CellLabels>(
    () => ({ yes: labels.yes, no: labels.no, points: labels.points, openTarget: labels.openTarget, moreInstances: labels.moreInstances }),
    [labels.yes, labels.no, labels.points, labels.openTarget, labels.moreInstances],
  );
  const cells = useCellContent(options, grid, { where, cellLabels, renderers, termHref, rules, editable, isPrimary, rowName, openDetail, ...related, refusedCell: edits.refusedCell, ruleProblems: edits.ruleProblems });
  const controls = useColumnControls(options, grid, { tableRef, layout, setLayout, rules, onHide, ownHidden, setOwnHidden });
  // What the source answered, handed to the host as it arrives: the page a person is looking at is
  // what they mean by "this page" when they export it.
  React.useEffect(() => {
    onRows?.(rows, state.offset);
  }, [rows, state.offset, onRows]);

  const { q: askedQ, idPattern: askedIdPattern } = grid.askedQuery;
  React.useEffect(() => {
    onQuery?.({ q: askedQ, idPattern: askedIdPattern });
  }, [askedQ, askedIdPattern, onQuery]);
  const map = useGridMap(options, grid, rows);
  // One stable reader per open row, so the dialog's Changes read runs once per row (T-6333).
  const changesFeed = useChangesFeed(options.source, detailRow?.id, where, options.labels?.changesLabels, (cell) => (
    <CellView cell={cell} locale={where} labels={cellLabels} targets={targets} />
  ));
  const historyOn = Boolean(options.source.history) && options.config.history.enabled !== false;
  const keys = useGridKeys(options, grid, { editing, editable, inspects, locks, rules, accessRows, accessOf, setInspected, setPasteNote: edits.setPasteNote });
  return {
    props,
    options,
    grid,
    labels,
    columns,
    where,
    selection,
    layout,
    setLayout,
    ownHidden,
    unreadable,
    accessRows,
    accessFailed,
    accessOf,
    sortLabel,
    filterable,
    asText,
    filtersOpen,
    setFiltersOpen,
    activeFilters,
    checkRow,
    allChecked,
    lead,
    editing,
    editable,
    inspected,
    setInspected,
    inspectorLabels,
    inspects,
    locks,
    history,
    setHistory,
    historyBox,
    rowName,
    tableRef,
    virtual,
    cellLabels,
    changesFeed,
    historyOn,
    ...detail,
    ...rowWindow,
    ...edits,
    ...related,
    ...cells,
    ...controls,
    ...map,
    ...keys,
  };
}
