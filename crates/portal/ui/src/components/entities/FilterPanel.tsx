/**
 * The grid's filter panel, as Baserow has it (T-4883): conditions per field with the operators the
 * field's kind allows, joined by AND or OR, and groups of their own one level down. The panel only
 * builds the condition tree; `queryFromGroup` turns it into the NGSI-LD `q` the endpoint runs, so a
 * filter never narrows only the loaded page.
 */
import { useId } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { opsForKind } from "@joinedcontext/sdk";
import type { ColumnFilter, FilterColumn, FilterCondition, FilterGroup, FilterOp } from "@joinedcontext/sdk";
import { Button, Input, Select } from "../ui";

/** Conditions a view keeps at most, groups' members included: a longer list is a model question. */
export const MAX_CONDITIONS = 20;

export const EMPTY_GROUP: FilterGroup = { join: "and", conditions: [] };

/** How many conditions a group holds, its groups' members included. */
export function conditionCount(group: FilterGroup): number {
  return group.conditions.reduce((sum, item) => sum + ("conditions" in item ? conditionCount(item) : 1), 0);
}

/**
 * A condition tree read back from a saved view, `null` when it is not one: the view's settings are
 * a map anyone with the view's edit right wrote, so the shape is checked before it is used.
 */
export function readGroup(raw: unknown, depth = 0): FilterGroup | null {
  if (typeof raw !== "object" || raw === null || depth > 1) return null;
  const { join, conditions } = raw as { join?: unknown; conditions?: unknown };
  if ((join !== "and" && join !== "or") || !Array.isArray(conditions)) return null;
  const kept: FilterGroup["conditions"] = [];
  for (const item of conditions) {
    if (typeof item === "object" && item !== null && "conditions" in item) {
      const nested = readGroup(item, depth + 1);
      if (nested) kept.push(nested);
      continue;
    }
    const { key, filter } = (item ?? {}) as { key?: unknown; filter?: Partial<ColumnFilter> };
    if (typeof key !== "string" || typeof filter !== "object" || filter === null || typeof filter.op !== "string") continue;
    kept.push({
      key,
      filter: {
        op: filter.op as FilterOp,
        value: typeof filter.value === "string" ? filter.value : "",
        ...(typeof filter.value2 === "string" ? { value2: filter.value2 } : {}),
        ...(Array.isArray(filter.values) ? { values: filter.values.filter((v): v is string => typeof v === "string") } : {}),
      },
    });
  }
  return { join, conditions: kept };
}

export interface FilterPanelProps {
  /** The columns the endpoint can be asked about, as the grid knows them. */
  columns: FilterColumn[];
  /** A column's title as the model gives it. */
  titleOf: (column: FilterColumn) => string;
  value: FilterGroup;
  onChange: (next: FilterGroup) => void;
}

function firstCondition(columns: FilterColumn[]): FilterCondition | null {
  const column = columns[0];
  const op = column ? opsForKind(column.kind)[0] : undefined;
  return column && op ? { key: column.key, filter: { op, value: "" } } : null;
}

export function FilterPanel({ columns, titleOf, value, onChange }: FilterPanelProps): JSX.Element {
  const { t } = useTranslation();
  const usable = columns.filter((column) => column.attr !== null && column.kind !== "id" && opsForKind(column.kind).length > 0);
  const full = conditionCount(value) >= MAX_CONDITIONS;
  const fresh = firstCondition(usable);

  return (
    <div className="flex flex-col gap-3" data-testid="filter-panel">
      {value.conditions.length === 0 ? <p className="text-body text-fg-muted">{t("spaces.toolbar.noFilters")}</p> : null}
      <Conditions columns={usable} titleOf={titleOf} group={value} onChange={onChange} nested={false} />
      <div className="flex flex-wrap gap-2">
        <Button
          size="sm"
          variant="secondary"
          disabled={full || !fresh}
          disabledReason={full ? t("spaces.toolbar.filterFull", { n: MAX_CONDITIONS }) : t("spaces.toolbar.noFilterable")}
          onClick={() => fresh && onChange({ ...value, conditions: [...value.conditions, fresh] })}
        >
          {t("spaces.toolbar.addCondition")}
        </Button>
        <Button
          size="sm"
          variant="ghost"
          disabled={full || !fresh}
          disabledReason={full ? t("spaces.toolbar.filterFull", { n: MAX_CONDITIONS }) : t("spaces.toolbar.noFilterable")}
          onClick={() => fresh && onChange({ ...value, conditions: [...value.conditions, { join: "or", conditions: [fresh] }] })}
        >
          {t("spaces.toolbar.addGroup")}
        </Button>
      </div>
    </div>
  );
}

