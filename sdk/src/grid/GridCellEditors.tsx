import React from "react";
import type { RichCell } from "./model";
import type { ValueRule } from "./rules";
import { optionLabel } from "../enums";
import type { EnumOption } from "../enums";

export function storedText(cell: RichCell | RichCell[] | undefined): string {
  const one = Array.isArray(cell) ? cell[0] : cell;
  const own = one?.kind === "relationship" ? one.object : one?.value;
  return own === undefined || own === null ? "" : String(own);
}

export function coerce(next: string, before: string, rule?: ValueRule): unknown {
  switch (rule?.kind) {
    case "integer":
    case "number":
      return next.trim() !== "" && Number.isFinite(Number(next)) ? Number(next) : next;
    case "boolean":
      return next === "true" ? true : next === "false" ? false : next;
    case "string":
    case "date":
    case "datetime":
    case "uri":
      return next;
  }
  if (before !== "" && Number.isFinite(Number(before)) && Number.isFinite(Number(next))) {
    return Number(next);
  }
  if (next === "true" || next === "false") {
    return next === "true";
  }
  return next;
}

/**
 * An enum cell: a picker of exactly the permissible values, showing each one's title and storing
 * the value (UI-86). A stored value the enum does not list stays selected and is marked invalid,
 * so looking at a row never rewrites it; only picking another value is a change.
 */
export function EnumCell({
  label,
  value,
  options,
  changed,
  notInList,
  onChange,
}: {
  label: string;
  value: string;
  options: EnumOption[];
  changed: boolean;
  notInList: string;
  onChange: (next: string) => void;
}): React.JSX.Element {
  const outside = value !== "" && !options.some((option) => option.value === value);
  return (
    <select
      aria-label={label}
      aria-invalid={outside || undefined}
      className={`jc-grid-cell-input${changed ? " jc-grid-cell-changed" : ""}${outside ? " jc-grid-cell-invalid" : ""}`}
      data-changed={changed ? "true" : undefined}
      value={value}
      onChange={(e) => onChange(e.target.value)}
    >
      {value === "" && <option value="">—</option>}
      {outside && <option value={value}>{`${value} (${notInList})`}</option>}
      {options.map((option) => (
        <option key={option.value} value={option.value} title={option.description}>
          {optionLabel(option)}
        </option>
      ))}
    </select>
  );
}

/** The input a value is typed in: the model's range first, else the stored value's shape. */
export function inputKindOf(rule: ValueRule | undefined, own: unknown): CellInputKind {
  switch (rule?.kind) {
    case "integer":
    case "number":
      return "number";
    case "date":
      return "date";
    case "uri":
      return "url";
    case "string":
    case "datetime":
      // A datetime is typed as ISO text: the browser's datetime-local drops the zone NGSI-LD keeps.
      return "text";
  }
  return typeof own === "number" ? "number" : "text";
}

type CellInputKind = "text" | "number" | "date" | "url";

/** One cell a person may correct: an input that says what it belongs to, marked while pending. */
export function EditableCell({
  label,
  value,
  changed,
  kind,
  invalid,
  onChange,
}: {
  label: string;
  value: string;
  changed: boolean;
  kind: CellInputKind;
  /** Why the value cannot be sent as it is: its model's rule, or the gateway's refusal. */
  invalid?: string;
  onChange: (next: string) => void;
}): React.JSX.Element {
  const messageId = React.useId();
  return (
    <>
      <input
        aria-label={label}
        aria-invalid={invalid ? true : undefined}
        aria-describedby={invalid ? messageId : undefined}
        className={`jc-grid-cell-input${changed ? " jc-grid-cell-changed" : ""}${invalid ? " jc-grid-cell-invalid" : ""}`}
        data-changed={changed ? "true" : undefined}
        type={kind}
        value={value}
        onChange={(e) => onChange(e.target.value)}
      />
      {invalid && (
        <span id={messageId} className="jc-grid-cell-message">
          {invalid}
        </span>
      )}
    </>
  );
}

/** The hover of a value in a unit: its unit's name and code, `microgram per cubic metre (GQ)` (DM-06). */
