/**
 * Grafana 12.4.14's field reducers (`@grafana/data` fieldReducer), ported line for line from the
 * reference instance's own bundle so a legend, a stat or a gauge computes what Grafana computes:
 * `delta` adds only rises and skips the first rise after a counter reset, `count` counts NaN but
 * not null, `mean` divides by the values that are numbers, and a percentile picks the sorted
 * value at `round((n - 1) * p)`. tests/dashboard_calcs.test.ts compares every reducer with what
 * the reference computed (tests/fixtures/grafana-reference/calcs.json).
 */

/** A field value as a reducer meets it: a number (NaN and ±Infinity kept), a text, a gap. */
export type CalcValue = number | string | boolean | null | undefined;

/** Grafana's `nullValueMode`: `null` ignores gaps (the default), `null as zero` counts them as 0. */
export interface CalcField {
  values: readonly CalcValue[];
  /** Whether the field is Grafana's `number` or `time` type; a text field gets no sums. */
  numeric: boolean;
  nullValueMode?: "null" | "connected" | "null as zero";
}

type Calcs = Record<string, unknown>;

interface ReducerInfo {
  name: string;
  /** What an empty field reduces to; `null` when the registry names none. */
  empty?: unknown;
  /** Its own function; the rest come from the standard pass. */
  reduce?: (field: CalcField, ignoreNulls: boolean, nullAsZero: boolean) => Calcs;
}

/** The English ordinal ending Grafana names a percentile with (`95th %`). */
export function percentSuffix(n: number): string {
  return n > 3 && n < 21 ? "th" : n % 10 === 1 ? "st" : n % 10 === 2 ? "nd" : n % 10 === 3 ? "rd" : "th";
}

function percentile(field: CalcField, p: number, ignoreNulls: boolean, nullAsZero: boolean): unknown {
  let values = [...field.values];
  if (ignoreNulls) values = values.filter((value) => value !== null);
  if (nullAsZero) values = values.map((value) => (value === null ? 0 : value));
  const sorted = values.slice().sort((a, b) => (a as number) - (b as number));
  return sorted[Math.round((sorted.length - 1) * p)];
}

function lastOf(field: CalcField): Calcs {
  return { last: field.values[field.values.length - 1] };
}

function firstNotNullOf(field: CalcField): Calcs {
  for (const value of field.values) if (value != null && !Number.isNaN(value)) return { firstNotNull: value };
  return { firstNotNull: null };
}

function lastNotNullOf(field: CalcField): Calcs {
  for (let at = field.values.length - 1; at >= 0; at -= 1) {
    const value = field.values[at];
    if (value != null && !Number.isNaN(value)) return { lastNotNull: value };
  }
  return { lastNotNull: null };
}

function varianceOf(field: CalcField): Calcs {
  if (!field.numeric) return { variance: 0, stdDev: 0 };
  let squares = 0;
  let mean = 0;
  let count = 0;
  for (const value of field.values) {
    if (value == null) continue;
    count += 1;
    const before = mean;
    mean += ((value as number) - before) / count;
    squares += ((value as number) - before) * ((value as number) - mean);
  }
  if (count === 0) return { variance: 0, stdDev: 0 };
  return { variance: squares / count, stdDev: Math.sqrt(squares / count) };
}

function gapAware(field: CalcField, ignoreNulls: boolean, nullAsZero: boolean): CalcValue[] {
  const out: CalcValue[] = [];
  for (const value of field.values) {
    if (value === null && ignoreNulls) continue;
    out.push(value === null && nullAsZero ? 0 : value);
  }
  return out;
}

function changeCountOf(field: CalcField, ignoreNulls: boolean, nullAsZero: boolean): Calcs {
  let changes = 0;
  let first = true;
  let previous: CalcValue = null;
  for (const value of gapAware(field, ignoreNulls, nullAsZero)) {
    if (!first && previous !== value) changes += 1;
    first = false;
    previous = value;
  }
  return { changeCount: changes };
}

