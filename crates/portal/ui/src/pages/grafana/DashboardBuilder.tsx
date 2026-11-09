/**
 * The dashboard builder (owner 2026-11-08: "let the users manually create these dashboards in
 * the UI"). A person who may propose a Dashboard lays panels out on Grafana's 24-column grid and
 * picks what each reads from lists the server filled for them: Endpoint, type, attribute,
 * aggregation, interval and filters. The page sends choices only; the Portal compiles the
 * Grafana file, previews a panel as the person, and saves the dashboard as a Change that waits
 * for approval. Once approved, the dashboard page draws it.
 */
import { createContext, useContext, useEffect, useRef, useState } from "react";
import type { ComponentProps } from "react";
import type { JSX, PointerEvent as ReactPointerEvent } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Link, useNavigate } from "@tanstack/react-router";
import { api, ApiError, reasonOf, unwrap } from "../../api/client";
import type { components } from "../../api/schema";
import {
  Alert,
  Badge,
  Button,
  buttonClass,
  Card,
  CardHeader,
  Checkbox,
  EmptyState,
  Field,
  Icon,
  Input,
  PageFailed,
  PageHeader,
  PageLoading,
  RadioGroup,
  Select,
  Textarea,
} from "../../components/ui";
import { ROW_PX } from "../../components/panels/DashboardGrid";
import type { Frame } from "../../components/panels/frames";
import type { PanelDefinition } from "../../components/panels/PanelView";
import { PanelView, plainText } from "../../components/panels/PanelView";
import { panelRange } from "../../components/panels/timeRange";
import { COLUMNS, clamp, dragged, settle } from "./builderGrid";
import type { Grid } from "./builderGrid";
import {
  fieldId,
  lineInWords,
  AGGREGATIONS,
  boardOf,
  drawable,
  emptyBoard,
  fieldWords,
  INTERVALS,
  currentOnly,
  DISPLAY_MODES,
  isNumber,
  isText,
  MAX_CONTENT,
  missing,
  ORIENTATIONS,
  PIE_TYPES,
  SCALED,
  newPanel,
  OPS,
  PANEL_KINDS,
  RANGES,
  refusals,
  withKind,
} from "./builderModel";
import type { Board, BoardPanel, BoardVariable, Filter, PanelKind } from "./builderModel";

type Start = components["schemas"]["BuilderStart"];
type Options = components["schemas"]["GrafanaOptions"];

/** The refused fields of the last save, by control id, while the board is the one that was sent. */
const Invalid = createContext<Record<string, string>>({});

/** A Field that shows its refusal under the control, with the red border, until the board changes. */
function BField(props: ComponentProps<typeof Field>): JSX.Element {
  const own = useContext(Invalid)[props.id];
  return <Field {...props} errors={own ? [own] : props.errors} />;
}

const AGGREGATION_HINTS: Record<PanelKind, string> = {
  timeseries: "builder.hint.aggregationTimeseries",
  stat: "builder.hint.aggregationStat",
  gauge: "builder.hint.aggregationGauge",
  bargauge: "builder.hint.aggregationGauge",
  piechart: "builder.hint.aggregationPie",
  table: "builder.hint.aggregationStat",
  barchart: "builder.hint.aggregationStat",
  text: "builder.hint.aggregationStat",
};

const builderKey = (project: string) => ["projects", project, "grafana", "builder"];

/** The choices one Endpoint and type offer, from the visualization form's own route. */
function useChoices(project: string, endpoint: string, type: string) {
  return useQuery({
    queryKey: ["projects", project, "grafana", "options", endpoint, type],
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/grafana/options", {
          params: { path: { project }, query: { endpoint: endpoint || undefined, type: type || undefined } },
        }),
      ),
  });
}

/** Endpoint, type and attribute: three lists, each filled once the one before it is chosen. */
export function SourcePicker({
  project,
  id,
  value,
  onChange,
  attributesOf,
  hints = "panel",
}: {
  project: string;
  id: string;
  value: { endpoint: string; type: string; attribute: string };
  onChange: (next: { endpoint: string; type: string; attribute: string }) => void;
  attributesOf: (options: Options) => Options["attributes"];
  /** Whose words the help lines use: a panel's, or a variable's. */
  hints?: "panel" | "variable";
}): JSX.Element {
  const { t } = useTranslation();
  const choices = useChoices(project, value.endpoint, value.type);
  const endpoints = (choices.data?.endpoints ?? []).filter((endpoint) => endpoint.usable);
  const attributes = choices.data ? attributesOf(choices.data) : [];
  // Three columns only where the form is wide: the panel form beside the grid is not.
  return (
    <div className="@container">
    <div className="grid gap-3 @2xl:grid-cols-3 [&>*]:min-w-0">
      <BField id={`${id}-endpoint`} label={t("builder.field.endpoint")} help={choices.isError ? reasonOf(choices.error) : hints === "variable" ? t("builder.hint.variableEndpoint") : t("builder.hint.endpoint")}>
        <Select id={`${id}-endpoint`} value={value.endpoint} onChange={(event) => onChange({ endpoint: event.target.value, type: "", attribute: "" })}>
          <option value="">{t("builder.choose")}</option>
          {endpoints.map((endpoint) => (
            <option key={endpoint.name} value={endpoint.name}>
              {endpoint.title}
            </option>
          ))}
          {value.endpoint && !endpoints.some((endpoint) => endpoint.name === value.endpoint) ? <option value={value.endpoint}>{value.endpoint}</option> : null}
        </Select>
      </BField>
      <BField id={`${id}-type`} label={t("builder.field.type")} help={hints === "variable" ? t("builder.hint.variableType") : t("builder.hint.type")}>
        <Select id={`${id}-type`} value={value.type} disabled={!value.endpoint} title={value.endpoint ? undefined : t("builder.chooseEndpointFirst")} onChange={(event) => onChange({ ...value, type: event.target.value, attribute: "" })}>
          <option value="">{t("builder.choose")}</option>
          {(choices.data?.types ?? []).map((type) => (
            <option key={type} value={type}>
              {type}
            </option>
          ))}
          {value.type && !(choices.data?.types ?? []).includes(value.type) ? <option value={value.type}>{value.type}</option> : null}
        </Select>
      </BField>
      <BField id={`${id}-attribute`} label={t("builder.field.attribute")} help={hints === "variable" ? t("builder.hint.variableAttribute") : t("builder.hint.attribute")}>
        <Select id={`${id}-attribute`} value={value.attribute} disabled={!value.type} title={value.type ? undefined : t("builder.chooseTypeFirst")} onChange={(event) => onChange({ ...value, attribute: event.target.value })}>
          <option value="">{t("builder.choose")}</option>
          {attributes.map((attribute) => (
            <option key={attribute.name} value={attribute.name}>
              {attribute.name}
            </option>
          ))}
          {value.attribute && !attributes.some((attribute) => attribute.name === value.attribute) ? <option value={value.attribute}>{value.attribute}</option> : null}
        </Select>
      </BField>
    </div>
    </div>
  );
}

