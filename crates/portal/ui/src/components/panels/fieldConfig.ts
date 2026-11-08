/**
 * Grafana's field configuration as a dashboard saves it (`fieldConfig.defaults` and `overrides`):
 * unit, decimals, min and max, value mappings, thresholds and the colours they name, resolved per
 * field the way Grafana resolves them.
 */
import type { FrameField } from "./frames";
import { displayNameOf } from "./frames";
import { colorOf } from "./palette";

export { colorOf };

export interface ThresholdStep {
  color: string;
  value: number | null;
}

export interface Thresholds {
  mode?: "absolute" | "percentage";
  steps: ThresholdStep[];
}

export interface ValueMapping {
  type: "value" | "range" | "regex" | "special";
  options: Record<string, unknown>;
}

/** One field's configuration after the overrides that match it. */
export interface FieldSettings {
  unit?: string;
  decimals?: number;
  min?: number;
  max?: number;
  displayName?: string;
  noValue?: string;
  mappings: ValueMapping[];
  thresholds?: Thresholds;
  color?: { mode?: string; fixedColor?: string };
  custom: Record<string, unknown>;
  /** A time field spanning less than a minute shows milliseconds, as Grafana's display processor does. */
  showMs?: boolean;
}

export interface FieldConfigSource {
  defaults?: Record<string, unknown>;
  overrides?: { matcher?: { id?: string; options?: unknown }; properties?: { id?: string; value?: unknown }[] }[];
}

function settingsFrom(raw: Record<string, unknown> | undefined): FieldSettings {
  const source = raw ?? {};
  return {
    unit: typeof source.unit === "string" ? source.unit : undefined,
    decimals: typeof source.decimals === "number" ? source.decimals : undefined,
    min: typeof source.min === "number" ? source.min : undefined,
    max: typeof source.max === "number" ? source.max : undefined,
    displayName: typeof source.displayName === "string" && source.displayName ? source.displayName : undefined,
    noValue: typeof source.noValue === "string" ? source.noValue : undefined,
    mappings: Array.isArray(source.mappings) ? (source.mappings as ValueMapping[]) : [],
    thresholds: isThresholds(source.thresholds) ? source.thresholds : undefined,
    color: typeof source.color === "object" && source.color !== null ? (source.color as FieldSettings["color"]) : undefined,
    custom: typeof source.custom === "object" && source.custom !== null ? { ...(source.custom as Record<string, unknown>) } : {},
  };
}

function isThresholds(value: unknown): value is Thresholds {
  return typeof value === "object" && value !== null && Array.isArray((value as Thresholds).steps);
}

/** Sets one override property (`unit`, `decimals`, `custom.width`, `mappings` ...) on `settings`. */
function apply(settings: FieldSettings, id: string, value: unknown): void {
  if (id.startsWith("custom.")) {
    settings.custom[id.slice("custom.".length)] = value;
    return;
  }
  switch (id) {
    case "unit":
    case "displayName":
    case "noValue":
      if (typeof value === "string") settings[id] = value;
      return;
    case "decimals":
    case "min":
    case "max":
      if (typeof value === "number") settings[id] = value;
      return;
    case "mappings":
      if (Array.isArray(value)) settings.mappings = value as ValueMapping[];
      return;
    case "thresholds":
      if (isThresholds(value)) settings.thresholds = value;
      return;
    case "color":
      if (typeof value === "object" && value !== null) settings.color = value as FieldSettings["color"];
      return;
    default:
      return;
  }
}

/** Whether an override's matcher picks `field` (byName, byRegexp, byType, byFrameRefID). */
function matches(matcher: { id?: string; options?: unknown } | undefined, field: FrameField, name: string, refId?: string): boolean {
  const options = matcher?.options;
  switch (matcher?.id) {
    case "byName":
      return options === name || options === field.name;
    case "byRegexp":
      try {
        return typeof options === "string" && new RegExp(options).test(name);
      } catch {
        return false;
      }
    case "byType":
      return options === field.type;
    case "byFrameRefID":
      return options === refId;
    default:
      return false;
  }
}

