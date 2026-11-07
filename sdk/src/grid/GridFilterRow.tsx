import React from "react";
import { optionLabel } from "../enums";
import { opsForKind, valuesNeeded } from "./filters";
import type { ColumnFilter, FilterColumn } from "./filters";
import type { GridLabels } from "./useEntityGrid";
import type { GridView } from "./useGridView";

/** The filter row (T-4883): one control per column, an area picker on a geo column, the type choice on Type. */
export function GridFilterRow({ view }: { view: GridView }): React.JSX.Element | null {
  const { props, filterable, labels, columns, kindOf, asText, grid, areaMap, setAreaMap } = view;
  const { filterRow = true, rowNumbers, typeChoice } = props;
  const { state, setFilter } = grid;
  if (!filterRow || filterable.size === 0) return null;
  return (
    <tr className="jc-grid-filter-row" aria-label={labels.filterRow}>
      {rowNumbers && <td className="jc-grid-rownum" />}
      {columns.map((col) => {
        const column = filterable.get(col.key);
        const className = `jc-grid-filter${col.pinned ? " jc-grid-pinned" : ""}`;
        if (col.attr && !col.meta && kindOf.get(col.key) === "geo" && !asText) {
          const drawn = grid.area?.geoproperty === col.attr;
          return (
            <th key={`filter-${col.key}`} className={className} data-filter-for={col.key}>
              <div className="jc-grid-filter-cell">
                <button
                  type="button"
                  className="jc-grid-area-button"
                  aria-pressed={drawn || areaMap === col.attr}
                  onClick={() => {
                    if (drawn) {
                      grid.setArea(null);
                      setAreaMap(null);
                    } else setAreaMap(areaMap === col.attr ? null : col.attr!);
                  }}
                >
                  {drawn ? labels.areaClear : labels.areaPick}
                </button>
              </div>
            </th>
          );
        }
        if (typeChoice && col.key === "entity:type") {
          return (
            <th key={`filter-${col.key}`} className={className} data-filter-for={col.key}>
              <TypeChoiceCell choice={typeChoice} />
            </th>
          );
        }
        // A column the endpoint cannot be asked about has no filter, and an empty
        // header cell is a header that names nothing to a screen reader.
        if (!column || asText) return <td key={`filter-${col.key}`} className={className} />;
        return (
          <th key={`filter-${col.key}`} className={className} data-filter-for={col.key}>
            <FilterCell
              column={column}
              label={col.label}
              labels={labels}
              filter={state.filters[col.key]}
              onChange={(next) => setFilter(col.key, next)}
            />
          </th>
        );
      })}
    </tr>
  );
}

type FilterCellLabels = Pick<
  GridLabels,
  | "filter" | "value" | "upperValue" | "noFilter" | "unit" | "filterHintText" | "filterHintNumber" | "filterHintDate" | "filterHintEnum" | "filterHintOther"
  | "filterExampleText" | "filterExampleNumber" | "filterExampleId" | "yes" | "no"
> & { ops: Record<string, string> };

/** A filter's value box by what the column holds (T-6328): a date or date-time picker, a number, a text with an example. */
function valueBox(column: FilterColumn, labels: FilterCellLabels): { type: string; placeholder?: string } {
  if (column.kind === "number") return { type: "number", placeholder: labels.filterExampleNumber };
  if (column.kind === "date") return { type: column.datetime ? "datetime-local" : "date" };
  return { type: "text", placeholder: column.kind === "id" ? labels.filterExampleId : labels.filterExampleText };
}

