/**
 * The parts of the grid's one toolbar row (T-4883): a button that opens a panel under it, the row
 * sizes, and the footer's per-column aggregation, each counted by the endpoint over the whole
 * filtered set with the reader's session, never over the loaded page.
 */
import { useEffect, useRef } from "react";
import type { JSX, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { useQuery } from "@tanstack/react-query";
import { clsx } from "clsx";
import { andQ, cellText } from "@joinedcontext/sdk";
import type { EntitySource, RichCell } from "@joinedcontext/sdk";
import { Button, buttonClass, Select } from "../ui";
import { Icon } from "../ui/icons";

export const ROW_SIZES = ["small", "medium", "large"] as const;
export type RowSize = (typeof ROW_SIZES)[number];

export const AGGREGATIONS = ["none", "count", "filled", "empty", "percentFilled", "percentEmpty", "min", "max", "median", "earliest", "latest"] as const;
export type Aggregation = (typeof AGGREGATIONS)[number];

/**
 * The aggregations read off the ordered set rather than counted (T-5188), by the column kind they
 * fit: the first row of `orderBy=attr` (or `attr;desc`) among the rows that hold the attribute,
 * and the middle one at its offset. Sum, average and distinct values need the broker's own
 * aggregate and are not offered.
 */
const ORDERED: Record<string, Partial<Record<Aggregation, true>>> = {
  number: { min: true, max: true, median: true },
  date: { earliest: true, latest: true },
};

/** The aggregations a column offers: the counts always, the ordered ones by its kind where the source orders. */
export function aggregationsFor(kind: string | undefined, orders: boolean): Aggregation[] {
  return AGGREGATIONS.filter((each) => !ORDERED_ANY.has(each) || (orders && kind !== undefined && ORDERED[kind]?.[each] === true));
}

const ORDERED_ANY = new Set<Aggregation>(["min", "max", "median", "earliest", "latest"]);

/** The value an ordered aggregation reads: the row's offset and order, given how many rows hold the attribute. */
export function orderedRead(value: Aggregation, filled: number): { desc: boolean; offset: number; limit: number } | null {
  if (filled <= 0) return null;
  if (value === "min" || value === "earliest") return { desc: false, offset: 0, limit: 1 };
  if (value === "max" || value === "latest") return { desc: true, offset: 0, limit: 1 };
  if (value === "median") return { desc: false, offset: Math.floor((filled - 1) / 2), limit: filled % 2 === 0 ? 2 : 1 };
  return null;
}

/**
 * A toolbar button with its panel: a `<details>` element, so the browser gives it a keyboard, a
 * role and a state a screen reader announces; Escape and a click outside close it. On a phone the
 * panel is a sheet at the bottom of the screen rather than a box running off its edge.
 */
export function ToolbarPopover({
  label,
  count,
  wide,
  end,
  outlined,
  tall,
  anchored,
  inPlace,
  closeLabel,
  children,
}: {
  label: string;
  /** A sheet over the page: names a button that closes it, for a person who cannot tap beside it. */
  closeLabel?: string;
  /** One line of a folded menu: the panel opens under its line, inside the menu. */
  inPlace?: boolean;
  /** Under its button on a phone too, for a row's first control: no bottom sheet over the toolbar. */
  anchored?: boolean;
  /** Drawn as a button even with nothing set: outside a toolbar row a ghost summary reads as text. */
  outlined?: boolean;
  /** How many settings the panel holds, shown beside the label so a set filter is visible. */
  count?: number;
  wide?: boolean;
  /** At the row's right end: the panel opens leftwards so it stays on the screen. */
  end?: boolean;
  /** Reading text or a folded menu: the panel grows to its content on a wide screen, to most of a phone's. */
  tall?: boolean;
  children: ReactNode;
}): JSX.Element {
  const ref = useRef<HTMLDetailsElement>(null);
  useEffect(() => {
    // Inside a menu `name` keeps one tool open: closing it on the press would move Close from under the finger.
    if (inPlace) return;
    const onDown = (event: PointerEvent) => {
      const box = ref.current;
      if (box?.open && !box.contains(event.target as Node)) box.open = false;
    };
    document.addEventListener("pointerdown", onDown);
    return () => document.removeEventListener("pointerdown", onDown);
  }, [inPlace]);
  return (
    <details
      ref={ref}
      name={inPlace ? "toolbar-tool" : undefined}
      className="group/tool relative"
      onKeyDown={(event) => {
        if (event.key === "Escape" && ref.current?.open) {
          event.stopPropagation();
          ref.current.open = false;
          ref.current.querySelector("summary")?.focus();
        }
      }}
    >
      <summary
        className={buttonClass(
          count || outlined ? "secondary" : "ghost",
          "sm",
          clsx(
            "cursor-pointer list-none [&::-webkit-details-marker]:hidden",
            count ? "border-primary text-primary-soft-fg" : undefined,
            inPlace ? "h-control-lg w-full justify-between" : undefined,
          ),
        )}
      >
        {label}
        {count ? <span className="ml-1 rounded-full bg-primary-soft px-1.5 text-caption">{count}</span> : null}
        {inPlace ? <Icon name="chevronDown" className="ml-auto size-4 group-open/tool:rotate-180" /> : null}
      </summary>
      <div
        className={
          inPlace
            ? "border-l-2 border-border py-2 pl-3 text-body"
            : clsx(
                "z-30 overflow-auto rounded-lg border border-border bg-surface-raised p-3 text-body shadow-2",
                tall ? "max-h-sheet sm:max-h-none" : "max-h-96",
                anchored ? "absolute left-0 mt-1 max-sm:w-max max-sm:max-w-xs" : "max-sm:fixed max-sm:inset-x-4 max-sm:bottom-4",
                end ? "sm:absolute sm:right-0 sm:mt-1" : "sm:absolute sm:left-0 sm:mt-1",
                wide ? "sm:w-max sm:max-w-2xl" : "sm:w-72",
              )
        }
      >
        {closeLabel ? (
          <div className="sticky -top-3 z-10 -mx-3 -mt-3 mb-1 flex justify-end border-b border-border bg-surface-raised p-2">
            <Button
              size="sm"
              variant="secondary"
              onClick={() => {
                if (ref.current) ref.current.open = false;
                ref.current?.querySelector("summary")?.focus();
              }}
            >
              {closeLabel}
            </Button>
          </div>
        ) : null}
        {children}
      </div>
    </details>
  );
}

/** The share of `part` in `whole` as a person reads it, `null` without a whole to divide by. */
export function percent(part: number, whole: number): string | null {
  if (whole <= 0) return null;
  return `${Math.round((part / whole) * 1000) / 10} %`;
}

/**
 * The footer text of `value` from the whole count, the filled count and an ordered read; `null`
 * while a count it needs is missing.
 */
export function footerText(value: Aggregation, whole: number | null, some: number | null, picked: string | null): string | null {
  if (ORDERED_ANY.has(value)) return picked;
  if (value === "count") return whole === null ? null : String(whole);
  if (value === "filled") return some === null ? null : String(some);
  if (whole === null || some === null) return null;
  if (value === "empty") return String(whole - some);
  if (value === "percentFilled") return percent(some, whole);
  if (value === "percentEmpty") return percent(whole - some, whole);
  return null;
}

/** Whether any query the footer runs failed or still waits; a disabled query counts for neither. */
function footerState(runs: { on: boolean; isError: boolean; isPending: boolean }[]): { failed: boolean; waiting: boolean } {
  const on = runs.filter((run) => run.on);
  return { failed: on.some((run) => run.isError), waiting: on.some((run) => run.isPending) };
}

/**
 * The counts and the ordered read behind one footer, counted by the endpoint (`count=true`) over
 * the view's whole query. A filled count is the query with the attribute present (`q=attr`), so a
 * reader's grant narrows it exactly as it narrows the rows.
 */
function useFooterValue(scope: string, source: EntitySource, type: string, attr: string, q: string | undefined, value: Aggregation) {
  const ordered = ORDERED_ANY.has(value);
  const needsTotal = value !== "none" && value !== "filled" && !ordered;
  const needsFilled = value !== "none" && value !== "count";
  const count = async (asked: string | undefined) => (await source.query({ type, q: asked }, { offset: 0, limit: 1 })).total ?? null;
  const total = useQuery({
    queryKey: ["grid-aggregate", scope, type, "total", q ?? ""],
    enabled: needsTotal,
    retry: false,
    staleTime: 30_000,
    queryFn: () => count(q),
  });
  const filled = useQuery({
    queryKey: ["grid-aggregate", scope, type, attr, q ?? ""],
    enabled: needsFilled,
    retry: false,
    staleTime: 30_000,
    queryFn: () => count(andQ(q, attr)),
  });
  const some = filled.data ?? null;
  const read = ordered && some !== null ? orderedRead(value, some) : null;
  const picked = useQuery({
    queryKey: ["grid-aggregate", scope, type, attr, value, q ?? "", some ?? -1],
    enabled: read !== null,
    retry: false,
    staleTime: 30_000,
    queryFn: async () => {
      const at = read ?? { desc: false, offset: 0, limit: 1 };
      const page = await source.query(
        { type, q: andQ(q, attr), attrs: [attr], orderBy: at.desc ? `${attr};desc` : attr },
        { offset: at.offset, limit: at.limit },
      );
      return orderedValue(page.rows.map((row) => row.cells[attr]), value);
    },
  });
  return {
    ...footerState([
      { on: needsTotal, isError: total.isError, isPending: total.isPending },
      { on: needsFilled, isError: filled.isError, isPending: filled.isPending },
      { on: read !== null, isError: picked.isError, isPending: picked.isPending },
    ]),
    shown: footerText(value, total.data ?? null, some, picked.data ?? null),
  };
}

/** One column's footer: the aggregation picked for it and its value over the view's whole query. */
export function FooterAggregate({
  scope,
  source,
  type,
  attr,
  label,
  q,
  kind,
  value,
  onChange,
}: {
  /** Whose counts these are (`{project}/{space}`): every count's cache key names it (F-UI1-004). */
  scope: string;
  source: EntitySource;
  type: string;
  attr: string;
  label: string;
  q?: string;
  /** The column's filter kind (`number`, `date`, …): which ordered aggregations it offers. */
  kind?: string;
  value: Aggregation;
  onChange: (next: Aggregation) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const { failed, waiting, shown } = useFooterValue(scope, source, type, attr, q, value);
  const offered = aggregationsFor(kind, source.orders === true);
  return (
    <div className="flex items-center gap-1">
      {/* Baserow's quiet "Summarize" (T-5093): a text button under the column, its choice the
          browser's own list, so a keyboard, a phone and a screen reader open it as any select. */}
      <Select
        quiet
        aria-label={t("spaces.toolbar.aggregateOf", { field: label })}
        className="max-w-32"
        value={value}
        onChange={(event) => onChange(event.target.value as Aggregation)}
      >
        {(offered.includes(value) ? offered : [...offered, value]).map((each) => (
          <option key={each} value={each}>
            {t(`spaces.toolbar.aggregate.${each}`)}
          </option>
        ))}
      </Select>
      {value === "none" ? null : failed ? (
        <span className="text-caption text-danger" title={t("spaces.toolbar.aggregateFailed")}>
          {t("spaces.toolbar.aggregateFailedShort")}
        </span>
      ) : waiting ? (
        <span className="text-caption text-fg-muted" role="status">
          …
        </span>
      ) : (
        <output className="text-caption font-semibold tabular-nums">{shown ?? t("spaces.toolbar.aggregateNone")}</output>
      )}
    </div>
  );
}

/** The keys a view orders by and the fields it groups by: at most three of each (T-5188, API/01 §30). */
export const MAX_SORT_KEYS = 3;
export const MAX_GROUP_FIELDS = 3;

export interface SortKey {
  attr: string;
  dir: "asc" | "desc";
}

/** A field a panel offers: only those the person's own answer carries, so nothing names a hidden one. */
export interface PanelField {
  value: string;
  label: string;
}

/**
 * The Sort panel (T-5188): up to three keys, each ordering what the one before leaves equal, sent
 * as one `orderBy` (CIM 009 4.23) so the endpoint orders the whole filtered set.
 */
export function SortPanel({ fields, value, onChange }: { fields: PanelField[]; value: SortKey[]; onChange: (next: SortKey[]) => void }): JSX.Element {
  const { t } = useTranslation();
  const set = (at: number, key: SortKey) => onChange(value.map((each, index) => (index === at ? key : each)));
  return (
    <KeyList
      legend={t("spaces.toolbar.sortLegend")}
      hint={t("spaces.toolbar.sortHint")}
      empty={t("spaces.toolbar.sortEmpty")}
      add={t("spaces.toolbar.sortAdd")}
      full={t("spaces.toolbar.sortFull", { n: MAX_SORT_KEYS })}
      fields={fields}
      chosen={value.map((key) => key.attr)}
      max={MAX_SORT_KEYS}
      onAdd={(attr) => onChange([...value, { attr, dir: "asc" }])}
      onPick={(at, attr) => set(at, { ...value[at], attr })}
      onRemove={(at) => onChange(value.filter((_, index) => index !== at))}
      extra={(at) => (
        <Select
          quiet
          aria-label={t("spaces.toolbar.sortDirectionOf", { n: at + 1 })}
          value={value[at].dir}
          onChange={(event) => set(at, { ...value[at], dir: event.target.value === "desc" ? "desc" : "asc" })}
        >
          <option value="asc">{t("spaces.toolbar.sortAsc")}</option>
          <option value="desc">{t("spaces.toolbar.sortDesc")}</option>
        </Select>
      )}
    />
  );
}

/** The Group panel (T-5188): up to three fields, outermost first; each group gets a header in the grid. */
export function GroupPanel({ fields, value, onChange }: { fields: PanelField[]; value: string[]; onChange: (next: string[]) => void }): JSX.Element {
  const { t } = useTranslation();
  return (
    <KeyList
      legend={t("spaces.toolbar.groupLegend")}
      hint={t("spaces.toolbar.groupHint")}
      empty={t("spaces.toolbar.groupEmpty")}
      add={t("spaces.toolbar.groupAdd")}
      full={t("spaces.toolbar.groupFull", { n: MAX_GROUP_FIELDS })}
      fields={fields}
      chosen={value}
      max={MAX_GROUP_FIELDS}
      onAdd={(attr) => onChange([...value, attr])}
      onPick={(at, attr) => onChange(value.map((each, index) => (index === at ? attr : each)))}
      onRemove={(at) => onChange(value.filter((_, index) => index !== at))}
    />
  );
}

/** One list of fields to pick, each once, with its own remove and an add while there is room. */
function KeyList({
  legend,
  hint,
  empty,
  add,
  full,
  fields,
  chosen,
  max,
  onAdd,
  onPick,
  onRemove,
  extra,
}: {
  legend: string;
  hint: string;
  empty: string;
  add: string;
  full: string;
  fields: PanelField[];
  chosen: string[];
  max: number;
  onAdd: (attr: string) => void;
  onPick: (at: number, attr: string) => void;
  onRemove: (at: number) => void;
  extra?: (at: number) => ReactNode;
}): JSX.Element {
  const { t } = useTranslation();
  const free = fields.filter((field) => !chosen.includes(field.value));
  const titleOf = (attr: string) => fields.find((field) => field.value === attr)?.label ?? attr;
  return (
    <fieldset className="flex flex-col gap-2">
      <legend className="text-body font-semibold">{legend}</legend>
      <p className="text-caption text-fg-muted">{hint}</p>
      {chosen.length === 0 ? <p className="text-caption text-fg-muted">{empty}</p> : null}
      <ol className="flex flex-col gap-1">
        {chosen.map((attr, at) => (
          <li key={attr} className="flex flex-wrap items-center gap-1">
            <Select
              aria-label={t("spaces.toolbar.keyFieldOf", { legend, n: at + 1 })}
              className="max-w-44"
              value={attr}
              onChange={(event) => onPick(at, event.target.value)}
            >
              {[{ value: attr, label: titleOf(attr) }, ...free].map((field) => (
                <option key={field.value} value={field.value}>
                  {field.label}
                </option>
              ))}
            </Select>
            {extra?.(at)}
            <Button size="sm" variant="ghost" aria-label={t("spaces.toolbar.keyRemoveOf", { field: titleOf(attr) })} onClick={() => onRemove(at)}>
              ✕
            </Button>
          </li>
        ))}
      </ol>
      <Button
        size="sm"
        className="w-fit"
        disabled={chosen.length >= max || free.length === 0}
        disabledReason={chosen.length >= max ? full : t("spaces.toolbar.keyNoField")}
        onClick={() => {
          const next = free[0];
          if (next) onAdd(next.value);
        }}
      >
        {add}
      </Button>
    </fieldset>
  );
}

/**
 * The `q` term that holds one group's value, `null` for a value no term states (a structured one):
 * an absent value is the attribute missing, a text is quoted, a number or a truth value is bare.
 */
export function groupValueTerm(attr: string, value: unknown): string | null {
  if (value === undefined || value === null) return `!${attr}`;
  if (typeof value === "string") return `${attr}==${JSON.stringify(value)}`;
  if (typeof value === "number" || typeof value === "boolean") return `${attr}==${String(value)}`;
  return null;
}

/** A group header's count (T-5188): the endpoint's `count=true` over the view's query and the group's values. */
export function GroupCount({ scope, source, type, q, path }: { scope: string; source: EntitySource; type: string; q?: string; path: { attr: string; value: unknown }[] }): JSX.Element | null {
  const { t } = useTranslation();
  const terms = path.map((step) => groupValueTerm(step.attr, step.value));
  const asked = terms.every((term): term is string => term !== null) ? andQ(q, ...terms) : null;
  const count = useQuery({
    queryKey: ["group-header-count", scope, type, asked ?? ""],
    enabled: asked !== null,
    retry: false,
    staleTime: 30_000,
    queryFn: async () => (await source.query({ type, q: asked ?? undefined }, { offset: 0, limit: 1 })).total ?? null,
  });
  if (asked === null || count.isError || count.data === null) return null;
  if (count.isPending) return <span className="text-caption text-fg-muted">…</span>;
  return <span className="text-caption text-fg-muted">{t("spaces.toolbar.groupCount", { count: count.data })}</span>;
}

/**
 * The text of an ordered read: the one value, or for a median of an even count the mean of the two
 * middle numbers; `null` where the rows carry no value to show.
 */
export function orderedValue(cells: (RichCell | RichCell[] | undefined)[], value: Aggregation): string | null {
  const values = cells.map((cell) => (Array.isArray(cell) ? cell[0] : cell)?.value);
  if (value === "median" && values.length === 2) {
    const [a, b] = values;
    return typeof a === "number" && typeof b === "number" ? String((a + b) / 2) : null;
  }
  const one = cells[0];
  return one === undefined ? null : cellText(one) || null;
}