function medianOf(field: CalcField, ignoreNulls: boolean, nullAsZero: boolean): Calcs {
  const values: CalcValue[] = [];
  for (let value of field.values) {
    if (value == null) {
      if (ignoreNulls) continue;
      if (nullAsZero) value = 0;
    }
    values.push(value);
  }
  values.sort((a, b) => (a as number) - (b as number));
  const half = Math.floor(values.length / 2);
  return { median: values.length % 2 === 0 ? ((values[half - 1] as number) + (values[half] as number)) / 2 : values[half] };
}

/** Grafana's registry, in its own order, with its own names. */
export const REDUCERS: Readonly<Record<string, ReducerInfo>> = {
  lastNotNull: { name: "Last *", reduce: lastNotNullOf },
  last: { name: "Last", reduce: lastOf },
  firstNotNull: { name: "First *", reduce: firstNotNullOf },
  first: { name: "First", reduce: (field) => ({ first: field.values[0] }) },
  min: { name: "Min" },
  max: { name: "Max" },
  mean: { name: "Mean" },
  median: { name: "Median", reduce: medianOf },
  variance: { name: "Variance", reduce: varianceOf },
  stdDev: { name: "StdDev", reduce: varianceOf },
  sum: { name: "Total", empty: 0 },
  count: { name: "Count", empty: 0 },
  countAll: { name: "Count all", empty: 0, reduce: (field) => ({ countAll: field.values.length }) },
  range: { name: "Range" },
  delta: { name: "Delta" },
  step: { name: "Step" },
  diff: { name: "Difference" },
  logmin: { name: "Min (above zero)" },
  allIsZero: { name: "All Zeros", empty: false },
  allIsNull: { name: "All Nulls", empty: true },
  changeCount: { name: "Change Count", reduce: changeCountOf },
  distinctCount: { name: "Distinct Count", reduce: (field, ignoreNulls, nullAsZero) => ({ distinctCount: new Set(gapAware(field, ignoreNulls, nullAsZero)).size }) },
  diffperc: { name: "Difference percent" },
  allValues: { name: "All values", reduce: (field) => ({ allValues: [...field.values] }) },
  uniqueValues: { name: "All unique values", reduce: (field) => ({ uniqueValues: [...new Set(field.values)] }) },
  ...Object.fromEntries(
    Array.from({ length: 99 }, (_, at) => at + 1).map((n) => [
      `p${n}`,
      { name: `${n}${percentSuffix(n)} %`, reduce: (field: CalcField, ignoreNulls: boolean, nullAsZero: boolean) => ({ [`p${n}`]: percentile(field, n / 100, ignoreNulls, nullAsZero) }) },
    ]),
  ),
};

/** Grafana's other names for a reducer (`aliasIds`). */
const ALIASES: Readonly<Record<string, string>> = { current: "lastNotNull", avg: "mean", total: "sum" };

/** The reducer id an option names, aliases resolved; unknown ids stay as they are. */
export function reducerId(id: string): string {
  return ALIASES[id] ?? id;
}