/** The dashboard's title, description, default range and refresh. */
export function BoardSettings({ board, start, onChange }: { board: Board; start: Start; onChange: (board: Board) => void }): JSX.Element {
  const { t } = useTranslation();
  return (
    <Card as="section" aria-labelledby="builder-settings">
      <CardHeader as="h2" titleId="builder-settings" title={t("builder.settings")} className="mb-3" />
      <div className="grid gap-3 md:grid-cols-2 [&>*]:min-w-0">
        <BField id="builder-title" label={t("builder.field.title")} help={t("builder.hint.title")} required>
          <Input id="builder-title" value={board.title} maxLength={120} onChange={(event) => onChange({ ...board, title: event.target.value })} />
        </BField>
        <BField id="builder-description" label={t("builder.field.description")} help={t("builder.hint.description")}>
          <Input id="builder-description" value={board.description ?? ""} maxLength={500} onChange={(event) => onChange({ ...board, description: event.target.value })} />
        </BField>
        <BField id="builder-from" label={t("builder.field.from")} help={t("builder.hint.from")}>
          <Select id="builder-from" value={board.from ?? "now-6h"} onChange={(event) => onChange({ ...board, from: event.target.value, to: "now" })}>
            {RANGES.map((from) => (
              <option key={from} value={from}>
                {t(`builder.range.${from.slice(4)}`)}
              </option>
            ))}
            {board.from && !RANGES.includes(board.from as (typeof RANGES)[number]) ? <option value={board.from}>{board.from}</option> : null}
          </Select>
        </BField>
        <BField id="builder-refresh" label={t("builder.field.refresh")} help={t("builder.hint.refresh")}>
          <Select id="builder-refresh" value={board.refresh ?? ""} onChange={(event) => onChange({ ...board, refresh: event.target.value })}>
            {start.refreshes.map((refresh) => (
              <option key={refresh} value={refresh}>
                {refresh ? t("builder.every", { interval: refresh }) : t("builder.refreshOff")}
              </option>
            ))}
          </Select>
        </BField>
      </div>
    </Card>
  );
}

/** One variable: its name, label and the text attribute whose values it offers. */
function VariableRow({ project, variable, index, onChange, onRemove }: { project: string; variable: BoardVariable; index: number; onChange: (variable: BoardVariable) => void; onRemove: () => void }): JSX.Element {
  const { t } = useTranslation();
  const id = `builder-var-${index}`;
  return (
    <li className="flex flex-col gap-3 border-b border-border py-3 last:border-b-0">
      <div className="grid gap-3 sm:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto] sm:items-start [&>*]:min-w-0">
        <BField id={`${id}-name`} label={t("builder.field.variableName")} help={t("builder.hint.variableName")}>
          <Input id={`${id}-name`} value={variable.name} maxLength={32} onChange={(event) => onChange({ ...variable, name: event.target.value })} />
        </BField>
        <BField id={`${id}-label`} label={t("builder.field.variableLabel")} help={t("builder.hint.variableLabel")}>
          <Input id={`${id}-label`} value={variable.label ?? ""} maxLength={120} onChange={(event) => onChange({ ...variable, label: event.target.value })} />
        </BField>
        <Button variant="ghost" size="sm" className="sm:mt-6" aria-label={t("builder.removeVariable", { name: variable.name || index + 1 })} onClick={onRemove}>
          <Icon name="trash" className="size-4" />
          {t("builder.remove")}
        </Button>
      </div>
      <SourcePicker
        project={project}
        id={id}
        value={{ endpoint: variable.endpoint, type: variable.type, attribute: variable.attribute }}
        onChange={(source) => onChange({ ...variable, ...source })}
        attributesOf={(options) => options.attributes.filter(isText)}
        hints="variable"
      />
      <Checkbox label={t("builder.includeAll")} checked={variable.includeAll ?? false} onChange={(event) => onChange({ ...variable, includeAll: event.target.checked })} />
    </li>
  );
}

/** The dashboard's variables: each offers the current values of one text attribute. */
export function VariablesEditor({ project, board, onChange }: { project: string; board: Board; onChange: (board: Board) => void }): JSX.Element {
  const { t } = useTranslation();
  const variables = board.variables ?? [];
  const set = (index: number, variable: BoardVariable) => onChange({ ...board, variables: variables.map((each, i) => (i === index ? variable : each)) });
  return (
    <Card as="section" aria-labelledby="builder-variables">
      <CardHeader
        as="h2"
        titleId="builder-variables"
        title={t("builder.variables")}
        description={t("builder.hint.variables")}
        className="mb-1"
        actions={
          variables.length < 8 ? (
            <Button
              size="sm"
              variant="secondary"
              onClick={() => onChange({ ...board, variables: [...variables, { name: `var${variables.length + 1}`, label: "", endpoint: "", type: "", attribute: "", includeAll: true }] })}
            >
              <Icon name="plus" className="size-4" />
              {t("builder.addVariable")}
            </Button>
          ) : null
        }
      />
      {variables.length === 0 ? (
        <p className="py-2 text-body text-fg-muted">{t("builder.noVariables")}</p>
      ) : (
        <ul>
          {variables.map((variable, index) => (
            <VariableRow
              key={index}
              project={project}
              variable={variable}
              index={index}
              onChange={(next) => set(index, next)}
              onRemove={() => onChange({ ...board, variables: variables.filter((_, i) => i !== index) })}
            />
          ))}
        </ul>
      )}
    </Card>
  );
}

/** A drag in progress: the panel, what the pointer does to it, and where it would land. */
interface Drag {
  id: number;
  mode: "move" | "resize";
  startX: number;
  startY: number;
  start: Grid;
  at: Grid;
}

function useWidth(): [React.RefObject<HTMLDivElement | null>, number] {
  const ref = useRef<HTMLDivElement | null>(null);
  const [width, setWidth] = useState(0);
  useEffect(() => {
    const node = ref.current;
    if (!node) return undefined;
    setWidth(node.clientWidth);
    if (typeof ResizeObserver === "undefined") return undefined;
    const observer = new ResizeObserver(() => setWidth(node.clientWidth));
    observer.observe(node);
    return () => observer.disconnect();
  }, []);
  return [ref, width];
}

