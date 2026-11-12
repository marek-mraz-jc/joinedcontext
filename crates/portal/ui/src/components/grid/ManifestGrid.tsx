/**
 * The one grid of a project's manifests (T-6115, owner 2026-10-29: "one data grid ... easy to
 * sort, query, paginate"): every list page of the sidebar draws its rows through it, so the
 * search, the filter chips, the footer and the cell panel look and behave the same on Endpoints,
 * Subscriptions, Data models and Pipelines. The table itself is the SDK `DataGrid` (T-6750, owner
 * 2026-11-11: one data grid across the Portal, open data and Apps): its column menu sorts and
 * filters one column, Arrange columns shows, hides, renames and orders, and its pager pages.
 *
 * The page passes its rows and columns; a column that names a `spec` field is edited in the cell
 * panel with the control the kind's jc-core JSON Schema gives it (`fieldWidget`), and the edit is
 * a checked proposal through the manifest write path, never a write of its own. The server
 * decides every edit: a refusal (403, 422) is shown on the field it was for.
 */
import { useId, useMemo, useState } from "react";
import type { JSX, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Link } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { clsx } from "clsx";
import { api, queryKeys, unwrap } from "../../api/client";
import { isChange } from "../../api/manifest";
import type { Change, Manifest, ResourceProposal } from "../../api/manifest";
import { proposeChecked } from "../../api/proposal";
import { DataGrid, EMPTY_QUERY } from "@joinedcontext/sdk/grid";
import type { DataColumn, DataGridLabels, DataQuery } from "@joinedcontext/sdk/grid";
import { gridLabels } from "../entities/PortalEntityGrid";
import { ResourceNamePicker } from "../pickers/ResourceNamePicker";
import { ToolbarPopover } from "../entities/GridToolbar";
import { PageFailed } from "../ui/PageState";
import { RowActions, RowDetails } from "../ui/RowActions";
import {
  Button,
  buttonClass,
  Card,
  Checkbox,
  Dialog,
  Field,
  Icon,
  Input,
  PermissionGuard,
  Select,
  recordCard,
  Switch,
} from "../ui";
import { fieldWidget, hintText, specValue, withSpecValue } from "./fieldWidget";
import type { FieldWidget } from "./fieldWidget";

export interface GridColumn<T> {
  id: string;
  label: string;
  /** The column's value as text: what search, sort and the filter chips read. */
  text: (row: T) => string;
  /** What the cell draws; the text when absent. */
  cell?: (row: T) => ReactNode;
  /** What the cell panel draws, with room for more than one line; the cell when absent. */
  detail?: (row: T) => ReactNode;
  /** The `spec` path (`audience`, `notification.format`) the cell panel edits for this column. */
  field?: string;
  /** How a stored value reads to a person (`project-list` -> "Selected projects"), in the filter, the chips and the panel's choices. */
  valueLabel?: (value: string) => string;
  /** Offers a filter chip per value the rows hold. */
  filter?: boolean;
  /** Left out until the person ticks it under Columns. */
  hidden?: boolean;
  /** Sorted as a number rather than as text. */
  numeric?: boolean;
  align?: "left" | "right";
}

