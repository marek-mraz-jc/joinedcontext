/**
 * A Policy's filters in words (T-5593): `temporalQ=timerel=after;timeAt=P-1D` reads "history up to
 * 1 day back", `q` with the model's labels and ≥ for >=. One helper for the Policy sentence and the
 * Data access table, so both say the same thing. A filter of a shape it cannot read keeps its text
 * behind the same lead-in, never a guess.
 */
import { filterWords } from "../../components/entities/EndpointDataView";

type T = (key: string, options?: Record<string, unknown>) => string;

export type ConstraintName = "q" | "scopeQ" | "geoQ" | "temporalQ";

/** `timerel=after;timeAt=P-1D` (or `&`-joined) as its members. */
function members(text: string): Map<string, string> {
  const out = new Map<string, string>();
  for (const part of text.split(/[;&]/)) {
    const at = part.indexOf("=");
    if (at > 0) out.set(part.slice(0, at).trim(), part.slice(at + 1).trim());
  }
  return out;
}

const DURATION = /^-?P(?:(\d+)Y)?(?:(\d+)M)?(?:(\d+)W)?(?:(\d+)D)?(?:T(?:(\d+)H)?(?:(\d+)M)?(?:(\d+)S)?)?$/;
const UNITS = ["year", "month", "week", "day", "hour", "minute", "second"] as const;

/** `P-1D`, `-P7D`, `PT-1H30M` as "1 day", "7 days", "1 hour and 30 minutes"; `undefined` for anything else. */
export function durationWords(raw: string, language: string): string | undefined {
  const text = raw.replace(/(P|T)-/g, "$1").replace(/^-/, "-");
  const match = DURATION.exec(text);
  if (!match || text.endsWith("T") || text === "P" || text === "-P") return undefined;
  const parts = UNITS.flatMap((unit, index) => {
    const amount = Number(match[index + 1] ?? 0);
    return amount > 0
      ? [new Intl.NumberFormat(language, { style: "unit", unit, unitDisplay: "long" }).format(amount)]
      : [];
  });
  return parts.length > 0 ? new Intl.ListFormat(language, { style: "long", type: "conjunction" }).format(parts) : undefined;
}

function moment(raw: string, language: string): string | undefined {
  if (!/^\d{4}-\d{2}-\d{2}/.test(raw)) return undefined;
  const at = new Date(raw);
  if (Number.isNaN(at.getTime())) return undefined;
  const dateOnly = raw.length === 10;
  return new Intl.DateTimeFormat(language, dateOnly ? { dateStyle: "medium" } : { dateStyle: "medium", timeStyle: "short" }).format(at);
}

function temporalWords(text: string, t: T, language: string): string | undefined {
  const parts = members(text);
  const timerel = parts.get("timerel");
  const timeAt = parts.get("timeAt") ?? "";
  // Another time property than observedAt changes what the window counts: kept as written.
  const property = parts.get("timeproperty");
  if ([...parts.keys()].some((key) => !["timerel", "timeAt", "endTimeAt", "timeproperty"].includes(key))) return undefined;
  if (property && property !== "observedAt") return undefined;
  if (timerel === "between") {
    const from = moment(timeAt, language);
    const to = moment(parts.get("endTimeAt") ?? "", language);
    return from && to ? t("access.constraint.between", { from, to }) : undefined;
  }
  if (timerel !== "after" && timerel !== "before") return undefined;
  // A relative window counts back from now; a forward one (`P1D`) has no reading of its own.
  const back = timeAt.includes("-") && timeAt.includes("P") ? durationWords(timeAt, language) : undefined;
  if (back) return t(`access.constraint.${timerel === "after" ? "back" : "older"}`, { duration: back });
  const at = moment(timeAt, language);
  return at ? t(`access.constraint.${timerel === "after" ? "since" : "until"}`, { at }) : undefined;
}

/** `/city/#` reads "in /city and everything under it"; a scope with `+` stays as written. */
function scopeWords(text: string, t: T): string | undefined {
  if (!/^\/[^#+,;|]*(\/#)?$/.test(text)) return undefined;
  return text.endsWith("/#")
    ? t("access.constraint.scopeBelow", { scope: text.slice(0, -2) || "/" })
    : t("policies.sentence.scope", { scope: text });
}

/** `georel=near;maxDistance==500&geometry=Point&coordinates=[17.1,48.1]` as a distance from a point. */
function geoWords(text: string, t: T, language: string): string | undefined {
  const parts = members(text.replace(/maxDistance==/g, "maxDistance=").replace(/minDistance==/g, "minDistance="));
  const georel = parts.get("georel");
  const geometry = parts.get("geometry");
  let coordinates: unknown;
  try {
    coordinates = JSON.parse(parts.get("coordinates") ?? "null");
  } catch {
    return undefined;
  }
  const point =
    geometry === "Point" && Array.isArray(coordinates) && coordinates.length === 2 && coordinates.every((c) => typeof c === "number")
      ? t("access.constraint.point", {
          lat: new Intl.NumberFormat(language, { maximumFractionDigits: 5 }).format(coordinates[1] as number),
          lon: new Intl.NumberFormat(language, { maximumFractionDigits: 5 }).format(coordinates[0] as number),
        })
      : undefined;
  const where = point ?? (geometry === "Polygon" || geometry === "MultiPolygon" ? t("access.constraint.area") : undefined);
  if (!where) return undefined;
  const max = Number(parts.get("maxDistance"));
  if (georel === "near" && point && Number.isFinite(max) && max > 0) {
    const distance =
      max >= 1000
        ? new Intl.NumberFormat(language, { style: "unit", unit: "kilometer", maximumFractionDigits: 1 }).format(max / 1000)
        : new Intl.NumberFormat(language, { style: "unit", unit: "meter" }).format(max);
    return t("access.constraint.near", { distance, where });
  }
  if (georel === "within" || georel === "intersects") return t(`access.constraint.${georel}`, { where });
  return undefined;
}

/**
 * One filter of a grant as a phrase: "where PM10 ≥ 40", "history up to 1 day back". `labels` names
 * attributes by the model's labels (`{ pm10: "PM10" }`); an attribute the model does not name stays.
 */
export function constraintInWords(
  name: ConstraintName,
  value: string,
  t: T,
  language: string,
  labels: Record<string, string> = {},
): string {
  const text = value.trim();
  switch (name) {
    case "q":
      return t("policies.sentence.q", { q: filterWords(text, labels, t("endpoints.data.today")) });
    case "scopeQ":
      return scopeWords(text, t) ?? t("policies.sentence.scope", { scope: text });
    case "geoQ":
      return geoWords(text, t, language) ?? t("policies.sentence.geo", { geo: text });
    case "temporalQ":
      return temporalWords(text, t, language) ?? t("policies.sentence.temporal", { temporal: text });
  }
}
