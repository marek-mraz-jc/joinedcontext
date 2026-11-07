/**
 * What a unit pick changes beside the unit (T-6101, DM-06): a unit measures a number, so a unit
 * on a text field turns it into a Number (an Integer for C62, "one", a count), and a field that
 * stops being a number loses its unit. The editor says so and offers Undo.
 */
export const NUMBER_RANGES = new Set(["float", "double", "decimal", "integer"]);

export type UnitNotice = "madeNumber" | "madeInteger" | "droppedUnit";

/** The range a slot takes when `code` becomes its unit, or undefined when it keeps its own. */
export function rangeForUnit(range: string | undefined, code: string): string | undefined {
  if (!code || (range !== undefined && NUMBER_RANGES.has(range))) return undefined;
  return code === "C62" ? "integer" : "float";
}

/** The notice a unit pick shows, when it changed the type of value. */
export function unitNotice(range: string | undefined, code: string): UnitNotice | undefined {
  const next = rangeForUnit(range, code);
  if (next === undefined) return undefined;
  return next === "integer" ? "madeInteger" : "madeNumber";
}

/** Whether moving a slot with a unit to `range` drops the unit. */
export function dropsUnit(range: string, unit: string | undefined): boolean {
  return Boolean(unit) && !NUMBER_RANGES.has(range);
}