export interface ManifestGridProps<T> {
  /** Names the table for a screen reader and keys the columns a person chose in this browser. */
  caption: string;
  storageKey: string;
  rows: T[];
  rowKey: (row: T) => string;
  /** The list's read, for the skeleton, the failure panel and its retry. */
  query: { isPending: boolean; isError: boolean; error: unknown; refetch: () => unknown };
  /** The first column, the row's name: a link to the object, never hidden. */
  name: GridColumn<T>;
  columns: GridColumn<T>[];
  /** The row's menu, at the row's end. */
  actions?: (row: T) => ReactNode;
  /** What an empty list says, with the page's create action. */
  empty: ReactNode;
  /**
   * The search a page holds in its header and has already applied to `rows`; without it the grid
   * draws its own box and searches every column.
   */
  search?: string;
  onSearchChange?: (text: string) => void;
  searchLabel: string;
  /** The page's header around the toolbar, so search and Filter share the title's row (owner 2026-10-31). */
  header?: (toolbar: ReactNode) => ReactNode;
  /** Where cell edits go; without it the cell panel only reads. */
  edit?: {
    project: string;
    plural: string;
    kind: string;
    manifest: (row: T) => Manifest | null;
    onChange: (change: Change) => void;
  };
  pageSize?: number;
  /** How many rows the list holds before the page's own search, when `rows` is already narrowed. */
  total?: number;
  /**
   * The rows are pages the server reads (API/13 section 2, T-6175): the grid's search goes to the
   * server as `q`, Show more reads the next cursor and the footer counts the server's `totalCount`.
   */
  server?: ServerPages;
}

export interface ServerPages {
  onSearch: (text: string) => void;
  /** The listing's `totalCount`, every page together. */
  total?: number;
  hasMore: boolean;
  loadMore: () => void;
}

type Sort = { column: string; desc: boolean } | null;

const PAGE = 50;


/** The rows `search`, `filters` and `sort` leave, in order. Exported for its test. */
export function visibleRows<T>(
  rows: T[],
  columns: GridColumn<T>[],
  search: string,
  filters: Record<string, string[]>,
  sort: Sort,
): T[] {
  const needle = search.trim().toLowerCase();
  const kept = rows.filter((row) => {
    if (needle && !columns.some((column) => column.text(row).toLowerCase().includes(needle))) return false;
    return Object.entries(filters).every(([id, values]) => {
      const column = columns.find((each) => each.id === id);
      return !column || values.length === 0 || values.includes(column.text(row));
    });
  });
  const by = sort ? columns.find((each) => each.id === sort.column) : undefined;
  if (!sort || !by) return kept;
  const sign = sort.desc ? -1 : 1;
  return [...kept].sort((a, b) => {
    const left = by.text(a);
    const right = by.text(b);
    if (by.numeric) return sign * ((Number(left) || 0) - (Number(right) || 0));
    return sign * left.localeCompare(right, undefined, { numeric: true, sensitivity: "base" });
  });
}

/**
 * A column's values with how many rows hold each, sorted: one pass over the rows. The filter panel
 * renders closed too, on every render of the page, so a scan per value cost 10 000 Endpoints a
 * visible part of their first paint (T-5181). Exported for its test.
 */
export function valueCounts<T>(rows: T[], column: GridColumn<T>): [string, number][] {
  const counts = new Map<string, number>();
  for (const row of rows) {
    const value = column.text(row);
    if (value) counts.set(value, (counts.get(value) ?? 0) + 1);
  }
  return [...counts.keys()].sort().map((value) => [value, counts.get(value) ?? 0]);
}