function Conditions({
  columns,
  titleOf,
  group,
  onChange,
  nested,
}: {
  columns: FilterColumn[];
  titleOf: (column: FilterColumn) => string;
  group: FilterGroup;
  onChange: (next: FilterGroup) => void;
  nested: boolean;
}): JSX.Element {
  const { t } = useTranslation();
  const id = useId();
  const setItem = (index: number, next: FilterGroup["conditions"][number] | null) =>
    onChange({
      ...group,
      conditions: next === null ? group.conditions.filter((_, at) => at !== index) : group.conditions.map((item, at) => (at === index ? next : item)),
    });
  const fresh = firstCondition(columns);
  return (
    <ul className={nested ? "flex flex-col gap-2 rounded-md border border-border bg-surface-muted p-2" : "flex flex-col gap-2"}>
      {group.conditions.map((item, index) => (
        <li key={index} className="flex flex-wrap items-start gap-2">
          {/* The first line says "Where"; each one after it carries the group's AND/OR, set once for the group. */}
          {index === 0 ? (
            <span className="w-16 pt-2 text-caption text-fg-muted">{t("spaces.toolbar.where")}</span>
          ) : index === 1 ? (
            <Select
              aria-label={t("spaces.toolbar.join")}
              className="w-16"
              value={group.join}
              onChange={(e) => onChange({ ...group, join: e.target.value === "or" ? "or" : "and" })}
            >
              <option value="and">{t("spaces.toolbar.and")}</option>
              <option value="or">{t("spaces.toolbar.or")}</option>
            </Select>
          ) : (
            <span className="w-16 pt-2 text-caption text-fg-muted">{t(`spaces.toolbar.${group.join}`)}</span>
          )}
          {"conditions" in item ? (
            <div className="flex min-w-0 flex-1 flex-col gap-1">
              <Conditions columns={columns} titleOf={titleOf} group={item} onChange={(next) => setItem(index, next)} nested />
              <div className="flex gap-2">
                <Button
                  size="sm"
                  variant="ghost"
                  disabled={!fresh}
                  disabledReason={t("spaces.toolbar.noFilterable")}
                  onClick={() => fresh && setItem(index, { ...item, conditions: [...item.conditions, fresh] })}
                >
                  {t("spaces.toolbar.addCondition")}
                </Button>
                <Button size="sm" variant="ghost" onClick={() => setItem(index, null)}>
                  {t("spaces.toolbar.removeGroup")}
                </Button>
              </div>
            </div>
          ) : (
            <ConditionRow
              id={`${id}-${index}`}
              columns={columns}
              titleOf={titleOf}
              value={item}
              onChange={(next) => setItem(index, next)}
              onRemove={() => setItem(index, null)}
            />
          )}
        </li>
      ))}
    </ul>
  );
}

