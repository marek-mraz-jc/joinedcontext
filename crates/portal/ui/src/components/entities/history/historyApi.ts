import { readCsrfToken } from "../../../api/client";
import { parseCsv as parseTable } from "../../../pages/spaces/importRows";
import { reasonProblem } from "../reason";

/** Provenance audit fields attached to an instance (DM-111). */
export type Provenance = {
  channel?: string;
  actor?: string;
  operation?: string;
  reason?: string;
  writtenAt?: string;
};

/** One recorded version of a measurement (ADR-N-054 2.4). */
export type Version = {
  value: unknown;
  observedAt: string;
  instanceId?: string;
  supersededAt?: string;
  provenance?: Provenance;
  unitCode?: string;
};

/** A measurement: one observation instant with its versions, oldest first (DM-104). */
export type Measurement = {
  observedAt: string;
  current: Version;
  versions: Version[];
};

// ponytail: one PATCH per row stays inside the edge's per-minute budget at this size; a larger
// import is the space's Import rows or a pipeline.
export const MAX_CSV_ROWS = 500;
export const MAX_CSV_BYTES = 1_000_000;

export interface CsvRow {
  observedAt: string;
  value: unknown;
  line: number;
}

export interface CsvProblem {
  line: number;
  reason: "emptyFile" | "tooLarge" | "tooManyRows" | "columns" | "badInstant" | "futureInstant" | "badValue";
}

function endpointBase(slug: string): string {
  return `${window.location.origin}/api/endpoint/${encodeURIComponent(slug)}/ngsi-ld/v1`;
}

async function problemOf(response: Response): Promise<Error> {
  const body = (await response.json().catch(() => null)) as { detail?: unknown; title?: unknown } | null;
  const detail = [body?.detail, body?.title].find((text) => typeof text === "string");
  return new Error(detail ? String(detail) : String(response.status));
}

/** The instances of one attribute, the ones of the default instance only: a write here names no `datasetId`. */
function instancesOf(body: Record<string, unknown>, attribute: string): Record<string, unknown>[] {
  const raw = body[attribute];
  const values = (raw as { values?: unknown } | undefined)?.values;
  const list: unknown[] = Array.isArray(raw) ? raw : Array.isArray(values) ? values : raw ? [raw] : [];
  return list.filter(
    (item): item is Record<string, unknown> => typeof item === "object" && item !== null && !("datasetId" in item),
  );
}

function versionOf(raw: Record<string, unknown>): Version | undefined {
  const text = (key: string) => (typeof raw[key] === "string" ? raw[key] : undefined);
  const observedAt = text("observedAt");
  if (!observedAt) return undefined;
  return {
    value: raw.value,
    observedAt,
    instanceId: text("instanceId"),
    supersededAt: text("supersededAt"),
    provenance: typeof raw.provenance === "object" && raw.provenance !== null ? (raw.provenance as Provenance) : undefined,
    unitCode: text("unitCode"),
  };
}

/** Oldest version first: by when it was written, else superseded ones before the live one. */
function versionOrder(a: Version, b: Version): number {
  const written = Date.parse(a.provenance?.writtenAt ?? "") - Date.parse(b.provenance?.writtenAt ?? "");
  return Number.isNaN(written) ? Number(!a.supersededAt) - Number(!b.supersededAt) : written;
}

/** A `versions=all` or plain temporal answer as measurements, newest first (ADR-N-054 2.1). */
export function measurementsOf(body: unknown, attribute: string): Measurement[] {
  if (!body || typeof body !== "object") return [];
  const byInstant = new Map<number, Version[]>();
  for (const instance of instancesOf(body as Record<string, unknown>, attribute)) {
    const version = versionOf(instance);
    if (version) byInstant.set(Date.parse(version.observedAt), [...(byInstant.get(Date.parse(version.observedAt)) ?? []), version]);
  }
  return [...byInstant.entries()]
    .sort(([a], [b]) => b - a)
    .map(([, versions]) => {
      versions.sort(versionOrder);
      const current = versions.filter((each) => !each.supersededAt).at(-1) ?? versions[versions.length - 1];
      return { observedAt: current.observedAt, current, versions };
    });
}