export function ManifestGrid<T>(props: ManifestGridProps<T>): JSX.Element {
  const { caption, storageKey, rows, rowKey, query, name, columns, actions, empty, edit, searchLabel } = props;
  const { t } = useTranslation();
  const [ownSearch, setOwnSearch] = useState("");
  const search = props.search ?? ownSearch;
  const [filters, setFilters] = useState<Record<string, string[]>>({});
  const step = props.pageSize ?? PAGE;
  const [panel, setPanel] = useState<string | null>(null);
  // The table's sort, column filters and page; a new search or chip starts again on page one.
  const [gridQuery, setGridQuery] = useState<DataQuery>({ ...EMPTY_QUERY, pageSize: step });
  const [askedBefore, setAskedBefore] = useState("");
  const asked = JSON.stringify([search, filters]);
  if (askedBefore !== asked) {
    setAskedBefore(asked);
    if (gridQuery.offset !== 0) setGridQuery({ ...gridQuery, offset: 0 });
  }

  const all = useMemo(() => [name, ...columns], [name, columns]);
  const server = props.server;
  const localSearch = props.search === undefined && server === undefined ? ownSearch : "";
  // The search box and the filter chips narrow first; the table's own header menu (sort, the value
  // filter of one column) and its pager then work on what they keep.
  const kept = useMemo(() => visibleRows(rows, all, localSearch, filters, null), [rows, all, localSearch, filters]);
  const opened = panel ? rows.find((row) => rowKey(row) === panel) : undefined;
  const typeSearch = (text: string) => {
    setOwnSearch(text);
    server?.onSearch(text);
  };
  const dataColumns = useMemo<DataColumn<T>[]>(
    () => [
      ...all.map((column, at) => ({
        key: column.id,
        label: column.label,
        text: column.text,
        valueLabel: column.valueLabel,
        hidden: at > 0 && column.hidden,
        align: column.align,
        kind: column.numeric ? ("number" as const) : ("text" as const),
        cell: (row: T) =>
          at === 0 ? (
            column.cell ? column.cell(row) : column.text(row)
          ) : (
            // One element per cell: a phone card lays label and value out as a two-column grid, and a
            // second child (a list of chips) would wrap into the label column (T-6115 judge).
            <div className="min-w-0">{column.cell ? column.cell(row) : column.text(row) || <span className="text-fg-subtle">—</span>}</div>
          ),
      })),
      {
        key: "actions",
        label: t("approvals.actions"),
        text: () => "",
        actions: true,
        align: "right" as const,
        // One menu ends the row, Details first (owner 2026-10-31: no pinned column, no second button).
        // A click in the cell itself is no row click; a click in a dialog the row menu opened reaches
        // this handler through React's portal bubbling and keeps its default, or its submit never
        // fires (T-6169).
        cell: (row: T) => (
          <div
            className="flex justify-end"
            onClick={(event) => (event.currentTarget.contains(event.target as Node) ? event.preventDefault() : undefined)}
          >
            <RowDetails.Provider value={{ label: t("grid.details"), onSelect: () => setPanel(rowKey(row)) }}>
              {actions ? actions(row) : <RowActions label={name.text(row)} actions={[]} />}
            </RowDetails.Provider>
          </div>
        ),
      },
    ],
    [all, actions, name, rowKey, t],
  );
  const labels = useMemo<DataGridLabels>(
    () => ({
      ...gridLabels(t),
      searchRows: t("dataGrid.searchRows"),
      searchRowsHint: t("dataGrid.searchRowsHint"),
      noMatch: t("dataGrid.noMatch"),
      rowCount: t("dataGrid.rowCount", { n: "{n}" }),
      loading: t("app.loading"),
    }),
    [t],
  );

  if (query.isError) {
    // The page's own header stays above the failure, so a refused or missing project still
    // names the page it is (one h1, T-6048) and says why beneath it.
    // A list that is not there (a project out of reach answers 404, PF-59) offers the way to
    // the start page, where the person's own projects are.
    const failed = (
      <PageFailed
        error={query.error}
        onRetry={() => void query.refetch()}
        back={
          <Link to="/" className={buttonClass("secondary", "sm")}>
            {t("app.notFound.home")}
          </Link>
        }
      />
    );
    return props.header ? (
      <div className="flex flex-col gap-3" data-manifest-grid={storageKey}>
        {props.header(null)}
        {failed}
      </div>
    ) : (
      failed
    );
  }

  // The search sits with the page's title (owner 2026-10-31); Filter and its chips sit in the
  // table's own toolbar beside Arrange columns, over the rows they narrow.
  const toolbar =
    props.search === undefined ? (
      <div className="w-full sm:w-96">
        <Input type="search" aria-label={searchLabel} placeholder={searchLabel} value={ownSearch} onChange={(event) => typeSearch(event.target.value)} />
      </div>
    ) : null;
  const tools = <GridToolbar caption={caption} rows={rows} all={all} columns={columns} filters={filters} setFilters={setFilters} />;
  return (
    <div className="flex flex-col gap-3" data-manifest-grid={storageKey}>
      {props.header ? props.header(toolbar) : toolbar}

      {!query.isPending && rows.length === 0 && !(server && search.trim()) ? (
        <Card padding="md">{empty}</Card>
      ) : (
        <DataGrid
          caption={caption}
          rows={kept}
          columns={dataColumns}
          rowKey={rowKey}
          loading={query.isPending}
          empty={
            <div className="flex flex-col items-center gap-2 py-6 text-center">
              <p className="text-body font-medium text-fg">{t("grid.noMatch")}</p>
              <Button variant="secondary" size="sm" onClick={() => { setFilters({}); typeSearch(""); props.onSearchChange?.(""); }}>
                {t("grid.clearAll")}
              </Button>
            </div>
          }
          labels={labels}
          storageKey={`portal:list:${storageKey}`}
          query={gridQuery}
          onQueryChange={setGridQuery}
          pageSizes={[step, step * 2, step * 4]}
          stack
          noSearch
          // A click on a plain cell opens the row's record link, as on every list (T-2875).
          tools={tools}
          tableProps={{ "data-records": "" }}
          rowProps={() => recordCard}
        />
      )}

      {query.isPending || !server ? null : (
        <GridFooter
          shown={rows.length}
          kept={rows.length}
          total={server.total ?? props.total ?? rows.length}
          step={step}
          onMore={() => undefined}
          onMoreFromServer={
            server.hasMore
              ? () => {
                  // The table turns to the page the next rows land on.
                  setGridQuery({ ...gridQuery, offset: rows.length });
                  server.loadMore();
                }
              : undefined
          }
        />
      )}

      {opened !== undefined ? (
        <CellPanel
          row={opened}
          name={name}
          columns={columns}
          edit={edit}
          onClose={() => setPanel(null)}
        />
      ) : null}
    </div>
  );
}