/** One panel on the canvas: its title, what it reads, and its drag and resize handles. */
function CanvasCard({ panel, selected, onSelect, onDrag }: { panel: BoardPanel; selected: boolean; onSelect: () => void; onDrag?: (event: ReactPointerEvent, mode: Drag["mode"]) => void }): JSX.Element {
  const { t } = useTranslation();
  const reads = panel.kind === "text" ? plainText(panel.text?.content ?? "").split("\n")[0] || t("builder.notWritten") : panel.attribute ? `${panel.type} · ${panel.attribute}` : t("builder.notChosen");
  return (
    <div className={`flex h-full min-w-0 flex-col rounded-md border bg-surface shadow-sm ${selected ? "border-primary ring-2 ring-primary/40" : "border-border"}`}>
      <div
        className={`flex items-center gap-2 border-b border-border px-2 py-1 ${onDrag ? "cursor-move touch-none" : ""}`}
        onPointerDown={onDrag ? (event) => onDrag(event, "move") : undefined}
        data-drag-handle={onDrag ? "" : undefined}
      >
        {onDrag ? <Icon name="menu" className="size-4 shrink-0 text-fg-muted" /> : null}
        <Badge>{t(`builder.kind.${panel.kind}`)}</Badge>
      </div>
      <Button variant="ghost" className="h-auto min-h-11 w-full flex-1 flex-col items-start justify-start gap-1 overflow-hidden rounded-none p-2 text-left" aria-pressed={selected} onClick={onSelect}>
        <span className="w-full break-words text-body font-medium text-fg">{panel.title.trim() || t("builder.untitledPanel")}</span>
        <span className="w-full break-words text-caption font-normal text-fg-muted">{reads}</span>
        <KindSketch kind={panel.kind} />
      </Button>
      {onDrag ? (
        <span
          role="presentation"
          className="ml-auto size-4 cursor-se-resize touch-none border-b-2 border-r-2 border-fg-muted"
          onPointerDown={(event) => onDrag(event, "resize")}
          data-resize-handle=""
        />
      ) : null}
    </div>
  );
}

/** A quiet drawing of what a panel kind shows, so the grid reads as a dashboard before any preview. */
function KindSketch({ kind }: { kind: PanelKind }): JSX.Element {
  const common = { "aria-hidden": true, viewBox: "0 0 120 48", preserveAspectRatio: "xMidYMid meet", className: "mt-auto max-h-16 w-full flex-1 text-fg-subtle" } as const;
  if (kind === "timeseries") {
    return (
      <svg {...common} fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
        <path d="M4 40 L20 30 L34 34 L50 18 L66 24 L82 10 L98 16 L116 6" />
        <path d="M4 44 H116" strokeWidth="1" />
      </svg>
    );
  }
  if (kind === "stat") {
    return (
      <svg {...common} fill="currentColor">
        <rect x="34" y="10" width="52" height="16" rx="3" opacity="0.7" />
        <rect x="44" y="32" width="32" height="5" rx="2" opacity="0.4" />
      </svg>
    );
  }
  if (kind === "gauge") {
    return (
      <svg {...common} fill="none" stroke="currentColor" strokeWidth="6" strokeLinecap="round">
        <path d="M36 44 A24 24 0 0 1 84 44" opacity="0.4" />
        <path d="M36 44 A24 24 0 0 1 72 23" />
      </svg>
    );
  }
  if (kind === "bargauge") {
    return (
      <svg {...common} fill="currentColor">
        {[8, 20, 32].map((y, i) => (
          <g key={y}>
            <rect x="4" y={y} width="112" height="7" rx="2" opacity="0.2" />
            <rect x="4" y={y} width={[96, 64, 40][i]} height="7" rx="2" opacity="0.7" />
          </g>
        ))}
      </svg>
    );
  }
  if (kind === "piechart") {
    return (
      <svg {...common} fill="currentColor">
        <circle cx="60" cy="24" r="20" opacity="0.3" />
        <path d="M60 24 L60 4 A20 20 0 0 1 79 30 Z" opacity="0.8" />
      </svg>
    );
  }
  if (kind === "barchart") {
    return (
      <svg {...common} fill="currentColor">
        {[30, 14, 22, 6, 18].map((y, i) => (
          <rect key={i} x={14 + i * 20} y={y} width="12" height={44 - y} rx="2" opacity="0.6" />
        ))}
      </svg>
    );
  }
  if (kind === "text") {
    return (
      <svg {...common} fill="currentColor">
        <rect x="4" y="6" width="60" height="7" rx="2" opacity="0.7" />
        {[20, 30, 40].map((y) => (
          <rect key={y} x="4" y={y} width={y === 40 ? 70 : 112} height="4" rx="2" opacity="0.35" />
        ))}
      </svg>
    );
  }
  return (
    <svg {...common} fill="currentColor">
      {[6, 18, 30].map((y) => (
        <g key={y} opacity={y === 6 ? 0.8 : 0.4}>
          <rect x="4" y={y} width="50" height="6" rx="2" />
          <rect x="62" y={y} width="54" height="6" rx="2" />
        </g>
      ))}
    </svg>
  );
}

/**
 * The 24-column grid with the panels where they sit: drag a panel by its top bar to move it,
 * by its corner to resize it; the others make room as Grafana's do. Below `lg` the panels are a
 * list in reading order and the panel form's position fields place them.
 */
