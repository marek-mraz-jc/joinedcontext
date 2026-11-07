import type { ReactNode } from "react";
import { distinct, extent } from "@joinedcontext/sdk";
import type { FilterBinding } from "@joinedcontext/sdk";

export function FilterBar({
  children,
  onReset,
  shown,
  total,
}: {
  children?: ReactNode;
  onReset?: () => void;
  shown?: number;
  total?: number;
}): React.JSX.Element {
  return (
    <div className="jc-filterbar" role="search">
      {children}
      {shown !== undefined && total !== undefined && (
        <span className="jc-count">
          {shown} of {total}
        </span>
      )}
      {onReset && (
        <button type="button" onClick={onReset}>
          Reset
        </button>
      )}
    </div>
  );
}

export function SearchBox({
  binding,
  placeholder,
}: {
  binding: FilterBinding;
  placeholder?: string;
}): React.JSX.Element {
  if (binding.def.kind !== "search") {
    throw new Error("SearchBox needs a search filter");
  }
  const label = binding.def.label ?? "Search";
  const val = typeof binding.value === "string" ? binding.value : "";
  return (
    <label className="jc-filter">
      <span>{label}</span>
      <input
        type="search"
        placeholder={placeholder}
        value={val}
        onChange={(e) => binding.onChange(e.target.value === "" ? undefined : e.target.value)}
      />
    </label>
  );
}

export function SelectFilter({
  binding,
  allLabel,
}: {
  binding: FilterBinding;
  allLabel?: string;
}): React.JSX.Element {
  if (binding.def.kind !== "select") {
    throw new Error("SelectFilter needs a select filter");
  }
  const label = binding.def.label ?? binding.def.attr;
  const val = typeof binding.value === "string" ? binding.value : "";
  const options = distinct(binding.rows, binding.def.attr).slice(0, 200);
  return (
    <label className="jc-filter">
      <span>{label}</span>
      <select
        value={val}
        onChange={(e) => binding.onChange(e.target.value === "" ? undefined : e.target.value)}
      >
        <option value="">{allLabel ?? "All"}</option>
        {options.map((opt) => (
          <option key={opt} value={opt}>
            {opt}
          </option>
        ))}
      </select>
    </label>
  );
}

export function RangeFilter({ binding }: { binding: FilterBinding }): React.JSX.Element {
  if (binding.def.kind !== "range") {
    throw new Error("RangeFilter needs a range filter");
  }
  const label = binding.def.label ?? binding.def.attr;
  const ext = extent(binding.rows, binding.def.attr);
  const arr = Array.isArray(binding.value) ? binding.value : undefined;
  const fromVal = arr && typeof arr[0] === "number" && arr[0] !== -Infinity ? String(arr[0]) : "";
  const toVal = arr && typeof arr[1] === "number" && arr[1] !== Infinity ? String(arr[1]) : "";

  const update = (nextFrom: string, nextTo: string) => {
    const trimmedFrom = nextFrom.trim();
    const trimmedTo = nextTo.trim();
    if (trimmedFrom === "" && trimmedTo === "") {
      binding.onChange(undefined);
    } else {
      const min = trimmedFrom === "" ? -Infinity : Number(trimmedFrom);
      const max = trimmedTo === "" ? Infinity : Number(trimmedTo);
      binding.onChange([min, max]);
    }
  };

  return (
    <label className="jc-filter">
      <span>{label}</span>
      <input
        type="number"
        aria-label={`${label} from`}
        placeholder={ext ? String(ext[0]) : undefined}
        value={fromVal}
        onChange={(e) => update(e.target.value, toVal)}
      />
      <input
        type="number"
        aria-label={`${label} to`}
        placeholder={ext ? String(ext[1]) : undefined}
        value={toVal}
        onChange={(e) => update(fromVal, e.target.value)}
      />
    </label>
  );
}

export function DateRangeFilter({ binding }: { binding: FilterBinding }): React.JSX.Element {
  if (binding.def.kind !== "dateRange") {
    throw new Error("DateRangeFilter needs a dateRange filter");
  }
  const label = binding.def.label ?? binding.def.attr;
  const arr = Array.isArray(binding.value) ? binding.value : undefined;
  const fromVal = arr && typeof arr[0] === "string" ? arr[0] : "";
  const toRaw = arr && typeof arr[1] === "string" ? arr[1] : "";
  const toVal = toRaw ? toRaw.slice(0, 10) : "";

  const update = (nextFrom: string, nextToDate: string) => {
    const trimmedFrom = nextFrom.trim();
    const trimmedTo = nextToDate.trim();
    if (trimmedFrom === "" && trimmedTo === "") {
      binding.onChange(undefined);
    } else {
      const toSent = trimmedTo !== "" ? `${trimmedTo}T23:59:59.999Z` : "";
      binding.onChange([trimmedFrom, toSent]);
    }
  };

  return (
    <label className="jc-filter">
      <span>{label}</span>
      <input
        type="date"
        aria-label={`${label} from`}
        value={fromVal}
        onChange={(e) => update(e.target.value, toVal)}
      />
      <input
        type="date"
        aria-label={`${label} to`}
        value={toVal}
        onChange={(e) => update(fromVal, e.target.value)}
      />
    </label>
  );
}
