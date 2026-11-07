/**
 * The unit code list for the Portal: the SDK's one copy of it (DM-06) and the search the pickers
 * run over it.
 */
import { UNITS } from "@joinedcontext/sdk";
import type { Unit } from "@joinedcontext/sdk";

export { UNITS, conversion, convertible, formatValue, unitLabel, unitOf, unitSymbol, unitTitle } from "@joinedcontext/sdk";
export type { Unit } from "@joinedcontext/sdk";

const SUPERSCRIPT: Record<string, string> = { "²": "2", "³": "3", "⁻": "-", "¹": "1" };

/**
 * Text as a search compares it: lower case, `µ` and `μ` as `u`, superscripts as digits, and no
 * punctuation, so `ug/m3`, `µg/m³` and `ug.m-3` are one word.
 */
function fold(text: string): string {
  return text
    .toLowerCase()
    .replace(/[µμ]/g, "u")
    .replace(/[²³⁻¹]/g, (c) => SUPERSCRIPT[c] ?? c)
    .replace(/[^a-z0-9]/g, "");
}

/** How well a unit answers a query: 0 is its code, higher is looser, undefined is no match. */
function rank(unit: Unit, query: string, folded: string): number | undefined {
  if (unit.code.toLowerCase() === query) return 0;
  const symbol = fold(unit.symbol);
  const ucum = fold(unit.ucum ?? "");
  if ((symbol !== "" && symbol === folded) || (ucum !== "" && ucum === folded)) return 1;
  if (unit.code.toLowerCase().startsWith(query)) return 2;
  const name = unit.name.toLowerCase();
  if (name.startsWith(query)) return 2;
  if (name.includes(query) || (folded !== "" && (symbol.includes(folded) || ucum.includes(folded)))) return 3;
  if (unit.quantityKinds.some((kind) => kind.toLowerCase().includes(folded))) return 4;
  return undefined;
}

export interface UnitSearch {
  units: Unit[];
  /** How many matched before the limit. */
  total: number;
}

/**
 * The units a typed text finds (DM-06): by code, symbol, UCUM, name or quantity kind. The
 * frequent set comes first, then the rest grouped by quantity kind, the best-matching group
 * first. An empty text answers the frequent set. A deprecated code is found only by its code.
 */
export function searchUnits(text: string, limit = 100): UnitSearch {
  const query = text.trim().toLowerCase();
  const folded = fold(query);
  if (query === "") {
    const frequent = UNITS.filter((unit) => unit.frequent);
    return { units: sortByKind(frequent.map((unit) => ({ unit, score: 0 }))), total: frequent.length };
  }
  const found: { unit: Unit; score: number }[] = [];
  for (const unit of UNITS) {
    const score = rank(unit, query, folded);
    if (score === undefined || (unit.deprecated && score !== 0)) continue;
    found.push({ unit, score });
  }
  return { units: sortByKind(found).slice(0, limit), total: found.length };
}

/** The group a unit is listed under in a picker: frequent, else its first quantity kind. */
export function unitGroup(unit: Unit): string | undefined {
  return unit.frequent ? undefined : unit.quantityKinds[0];
}

function sortByKind(found: { unit: Unit; score: number }[]): Unit[] {
  const key = (unit: Unit) => (unit.frequent ? "" : (unit.quantityKinds[0] ?? "￿"));
  const best = new Map<string, number>();
  for (const { unit, score } of found) {
    const group = key(unit);
    best.set(group, Math.min(best.get(group) ?? score, score));
  }
  return found
    .sort((a, b) => {
      const [ga, gb] = [key(a.unit), key(b.unit)];
      if (ga !== gb) {
        if (ga === "" || gb === "") return ga === "" ? -1 : 1;
        return (best.get(ga) ?? 0) - (best.get(gb) ?? 0) || ga.localeCompare(gb);
      }
      return a.score - b.score || a.unit.code.localeCompare(b.unit.code);
    })
    .map(({ unit }) => unit);
}