async function readTemporal(slug: string, query: string): Promise<Response> {
  return globalThis.fetch(
    new Request(`${endpointBase(slug)}/temporal/entities/${query}`, {
      method: "GET",
      credentials: "same-origin",
      headers: { Accept: "application/json" },
    }),
  );
}

/**
 * One page of an attribute's measurements. A role without `readVersions` (403 on the versions
 * read) gets the plain history, and a 404 is an attribute nothing recorded yet (DM-114).
 */
export async function readHistory(
  slug: string,
  entityId: string,
  attribute: string,
  options?: { before?: string },
): Promise<{ measurements: Measurement[]; versions: boolean }> {
  const before = options?.before ? `&timerel=before&timeAt=${encodeURIComponent(options.before)}` : "";
  const query = `${encodeURIComponent(entityId)}?attrs=${encodeURIComponent(attribute)}&lastN=9${before}`;
  const all = await readTemporal(slug, `${query}&versions=all`);
  const versions = all.status !== 403;
  const answer = versions ? all : await readTemporal(slug, query);
  if (answer.status === 404) return { measurements: [], versions };
  if (!answer.ok) throw await problemOf(answer);
  return { measurements: measurementsOf(await answer.json(), attribute), versions };
}

/**
 * The one write of this panel, through the Endpoint and its Policy. The instant says what the
 * broker does with it (ADR-N-054 2.1, DM-105, DM-106): a new instant at or after the current one
 * is the current value, an older one a row in history only, and an instant it holds with another
 * value a new version, which is how a correction and a revert are written. `JC-Record: version`
 * keeps the change for a type that records no history (DM-143).
 */
export async function writeMeasurement(
  slug: string,
  entityId: string,
  attribute: string,
  measurement: { value: unknown; observedAt: string; unitCode?: string },
  reason: string,
): Promise<void> {
  if (reasonProblem(reason)) throw new Error("a write names why in 1 to 500 characters");
  const { value, observedAt, unitCode } = measurement;
  const response = await globalThis.fetch(
    new Request(`${endpointBase(slug)}/entities/${encodeURIComponent(entityId)}/attrs`, {
      method: "PATCH",
      credentials: "same-origin",
      headers: {
        "Content-Type": "application/json",
        "JC-Reason": encodeURIComponent(reason.trim()),
        "JC-Record": "version",
        "x-csrf-token": readCsrfToken() ?? "",
      },
      body: JSON.stringify({ [attribute]: { type: "Property", value, observedAt, ...(unitCode ? { unitCode } : {}) } }),
    }),
  );
  if (!response.ok) throw await problemOf(response);
}

/** A `datetime-local` value in the reader's zone as UTC, or undefined when invalid or in the future. */
export function instantOf(local: string): string | undefined {
  const at = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}/.test(local) ? new Date(local).getTime() : Number.NaN;
  return Number.isNaN(at) || at > Date.now() ? undefined : new Date(at).toISOString();
}

/** Typed text as a value of the current value's type, or undefined when it is not one. */
export function valueOf(text: string, sample: unknown): unknown {
  const trimmed = text.trim();
  if (typeof sample === "number") {
    const number = trimmed === "" ? Number.NaN : Number(trimmed);
    return Number.isFinite(number) ? number : undefined;
  }
  if (typeof sample === "boolean" || sample === undefined) {
    if (trimmed.toLowerCase() === "true") return true;
    if (trimmed.toLowerCase() === "false") return false;
    return typeof sample === "boolean" || trimmed === "" ? undefined : text;
  }
  if (typeof sample === "object" && sample !== null) {
    try {
      return JSON.parse(text);
    } catch {
      return undefined;
    }
  }
  return text;
}