/** The configuration of `field`: the defaults, then each matching override in order. */
export function settingsFor(config: FieldConfigSource | undefined, field: FrameField, refId?: string): FieldSettings {
  const settings = settingsFrom(config?.defaults);
  if (field.unit && !settings.unit) settings.unit = field.unit;
  const name = displayNameOf(field);
  for (const override of config?.overrides ?? []) {
    if (!matches(override.matcher, field, name, refId)) continue;
    for (const property of override.properties ?? []) {
      if (property.id) apply(settings, property.id, property.value);
    }
  }
  // Grafana draws a time field as a date whatever unit the panel set for its numbers.
  if (field.type === "time" && !isDateUnit(settings.unit)) {
    settings.unit = "dateTimeAsSystem";
    const first = instantOf(field.values[0]);
    const last = instantOf(field.values[field.values.length - 1]);
    settings.showMs = field.values.length > 1 && first !== undefined && last !== undefined && Math.abs(last - first) < 60_000;
  }
  return settings;
}

/** The units Grafana formats as a date (`time:<format>` is its custom one). */
const DATE_UNITS = new Set(["dateTimeAsIso", "dateTimeAsIsoNoDateIfToday", "dateTimeAsUS", "dateTimeAsUSNoDateIfToday", "dateTimeAsLocal", "dateTimeAsLocalNoDateIfToday", "dateTimeAsSystem", "dateTimeFromNow"]);

export function isDateUnit(unit: string | undefined): boolean {
  return unit !== undefined && (DATE_UNITS.has(unit) || unit.startsWith("time:"));
}

/** Epoch milliseconds of a number or an ISO text. */
function instantOf(value: unknown): number | undefined {
  if (typeof value === "number" && Number.isFinite(value)) return value;
  if (typeof value === "string") {
    const ms = Date.parse(value);
    return Number.isNaN(ms) ? undefined : ms;
  }
  return undefined;
}

const pad = (n: number, width = 2): string => String(n).padStart(width, "0");

/** A moment.js format Grafana writes (`YYYY-MM-DD HH:mm:ss.SSS`, `MM/DD/YYYY h:mm:ss a`) in local time. */
function momentFormat(date: Date, format: string): string {
  const hours = date.getHours();
  const tokens: Record<string, string> = {
    YYYY: String(date.getFullYear()),
    MM: pad(date.getMonth() + 1),
    DD: pad(date.getDate()),
    HH: pad(hours),
    hh: pad(hours % 12 || 12),
    h: String(hours % 12 || 12),
    mm: pad(date.getMinutes()),
    ss: pad(date.getSeconds()),
    SSS: pad(date.getMilliseconds(), 3),
    a: hours < 12 ? "am" : "pm",
    A: hours < 12 ? "AM" : "PM",
  };
  return format.replace(/YYYY|MM|DD|HH|hh|h|mm|ss|SSS|a|A/g, (token) => tokens[token]);
}

/** An instant in one of Grafana's date units, `now` for the units that read it relative to today. */
export function formatDate(ms: number, unit: string, showMs = false, locale = "en", now = Date.now()): string {
  const date = new Date(ms);
  const today = new Date(now).toDateString() === date.toDateString();
  switch (unit) {
    case "dateTimeAsIso":
      return momentFormat(date, "YYYY-MM-DD HH:mm:ss");
    case "dateTimeAsIsoNoDateIfToday":
      return momentFormat(date, today ? "HH:mm:ss" : "YYYY-MM-DD HH:mm:ss");
    case "dateTimeAsUS":
      return momentFormat(date, "MM/DD/YYYY h:mm:ss a");
    case "dateTimeAsUSNoDateIfToday":
      return momentFormat(date, today ? "h:mm:ss a" : "MM/DD/YYYY h:mm:ss a");
    case "dateTimeAsLocal":
      return date.toLocaleString(locale);
    case "dateTimeAsLocalNoDateIfToday":
      return today ? date.toLocaleTimeString(locale) : date.toLocaleString(locale);
    case "dateTimeFromNow": {
      const seconds = Math.round((ms - now) / 1000);
      const steps: [Intl.RelativeTimeFormatUnit, number][] = [["year", 31_536_000], ["month", 2_592_000], ["day", 86_400], ["hour", 3_600], ["minute", 60], ["second", 1]];
      const [name, size] = steps.find(([, size]) => Math.abs(seconds) >= size) ?? ["second", 1];
      return new Intl.RelativeTimeFormat(locale, { numeric: "auto" }).format(Math.round(seconds / size), name);
    }
    default:
      if (unit.startsWith("time:")) return momentFormat(date, unit.slice("time:".length));
      return momentFormat(date, showMs ? "YYYY-MM-DD HH:mm:ss.SSS" : "YYYY-MM-DD HH:mm:ss");
  }
}

