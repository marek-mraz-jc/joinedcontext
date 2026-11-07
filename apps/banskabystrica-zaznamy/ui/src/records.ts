/**
 * What a record is on this screen, and the one thing a person may change about it (T-2436,
 * T-2437, AP-62, EP-14).
 *
 * Every figure of a `KeyPerformanceIndicator` belongs to the publisher and is written by a
 * pipeline: the next run replaces it, so a screen that let a person correct one would lose what
 * they typed without telling them. `stewardNote` is the one attribute no pipeline writes
 * (`statistical-observation` 1.1.0), and it is the one attribute this application sends.
 */
import { SourceError } from "@joinedcontext/sdk";
import type { EntitySource, GridColumn, ResolvedGridConfig } from "@joinedcontext/sdk";

/** The attribute the grant opens, and the only one this application ever writes. */
export const NOTE = "stewardNote";

/**
 * The bound the model puts on the note: `^[^<>]{0,500}$` in `statistical-observation.linkml.yaml`.
 * The gateway does not yet validate writes against a space's published schema (DM-27 is open),
 * so this is where the bound holds today — which is why it is refused here in the model's own
 * terms rather than trimmed quietly.
 */
export const NOTE_MAX = 500;
export const NOTE_FORBIDDEN = /[<>]/;

/** The columns a person needs to recognise a record, in the order they read them. */
export const COLUMNS: readonly string[] = [
  "indicator",
  "refArea",
  "refPeriod",
  "kpiValue",
  "dateObserved",
  NOTE,
];

export interface NoteWords {
  tooLong: (max: number) => string;
  forbidden: string;
  notText: string;
}

/** Why this write cannot be sent, in the reader's own language, or `null` when it can. */
export function noteFault(attrs: Record<string, unknown>, words: NoteWords): string | null {
  for (const [attribute, held] of Object.entries(attrs)) {
    if (attribute !== NOTE) {
      // Unreachable through the grid, which opens one column; kept because it is the property
      // this application promises, and a refusal here is cheaper than a refused write.
      return words.notText;
    }
    // The grid sends an NGSI-LD Property; `?? held` would take the wrapper itself for a note
    // that was emptied, and an emptied note is how a note is removed.
    const wrapped = typeof held === "object" && held !== null && "value" in held;
    const value = wrapped ? (held as { value: unknown }).value : held;
    if (value === null || value === undefined || value === "") continue;
    if (typeof value !== "string") return words.notText;
    if (value.length > NOTE_MAX) return words.tooLong(NOTE_MAX);
    if (NOTE_FORBIDDEN.test(value)) return words.forbidden;
  }
  return null;
}

/**
 * The endpoint, narrowed to what this application does: read the rows, write the note.
 *
 * `remove` is not carried over, so the surface the screen holds cannot delete a published row
 * whatever a later version of the grid decides to offer; a write that breaks the model's bound is
 * refused here and never sent, and the refusal arrives where every other refusal does — beside
 * the row it belongs to (AP-62).
 */
export function notesOnly(base: EntitySource, words: NoteWords): EntitySource {
  const patch = base.patch;
  return {
    query: base.query.bind(base),
    get: base.get.bind(base),
    patch: patch
      ? async (id: string, attrs: Record<string, unknown>) => {
          const fault = noteFault(attrs, words);
          if (fault) throw new SourceError(422, fault);
          await patch.call(base, id, attrs);
        }
      : undefined,
  };
}

/**
 * The grid this application is: one type, six columns, the note open to a reader whose access
 * document grants `updateAttrs` on it (UI-64, UI-71). Since T-5091 that is the steward role alone:
 * everyone else gets the same grid read-only, never an editor whose save the gateway refuses.
 */
export function gridConfig(slug: string, labels: Record<string, string>, mayNote: boolean): ResolvedGridConfig {
  const columns: GridColumn[] = COLUMNS.map((attr) => ({
    attr,
    label: labels[attr] ?? attr,
    // No unit column: the code on the Property is UN/CEFACT's (`C62`, "one"), and the number is
    // drawn with the publisher's own unit text beside it instead (`cells.tsx`, T-2966).
    ...(attr === NOTE && mayNote ? { editable: true } : {}),
  }));
  return {
    source: { kind: "endpoint", slug },
    type: "KeyPerformanceIndicator",
    columns,
    entityTimestamps: false,
    // Every column the grid shows is one the endpoint can answer a `q` about, so a filter is
    // sent to the endpoint and the application never reads a space to filter it.
    filters: { allowed: [...COLUMNS] },
    pageSize: 25,
    mode: mayNote ? "edit" : "view",
    editableAttrs: mayNote ? [NOTE] : [],
    // The steward's grant is `retrieveOps` and `updateAttrs` (T-2434); it reaches no temporal
    // operation, so an offered history would be a 403 on every click.
    history: { enabled: false },
    density: "comfortable",
    rowActions: [],
    map: { enabled: false },
  };
}