function csvRow(record: Record<string, string>, columns: { at: string; value: string }, line: number, sample: unknown): CsvRow | CsvProblem {
  const at = Date.parse((record[columns.at] ?? "").trim());
  if (Number.isNaN(at)) return { line, reason: "badInstant" };
  if (at > Date.now()) return { line, reason: "futureInstant" };
  const value = valueOf(record[columns.value] ?? "", sample);
  return value === undefined ? { line, reason: "badValue" } : { observedAt: new Date(at).toISOString(), value, line };
}

/** A CSV with `observedAt` and `value` columns as rows, capped in bytes and rows (DM-81). */
export function parseCsv(text: string, sample?: unknown): { rows: CsvRow[]; problems: CsvProblem[] } {
  if (new TextEncoder().encode(text).length > MAX_CSV_BYTES) return { rows: [], problems: [{ line: 1, reason: "tooLarge" }] };
  const table = parseTable(text.replace(/^\uFEFF/, ""));
  if (table.columns.length === 0) return { rows: [], problems: [{ line: 1, reason: "emptyFile" }] };
  const column = (name: string) => table.columns.find((each) => each.trim().toLowerCase() === name);
  const [at, value] = [column("observedat"), column("value")];
  if (!at || !value) return { rows: [], problems: [{ line: 1, reason: "columns" }] };
  if ((table.total ?? table.rows.length) > MAX_CSV_ROWS) return { rows: [], problems: [{ line: 1, reason: "tooManyRows" }] };
  const rows: CsvRow[] = [];
  const problems: CsvProblem[] = [];
  table.rows.forEach((record, index) => {
    const parsed = csvRow(record, { at, value }, index + 2, sample);
    if ("reason" in parsed) problems.push(parsed);
    else rows.push(parsed);
  });
  return { rows, problems };
}

/** What an import sends and skips; `unchecked` rows lie before the history read and go as new. */
export interface ImportPlan {
  added: CsvRow[];
  duplicates: CsvRow[];
  corrections: CsvRow[];
  unchecked: CsvRow[];
}

/**
 * Sorts the file against the history read (ADR-N-054 2.1). Unless `complete`, a row older than
 * the oldest measurement read cannot be compared: it goes as new, and the broker keeps one row
 * for an identical value at that instant and records another value as a correction (DM-109).
 */
export function planImport(rows: CsvRow[], measurements: Measurement[], complete = false): ImportPlan {
  const held = new Map(measurements.map((each) => [Date.parse(each.observedAt), each]));
  const oldest = Math.min(...held.keys());
  const plan: ImportPlan = { added: [], duplicates: [], corrections: [], unchecked: [] };
  // The last row of an instant wins, as a person reading the file expects.
  for (const row of new Map(rows.map((each) => [each.observedAt, each])).values()) {
    const at = Date.parse(row.observedAt);
    const existing = held.get(at);
    if (existing) {
      (JSON.stringify(row.value) === JSON.stringify(existing.current.value) ? plan.duplicates : plan.corrections).push(row);
    } else {
      (!complete && at < oldest ? plan.unchecked : plan.added).push(row);
    }
  }
  return plan;
}

/**
 * Sends a plan oldest first, so a row newer than the current value ends as the current value, one
 * at a time, stopping at the first refusal: importing the file again skips what landed.
 */
export async function runImport(
  target: { slug: string; entityId: string; attribute: string },
  plan: ImportPlan,
  reason: string,
  onProgress?: (progress: { sent: number; total: number }) => void,
): Promise<{ sent: number; failed?: { line: number; message: string } }> {
  const rows = [...plan.added, ...plan.unchecked, ...plan.corrections].sort((a, b) => a.observedAt.localeCompare(b.observedAt));
  for (const [sent, row] of rows.entries()) {
    try {
      await writeMeasurement(target.slug, target.entityId, target.attribute, row, reason);
    } catch (error) {
      return { sent, failed: { line: row.line, message: error instanceof Error ? error.message : String(error) } };
    }
    onProgress?.({ sent: sent + 1, total: rows.length });
  }
  return { sent: rows.length };
}