function ConditionRow({
  id,
  columns,
  titleOf,
  value,
  onChange,
  onRemove,
}: {
  id: string;
  columns: FilterColumn[];
  titleOf: (column: FilterColumn) => string;
  value: FilterCondition;
  onChange: (next: FilterCondition) => void;
  onRemove: () => void;
}): JSX.Element {
  const { t } = useTranslation();
  const column = columns.find((each) => each.key === value.key);
  const ops = column ? opsForKind(column.kind) : [];
  const { filter } = value;
  const set = (patch: Partial<ColumnFilter>) => onChange({ ...value, filter: { ...filter, ...patch } });
  const needs = filter.op === "empty" || filter.op === "present" ? 0 : filter.op === "between" ? 2 : 1;
  const inputType = column?.kind === "number" ? "number" : column?.kind === "date" ? "date" : "text";
  return (
    <div className="flex min-w-0 flex-1 flex-wrap items-start gap-2">
      <Select
        aria-label={t("spaces.toolbar.field")}
        className="w-40"
        value={value.key}
        onChange={(e) => {
          const next = columns.find((each) => each.key === e.target.value);
          if (next) onChange({ key: next.key, filter: { op: opsForKind(next.kind)[0] ?? "equals", value: "" } });
        }}
      >
        {/* A condition saved on a field this reader's grant no longer shows stays visible as itself. */}
        {column ? null : <option value={value.key}>{value.key}</option>}
        {columns.map((each) => (
          <option key={each.key} value={each.key}>
            {titleOf(each)}
          </option>
        ))}
      </Select>
      <Select aria-label={t("spaces.toolbar.operator")} className="w-36" value={filter.op} onChange={(e) => set({ op: e.target.value as FilterOp })}>
        {ops.map((op) => (
          <option key={op} value={op}>
            {t(`entityGrid.ops.${op}`, { defaultValue: op })}
          </option>
        ))}
      </Select>
      {filter.op === "anyOf" && column?.options ? (
        <Select
          id={`${id}-values`}
          aria-label={t("spaces.toolbar.value")}
          multiple
          className="min-w-40"
          value={filter.values ?? []}
          onChange={(e) => set({ values: Array.from(e.target.selectedOptions, (option) => option.value) })}
        >
          {column.options.map((option) => (
            <option key={option.value} value={option.value}>
              {option.title ?? option.value}
            </option>
          ))}
        </Select>
      ) : null}
      {filter.op !== "anyOf" && needs >= 1 ? (
        <div className="w-40 shrink-0">
        <Input
          aria-label={t("spaces.toolbar.value")}
          type={inputType}
          value={filter.value}
          onChange={(e) => set({ value: e.target.value })}
        />
        </div>
      ) : null}
      {needs === 2 ? (
        <div className="w-40 shrink-0">
        <Input
          aria-label={t("spaces.toolbar.upperValue")}
          type={inputType}
          value={filter.value2 ?? ""}
          onChange={(e) => set({ value2: e.target.value })}
        />
        </div>
      ) : null}
      <Button size="sm" variant="ghost" aria-label={t("spaces.toolbar.removeCondition")} onClick={onRemove}>
        ✕
      </Button>
    </div>
  );
}

/** One condition in words, as its chip says it: "Available bikes is greater than 3" (T-6161). */
export function conditionText(
  item: FilterCondition | FilterGroup,
  columns: FilterColumn[],
  titleOf: (column: FilterColumn) => string,
  t: (key: string, options?: Record<string, unknown>) => string,
): string {
  if ("conditions" in item) {
    return t(item.join === "or" ? "spaces.toolbar.chipAny" : "spaces.toolbar.chipAll", { n: conditionCount(item) });
  }
  const column = columns.find((each) => each.key === item.key);
  const { op, value, value2, values } = item.filter;
  const said =
    op === "anyOf"
      ? (values ?? []).map((each) => column?.options?.find((option) => option.value === each)?.title ?? each).join(", ")
      : op === "between"
        ? `${value} – ${value2 ?? ""}`
        : op === "empty" || op === "present"
          ? ""
          : value;
  return [column ? titleOf(column) : item.key, t(`entityGrid.ops.${op}`, { defaultValue: op }), said].filter((part) => part !== "").join(" ");
}

/**
 * The filters in force as removable chips under the toolbar (T-6161): each top-level condition or
 * group one chip, its ✕ drops it and the read follows, the panel behind Filter still edits them.
 */
export function FilterChips({ columns, titleOf, value, onChange }: FilterPanelProps): JSX.Element | null {
  const { t } = useTranslation();
  if (value.conditions.length === 0) return null;
  return (
    <ul aria-label={t("spaces.toolbar.activeFilters")} className="flex flex-wrap items-center gap-1" data-testid="filter-chips">
      {value.conditions.map((item, index) => {
        const text = conditionText(item, columns, titleOf, t);
        return (
          <li key={index} className="inline-flex max-w-full items-center gap-1 text-caption">
            {/* How the chips meet, said between them: two chips side by side otherwise read as AND. */}
            {index > 0 ? <span className="px-1 font-medium text-fg">{t(`spaces.toolbar.${value.join}`)}</span> : null}
            <span className="inline-flex min-w-0 items-center gap-1 rounded-full border border-border bg-surface-muted pl-3 text-fg">
              <span className="truncate" title={text}>{text}</span>
              <Button
                size="sm"
                variant="ghost"
                aria-label={t("spaces.toolbar.removeFilter", { name: text })}
                onClick={() => onChange({ ...value, conditions: value.conditions.filter((_, at) => at !== index) })}
              >
                ✕
              </Button>
            </span>
          </li>
        );
      })}
      {value.conditions.length > 1 ? (
        <li>
          <Button size="sm" variant="secondary" onClick={() => onChange({ ...value, conditions: [] })}>
            {t("spaces.toolbar.clearFilters")}
          </Button>
        </li>
      ) : null}
    </ul>
  );
}