/** The standard pass (`doStandardCalcs`): every reducer without its own function at once. */
function standardCalcs(field: CalcField, ignoreNulls: boolean, nullAsZero: boolean): Calcs {
  const calcs = {
    sum: 0,
    max: -Number.MAX_VALUE as number | null,
    min: Number.MAX_VALUE as number | null,
    logmin: Number.MAX_VALUE,
    mean: null as number | null,
    last: null as CalcValue,
    first: null as CalcValue,
    lastNotNull: null as CalcValue,
    firstNotNull: null as CalcValue,
    count: 0,
    nonNullCount: 0,
    allIsNull: true,
    allIsZero: true,
    range: null as number | null,
    diff: null as number | null,
    delta: 0,
    step: Number.MAX_VALUE as number | null,
    diffperc: 0,
    previousDeltaUp: true,
  };
  field.values.forEach((raw, at) => {
    let value = raw;
    if (at === 0) calcs.first = value;
    calcs.last = value;
    if (value == null) {
      if (ignoreNulls) return;
      if (nullAsZero) value = 0;
    }
    calcs.count += 1;
    if (value == null || Number.isNaN(value)) return;
    const isFirst = calcs.firstNotNull === null;
    if (isFirst) calcs.firstNotNull = value;
    if (field.numeric) {
      const number = value as number;
      calcs.sum += number || 0;
      calcs.allIsNull = false;
      calcs.nonNullCount += 1;
      if (!isFirst) {
        const step = number - (calcs.lastNotNull as number);
        if ((calcs.step as number) > step) calcs.step = step;
        if ((calcs.lastNotNull as number) > number) calcs.previousDeltaUp = false;
        else {
          if (calcs.previousDeltaUp) calcs.delta += step;
          calcs.previousDeltaUp = true;
        }
      }
      if (number > (calcs.max as number)) calcs.max = number;
      if (number < (calcs.min as number)) calcs.min = number;
      if (number < calcs.logmin && number > 0) calcs.logmin = number;
    }
    if (value !== 0) calcs.allIsZero = false;
    calcs.lastNotNull = value;
  });
  if (calcs.max === -Number.MAX_VALUE) calcs.max = null;
  if (calcs.min === Number.MAX_VALUE) calcs.min = null;
  if (calcs.step === Number.MAX_VALUE) calcs.step = null;
  if (calcs.nonNullCount > 0) calcs.mean = calcs.sum / calcs.nonNullCount;
  if (calcs.allIsNull) calcs.allIsZero = false;
  if (calcs.max !== null && calcs.min !== null) calcs.range = calcs.max - calcs.min;
  if (typeof calcs.firstNotNull === "number" && typeof calcs.lastNotNull === "number") calcs.diff = calcs.lastNotNull - calcs.firstNotNull;
  if (typeof calcs.firstNotNull === "number" && typeof calcs.diff === "number") calcs.diffperc = (calcs.diff / calcs.firstNotNull) * 100;
  return calcs;
}

/**
 * Grafana's `reduceField`: the asked reducers over one field. An empty field gives each reducer's
 * empty result; one reducer with its own function runs alone; otherwise the standard pass runs and
 * each remaining reducer adds what it computes.
 */
export function reduceField(field: CalcField, ids: readonly string[]): Calcs {
  const asked = ids.map(reducerId).filter((id) => id in REDUCERS);
  if (asked.length === 0) return {};
  if (field.values.length < 1) return Object.fromEntries(asked.map((id) => [id, REDUCERS[id].empty ?? null]));
  const mode = field.nullValueMode ?? "null";
  const ignoreNulls = mode === "null";
  const nullAsZero = mode === "null as zero";
  const only = asked.length === 1 ? REDUCERS[asked[0]].reduce : undefined;
  if (only) return only(field, ignoreNulls, nullAsZero);
  let calcs = standardCalcs(field, ignoreNulls, nullAsZero);
  for (const id of asked) {
    const own = REDUCERS[id].reduce;
    if (!(id in calcs) && own) calcs = { ...calcs, ...own(field, ignoreNulls, nullAsZero) };
  }
  return calcs;
}

/** A value as a number field holds it: Prometheus' `NaN` and `±Inf` texts become those numbers. */
export function calcValueOf(value: unknown): number | null {
  if (typeof value === "number") return value;
  if (typeof value === "boolean") return value ? 1 : 0;
  if (typeof value === "string") {
    const text = value.trim();
    if (text === "NaN") return Number.NaN;
    if (text === "+Inf" || text === "Inf") return Number.POSITIVE_INFINITY;
    if (text === "-Inf") return Number.NEGATIVE_INFINITY;
    const parsed = text === "" ? Number.NaN : Number(text);
    return Number.isNaN(parsed) ? null : parsed;
  }
  return null;
}
