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
  return settings;
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
  const auto = Math.abs(value) >= 100 ? 0 : Math.abs(value) >= 10 ? 1 : 2;
  const fraction = decimals ?? auto;
  return new Intl.NumberFormat(locale, {
    minimumFractionDigits: decimals ?? 0,
    maximumFractionDigits: fraction,
  }).format(value);
}

function duration(ms: number, decimals: number | undefined, locale: string): string {
  const abs = Math.abs(ms);
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
  if (!unit || unit === "none" || unit === "locale" || unit === "") {
    return digits(value, decimals, locale);
  }
  if (!shape) {
    // `suffix:` and `prefix:` are Grafana's custom units.
    if (unit.startsWith("suffix:")) return `${digits(value, decimals, locale)}${unit.slice("suffix:".length)}`;
    if (unit.startsWith("prefix:")) return `${unit.slice("prefix:".length)}${digits(value, decimals, locale)}`;
    return `${digits(value, decimals, locale)} ${unit}`;
  }
  const scaled = value * (shape.scale ?? 1);
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
  if (typeof value === "number") {
    return { text: formatUnit(value, settings.unit, settings.decimals, locale), color: mapped?.color ? colorOf(mapped.color) : undefined };
  }
  if (typeof value === "object") return { text: "…" };
  return { text: String(value), color: mapped?.color ? colorOf(mapped.color) : undefined };
}