export function BuilderCanvas({ panels, selected, onSelect, onPlace, locked = false }: { panels: readonly BoardPanel[]; selected: number | null; onSelect: (id: number) => void; onPlace: (id: number, grid: Grid) => void; locked?: boolean }): JSX.Element {
  const { t } = useTranslation();
  const [ref, width] = useWidth();
  const [drag, setDrag] = useState<Drag | null>(null);
  const wide = width >= 640;
  const column = width / COLUMNS;
  const begin = (panel: BoardPanel) => (event: ReactPointerEvent, mode: Drag["mode"]) => {
    event.preventDefault();
    (event.target as Element).setPointerCapture?.(event.pointerId);
    setDrag({ id: panel.id, mode, startX: event.clientX, startY: event.clientY, start: panel.grid, at: panel.grid });
  };
  const shown = drag ? settle(panels.map((panel) => ({ ...panel })), drag.id, drag.at) : [...panels];
  const bottom = shown.reduce((most, panel) => Math.max(most, panel.grid.y + panel.grid.h), 0);
  // Computed places on the grid, as DashboardGrid computes them: two rows of room below the last panel.
  const tall = { height: Math.max(4, bottom + 2) * ROW_PX };
  const at = (grid: Grid) => ({ left: grid.x * column, top: grid.y * ROW_PX, width: grid.w * column, height: grid.h * ROW_PX });
  return (
    <div
      ref={ref}
      aria-label={t("builder.canvas")}
      role="group"
      className="relative w-full"
      style={wide ? tall : undefined}
      onPointerMove={(event) => drag && setDrag({ ...drag, at: dragged(drag.start, event.clientX - drag.startX, event.clientY - drag.startY, column, ROW_PX, drag.mode) })}
      onPointerUp={() => {
        if (drag) onPlace(drag.id, drag.at);
        setDrag(null);
      }}
      onPointerCancel={() => setDrag(null)}
    >
      {wide ? (
        <div aria-hidden="true" className="pointer-events-none absolute inset-0 grid grid-cols-24 gap-0 opacity-60">
          {Array.from({ length: COLUMNS }, (_, i) => (
            <div key={i} className="border-l border-dashed border-border" />
          ))}
        </div>
      ) : null}
      {wide ? (
        shown.map((panel) => (
          <div
            key={panel.id}
            className="absolute p-1 transition-[left,top,width,height] duration-100"
            style={at(panel.grid)}
          >
            <CanvasCard panel={panel} selected={selected === panel.id} onSelect={() => onSelect(panel.id)} onDrag={locked ? undefined : begin(panel)} />
          </div>
        ))
      ) : (
        <ul className="flex flex-col gap-2">
          {[...shown].sort((a, b) => a.grid.y - b.grid.y || a.grid.x - b.grid.x).map((panel) => (
            <li key={panel.id}>
              <CanvasCard panel={panel} selected={selected === panel.id} onSelect={() => onSelect(panel.id)} />
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

/** One filter row: an attribute compared with a value, or matched against a variable's picks. */
function FilterRow({ id, filter, attributes, variables, onChange, onRemove }: { id: string; filter: Filter; attributes: Options["attributes"]; variables: BoardVariable[]; onChange: (filter: Filter) => void; onRemove: () => void }): JSX.Element {
  const { t } = useTranslation();
  const byVariable = filter.variable !== undefined && filter.variable !== null;
  const numeric = attributes.find((option) => option.name === filter.attribute);
  const ops = numeric && isNumber(numeric) ? OPS : (["eq", "ne"] as const);
  return (
    <li className="@container border-b border-border py-2 last:border-b-0">
      <div className="grid grid-cols-2 gap-2 @xl:grid-cols-[minmax(0,1fr)_7rem_minmax(0,1fr)_auto] @xl:items-start [&>*]:min-w-0">
      <BField id={`${id}-attribute`} label={t("builder.field.attribute")} srLabel>
        <Select id={`${id}-attribute`} value={filter.attribute} onChange={(event) => onChange({ ...filter, attribute: event.target.value })}>
          <option value="">{t("builder.choose")}</option>
          {attributes.map((option) => (
            <option key={option.name} value={option.name}>
              {option.name}
            </option>
          ))}
        </Select>
      </BField>
      {byVariable ? (
        // A variable matches what people pick: the comparison is fixed, so it is a word, not a list.
        <span className="self-center text-body text-fg-muted" title={t("builder.opByVariable")}>
          {t("builder.op.eq")}
        </span>
      ) : (
        <BField id={`${id}-op`} label={t("builder.field.filterOp")} srLabel>
          <Select id={`${id}-op`} value={filter.op} onChange={(event) => onChange({ ...filter, op: event.target.value as Filter["op"] })}>
            {ops.map((op) => (
              <option key={op} value={op}>
                {t(`builder.op.${op}`)}
              </option>
            ))}
          </Select>
        </BField>
      )}
      {byVariable ? (
        <BField id={`${id}-variable`} label={t("builder.field.filterVariable")} srLabel>
          <Select id={`${id}-variable`} value={filter.variable ?? ""} onChange={(event) => onChange({ ...filter, op: "eq", variable: event.target.value })}>
            {variables.map((variable) => (
              <option key={variable.name} value={variable.name}>
                {t("builder.picked", { name: variable.label || variable.name })}
              </option>
            ))}
          </Select>
        </BField>
      ) : (
        <BField id={`${id}-value`} label={t("builder.field.filterValue")} srLabel>
          <Input id={`${id}-value`} value={filter.value ?? ""} maxLength={128} placeholder={t("builder.hint.value")} onChange={(event) => onChange({ ...filter, value: event.target.value })} />
        </BField>
      )}
      <div className="flex gap-1">
        {variables.length > 0 ? (
          <Button
            size="sm"
            variant="ghost"
            onClick={() => onChange(byVariable ? { attribute: filter.attribute, op: "eq", value: "" } : { attribute: filter.attribute, op: "eq", variable: variables[0].name })}
          >
            {byVariable ? t("builder.byValue") : t("builder.byVariable")}
          </Button>
        ) : null}
        <Button size="sm" variant="ghost" aria-label={t("builder.removeFilter")} title={t("builder.removeFilter")} onClick={onRemove}>
          <Icon name="trash" className="size-4" />
        </Button>
      </div>
      </div>
    </li>
  );
}

/** The filters of a panel: which of the type's entities it reads. */
export function FiltersEditor({ project, panel, variables, onChange }: { project: string; panel: BoardPanel; variables: BoardVariable[]; onChange: (filters: Filter[]) => void }): JSX.Element {
  const { t } = useTranslation();
  const choices = useChoices(project, panel.endpoint ?? "", panel.type ?? "");
  const attributes = (choices.data?.attributes ?? []).filter((option) => isNumber(option) || isText(option));
  const filters = panel.filters ?? [];
  return (
    <fieldset className="flex flex-col gap-1">
      <legend className="text-body font-medium text-fg">{t("builder.field.filters")}</legend>
      <p className="text-caption text-fg-muted">{t("builder.hint.filters")}</p>
      {filters.length ? (
        <ul>
          {filters.map((filter, index) => (
            <FilterRow
              key={index}
              id={`builder-filter-${panel.id}-${index}`}
              filter={filter}
              attributes={attributes}
              variables={variables.filter((variable) => variable.name)}
              onChange={(next) => onChange(filters.map((each, i) => (i === index ? next : each)))}
              onRemove={() => onChange(filters.filter((_, i) => i !== index))}
            />
          ))}
        </ul>
      ) : null}
      {filters.length < 8 ? (
        <div>
          <Button size="sm" variant="secondary" disabled={!panel.type} disabledReason={t("builder.chooseTypeFirst")} onClick={() => onChange([...filters, { attribute: "", op: "eq", value: "" }])}>
            <Icon name="plus" className="size-4" />
            {t("builder.addFilter")}
          </Button>
        </div>
      ) : null}
    </fieldset>
  );
}

/** Unit, decimals and the colours by value. */
export function DisplayEditor({ panel, start, onChange }: { panel: BoardPanel; start: Start; onChange: (panel: BoardPanel) => void }): JSX.Element {
  const { t } = useTranslation();
  const id = `builder-panel-${panel.id}`;
  const thresholds = panel.thresholds ?? null;
  const steps = thresholds?.steps ?? [];
  const colour = (value: string, change: (color: string) => void, fieldId: string, label: string) => (
    <BField id={fieldId} label={label} srLabel>
      <Select id={fieldId} value={value} onChange={(event) => change(event.target.value)}>
        {start.colors.map((color) => (
          <option key={color} value={color}>
            {t(`builder.color.${color}`)}
          </option>
        ))}
      </Select>
    </BField>
  );
  return (
    <fieldset className="flex flex-col gap-3">
      <legend className="text-body font-medium text-fg">{t("builder.display")}</legend>
      <div className="grid gap-3 sm:grid-cols-2 [&>*]:min-w-0">
        <BField id={`${id}-unit`} label={t("builder.field.unit")} help={t("builder.hint.unit")}>
          <Select id={`${id}-unit`} value={panel.unit ?? ""} onChange={(event) => onChange({ ...panel, unit: event.target.value })}>
            {start.units.map((unit) => (
              <option key={unit} value={unit}>
                {unit ? t(`builder.unit.${unit}`, { defaultValue: unit }) : t("builder.unitAuto")}
              </option>
            ))}
          </Select>
        </BField>
        <BField id={`${id}-decimals`} label={t("builder.field.decimals")} help={t("builder.hint.decimals")}>
          <Select id={`${id}-decimals`} value={panel.decimals === undefined || panel.decimals === null ? "" : String(panel.decimals)} onChange={(event) => onChange({ ...panel, decimals: event.target.value === "" ? undefined : Number(event.target.value) })}>
            <option value="">{t("builder.decimalsAuto")}</option>
            {[0, 1, 2, 3, 4, 5, 6].map((n) => (
              <option key={n} value={n}>
                {n}
              </option>
            ))}
          </Select>
        </BField>
      </div>
      {SCALED.includes(panel.kind) ? (
        <div className="grid gap-3 sm:grid-cols-2 [&>*]:min-w-0">
          <OptionalNumber id={`${id}-min`} label={t("builder.field.min")} help={t("builder.hint.min")} value={panel.min} onChange={(min) => onChange({ ...panel, min })} />
          <OptionalNumber id={`${id}-max`} label={t("builder.field.max")} help={t("builder.hint.max")} value={panel.max} onChange={(max) => onChange({ ...panel, max })} />
        </div>
      ) : null}
      <div className="flex flex-col gap-2">
        <Checkbox
          label={t("builder.useThresholds")}
          checked={thresholds !== null}
          onChange={(event) => onChange({ ...panel, thresholds: event.target.checked ? { base: "green", steps: [{ value: 80, color: "red" }] } : undefined })}
        />
        {thresholds ? (
          <div className="flex flex-col gap-2 pl-6">
            <div className="grid grid-cols-[7rem_minmax(0,1fr)] items-center gap-2">
              <span className="text-caption text-fg-muted">{t("builder.base")}</span>
              {colour(thresholds.base, (base) => onChange({ ...panel, thresholds: { ...thresholds, base } }), `${id}-base`, t("builder.baseColour"))}
            </div>
            {steps.map((step, index) => (
              <div key={index} className="grid grid-cols-[7rem_minmax(0,1fr)_auto] items-center gap-2">
                <BField id={`${id}-step-${index}`} label={t("builder.stepFrom")} srLabel>
                  <Input
                    id={`${id}-step-${index}`}
                    type="number"
                    inputMode="decimal"
                    value={String(step.value)}
                    onChange={(event) => onChange({ ...panel, thresholds: { ...thresholds, steps: steps.map((each, i) => (i === index ? { ...each, value: Number(event.target.value) } : each)) } })}
                  />
                </BField>
                {colour(step.color, (color) => onChange({ ...panel, thresholds: { ...thresholds, steps: steps.map((each, i) => (i === index ? { ...each, color } : each)) } }), `${id}-step-colour-${index}`, t("builder.stepColour"))}
                <Button size="sm" variant="ghost" aria-label={t("builder.removeStep")} title={t("builder.removeStep")} onClick={() => onChange({ ...panel, thresholds: { ...thresholds, steps: steps.filter((_, i) => i !== index) } })}>
                  <Icon name="trash" className="size-4" />
                </Button>
              </div>
            ))}
            {steps.length < 8 ? (
              <div>
                <Button size="sm" variant="secondary" onClick={() => onChange({ ...panel, thresholds: { ...thresholds, steps: [...steps, { value: (steps[steps.length - 1]?.value ?? 0) + 10, color: "red" }] } })}>
                  <Icon name="plus" className="size-4" />
                  {t("builder.addStep")}
                </Button>
              </div>
            ) : null}
          </div>
        ) : null}
      </div>
    </fieldset>
  );
}

/** A text panel's words: Markdown the dashboard shows as plain text in the Portal. */
export function TextEditor({ panel, onChange }: { panel: BoardPanel; onChange: (panel: BoardPanel) => void }): JSX.Element {
  const { t } = useTranslation();
  const id = `builder-panel-${panel.id}-content`;
  const content = panel.text?.content ?? "";
  return (
    <BField id={id} label={t("builder.field.content")} help={t("builder.hint.content", { max: MAX_CONTENT, left: MAX_CONTENT - content.length })} required>
      <Textarea id={id} rows={6} value={content} maxLength={MAX_CONTENT} onChange={(event) => onChange({ ...panel, text: { content: event.target.value } })} />
    </BField>
  );
}

/** A number field that may be left empty: empty is no value. */
function OptionalNumber({ id, label, help, value, onChange }: { id: string; label: string; help: string; value: number | null | undefined; onChange: (value: number | undefined) => void }): JSX.Element {
  return (
    <BField id={id} label={label} help={help}>
      <Input id={id} type="number" inputMode="decimal" value={value === undefined || value === null ? "" : String(value)} onChange={(event) => onChange(event.target.value === "" ? undefined : Number(event.target.value))} />
    </BField>
  );
}

/** The kind's own choices: how a bar gauge fills, a pie chart's shape, what names a bar chart's bars. */
export function KindOptions({ project, panel, onChange }: { project: string; panel: BoardPanel; onChange: (panel: BoardPanel) => void }): JSX.Element | null {
  const { t } = useTranslation();
  const id = `builder-panel-${panel.id}`;
  const choices = useChoices(project, panel.endpoint ?? "", panel.type ?? "");
  if (panel.kind === "bargauge") {
    const own = panel.bargauge ?? { displayMode: "gradient", orientation: "auto" };
    return (
      <div className="grid gap-3 sm:grid-cols-2 [&>*]:min-w-0">
        <BField id={`${id}-displayMode`} label={t("builder.field.displayMode")} help={t("builder.hint.displayMode")}>
          <Select id={`${id}-displayMode`} value={own.displayMode ?? "gradient"} onChange={(event) => onChange({ ...panel, bargauge: { ...own, displayMode: event.target.value as typeof DISPLAY_MODES[number] } })}>
            {DISPLAY_MODES.map((mode) => (
              <option key={mode} value={mode}>
                {t(`builder.displayMode.${mode}`)}
              </option>
            ))}
          </Select>
        </BField>
        <BField id={`${id}-orientation`} label={t("builder.field.orientation")} help={t("builder.hint.orientation")}>
          <Select id={`${id}-orientation`} value={own.orientation ?? "auto"} onChange={(event) => onChange({ ...panel, bargauge: { ...own, orientation: event.target.value as typeof ORIENTATIONS[number] } })}>
            {ORIENTATIONS.map((orientation) => (
              <option key={orientation} value={orientation}>
                {t(`builder.orientation.${orientation}`)}
              </option>
            ))}
          </Select>
        </BField>
      </div>
    );
  }
  if (panel.kind === "piechart") {
    return (
      <BField id={`${id}-pieType`} label={t("builder.field.pieType")} help={t("builder.hint.pieType")}>
        <Select id={`${id}-pieType`} value={panel.piechart?.pieType ?? "pie"} onChange={(event) => onChange({ ...panel, piechart: { pieType: event.target.value as typeof PIE_TYPES[number] } })}>
          {PIE_TYPES.map((pie) => (
            <option key={pie} value={pie}>
              {t(`builder.pieType.${pie}`)}
            </option>
          ))}
        </Select>
      </BField>
    );
  }
  if (panel.kind === "barchart") {
    const names = (choices.data?.attributes ?? []).filter(isText);
    const xField = panel.barchart?.xField ?? "";
    return (
      <BField id={`${id}-xField`} label={t("builder.field.xField")} help={t("builder.hint.xField")} required>
        <Select id={`${id}-xField`} value={xField} disabled={!panel.type} title={panel.type ? undefined : t("builder.chooseTypeFirst")} onChange={(event) => onChange({ ...panel, barchart: { xField: event.target.value } })}>
          <option value="">{t("builder.choose")}</option>
          {names.map((option) => (
            <option key={option.name} value={option.name}>
              {option.name}
            </option>
          ))}
          {xField && !names.some((option) => option.name === xField) ? <option value={xField}>{xField}</option> : null}
        </Select>
      </BField>
    );
  }
  return null;
}

/** The panel's place in numbers, for a keyboard or a phone. */
export function PositionEditor({ panel, onPlace }: { panel: BoardPanel; onPlace: (grid: Grid) => void }): JSX.Element {
  const { t } = useTranslation();
  const id = `builder-panel-${panel.id}`;
  const fields: { key: keyof Grid; max: number }[] = [
    { key: "x", max: COLUMNS - 1 },
    { key: "y", max: 500 },
    { key: "w", max: COLUMNS },
    { key: "h", max: 40 },
  ];
  return (
    <fieldset className="flex flex-col gap-1">
      <legend className="text-body font-medium text-fg">{t("builder.field.position")}</legend>
      <p className="text-caption text-fg-muted">{t("builder.positionHelp")}</p>
      <div className="grid grid-cols-2 gap-3 sm:grid-cols-4 [&>*]:min-w-0">
        {fields.map(({ key, max }) => (
          <BField key={key} id={`${id}-${key}`} label={t(`builder.grid.${key}`)}>
            <Input id={`${id}-${key}`} type="number" inputMode="numeric" min={0} max={max} value={String(panel.grid[key])} onChange={(event) => onPlace(clamp({ ...panel.grid, [key]: Number(event.target.value) }))} />
          </BField>
        ))}
      </div>
    </fieldset>
  );
}

/** A preview's answer: the panel as the dashboard page draws it, and its frames or why there are none. */
interface Preview {
  panel: PanelDefinition;
  frames: Frame[];
  error?: string;
  accessDenied?: string;
}

/** One panel run as the person, through the Portal, from the unsaved choices. */
export function PanelPreviewCard({ project, board, panel }: { project: string; board: Board; panel: BoardPanel }): JSX.Element {
  const { t, i18n } = useTranslation();
  const [now] = useState(() => Date.now());
  const run = useMutation({
    mutationFn: async (): Promise<Preview> => {
      const answer = await unwrap(await api.POST("/api/v1/projects/{project}/grafana/builder/preview", { params: { path: { project } }, body: { board, panel: panel.id } }));
      return { panel: answer.panel as unknown as PanelDefinition, frames: (answer.frames ?? []) as unknown as Frame[], error: answer.error ?? undefined, accessDenied: answer.accessDenied ?? undefined };
    },
  });
  const need = missing(panel);
  const range = panelRange({}, { from: board.from ?? "now-6h", to: board.to ?? "now" }, now);
  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-2">
        <Button size="sm" variant="secondary" disabled={need.length > 0 || run.isPending} disabledReason={need.length ? t(need[0]) : t("builder.previewing")} onClick={() => run.mutate()}>
          <Icon name="refresh" className="size-4" />
          {t("builder.preview")}
        </Button>
        <span className="text-caption text-fg-muted">{t("builder.previewHelp")}</span>
      </div>
      {run.isError ? (
        <Alert tone="danger" title={t("builder.previewFailedTitle")}>
          {lineInWords(reasonOf(run.error), board, t)}
        </Alert>
      ) : null}
      {run.data ? (
        <PanelView
          panel={run.data.panel}
          data={{ frames: run.data.frames, error: run.data.error, accessDenied: run.data.accessDenied }}
          from={range.from}
          to={range.to}
          locale={i18n.resolvedLanguage ?? "en"}
          height={Math.max(140, panel.grid.h * ROW_PX - 68)}
          headingLevel="h3"
        />
      ) : null}
    </div>
  );
}

/** The chosen panel's form: kind, data, filters, display, position, preview and removal. */
export function PanelEditor({ project, board, panel, start, onChange, onPlace, onRemove }: { project: string; board: Board; panel: BoardPanel; start: Start; onChange: (panel: BoardPanel) => void; onPlace: (grid: Grid) => void; onRemove: () => void }): JSX.Element {
  const { t } = useTranslation();
  const [asking, setAsking] = useState(false);
  const id = `builder-panel-${panel.id}`;
  const aggregated = (panel.aggregation ?? "none") !== "none";
  return (
    <Card as="section" aria-labelledby={`${id}-heading`} className="flex flex-col gap-4">
      <CardHeader as="h2" titleId={`${id}-heading`} title={t("builder.panelTitle", { title: panel.title.trim() || t("builder.untitledPanel") })} />
      <RadioGroup<PanelKind>
        name={`${id}-kind`}
        legend={t("builder.field.kind")}
        layout="row"
        value={panel.kind}
        options={PANEL_KINDS.map((kind) => ({ value: kind, label: t(`builder.kind.${kind}`) }))}
        onChange={(kind) => onChange(withKind(panel, kind))}
      />
      <p className="-mt-2 text-caption text-fg-muted">{t(`builder.kindHint.${panel.kind}`)}</p>
      <BField id={`${id}-title`} label={t("builder.field.panelTitle")} help={t("builder.hint.panelTitle")} required>
        <Input id={`${id}-title`} value={panel.title} maxLength={120} onChange={(event) => onChange({ ...panel, title: event.target.value })} />
      </BField>
      <BField id={`${id}-description`} label={t("builder.field.description")} help={t("builder.hint.panelDescription")}>
        <Textarea id={`${id}-description`} rows={2} value={panel.description ?? ""} maxLength={500} onChange={(event) => onChange({ ...panel, description: event.target.value })} />
      </BField>
      {panel.kind === "text" ? (
        <TextEditor panel={panel} onChange={onChange} />
      ) : (
      <>
      <SourcePicker
        project={project}
        id={id}
        value={{ endpoint: panel.endpoint ?? "", type: panel.type ?? "", attribute: panel.attribute ?? "" }}
        onChange={(source) => onChange({ ...panel, ...source, filters: source.type === panel.type ? panel.filters : [], barchart: panel.barchart && source.type !== panel.type ? { xField: "" } : panel.barchart })}
        attributesOf={(options) => drawable(panel.kind, options.attributes)}
      />
      <KindOptions project={project} panel={panel} onChange={onChange} />
      {currentOnly(panel.kind) ? null : (
        <div className="grid gap-3 sm:grid-cols-2 [&>*]:min-w-0">
          <BField id={`${id}-aggregation`} label={t("builder.field.aggregation")} help={t(AGGREGATION_HINTS[panel.kind])}>
            <Select id={`${id}-aggregation`} value={panel.aggregation ?? "none"} onChange={(event) => onChange({ ...panel, aggregation: event.target.value as BoardPanel["aggregation"], interval: event.target.value === "none" ? "auto" : panel.interval })}>
              {AGGREGATIONS.map((aggregation) => (
                <option key={aggregation} value={aggregation}>
                  {t(`builder.aggregation.${aggregation}`)}
                </option>
              ))}
            </Select>
          </BField>
          <BField id={`${id}-interval`} label={t("builder.field.interval")} help={aggregated ? t("builder.hint.interval") : t("builder.intervalNeedsAggregation")}>
            <Select id={`${id}-interval`} value={panel.interval ?? "auto"} disabled={!aggregated} title={aggregated ? undefined : t("builder.intervalNeedsAggregation")} onChange={(event) => onChange({ ...panel, interval: event.target.value as BoardPanel["interval"] })}>
              {INTERVALS.map((interval) => (
                <option key={interval} value={interval}>
                  {t(`builder.interval.${interval}`)}
                </option>
              ))}
            </Select>
          </BField>
        </div>
      )}
      <FiltersEditor project={project} panel={panel} variables={board.variables ?? []} onChange={(filters) => onChange({ ...panel, filters })} />
      <DisplayEditor panel={panel} start={start} onChange={onChange} />
      </>
      )}
      <PositionEditor panel={panel} onPlace={onPlace} />
      <PanelPreviewCard project={project} board={board} panel={panel} />
      <div className="flex flex-wrap items-center gap-2 border-t border-border pt-3">
        {asking ? (
          <>
            <span className="text-body text-fg">{t("builder.removePanelAsk")}</span>
            <Button size="sm" variant="danger" onClick={onRemove}>
              {t("builder.removePanel")}
            </Button>
            <Button size="sm" variant="ghost" onClick={() => setAsking(false)}>
              {t("builder.keep")}
            </Button>
          </>
        ) : (
          <Button size="sm" variant="ghost" onClick={() => setAsking(true)}>
            <Icon name="trash" className="size-4" />
            {t("builder.removePanel")}
          </Button>
        )}
      </div>
    </Card>
  );
}

/** Why a save did not go through: each refused field in the form's words, or the conflict. */
export function SaveProblem({ error, board, onReload, onGo }: { error: unknown; board: Board; onReload?: () => void; onGo?: (path: string) => void }): JSX.Element {
  const { t } = useTranslation();
  if (error instanceof ApiError && error.status === 409) {
    return (
      <Alert
        tone="warning"
        title={t("builder.conflictTitle")}
        actions={
          onReload ? (
            <Button size="sm" variant="secondary" onClick={onReload}>
              <Icon name="refresh" className="size-4" />
              {t("builder.reload")}
            </Button>
          ) : null
        }
      >
        {t("builder.conflictLead")}
      </Alert>
    );
  }
  const lines = error instanceof ApiError ? refusals(error.problem?.errors, error.problem?.detail ?? "") : [];
  return (
    <Alert tone="danger" title={t("builder.saveFailedTitle")}>
      {lines.length ? (
        <ul className="list-disc pl-5">
          {lines.map((line, index) => (
            <li key={index}>
              {onGo && fieldId(line.path, board) ? (
                <Button size="md" variant="ghost" className="h-auto min-h-0 px-0 py-0 align-baseline font-medium underline" onClick={() => onGo(line.path)}>
                  {`${fieldWords(line.path, board, t)}:`}
                </Button>
              ) : (
                <span className="font-medium">{`${fieldWords(line.path, board, t)}:`}</span>
              )}{` ${line.message}`}
            </li>
          ))}
        </ul>
      ) : (
        reasonOf(error)
      )}
    </Alert>
  );
}

/** The page of one person who may not build here: what they may do instead. */
function NotAllowed({ project }: { project: string }): JSX.Element {
  const { t } = useTranslation();
  return (
    <div className="flex flex-col gap-section">
      <PageHeader title={t("builder.readOnlyHeading")} description={t("builder.readOnlyLead")} />
      <EmptyState
        icon="lock"
        title={t("builder.readOnlyTitle")}
        description={t("builder.readOnly")}
        action={
          <Link to="/projects/$project/grafana" params={{ project }} className={buttonClass("secondary", "sm")}>
            {t("builder.backToDashboards")}
          </Link>
        }
      />
    </div>
  );
}

/** The builder's working copy: the board, the chosen panel, and the panel edits. */
function useBoard(sent: components["schemas"]["Board"] | undefined) {
  const [board, setBoard] = useState<Board>(sent ? boardOf(sent) : emptyBoard());
  const [selected, setSelected] = useState<number | null>(sent?.panels?.[0]?.id ?? null);
  // A board read again (the saved version after a conflict) replaces the working copy.
  const [loaded, setLoaded] = useState(sent);
  if (sent !== loaded) {
    setLoaded(sent);
    setBoard(sent ? boardOf(sent) : emptyBoard());
    setSelected(sent?.panels?.[0]?.id ?? null);
  }
  return {
    board,
    selected,
    setBoard,
    setSelected,
    setPanel: (panel: BoardPanel) => setBoard({ ...board, panels: board.panels.map((each) => (each.id === panel.id ? panel : each)) }),
    place: (id: number, grid: Grid) => setBoard({ ...board, panels: settle(board.panels, id, grid) }),
    add: (title: string) => {
      const panel = newPanel(board, "timeseries", title);
      setBoard({ ...board, panels: [...board.panels, panel] });
      setSelected(panel.id);
    },
    remove: (id: number) => {
      setBoard({ ...board, panels: board.panels.filter((panel) => panel.id !== id) });
      setSelected(null);
    },
  };
}

/** The builder: a new dashboard, or the next edit of one the builder saved (`uid`). */
export function DashboardBuilderPage({ project, uid }: { project: string; uid?: string }): JSX.Element {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const start = useQuery({
    queryKey: builderKey(project),
    queryFn: async () => unwrap(await api.GET("/api/v1/projects/{project}/grafana/builder", { params: { path: { project } } })),
  });
  const saved = useQuery({
    queryKey: [...builderKey(project), uid],
    enabled: Boolean(uid) && start.data?.mayEdit === true,
    queryFn: async () => unwrap(await api.GET("/api/v1/projects/{project}/grafana/builder/{uid}", { params: { path: { project, uid: uid ?? "" } } })),
  });
  const work = useBoard(saved.data?.board);
  const save = useMutation({
    mutationFn: async (board: Board) =>
      uid
        ? unwrap(await api.PUT("/api/v1/projects/{project}/grafana/builder/{uid}", { params: { path: { project, uid } }, body: { board, base: saved.data?.base ?? "" } }))
        : unwrap(await api.POST("/api/v1/projects/{project}/grafana/builder", { params: { path: { project } }, body: { board } })),
    onSuccess: async (answer) => {
      await queryClient.invalidateQueries({ queryKey: builderKey(project) });
      if (!uid) void navigate({ to: "/projects/$project/grafana/dashboards/$uid/edit", params: { project, uid: answer.uid } });
    },
  });
  if (start.isPending || (uid && start.data?.mayEdit && saved.isPending)) return <PageLoading label={t("builder.title")} />;
  if (start.isError) {
    return (
      <div className="flex flex-col gap-section">
        <PageHeader title={t("builder.title")} description={t("builder.lead")} />
        <PageFailed error={start.error} onRetry={() => void start.refetch()} />
      </div>
    );
  }
  if (!start.data.mayEdit || (saved.error instanceof ApiError && saved.error.status === 403)) return <NotAllowed project={project} />;
  if (saved.isError) {
    return (
      <div className="flex flex-col gap-section">
        <PageHeader title={t("builder.editTitle")} description={t("builder.lead")} />
        <PageFailed error={saved.error} onRetry={() => void saved.refetch()} />
      </div>
    );
  }
  const pending = saved.data?.pending ?? (save.data ? start.data.pending.find((each) => each.uid === save.data.uid) : undefined);
  const panel = work.board.panels.find((each) => each.id === work.selected);
  const ready = work.board.title.trim() !== "" && work.board.panels.length > 0 && work.board.panels.every((each) => missing(each).length === 0);
  const why = !work.board.title.trim() ? t("builder.need.boardTitle") : work.board.panels.length === 0 ? t("builder.need.panel") : t("builder.need.panels");
  const conflicted = save.error instanceof ApiError && save.error.status === 409;
  const refused = save.error instanceof ApiError && save.error.status === 422 ? refusals(save.error.problem?.errors, save.error.problem?.detail ?? "") : [];
  // The marks hold for the board that was refused; the first edit takes them away.
  const invalid: Record<string, string> = {};
  if (save.variables === work.board) {
    for (const line of refused) {
      const at = fieldId(line.path, work.board);
      if (at && !invalid[at]) invalid[at] = line.message;
    }
  }
  // Nothing changes while a change waits for approval or a save is on its way.
  const locked = Boolean(pending) || save.isPending || conflicted;
  const lockedWhy = pending ? t("builder.pendingSave") : save.isPending ? t("builder.saving") : t("builder.lockedConflict");
  /** Selects a panel; on a narrow screen its form sits below the grid, so the page goes there. */
  const select = (id: number, then?: string) => {
    work.setSelected(id);
    window.setTimeout(() => {
      const target = then ? document.getElementById(then) : null;
      if (target) {
        target.scrollIntoView?.({ block: "center" });
        target.focus();
      } else if (window.matchMedia?.("(max-width: 1279px)").matches) {
        document.getElementById(`builder-panel-${id}-heading`)?.scrollIntoView?.({ block: "start" });
      }
    }, 50);
  };
  /** Takes the person to the field a refusal names. */
  const go = (path: string) => {
    const target = fieldId(path, work.board);
    const index = /^panels\[(\d+)\]/.exec(path);
    const id = index ? work.board.panels[Number(index[1])]?.id : undefined;
    if (id !== undefined) select(id, target);
    else if (target) window.setTimeout(() => document.getElementById(target)?.focus(), 0);
  };
  return (
    <Invalid.Provider value={invalid}>
    <div className="flex flex-col gap-section">
      <PageHeader
        title={uid ? t("builder.editTitle") : t("builder.title")}
        description={t("builder.lead")}
        titleAction={pending ? <Badge tone="warning">{t("builder.pending")}</Badge> : null}
        actions={
          <>
            <Link to="/projects/$project/grafana" params={{ project }} className={buttonClass("ghost", "sm")}>
              {t("builder.cancel")}
            </Link>
            <Button variant="primary" size="sm" disabled={!ready || locked} disabledReason={locked ? lockedWhy : why} onClick={() =>
                save.mutate(work.board, {
                  // The first refused field opens and takes the focus; the list above names the rest.
                  onError: (error) => {
                    const first = error instanceof ApiError && error.status === 422 ? refusals(error.problem?.errors)[0] : undefined;
                    if (first) go(first.path);
                  },
                })
              }>
              {save.isPending ? t("builder.saving") : t("builder.save")}
            </Button>
          </>
        }
      />
      {pending ? (
        <Alert tone="info" title={t("builder.pendingTitle")}>
          {t("builder.pendingLead")}{" "}
          <Link to="/projects/$project/approvals/$id" params={{ project, id: pending.change }} className="underline">
            {t("builder.openChange")}
          </Link>
        </Alert>
      ) : null}
      {save.isSuccess && !pending ? <Alert tone="success" title={t("builder.savedTitle")}>{t("builder.savedLead")}</Alert> : null}
      {save.isError ? <SaveProblem error={save.error} board={work.board} onReload={uid ? () => void saved.refetch().then(() => save.reset()) : undefined} onGo={go} /> : null}
      <fieldset disabled={locked} title={locked ? lockedWhy : undefined} className="contents">
        <legend className="sr-only">{t("builder.settings")}</legend>
        <BoardSettings board={work.board} start={start.data} onChange={work.setBoard} />
        <VariablesEditor project={project} board={work.board} onChange={work.setBoard} />
      </fieldset>
      <div className={`grid items-start gap-card [&>*]:min-w-0 ${work.board.panels.length ? "xl:grid-cols-[minmax(0,1fr)_28rem]" : ""}`}>
        <Card as="section" aria-labelledby="builder-panels">
          <CardHeader
            as="h2"
            titleId="builder-panels"
            title={t("builder.panels")}
            description={locked ? lockedWhy : t("builder.panelsLead")}
            className="mb-3"
            actions={
              work.board.panels.length < 24 && !locked ? (
                <Button size="sm" variant="secondary" onClick={() => work.add(t("builder.newPanelTitle", { n: work.board.panels.length + 1 }))}>
                  <Icon name="plus" className="size-4" />
                  {t("builder.addPanel")}
                </Button>
              ) : null
            }
          />
          {work.board.panels.length === 0 ? (
            <EmptyState bare icon="dashboards" title={t("builder.emptyTitle")} description={t("builder.empty")} />
          ) : (
            <BuilderCanvas panels={work.board.panels} selected={work.selected} onSelect={(id) => select(id)} onPlace={work.place} locked={locked} />
          )}
        </Card>
        {panel ? (
          <fieldset disabled={locked} title={locked ? lockedWhy : undefined} className="min-w-0">
          <legend className="sr-only">{t("builder.panelTitle", { title: panel.title })}</legend>
          <PanelEditor
            key={panel.id}
            project={project}
            board={work.board}
            panel={panel}
            start={start.data}
            onChange={work.setPanel}
            onPlace={(grid) => work.place(panel.id, grid)}
            onRemove={() => work.remove(panel.id)}
          />
          </fieldset>
        ) : work.board.panels.length ? (
          <Card>
            <EmptyState bare icon="pencil" title={t("builder.chooseTitle")} description={t("builder.choosePanel")} />
          </Card>
        ) : null}
      </div>
    </div>
    </Invalid.Provider>
  );
}
