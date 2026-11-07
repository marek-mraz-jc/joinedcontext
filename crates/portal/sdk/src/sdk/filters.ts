import { useCallback, useMemo, useState } from "react";
import type { Row } from "../ngsi";
import { format } from "../ngsi";

export type FilterDef =
  | { kind: "search"; attrs: string[]; label?: string }
  | { kind: "select"; attr: string; label?: string }
  | { kind: "range"; attr: string; label?: string }
  | { kind: "dateRange"; attr: string; label?: string };

export type FilterValue = string | [number, number] | [string, string] | undefined;

export function filterRows<T extends Row>(rows: T[], filters: FilterDef[], values: FilterValue[]): T[] {
  return rows.filter((row) =>
    filters.every((filter, index) => {
      const val = values[index];
      if (val === undefined || val === "") {
        return true;
      }

      if (filter.kind === "search") {
        const needle = String(val).toLowerCase();
        return filter.attrs.some((attr) => format(row[attr]).toLowerCase().includes(needle));
      }

      if (filter.kind === "select") {
        return format(row[filter.attr]) === String(val);
      }

      if (filter.kind === "range") {
        const num = row[filter.attr];
        if (typeof num !== "number" || !Array.isArray(val)) {
          return false;
        }
        return num >= (val[0] as number) && num <= (val[1] as number);
      }

      if (filter.kind === "dateRange") {
        if (!Array.isArray(val)) return true;
        const [start, end] = val as [string, string];
        const rawDate = row[filter.attr];
        if (typeof rawDate !== "string" || rawDate.trim() === "") {
          return false;
        }
        if (start !== "" && rawDate < start) return false;
        if (end !== "" && rawDate > end) return false;
        return true;
      }

      return true;
    }),
  );
}

export interface FilterBinding {
  def: FilterDef;
  rows: Row[];
  value: FilterValue;
  onChange(value: FilterValue): void;
}

export function useFilters<T extends Row>(
  rows: T[],
  filters: FilterDef[],
): {
  shown: T[];
  values: FilterValue[];
  set(index: number, value: FilterValue): void;
  bind(index: number): FilterBinding;
  reset(): void;
} {
  const [values, setValues] = useState<FilterValue[]>(() => filters.map(() => undefined));

  const shown = useMemo(() => filterRows(rows, filters, values), [rows, filters, values]);

  const set = useCallback((index: number, value: FilterValue) => {
    setValues((prev) => {
      const next = [...prev];
      next[index] = value;
      return next;
    });
  }, []);

  const bind = useCallback(
    (index: number): FilterBinding => {
      if (index < 0 || index >= filters.length) {
        throw new Error(`Filter index ${index} out of range (0..${filters.length - 1})`);
      }
      return {
        def: filters[index],
        rows,
        value: values[index],
        onChange: (v: FilterValue) => set(index, v),
      };
    },
    [filters, rows, values, set],
  );

  const reset = useCallback(() => {
    setValues(filters.map(() => undefined));
  }, [filters]);

  return { shown, values, set, bind, reset };
}