/** The threshold step `value` falls in: the last whose value is at most it, the base step below all. */
export function stepFor(thresholds: Thresholds | undefined, value: number | null, min = 0, max = 100): ThresholdStep | undefined {
  if (!thresholds || thresholds.steps.length === 0 || value === null) return undefined;
  const scaled = thresholds.mode === "percentage" ? ((value - min) / (max - min || 1)) * 100 : value;
  let found = thresholds.steps[0];
  for (const step of thresholds.steps) {
    if (step.value === null || step.value <= scaled) found = step;
  }
  return found;
}

/** What a value mapping turns a value into: its text, its colour, or nothing. */
export function mapValue(mappings: ValueMapping[], value: unknown): { text?: string; color?: string } | undefined {
  for (const mapping of mappings) {
    const options = mapping.options ?? {};
    if (mapping.type === "value") {
      const hit = (options as Record<string, { text?: string; color?: string }>)[String(value)];
      if (hit) return hit;
    } else if (mapping.type === "range") {
      const number = typeof value === "number" ? value : Number(value);
      const from = typeof options.from === "number" ? options.from : -Infinity;
      const to = typeof options.to === "number" ? options.to : Infinity;
      if (Number.isFinite(number) && number >= from && number <= to) return options.result as { text?: string; color?: string };
    } else if (mapping.type === "regex" && typeof options.pattern === "string") {
      try {
        const pattern = new RegExp(options.pattern);
        const text = String(value ?? "");
        const match = pattern.exec(text);
        if (match) {
          const result = (options.result ?? {}) as { text?: string; color?: string };
          return { ...result, text: result.text?.replace(/\$(\d)/g, (_, n: string) => match[Number(n)] ?? "") };
        }
      } catch {
        // A pattern this browser does not take maps nothing.
      }
    } else if (mapping.type === "special") {
      const match = options.match;
      const empty = value === null || value === undefined;
      const hit =
        (match === "null" && empty) ||
        (match === "nan" && typeof value === "number" && Number.isNaN(value)) ||
        (match === "null+nan" && (empty || (typeof value === "number" && Number.isNaN(value)))) ||
        (match === "true" && value === true) ||
        (match === "false" && value === false) ||
        (match === "empty" && value === "");
      if (hit) return options.result as { text?: string; color?: string };
    }
  }
  return undefined;
}

interface UnitShape {
  /** Multiply the value by this before it is printed. */
  scale?: number;
  suffix?: string;
  prefix?: string;
  /** Scale through SI prefixes (k, M, G) of this base. */
  si?: number;
  /** The unit is a duration of this many milliseconds. */
  durationMs?: number;
}

const UNITS: Record<string, UnitShape> = {
  percent: { suffix: "%" },
  percentunit: { scale: 100, suffix: "%" },
  ms: { durationMs: 1 },
  s: { durationMs: 1000 },
  m: { durationMs: 60_000 },
  h: { durationMs: 3_600_000 },
  d: { durationMs: 86_400_000 },
  µs: { durationMs: 0.001 },
  ns: { durationMs: 0.000001 },
  dtdurationms: { durationMs: 1 },
  dtdurations: { durationMs: 1000 },
  bytes: { si: 1024, suffix: "B" },
  decbytes: { si: 1000, suffix: "B" },
  bits: { si: 1024, suffix: "b" },
  kbytes: { si: 1024, suffix: "B", scale: 1024 },
  bps: { si: 1000, suffix: "b/s" },
  Bps: { si: 1000, suffix: "B/s" },
  binBps: { si: 1024, suffix: "B/s" },
  reqps: { si: 1000, suffix: " req/s" },
  rps: { si: 1000, suffix: " req/s" },
  ops: { si: 1000, suffix: " ops/s" },
  wps: { si: 1000, suffix: " wr/s" },
  short: { si: 1000 },
  celsius: { suffix: " °C" },
  fahrenheit: { suffix: " °F" },
  humidity: { suffix: " %H" },
  pressurehpa: { suffix: " hPa" },
  lengthm: { suffix: " m" },
  lengthkm: { suffix: " km" },
  velocityms: { suffix: " m/s" },
  velocitykmh: { suffix: " km/h" },
  conμgm3: { suffix: " µg/m³" },
  conugm3: { suffix: " µg/m³" },
  ppm: { suffix: " ppm" },
  watt: { si: 1000, suffix: "W" },
  kwatt: { suffix: " kW" },
  "currencyEUR": { prefix: "€" },
  "currencyUSD": { prefix: "$" },
};

