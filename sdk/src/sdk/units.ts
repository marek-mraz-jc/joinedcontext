import list from "./units.json";

/**
 * One UN/CEFACT unit of measure (DM-06, DM-59): UNECE Recommendation 20 joined with QUDT.
 *
 * The list is the platform's one code list, `crates/jc-core/data/unece-rec20.json`, generated in
 * the platform repository by `tools/units/generate.py` and copied here byte for byte; the Portal
 * UI reads this one copy too, and CI compares it with the source. Nothing in this repository maps
 * a unit by hand.
 */
export interface Unit {
  /** The Rec 20 common code NGSI-LD puts on the wire as `unitCode` (`GQ`). */
  code: string;
  name: string;
  /** What a person reads (`µg/m³`); empty where Rec 20 gives none (`piece`). */
  symbol: string;
  /** Still a valid code on the wire, not offered to a new model. */
  deprecated: boolean;
  ucum: string | null;
  qudt: string | null;
  quantityKinds: string[];
  dimension: string | null;
  /** To the SI unit of the dimension: `si = (value + offset) × factor`; null never converts. */
  factor: number | null;
  offset: number;
  /** In the municipal set a picker shows first. */
  frequent: boolean;
}

const source: { units: Unit[] } = list;

/** Every unit, sorted by code. */
export const UNITS: readonly Unit[] = source.units;

const BY_CODE = new Map(UNITS.map((unit) => [unit.code, unit]));

/** The unit of a Rec 20 code; codes are case-sensitive, as on the wire. */
export function unitOf(code: string | undefined): Unit | undefined {
  return code === undefined ? undefined : BY_CODE.get(code);
}

/** How a person reads a unit: its symbol and name, or the name alone where it has no symbol. */
export function unitLabel(unit: Unit): string {
  return unit.symbol ? `${unit.symbol} — ${unit.name}` : unit.name;
}

/**
 * The symbol a value is written with (DM-06): the list's symbol, nothing for a unit that has
 * none or for the dimensionless "one", and a code the list does not know as it was stored, so a
 * reader still sees it.
 */
export function unitSymbol(code: string | undefined): string {
  if (!code) return "";
  const unit = BY_CODE.get(code);
  if (!unit) return code;
  return unit.symbol === "1" ? "" : unit.symbol;
}

/** What hovering a value says about its unit: `microgram per cubic metre (GQ)`. */
export function unitTitle(code: string | undefined): string {
  if (!code) return "";
  const unit = BY_CODE.get(code);
  return unit ? `${unit.name} (${code})` : code;
}

/**
 * A value as a person reads it (DM-06, T-2812): a number in the reader's locale followed by its
 * unit's symbol, `23.4 µg/m³` in English and `23,4 µg/m³` in Slovak. Anything that is not a
 * number is written as it is; an empty value is empty.
 */
export function formatValue(value: unknown, unitCode?: string, locale?: string): string {
  if (value === null || value === undefined || value === "") return "";
  // A number that is not one reads as a dash, as `format` writes it: never `NaN µg/m³`.
  if (typeof value === "number" && !Number.isFinite(value)) return "—";
  const text =
    typeof value === "number"
      ? new Intl.NumberFormat(locale, { maximumFractionDigits: 6 }).format(value)
      : typeof value === "object"
        ? JSON.stringify(value)
        : String(value);
  const symbol = unitSymbol(unitCode);
  return symbol ? `${text} ${symbol}` : text;
}

/**
 * Whether a value in one unit can be written in the other (DM-06): both carry a factor, share
 * QUDT's dimension and at least one quantity kind. A percent and a minute of arc are both
 * dimensionless and still not one thing.
 */
export function convertible(from: Unit, to: Unit): boolean {
  if (from.code === to.code) return true;
  return (
    from.factor !== null &&
    to.factor !== null &&
    from.dimension !== null &&
    from.dimension === to.dimension &&
    from.quantityKinds.some((kind) => to.quantityKinds.includes(kind))
  );
}

/** Twelve significant digits: what a factor is written with, without a float's last-digit noise. */
function tidy(value: number): number {
  return Number(value.toPrecision(12));
}

/**
 * The linear conversion from one unit to another, `to = from × factor + offset`, or undefined
 * when they do not convert (DM-06, T-2811). Read from the code list's QUDT factors, never typed.
 */
export function conversion(from: Unit, to: Unit): { factor: number; offset: number } | undefined {
  if (!convertible(from, to)) return undefined;
  if (from.code === to.code || from.factor === null || to.factor === null) return { factor: 1, offset: 0 };
  const factor = from.factor / to.factor;
  return { factor: tidy(factor), offset: tidy(from.offset * factor - to.offset) };
}
