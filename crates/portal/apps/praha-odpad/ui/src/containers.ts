/**
 * Prague's sensor containers as a waste-collection desk reads them (T-2786): the kind of waste,
 * how full the container was at its sensor's last reading (0 empty to 1 full), when that was, and
 * the sorting isle it stands at, from `praha-mesto`. A reading is only as current as its time says:
 * the desk shows its age beside it. What stands out is judged against the city's own containers
 * (the fullest and the longest-unread tenth), never against a threshold this app would invent.
 */
import type { RichCell, RichRow } from "@joinedcontext/sdk";

export interface Container {
  id: string;
  code: string | null;
  kind: string | null;
  /** 0 empty to 1 full, or `null` when the sensor sent no valid reading. */
  fill: number | null;
  /** When the reading was taken, ISO 8601. */
  measuredAt: string | null;
  /** Hours between the reading and now; `null` without a reading time. */
  ageHours: number | null;
  /** The isle the container stands at, by its URN. */
  isle: string | null;
}

function first(cell: RichCell | RichCell[] | undefined): RichCell | undefined {
  return Array.isArray(cell) ? cell[0] : cell;
}

function text(row: RichRow, attr: string): string | null {
  const value = first(row.cells[attr])?.value;
  return typeof value === "string" && value.trim() !== "" ? value.trim() : null;
}

export function containerOf(row: RichRow, now: Date): Container {
  const raw = first(row.cells.fillingLevel)?.value;
  const fill = typeof raw === "number" && Number.isFinite(raw) && raw >= 0 && raw <= 1 ? raw : null;
  const measuredAt = text(row, "dateModified");
  const at = measuredAt ? Date.parse(measuredAt) : Number.NaN;
  const isleCell = first(row.cells.refWasteContainerIsle);
  const isle = typeof isleCell?.object === "string" ? isleCell.object : null;
  return {
    id: row.id,
    code: text(row, "containerCode"),
    kind: text(row, "wasteKind"),
    fill,
    measuredAt,
    // A reading time in the future is the sensor's clock, not a reading from the future.
    ageHours: Number.isNaN(at) ? null : Math.max(0, (now.getTime() - at) / 3_600_000),
    isle,
  };
}

export interface Totals {
  containers: number;
  /** The mean fill over the containers with a valid reading. */
  meanFill: number | null;
  /** Containers whose reading is missing or invalid. */
  unread: number;
}

export function totals(containers: Container[]): Totals {
  const fills = containers.map((c) => c.fill).filter((f): f is number => f !== null);
  return {
    containers: containers.length,
    meanFill: fills.length === 0 ? null : fills.reduce((sum, f) => sum + f, 0) / fills.length,
    unread: containers.length - fills.length,
  };
}

/**
 * The value at or above which a container is in the highest tenth of the city: ceil(n / 10) of
 * them. `null` with fewer than ten: a tenth of a handful is one container, and calling it an
 * outlier would be a claim the data cannot carry.
 */
export function highestTenth(values: (number | null)[]): number | null {
  const known = values.filter((v): v is number => v !== null).sort((a, b) => a - b);
  if (known.length < 10) return null;
  return known[known.length - Math.ceil(known.length / 10)];
}

export type SortKey = "code" | "fill" | "ageHours";

/** Sorted by `key`; a missing value goes last whichever the direction, so it never tops a list. */
export function sorted(containers: Container[], key: SortKey, ascending: boolean): Container[] {
  return [...containers].sort((a, b) => {
    const x = a[key];
    const y = b[key];
    if (x === null || y === null) return x === null ? (y === null ? 0 : 1) : -1;
    const order = typeof x === "string" ? x.localeCompare(String(y), "cs") : (x as number) - (y as number);
    return ascending ? order : -order;
  });
}

/** One CSV field: quoted when it holds a separator, a quote or a line break; quotes doubled. */
function field(value: string | number | null): string {
  if (value === null) return "";
  const text = typeof value === "number" ? String(Math.round(value * 100) / 100) : value;
  // A cell a spreadsheet would run as a formula is prefixed, so an exported code never executes.
  const safe = /^[=+\-@\t\r]/.test(text) ? `'${text}` : text;
  return /[",\n\r]/.test(safe) ? `"${safe.replace(/"/g, '""')}"` : safe;
}

/** The desk's table as CSV: fill as a percentage, the kind in words, the isle by its name. */
export function toCsv(containers: Container[], header: string[], words: Record<string, string>, isleName: (id: string | null) => string | null): string {
  const rows = containers.map((c) =>
    [c.code, c.kind === null ? null : (words[c.kind] ?? c.kind), c.fill === null ? null : c.fill * 100, c.measuredAt, isleName(c.isle)]
      .map(field)
      .join(","),
  );
  return [header.map(field).join(","), ...rows].join("\r\n") + "\r\n";
}