const SI_PREFIX = ["", "k", "M", "G", "T", "P"];
const BIN_PREFIX = ["", "Ki", "Mi", "Gi", "Ti", "Pi"];

function digits(value: number, decimals: number | undefined, locale: string): string {
  // Grafana's toFixed: without decimals, getDecimalsForValue picks them, and the text keeps its
  // trailing zeros (7.1 weeks is `7.10 weeks`, 0.005 is `0.00500`).
  const fraction = Math.max(0, decimals ?? decimalsFor(value));
  return new Intl.NumberFormat(locale, { useGrouping: false, minimumFractionDigits: fraction, maximumFractionDigits: fraction }).format(value);
}

/** Grafana's getDecimalsForValue: one significant digit past the first, two when the rest is above 2.25. */
export function decimalsFor(value: number): number {
  if (value === 0 || !Number.isFinite(value)) return 0;
  if (value % 1 === 0) return 0;
  const abs = Math.abs(value);
  let dec = -Math.floor(Math.log10(abs)) + 1;
  if (abs / 10 ** -dec > 2.25) dec += 1;
  return Math.max(0, dec);
}

/**
 * Grafana's time units (`ns` … `d`, valueFormats/dateTimeFormatters): each a ladder of
 * [below this, divide by, word]; the last step catches the rest. A scaled word takes an `s` above
 * 1 (`7.10 weeks`), the first step's never does (`3 day`), as Grafana writes them.
 */
const TIME_STEPS: Record<string, [number, number, string][]> = {
  ns: [[1e3, 1, " ns"], [1e6, 1e3, " µs"], [1e9, 1e6, " ms"], [6e10, 1e9, " s"], [36e11, 6e10, " min"], [864e11, 36e11, " hour"], [Infinity, 864e11, " day"]],
  µs: [[1e3, 1, " µs"], [1e6, 1e3, " ms"], [Infinity, 1e6, " s"]],
  ms: [[1e3, 1, " ms"], [6e4, 1e3, " s"], [36e5, 6e4, " min"], [864e5, 36e5, " hour"], [31536e6, 864e5, " day"], [Infinity, 31536e6, " year"]],
  s: [[1e-6, 1e-9, " ns"], [0.001, 1e-6, " µs"], [1, 0.001, " ms"], [60, 1, " s"], [3600, 60, " min"], [86400, 3600, " hour"], [604800, 86400, " day"], [31536e3, 604800, " week"], [Infinity, 31556900, " year"]],
  m: [[60, 1, " min"], [1440, 60, " hour"], [10080, 1440, " day"], [604800, 10080, " week"], [Infinity, 525948, " year"]],
  h: [[24, 1, " hour"], [168, 24, " day"], [8760, 168, " week"], [Infinity, 8760, " year"]],
  d: [[7, 1, " day"], [365, 7, " week"], [Infinity, 365, " year"]],
};
const PLURAL = new Set([" min", " hour", " day", " week", " year"]);

function timeUnit(value: number, steps: [number, number, string][], decimals: number | undefined, locale: string): string {
  // Seconds alone print zero as `0 s`; below its first step `s` scales down to ms, µs and ns.
  if (value === 0 && steps === TIME_STEPS.s) return "0 s";
  const abs = Math.abs(value);
  const at = steps.findIndex(([below]) => abs < below);
  const [, size, word] = steps[at];
  const shown = value / size;
  // Grafana's first step of each unit keeps its own word; `s` has no plural at all.
  const first = size === 1 && steps !== TIME_STEPS.s ? true : steps === TIME_STEPS.s && word === " s";
  const plural = !first && PLURAL.has(word) && Math.abs(shown) > 1 ? "s" : "";
  return `${digits(shown, decimals, locale)}${word}${plural}`;
}

