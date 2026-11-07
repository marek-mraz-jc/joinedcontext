/**
 * The grid's tools in one row (T-4883, T-6303): Filter, Hide fields, Sort, Group, Colour, Row
 * size, the search and the query box's switch. The space's data tab and an Endpoint's Data section
 * render this one component, so a person meets the same tools over the same grid on both pages.
 * The host holds every value; this row only draws and reports them. `useDataToolbarState` holds the
 * values a page does not share with the rest of its view.
 */
import { useCallback, useEffect, useMemo, useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { andQ, matchesQ, queryFromGroup, searchQuery } from "@joinedcontext/sdk";
import type { EnumOption, FilterColumn, FilterGroup, RichRow } from "@joinedcontext/sdk";
import { useIsPhone } from "../../phone";
import { Button, Input, RadioGroup } from "../ui";
import { conditionCount, EMPTY_GROUP, FilterPanel } from "./FilterPanel";
import { GroupPanel, MAX_GROUP_FIELDS, ROW_SIZES, SortPanel, ToolbarPopover } from "./GridToolbar";
import type { PanelField, RowSize, SortKey } from "./GridToolbar";
import { ViewOptions } from "./ViewOptions";
import type { ViewExtras } from "./ViewOptions";

export interface DataToolbarProps {
  /** The columns the grid can filter, as it reports them (`onFilterColumns`). */
  filterColumns: FilterColumn[];
  filterTitle: (column: FilterColumn) => string;
  filters: FilterGroup;
  onFilters: (next: FilterGroup) => void;
  /** Every attribute a person can hide or colour by, in the column order. */
  attributes: string[];
  enums: Record<string, EnumOption[]>;
  /** The hidden fields and the colour rules. */
  extras: ViewExtras;
  onExtras: (next: ViewExtras) => void;
  /** What the Sort and Group panels offer. */
  fields: PanelField[];
  sortKeys: SortKey[];
  onSortKeys: (next: SortKey[]) => void;
  groupBy: string[];
  onGroupBy: (next: string[]) => void;
  rowSize: RowSize;
  onRowSize: (next: RowSize) => void;
  searchText: string;
  onSearchText: (next: string) => void;
  showQuery: boolean;
  onShowQuery: (next: boolean) => void;
}

/** The toolbar's values a page passes straight through, as `useDataToolbarState` holds them. */
export type HeldToolbarProps = Pick<
  DataToolbarProps,
  | "filterColumns"
  | "filters"
  | "onFilters"
  | "extras"
  | "onExtras"
  | "groupBy"
  | "onGroupBy"
  | "rowSize"
  | "onRowSize"
  | "searchText"
  | "onSearchText"
  | "showQuery"
  | "onShowQuery"
>;

/** The order the Sort panel shows: the header's sort first, then the keys after it. */
export function sortKeysOf(sort: SortKey | null | undefined, thenBy: SortKey[]): SortKey[] {
  return sort ? [sort, ...thenBy.filter((key) => key.attr !== sort.attr)] : thenBy;
}

/**
 * The toolbar's own state (T-6303): the hidden fields and colours, the filter panel, the keys after
 * the header's sort, the groups, the row size, the search (asked 400 ms after the last key) and the
 * query box. `query` is what the filters and the search ask the grid, `rowTone` the colour rules.
 */
export function useDataToolbarState() {
  const { t } = useTranslation();
  const [extras, setExtras] = useState<ViewExtras>({});
  // Null until the reader or the page's default filter sets it, so a default opens in the panel once.
  const [filters, setFilters] = useState<FilterGroup | null>(null);
  const [filterColumns, setFilterColumns] = useState<FilterColumn[]>([]);
  const [thenBy, setThenBy] = useState<SortKey[]>([]);
  const [groupBy, setGroupBy] = useState<string[]>([]);
  const [rowSize, setRowSize] = useState<RowSize>("small");
  const [searchText, setSearchText] = useState("");
  const [search, setSearch] = useState("");
  const [showQuery, setShowQuery] = useState(false);
  useEffect(() => {
    const wait = setTimeout(() => setSearch(searchText), 400);
    return () => clearTimeout(wait);
  }, [searchText]);
  const group = filters ?? EMPTY_GROUP;
  // The grid reports its columns on every render; the same list keeps the held one.
  const onFilterColumns = useCallback(
    (next: FilterColumn[]) => setFilterColumns((before) => (JSON.stringify(before) === JSON.stringify(next) ? before : next)),
    [],
  );
  const q = useMemo(() => andQ(queryFromGroup(filterColumns, group), searchQuery(search, filterColumns)), [filterColumns, group, search]);
  const query = useMemo(() => (q ? { q } : undefined), [q]);
  const rowTone = useCallback(
    (row: RichRow) => {
      const rule = (extras.colour ?? []).find((each) => each.when.trim() !== "" && matchesQ(row, each.when) === true);
      return rule ? { tone: rule.colour, label: t("spaces.saved.toneLabel", { when: rule.when }) } : undefined;
    },
    [extras.colour, t],
  );
  /** Hides `attr` beside what is hidden now (the page's own hidden fields included). */
  const hide = useCallback((now: string[], attr: string) => setExtras((before) => ({ ...before, hidden: [...new Set([...now, attr])] })), []);
  /** Another type has other columns: every tool starts empty. */
  const reset = useCallback(() => {
    setFilters(EMPTY_GROUP);
    setExtras({});
    setThenBy([]);
    setGroupBy([]);
  }, []);
  // A default filter opens as the panel's conditions, once; from then on the panel is the reader's.
  const adoptDefault = (preset: FilterGroup | undefined) => {
    if (filters === null && preset && preset.conditions.length > 0) setFilters(preset);
  };
  const props: HeldToolbarProps = {
    filterColumns,
    filters: group,
    onFilters: setFilters,
    extras,
    onExtras: setExtras,
    groupBy,
    onGroupBy: setGroupBy,
    rowSize,
    onRowSize: setRowSize,
    searchText,
    onSearchText: setSearchText,
    showQuery,
    onShowQuery: setShowQuery,
  };
  return { props, extras, group, thenBy, setThenBy, groupBy, rowSize, showQuery, onFilterColumns, q, query, rowTone, hide, reset, adoptDefault };
}

export function DataToolbar(props: DataToolbarProps): JSX.Element {
  const { t } = useTranslation();
  const phone = useIsPhone();
  const { extras, onExtras, attributes, enums } = props;
  const labelOf = (attr: string) => props.fields.find((field) => field.value === attr)?.label ?? attr;
  const tools = [
    { label: t("spaces.toolbar.filter"), count: conditionCount(props.filters), wide: true, panel: <FilterPanel columns={props.filterColumns} titleOf={props.filterTitle} value={props.filters} onChange={props.onFilters} /> },
    { label: t("spaces.toolbar.hide"), count: extras.hidden?.length, panel: <ViewOptions part="fields" attributes={attributes} enums={enums} value={extras} onChange={onExtras} labelOf={labelOf} /> },
    { label: t("spaces.toolbar.sort"), count: props.sortKeys.length, panel: <SortPanel fields={props.fields} value={props.sortKeys} onChange={props.onSortKeys} /> },
    { label: t("spaces.toolbar.group"), count: props.groupBy.length, panel: <GroupPanel fields={props.fields} value={props.groupBy} onChange={(next) => props.onGroupBy(next.slice(0, MAX_GROUP_FIELDS))} /> },
    { label: t("spaces.toolbar.colour"), count: extras.colour?.length, wide: true, panel: <ViewOptions part="colours" attributes={attributes} enums={enums} value={extras} onChange={onExtras} /> },
    {
      label: t("spaces.toolbar.rowSize"),
      panel: (
        <RadioGroup
          name="grid-row-size"
          legend={t("spaces.toolbar.rowSize")}
          value={props.rowSize}
          options={ROW_SIZES.map((size) => ({ value: size, label: t(`spaces.toolbar.rowSizes.${size}`) }))}
          onChange={props.onRowSize}
        />
      ),
    },
  ];
  const panels = tools.map(({ panel, ...tool }) => (
    <ToolbarPopover key={tool.label} {...tool} inPlace={phone}>
      {panel}
    </ToolbarPopover>
  ));
  const search = (
    <Input
      type="search"
      aria-label={t("spaces.toolbar.search")}
      placeholder={t("spaces.toolbar.searchPlaceholder")}
      className="h-control w-full max-sm:order-last sm:w-36"
      value={props.searchText}
      onChange={(e) => props.onSearchText(e.target.value)}
    />
  );
  // The query the filters ask, folded until asked for (T-5093): the rows and their count stay together.
  const query = (
    <Button
      variant={props.showQuery ? "secondary" : "ghost"}
      size="sm"
      className={phone ? "h-control-lg justify-start" : undefined}
      aria-pressed={props.showQuery}
      onClick={() => props.onShowQuery(!props.showQuery)}
    >
      {t("spaces.toolbar.query")}
    </Button>
  );
  // A phone gets one menu: six tools wrapped to three rows above the first data row (T-6463).
  if (phone) {
    const set = tools.reduce((sum, tool) => sum + (tool.count ?? 0), 0);
    return (
      <>
        <ToolbarPopover label={t("spaces.toolbar.tools")} count={set} outlined tall closeLabel={t("app.close")}>
          <div className="flex flex-col gap-1">
            {panels}
            {query}
          </div>
        </ToolbarPopover>
        {search}
      </>
    );
  }
  return (
    <>
      {panels}
      {search}
      {query}
    </>
  );
}