/** A stored timestamp as a date-time picker shows it, in the reader's own time. */
function localInput(iso: string): string {
  const at = new Date(iso);
  if (Number.isNaN(at.getTime())) return iso;
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${at.getFullYear()}-${pad(at.getMonth() + 1)}-${pad(at.getDate())}T${pad(at.getHours())}:${pad(at.getMinutes())}`;
}

/** A date-time picker's local value as the UTC timestamp the endpoint compares with. */
function fromLocalInput(local: string): string {
  const at = new Date(local);
  return local === "" || Number.isNaN(at.getTime()) ? local : at.toISOString();
}

/**
 * The one line under a column's filter (UI-16), from the attribute's model: what to type for its
 * kind, the unit of a number and the allowed values of an enum (T-4949).
 */
function filterHint(column: FilterColumn, labels: FilterCellLabels): string {
  switch (column.kind) {
    case "number":
      return column.unit ? `${labels.filterHintNumber} ${labels.unit}: ${column.unit}.` : labels.filterHintNumber;
    case "date":
      return labels.filterHintDate;
    case "enum":
    case "type":
      return `${labels.filterHintEnum} ${(column.options ?? []).map(optionLabel).join(", ")}.`;
    case "text":
    case "id":
    case "relationship":
    case "untyped":
      return labels.filterHintText;
    default:
      return labels.filterHintOther;
  }
}

/**
 * One column's filter: the operators its content allows, and the value(s) the chosen one needs.
 * A filter is sent only once it is complete, so nothing narrows while it is being typed.
 */
export interface TypeChoice {
  value: string;
  options: readonly { value: string; label: string }[];
  label: string;
  onChange: (type: string) => void;
}

export function TypeChoiceCell({ choice }: { choice: TypeChoice }): React.JSX.Element {
  return (
    <div className="jc-grid-filter-cell">
      <select aria-label={choice.label} title={choice.label} value={choice.value} onChange={(e) => choice.onChange(e.target.value)}>
        {choice.options.map((each) => (
          <option key={each.value} value={each.value}>
            {each.label}
          </option>
        ))}
      </select>
    </div>
  );
}

function FilterCell({
  column,
  label,
  labels,
  filter,
  onChange,
}: {
  column: FilterColumn;
  label: string;
  labels: FilterCellLabels;
  filter: ColumnFilter | undefined;
  onChange: (next: ColumnFilter | undefined) => void;
}): React.JSX.Element {
  const hintId = React.useId();
  const hint = filterHint(column, labels);
  // Values ticked in the header's list stay readable here, whatever the column offers itself (T-6328).
  const ticked = filter?.op === "anyOf" && column.kind !== "enum" && column.kind !== "type";
  const ops = ticked ? [...opsForKind(column.kind), "anyOf" as const] : opsForKind(column.kind);
  const op = filter?.op ?? ops[0];
  const needed = valuesNeeded(op);
  const box = valueBox(column, labels);
  const stamp = box.type === "datetime-local";
  // An enum's values are picked, several at once: the query asks for any of them (UI-86).
  const picking = (column.kind === "enum" || column.kind === "type") && op === "anyOf";
  // A yes/no column is picked from its two values, never typed.
  const yesNo = column.kind === "boolean" && op === "equals";
  return (
    <div className="jc-grid-filter-cell">
      <span id={hintId} className="jc-grid-sr-only">
        {hint}
      </span>
      <select
        aria-label={`${labels.filter}: ${label}`}
        aria-describedby={hintId}
        title={hint}
        value={filter ? op : ""}
        onChange={(e) => {
          const next = e.target.value as ColumnFilter["op"] | "";
          if (next === "") {
            onChange(undefined);
            return;
          }
          onChange({ op: next, value: filter?.value ?? "", value2: filter?.value2 });
        }}
      >
        <option value="">{labels.noFilter}</option>
        {ops.map((each) => (
          <option key={each} value={each}>
            {labels.ops[each] ?? each}
          </option>
        ))}
      </select>
      {/* One value picked in the row keeps it one line tall; the header's funnel picks several (T-6328). */}
      {filter && picking && (
        <select
          aria-label={`${labels.value}: ${label}`}
          value={(filter.values ?? []).length === 1 ? filter.values![0] : ""}
          onChange={(e) => onChange({ ...filter, values: e.target.value === "" ? [] : [e.target.value] })}
        >
          <option value="">{(filter.values ?? []).length > 1 ? `${filter.values!.length} ✓` : labels.noFilter}</option>
          {(column.options ?? []).map((option) => (
            <option key={option.value} value={option.value} title={option.description}>
              {optionLabel(option)}
            </option>
          ))}
        </select>
      )}
      {filter && yesNo && (
        <select aria-label={`${labels.value}: ${label}`} value={filter.value} onChange={(e) => onChange({ ...filter, value: e.target.value })}>
          <option value="" disabled>{labels.noFilter}</option>
          <option value="true">{labels.yes}</option>
          <option value="false">{labels.no}</option>
        </select>
      )}
      {filter && ticked && <span className="jc-grid-filter-ticked" title={(filter.values ?? []).join(", ")}>{(filter.values ?? []).join(", ")}</span>}
      {filter && needed >= 1 && !picking && !yesNo && !ticked && (
        <input
          aria-label={`${labels.value}: ${label}`}
          type={box.type}
          placeholder={box.placeholder}
          value={stamp ? localInput(filter.value) : filter.value}
          onChange={(e) => onChange({ ...filter, value: stamp ? fromLocalInput(e.target.value) : e.target.value })}
        />
      )}
      {filter && needed === 2 && (
        <input
          aria-label={`${labels.upperValue}: ${label}`}
          type={box.type}
          placeholder={box.placeholder}
          value={stamp ? localInput(filter.value2 ?? "") : filter.value2 ?? ""}
          onChange={(e) => onChange({ ...filter, value2: stamp ? fromLocalInput(e.target.value) : e.target.value })}
        />
      )}
    </div>
  );
}

/**
 * A value typed into a cell, in the shape the cell had: a column of numbers stays numbers, so the
 * endpoint is not sent a string where it stored a measurement.
 */
/**
 * The typed text as the value it is sent as. With the attribute's rule the model decides: a number
 * slot sends a number, a boolean one `true`/`false`, a text, date or address slot the text as typed
 * (so "007" or "true" in a name stays text). Text the rule refuses is kept as typed, for the cell
 * to say why. Without a rule the stored value's shape is the guide.
 */
/** A cell's stored value as its text box shows it: the value, not the unit beside it. */
