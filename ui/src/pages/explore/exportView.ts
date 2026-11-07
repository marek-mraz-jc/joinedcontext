/**
 * Reading what the explorer shows (T-3253, UI-33): every entity the view's filters match, paged
 * through the endpoint by `collect`, its geometry, and a CSV field a spreadsheet keeps as text.
 * The file formats themselves moved to the space page's Export menu (T-6107).
 */

/** How many entities one export holds at most: past it, the person narrows the filter. */
export const MAX_EXPORT_ROWS = 100_000;
/** One page asked of the endpoint while exporting. */
export const EXPORT_PAGE = 500;

type Entity = Record<string, unknown>;

/** The two NGSI-LD queries joined, as the grid joins its preset and its filter row (EP-07). */
export function andQ(...parts: (string | undefined)[]): string | undefined {
  const kept = parts.map((part) => part?.trim()).filter((part): part is string => Boolean(part));
  return kept.length === 0 ? undefined : kept.map((part) => (kept.length > 1 && part.includes("|") ? `(${part})` : part)).join(";");
}

/** An attribute's value as keyValues answers it, or the `value` of a normalized one. */
function plain(value: unknown): unknown {
  if (value && typeof value === "object" && !Array.isArray(value)) {
    const record = value as Record<string, unknown>;
    if ("value" in record) return record.value;
    if ("object" in record) return record.object;
  }
  return value;
}

/**
 * A CSV field. A text starting with `=`, `+`, `-`, `@`, a tab or a carriage return is what a
 * spreadsheet runs as a formula, so it is written with a leading apostrophe and stays text.
 */
export function csvField(text: string, separator: string, numeric: boolean): string {
  const safe = !numeric && /^[=+\-@\t\r]/.test(text) ? `'${text}` : text;
  return /["\n\r]/.test(safe) || safe.includes(separator) ? `"${safe.replace(/"/g, '""')}"` : safe;
}

/** The first GeoJSON geometry an entity carries, the `location` attribute before any other. */
export function geometryOf(entity: Entity): { type: string; coordinates: unknown } | undefined {
  const candidates = ["location", ...Object.keys(entity).filter((key) => key !== "location")];
  for (const key of candidates) {
    const value = plain(entity[key]);
    if (value && typeof value === "object" && typeof (value as { type?: unknown }).type === "string" && "coordinates" in value) {
      return value as { type: string; coordinates: unknown };
    }
  }
  return undefined;
}

/**
 * Every entity the view matches, a page at a time, until the endpoint has no more or the export
 * is full. `progress` hears the count after each page; an aborted signal stops before the next.
 */
export async function collect(
  page: (offset: number, limit: number) => Promise<{ rows: Entity[]; count?: number }>,
  options: { signal?: AbortSignal; progress?: (done: number, total?: number) => void; max?: number; pageSize?: number } = {},
): Promise<{ entities: Entity[]; truncated: boolean; total?: number }> {
  const max = options.max ?? MAX_EXPORT_ROWS;
  const size = options.pageSize ?? EXPORT_PAGE;
  const entities: Entity[] = [];
  let total: number | undefined;
  for (;;) {
    if (options.signal?.aborted) throw new DOMException("the export was cancelled", "AbortError");
    const limit = Math.min(size, max - entities.length);
    const answer = await page(entities.length, limit);
    total = answer.count ?? total;
    entities.push(...answer.rows);
    options.progress?.(entities.length, total);
    if (answer.rows.length < limit) return { entities, truncated: false, total };
    if (entities.length >= max) return { entities, truncated: total === undefined || total > max, total };
  }
}
