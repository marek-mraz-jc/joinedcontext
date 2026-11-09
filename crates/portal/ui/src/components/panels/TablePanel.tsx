/**
 * A Grafana `table` panel drawn by the SDK grid (the same component the Endpoint data view and
 * generated Apps use), over the panel's frames held in memory: its sort, paging and column
 * widths, with each cell in its field's unit, mapping and threshold colour (`custom.cellOptions`).
 */
import { useMemo, useState } from "react";
import type { JSX, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { clsx } from "clsx";
import { EntityGrid, fixtureSource, parseGridConfig } from "@joinedcontext/sdk/grid";
import type { EntityGridProps } from "@joinedcontext/sdk/grid";
import { gridLabels } from "../entities/PortalEntityGrid";
import { InlineError, Select } from "../ui";
import type { Frame } from "./frames";
import { frameName, rowsOf } from "./frames";
import type { FieldConfigSource, FieldSettings } from "./fieldConfig";
import { colorOf, DEFAULT_THRESHOLDS, displayValue, settingsFor, stepFor } from "./fieldConfig";
import { Tinted } from "./paint";

/** The type the in-memory rows carry, which no Endpoint has. */
const ROW_TYPE = "PanelRow";
/** The narrowest column the grid's head fits its name, kind mark and menu into, in px. */
const MIN_WIDTH = 128;

export interface TableColumn {
  attr: string;
  label: string;
  settings: FieldSettings;
}

/**
 * A column's attribute name in the grid: its field name as a `q` term (`PM2.5` is `PM2_5`), so the
 * grid's filter and its example read the column a person sees; `c<index>` when nothing is left.
 */
export function attrOf(name: string, index: number, taken: Set<string>): string {
  let attr = name.normalize("NFKD").replace(/[^\w]+/g, "_").replace(/^_+|_+$/g, "");
  if (!/^[A-Za-z_]/.test(attr)) attr = attr ? `c_${attr}` : `c${index}`;
  // The grid keeps `id` and `type` for the entity itself.
  if (attr === "id" || attr === "type" || taken.has(attr)) attr = `${attr}_${index}`;
  taken.add(attr);
  return attr;
}

/** Grafana's saved table sort (`options.sortBy`): a column by its display name, descending or not. */
export type SortBy = { displayName?: string; desc?: boolean }[];

/** Rows in Grafana's saved order: numbers by value, text by locale, an empty cell last either way. */
function sorted(rows: Record<string, unknown>[], key: string | undefined, desc: boolean): Record<string, unknown>[] {
  if (key === undefined) return rows;
  const order = desc ? -1 : 1;
  return [...rows].sort((a, b) => {
    const x = a[key];
    const y = b[key];
    if (x === null || x === undefined) return y === null || y === undefined ? 0 : 1;
    if (y === null || y === undefined) return -1;
    if (typeof x === "number" && typeof y === "number") return order * (x - y);
    return order * String(x).localeCompare(String(y));
  });
}

/** The frames as grid entities, one attribute per column, and the columns that name them. */
export function tableOf(frames: Frame[], fieldConfig: FieldConfigSource | undefined, sortBy?: SortBy): { columns: TableColumn[]; entities: Record<string, unknown>[] } {
  const { columns: fields, names, rows: read } = rowsOf(frames);
  const taken = new Set<string>();
  const columns = names.map((name, index) => {
    const settings = settingsFor(fieldConfig, fields[index]);
    return { attr: attrOf(name, index, taken), label: settings.displayName ?? name, settings, hidden: settings.custom.hidden === true };
  });
  // Grafana sorts by its first sortBy entry, named by the column's display name.
  const first = sortBy?.[0];
  const at = first?.displayName === undefined ? -1 : columns.findIndex((column) => column.label === first.displayName || names[columns.indexOf(column)] === first.displayName);
  const rows = sorted(read, at < 0 ? undefined : names[at], first?.desc === true);
  const entities = rows.map((row, at) => {
    const entity: Record<string, unknown> = { id: `urn:ngsi-ld:${ROW_TYPE}:${at + 1}`, type: ROW_TYPE };
    names.forEach((name, index) => {
      const value = row[name];
      if (value !== null && value !== undefined) entity[columns[index].attr] = { type: "Property", value };
    });
    return entity;
  });
  return { columns: columns.filter((column) => !column.hidden), entities };
}

/** One cell as Grafana draws it: mapped or in its unit, coloured as `cellOptions.type` says. */
export function TableCellValue({ settings, value, locale }: { settings: FieldSettings; value: unknown; locale: string }): JSX.Element {
  const shown = displayValue(settings, value, locale);
  const cellType = (settings.custom.cellOptions as { type?: string } | undefined)?.type ?? (settings.custom.displayMode as string | undefined);
  const coloured = cellType === "color-text" || cellType === "color-background" || cellType === "color-background-solid";
  // A coloured cell takes its field's threshold colour, Grafana's defaults when none is saved; text the base step's.
  const thresholds = settings.thresholds ?? (coloured ? DEFAULT_THRESHOLDS : undefined);
  const step = typeof value === "number" ? stepFor(thresholds, value, settings.min, settings.max) : coloured ? thresholds?.steps[0] : undefined;
  const color = shown.color ?? (step ? colorOf(step.color) : undefined);
  // Grafana's `custom.align`: auto puts a number at the right edge, text at the left.
  const asked = settings.custom.align;
  const align = asked === "left" || asked === "center" || asked === "right" ? asked : typeof value === "number" ? "right" : "left";
  const aligned = align === "right" ? "text-right" : align === "center" ? "text-center" : undefined;
  if (color && (cellType === "color-background" || cellType === "color-background-solid")) {
    return (
      <Tinted color={color} background className="inline-block max-w-full truncate rounded-sm px-1.5 align-middle">
        {shown.text}
      </Tinted>
    );
  }
  if (color && (cellType === "color-text" || shown.color)) {
    return (
      <Tinted color={color} className={clsx("block truncate font-medium", aligned)}>
        {shown.text}
      </Tinted>
    );
  }
  // A long name stays in its own column: cut with an ellipsis, whole in the title.
  return (
    <span className={clsx("block truncate", aligned)} title={shown.text}>
      {shown.text}
    </span>
  );
}

export interface TablePanelProps {
  title: string;
  frames: Frame[];
  fieldConfig?: FieldConfigSource;
  options?: Record<string, unknown>;
  locale: string;
}

export function TablePanel({ title, frames, fieldConfig, options, locale }: TablePanelProps): JSX.Element {
  const { t } = useTranslation();
  // Several frames are one table each, picked below the grid, as Grafana's table panel does.
  const [picked, setPicked] = useState(0);
  const at = picked < frames.length ? picked : 0;
  const shown = useMemo(() => (frames.length > 1 ? [frames[at]] : frames), [frames, at]);
  const sortBy = Array.isArray(options?.sortBy) ? (options.sortBy as SortBy) : undefined;
  const table = useMemo(() => tableOf(shown, fieldConfig, sortBy), [shown, fieldConfig, sortBy]);
  const parsed = useMemo(
    () =>
      parseGridConfig({
        source: { kind: "fixture", name: "panel" },
        type: ROW_TYPE,
        density: "compact",
        pageSize: 50,
        columns: table.columns.map((column) => ({
          attr: column.attr,
          label: column.label,
          // Grafana's width holds the text alone; the grid's head adds its kind mark and menu.
          ...(typeof column.settings.custom.width === "number" ? { width: Math.max(MIN_WIDTH, column.settings.custom.width) } : {}),
        })),
      }),
    [table],
  );
  const source = useMemo(() => fixtureSource(table.entities, locale), [table, locale]);
  const renderers = useMemo(() => {
    const out: Record<string, NonNullable<EntityGridProps["renderers"]>[string]> = {};
    for (const column of table.columns) {
      out[column.attr] = (cell): ReactNode => {
        const one = Array.isArray(cell) ? cell[0] : cell;
        const value = one && "value" in one ? one.value : undefined;
        return <TableCellValue settings={column.settings} value={value} locale={locale} />;
      };
    }
    return out;
  }, [table, locale]);
  const labels = useMemo(() => ({ ...gridLabels(t), empty: t("dashboardView.noRows") }), [t]);
  if (!parsed.config) {
    return <InlineError>{parsed.findings.map((finding) => finding.message).join("; ")}</InlineError>;
  }
  return (
    <div aria-label={title} className="min-w-0">
      <EntityGrid key={at} config={parsed.config} source={source} labels={labels} renderers={renderers} hidden={["id", "entity:type"]} locale={locale} filterRow={false} queryBox={false} />
      {frames.length > 1 && (
        <Select className="mt-2 w-full" aria-label={t("dashboardView.frame", { title })} value={String(at)} onChange={(event) => setPicked(Number(event.target.value))}>
          {frames.map((frame, index) => (
            <option key={index} value={index}>
              {frameName(frame, index)}
            </option>
          ))}
        </Select>
      )}
    </div>
  );
}