function duration(ms: number, decimals: number | undefined, locale: string): string {
  const abs = Math.abs(ms);
  // Zero in the unit a person reads durations in, not the smallest one.
  if (abs === 0) return `${digits(0, decimals, locale)} s`;
  const steps: [number, string][] = [
    [86_400_000, " d"],
    [3_600_000, " h"],
    [60_000, " min"],
    [1000, " s"],
    [1, " ms"],
    [0.001, " µs"],
  ];
  for (const [size, label] of steps) {
    if (abs >= size || size === 0.001) return `${digits(ms / size, decimals, locale)}${label}`;
  }
  return `${digits(ms, decimals, locale)} ms`;
}

/**
 * `value` printed in its Grafana unit with the field's decimals: `0.42` in `percentunit` is
 * `42%`, `1536` in `bytes` `1.5 KiB`, `0.25` in `s` `250 ms`. A unit this table lacks is printed
 * after the number; `none` and no unit print the number alone.
 */
export function formatUnit(value: number | null, unit: string | undefined, decimals?: number, locale = "en"): string {
  if (value === null || !Number.isFinite(value)) return "";
  const shape: UnitShape | undefined = unit ? UNITS[unit] : undefined;
  // Grafana's `locale` unit is toLocaleString: grouped thousands (1,514).
  if (unit === "locale") return new Intl.NumberFormat(locale, decimals === undefined ? {} : { minimumFractionDigits: decimals, maximumFractionDigits: decimals }).format(value);
  if (!unit || unit === "none" || unit === "") {
    return digits(value, decimals, locale);
  }
  if (!shape) {
    // `suffix:` and `prefix:` are Grafana's custom units.
    if (unit.startsWith("suffix:")) return `${digits(value, decimals, locale)}${unit.slice("suffix:".length)}`;
    if (unit.startsWith("prefix:")) return `${unit.slice("prefix:".length)}${digits(value, decimals, locale)}`;
    return `${digits(value, decimals, locale)} ${unit}`;
  }
  const scaled = value * (shape.scale ?? 1);
  if (unit in TIME_STEPS) return timeUnit(scaled, TIME_STEPS[unit], decimals, locale);
  if (shape.durationMs !== undefined) return duration(scaled * shape.durationMs, decimals, locale);
  let shown = scaled;
  let prefix = "";
  if (shape.si) {
    const names = shape.si === 1024 ? BIN_PREFIX : SI_PREFIX;
    let at = 0;
    while (Math.abs(shown) >= shape.si && at < names.length - 1) {
      shown /= shape.si;
      at += 1;
    }
    prefix = names[at];
    if (prefix && shape.suffix && !shape.suffix.startsWith(" ")) prefix = ` ${prefix}`;
  }
  const number = digits(shown, decimals, locale);
  const suffix = shape.suffix ?? "";
  return `${shape.prefix ?? ""}${number}${prefix}${prefix && !suffix ? "" : suffix}`;
}

/** A value as a person reads it in a cell, a legend or a tooltip: mapped, else in its unit. */
export function displayValue(settings: FieldSettings, value: unknown, locale = "en"): { text: string; color?: string } {
  const mapped = mapValue(settings.mappings, value);
  if (mapped?.text !== undefined) return { text: mapped.text, color: mapped.color ? colorOf(mapped.color) : undefined };
  if (value === null || value === undefined || value === "") {
    return { text: settings.noValue ?? "–", color: mapped?.color ? colorOf(mapped.color) : undefined };
  }
  if (isDateUnit(settings.unit)) {
    const ms = instantOf(value);
    if (ms !== undefined) return { text: formatDate(ms, settings.unit!, settings.showMs, locale), color: mapped?.color ? colorOf(mapped.color) : undefined };
  }
  if (typeof value === "number") {
    return { text: formatUnit(value, settings.unit, settings.decimals, locale), color: mapped?.color ? colorOf(mapped.color) : undefined };
  }
  if (typeof value === "object") return { text: "…" };
  return { text: String(value), color: mapped?.color ? colorOf(mapped.color) : undefined };
}