/** Search (when the page has none in its header), Filter and the removable filter chips; the table's header menus sort, and Arrange columns shows and orders. */
export function GridToolbar<T>({
  caption,
  rows,
  all,
  columns,
  ownSearch,
  filters,
  setFilters,
}: {
  caption: string;
  rows: T[];
  all: GridColumn<T>[];
  columns: GridColumn<T>[];
  ownSearch?: { value: string; set: (text: string) => void; label: string };
  filters: Record<string, string[]>;
  setFilters: (next: Record<string, string[]> | ((now: Record<string, string[]>) => Record<string, string[]>)) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const chips = Object.entries(filters).flatMap(([id, values]) => values.map((value) => ({ id, value })));
  const filterable = columns.filter((column) => column.filter);
  const toggleFilter = (id: string, value: string) =>
    setFilters((now) => {
      const values = now[id] ?? [];
      return { ...now, [id]: values.includes(value) ? values.filter((each) => each !== value) : [...values, value] };
    });
  const labelOf = (id: string) => all.find((each) => each.id === id)?.label ?? id;
  const valueOf = (id: string, value: string) => all.find((each) => each.id === id)?.valueLabel?.(value) ?? value;
  return (
      <div role="toolbar" aria-label={t("grid.toolbar", { list: caption })} className="flex flex-wrap items-center gap-2">
      {ownSearch ? (
        <div className="w-full sm:w-96">
          <Input type="search" aria-label={ownSearch.label} placeholder={ownSearch.label} value={ownSearch.value} onChange={(event) => ownSearch.set(event.target.value)} />
        </div>
      ) : null}
      {filterable.length > 0 ? (
        <ToolbarPopover label={t("grid.filter")} count={chips.length} anchored outlined>
          <div className="flex flex-col gap-3">
            {filterable.map((column) => {
              const values = valueCounts(rows, column);
              return (
                <fieldset key={column.id} className="flex flex-col gap-1">
                  <legend className="mb-1 text-caption font-semibold text-fg-muted">{column.label}</legend>
                  {values.length === 0 ? <p className="text-caption text-fg-muted">{t("grid.noValues")}</p> : null}
                  {values.map(([value, count]) => (
                    <Checkbox
                      key={value}
                      label={column.valueLabel ? column.valueLabel(value) : value}
                      hint={String(count)}
                      checked={(filters[column.id] ?? []).includes(value)}
                      onChange={() => toggleFilter(column.id, value)}
                    />
                  ))}
                </fieldset>
              );
            })}
          </div>
        </ToolbarPopover>
      ) : null}
      {chips.map(({ id, value }) => (
        <Button
          key={`${id}=${value}`}
          variant="secondary"
          size="sm"
          icon={<Icon name="close" className="size-3.5" />}
          aria-label={t("grid.removeFilter", { column: labelOf(id), value: valueOf(id, value) })}
          onClick={() => toggleFilter(id, value)}
        >
          {labelOf(id)}: {valueOf(id, value)}
        </Button>
      ))}
      {chips.length > 1 ? (
        <Button variant="ghost" size="sm" onClick={() => setFilters({})}>
          {t("grid.clearFilters")}
        </Button>
      ) : null}
    </div>
  );
}

/** "Showing n of m" and Show more, when the list is cut or narrowed. */
export function GridFooter({
  shown,
  kept,
  total,
  step,
  onMore,
  onMoreFromServer,
}: {
  shown: number;
  kept: number;
  total: number;
  step: number;
  onMore: () => void;
  /** Reads the next page from the server, once every row read so far is shown. */
  onMoreFromServer?: () => void;
}): JSX.Element | null {
  const { t } = useTranslation();
  if (kept === 0 || (kept === shown && kept === total && !onMoreFromServer)) return null;
  return (
    <div className="flex flex-wrap items-center gap-3">
      <p role="status" className="text-caption text-fg-muted">
        {kept === total || onMoreFromServer
          ? t("grid.footer", { shown, count: total })
          : t("grid.footerOf", { shown, count: kept, total })}
      </p>
      {kept > shown ? (
        <Button variant="secondary" size="sm" onClick={onMore}>
          {t("grid.showMore", { count: Math.min(step, kept - shown) })}
        </Button>
      ) : onMoreFromServer ? (
        <Button variant="secondary" size="sm" onClick={onMoreFromServer}>
          {t("grid.showMore", { count: Math.max(1, Math.min(step, total - kept)) })}
        </Button>
      ) : null}
    </div>
  );
}

/** Every column of one row, the hidden ones too, each editable field with its own control and save. */
function CellPanel<T>({
  row,
  name,
  columns,
  edit,
  onClose,
}: {
  row: T;
  name: GridColumn<T>;
  columns: GridColumn<T>[];
  edit: ManifestGridProps<T>["edit"];
  onClose: () => void;
}): JSX.Element {
  const { t } = useTranslation();
  const manifest = edit?.manifest(row) ?? null;
  const schema = useQuery({
    queryKey: ["kind-schema", edit?.kind],
    enabled: edit !== undefined,
    staleTime: 60 * 60_000,
    retry: false,
    queryFn: async () =>
      unwrap(await api.GET("/api/v1/kinds/{kind}/schema", { params: { path: { kind: edit?.kind ?? "" } } })),
  });
  return (
    <Dialog
      open
      onOpenChange={(open) => (open ? undefined : onClose())}
      title={name.text(row)}
      description={t("grid.panelLead")}
      closeLabel={t("app.close")}
      size="md"
    >
      <div className="flex flex-col gap-4">
        {columns.map((column) => {
          const resolved = edit && manifest && column.field && schema.data ? fieldWidget(edit.kind, schema.data, column.field) : null;
          return (
            <div key={column.id} className="flex flex-col gap-1" data-panel-field={column.id}>
              {resolved && edit && manifest && column.field ? (
                <FieldEditor
                  label={column.label}
                  valueLabel={column.valueLabel}
                  description={resolved.description}
                  widget={resolved.widget}
                  manifest={manifest}
                  path={column.field}
                  edit={edit}
                                    onDone={onClose}
                />
              ) : (
                <dl className="flex flex-col gap-1">
                  <dt className="text-caption font-semibold text-fg-muted">{column.label}</dt>
                  <dd className="text-body text-fg">{column.detail ? column.detail(row) : column.cell ? column.cell(row) : column.text(row) || "—"}</dd>
                </dl>
              )}
            </div>
          );
        })}
      </div>
    </Dialog>
  );
}

/** The value as the control holds it: a reference by its name, a list as a list. */
function controlValue(widget: FieldWidget, value: unknown): unknown {
  if (widget.kind === "reference") {
    if (typeof value === "string") return value;
    const named = value as { name?: unknown } | null | undefined;
    return typeof named?.name === "string" ? named.name : "";
  }
  if (widget.kind === "multiEnum") return Array.isArray(value) ? value : [];
  if (widget.kind === "boolean") return value === undefined || value === null ? widget.absent : value === true;
  return value === undefined || value === null ? "" : String(value);
}

/** The control's value as the manifest stores it; `undefined` removes an optional member. */
function storedValue(widget: FieldWidget, held: unknown, before: unknown): unknown {
  if (widget.kind === "reference") {
    const text = String(held);
    // A typed reference keeps its kind and namespace; only the name moves.
    return typeof before === "object" && before !== null ? { ...before, name: text } : text;
  }
  if (widget.kind === "number") return held === "" ? undefined : Number(held);
  // An unset switch left at what unset means stays unset.
  if (widget.kind === "boolean") return (before === undefined || before === null) && held === widget.absent ? before ?? undefined : held;
  if (widget.kind === "multiEnum") return held;
  return held === "" ? undefined : held;
}

/** The example a field's hint shows, by its widget: a choice names exactly the values it offers. Exported for its test. */
export function example(
  widget: FieldWidget,
  t: (key: string, values?: Record<string, unknown>) => string,
  valueLabel: (value: string) => string = (value) => value,
): string {
  switch (widget.kind) {
    case "enum":
      return t("grid.choose.hint", { options: widget.options.map((option) => valueLabel(option.value)).join(", ") });
    case "multiEnum":
      return t("grid.multi.hint", { options: widget.options.map((option) => valueLabel(option.value)).join(", ") });
    case "reference":
      return t("grid.reference.hint");
    case "boolean":
      return t("grid.boolean.hint");
    case "number":
      return t("grid.number.hint", { example: widget.integer ? "60" : "1.5" });
    case "duration":
      return t("grid.duration.hint");
    case "dateTime":
      return t("grid.dateTime.hint");
    default:
      return t("grid.text.hint");
  }
}

function FieldEditor({
  label,
  valueLabel = (value) => value,
  description,
  widget,
  manifest,
  path,
  edit,
  onDone,
}: {
  label: string;
  valueLabel?: (value: string) => string;
  description?: string;
  widget: FieldWidget;
  manifest: Manifest;
  path: string;
  edit: Omit<NonNullable<ManifestGridProps<unknown>["edit"]>, "manifest">;
  onDone: () => void;
}): JSX.Element {
  const { t } = useTranslation();
  const id = useId();
  const queryClient = useQueryClient();
  const before = specValue(manifest, path);
  const [held, setHold] = useState<unknown>(() => controlValue(widget, before));
  const changed = JSON.stringify(held) !== JSON.stringify(controlValue(widget, before));
  const propose = useMutation({
    mutationFn: async () => {
      // The row is the list as it was read: a Change merged since then would be undone by a
      // manifest rebuilt from it (F-R2UI2-007). The edit goes onto the manifest as it is now, and
      // its ETag makes the server refuse it with 412 if it moves again before the write.
      const read = await api.GET("/api/v1/projects/{project}/{plural}/{name}", {
        params: { path: { project: edit.project, plural: edit.plural, name: manifest.metadata.name } },
      });
      const current = (await unwrap(read)) as unknown as Manifest;
      const next = withSpecValue(current, path, storedValue(widget, held, specValue(current, path)));
      const { status: _status, ...body } = next as Manifest & { status?: unknown };
      return proposeChecked(edit.project, edit.plural, body as unknown as ResourceProposal, false, read.response.headers.get("ETag") ?? undefined);
    },
    onSuccess: (result) => {
      if (isChange(result)) edit.onChange(result);
      void queryClient.invalidateQueries({ queryKey: queryKeys.changes(edit.project) });
      onDone();
    },
  });
  // A new value clears the last refusal; the same value again would only be refused again.
  const setHeld = (value: unknown) => {
    setHold(value);
    propose.reset();
  };
  const error = propose.error instanceof Error ? propose.error.message : propose.error ? String(propose.error) : null;
  // Only the schema's first sentence, without the requirement ids a person does not need here.
  const lead = hintText(description);
  const hint = [lead, example(widget, t, valueLabel)].filter(Boolean).join(" ");

  const control = (() => {
    switch (widget.kind) {
      case "enum":
        return (
          <Select id={id} value={String(held)} onChange={(event) => setHeld(event.target.value)}>
            {widget.nullable || held === "" ? <option value="">{t("grid.platformDefault")}</option> : null}
            {widget.options.map((option) => (
              <option key={option.value} value={option.value} title={option.description}>
                {valueLabel(option.value)}
              </option>
            ))}
          </Select>
        );
      case "multiEnum": {
        const list = held as string[];
        return (
          <div id={id} role="group" aria-label={label} className="flex flex-wrap gap-x-4 gap-y-1">
            {widget.options.map((option) => (
              <Checkbox
                key={option.value}
                label={valueLabel(option.value)}
                checked={list.includes(option.value)}
                onChange={() => setHeld(list.includes(option.value) ? list.filter((each) => each !== option.value) : [...list, option.value])}
              />
            ))}
          </div>
        );
      }
      case "reference":
        return (
          <ResourceNamePicker
            id={id}
            labelled
            label={label}
            from={{ project: edit.project, plural: widget.plural }}
            value={String(held)}
            onChange={setHeld}
            empty={t("grid.referenceEmpty")}
          />
        );
      case "boolean":
        return <Switch id={id} checked={held === true} onCheckedChange={setHeld} label={held === true ? t("grid.on") : t("grid.off")} />;
      case "number":
        return (
          <Input id={id} type="number" inputMode="numeric" min={widget.minimum} step={widget.integer ? 1 : "any"} value={String(held)} onChange={(event) => setHeld(event.target.value)} />
        );
      default:
        return <Input id={id} value={String(held)} onChange={(event) => setHeld(event.target.value)} />;
    }
  })();

  return (
    <form
      className="flex flex-col gap-2"
      onSubmit={(event) => {
        event.preventDefault();
        if (changed && !propose.isError) propose.mutate();
      }}
    >
      <Field id={id} label={label} help={hint} errors={error ? [error] : undefined}>
        {control}
      </Field>
      {changed ? (
        <div className={clsx("flex flex-wrap items-center gap-2")}>
          <PermissionGuard project={edit.project} kind={edit.kind} verb="propose">
            <Button type="submit" size="sm" variant="primary" loading={propose.isPending} disabled={propose.isError} disabledReason={propose.isError ? t("grid.retry") : undefined}>
              {t("grid.propose")}
            </Button>
          </PermissionGuard>
          <Button type="button" size="sm" variant="ghost" onClick={() => setHeld(controlValue(widget, before))}>
            {t("grid.undo")}
          </Button>
        </div>
      ) : null}
    </form>
  );
}
