/**
 * Who changed what and when, from the versions the broker keeps (ADR-N-054 2.8, 2.9; T-6333).
 *
 * A `versions=all` temporal answer lists, per attribute, every kept instance with its provenance.
 * A change is a version that replaced an earlier one of the same value: for a value kept by an
 * audited write (`JC-Record: version`, an audit-mode Endpoint) the earlier one is the previous
 * version of that attribute and datasetId; for a measurement it is the earlier version of the same
 * `observedAt` (a correction), so plain readings never read as changes.
 */
import { csvCell } from "../artifact";
import type { Provenance } from "./source";

export interface EntityChange {
  attr: string;
  datasetId?: string;
  /** When the change was written: the version's `writtenAt`, else the time the answer gives it. */
  at: string;
  /** The value it replaced, as the broker kept the instance (value, object, languageMap, ...). */
  before: Record<string, unknown>;
  after: Record<string, unknown>;
  provenance: Provenance;
}

const TEXT_KEYS = [
  "channel",
  "actor",
  "operation",
  "reason",
  "writtenAt",
  "organization",
  "actorKind",
] as const;

function provenanceOf(raw: unknown): { kept: Provenance; audit: boolean } {
  const source =
    typeof raw === "object" && raw !== null
      ? (raw as Record<string, unknown>)
      : {};
  const kept: Provenance = {};
  for (const key of TEXT_KEYS) {
    const value = source[key];
    if (typeof value === "string") kept[key] = value;
  }
  return { kept, audit: source.audit === true };
}

/** Every change of a `versions=all` answer, newest first; anything without a time is dropped. */
export function changesOf(body: unknown): EntityChange[] {
  if (typeof body !== "object" || body === null) return [];
  const changes: EntityChange[] = [];
  for (const [attr, raw] of Object.entries(body as Record<string, unknown>)) {
    if (!Array.isArray(raw)) continue;
    const chains = new Map<
      string,
      {
        at: string;
        instance: Record<string, unknown>;
        provenance: Provenance;
      }[]
    >();
    for (const item of raw) {
      if (typeof item !== "object" || item === null) continue;
      const instance = item as Record<string, unknown>;
      const { kept, audit } = provenanceOf(instance.provenance);
      const at =
        kept.writtenAt ??
        (typeof instance.observedAt === "string"
          ? instance.observedAt
          : undefined);
      if (at === undefined || Number.isNaN(Date.parse(at))) continue;
      const datasetId =
        typeof instance.datasetId === "string" ? instance.datasetId : "";
      const observed =
        typeof instance.observedAt === "string" && !audit
          ? instance.observedAt
          : "";
      const key = JSON.stringify([datasetId, observed]);
      const chain = chains.get(key) ?? [];
      chain.push({ at, instance, provenance: kept });
      chains.set(key, chain);
    }
    for (const chain of chains.values()) {
      chain.sort((a, b) => Date.parse(a.at) - Date.parse(b.at));
      for (let i = 1; i < chain.length; i += 1) {
        const datasetId = chain[i].instance.datasetId;
        changes.push({
          attr,
          ...(typeof datasetId === "string" ? { datasetId } : {}),
          at: chain[i].at,
          before: chain[i - 1].instance,
          after: chain[i].instance,
          provenance: chain[i].provenance,
        });
      }
    }
  }
  return changes.sort((a, b) => Date.parse(b.at) - Date.parse(a.at));
}

/** Who made a change, as a person reads it: the actor without its `user:` or `serviceAccount:` prefix. */
export function actorName(provenance: Provenance): string {
  const actor = provenance.actor ?? "";
  const at = actor.indexOf(":");
  return at >= 0 ? actor.slice(at + 1) : actor;
}

/** The changes of one attribute and of one actor, where either is chosen. */
export function filterChanges(
  changes: EntityChange[],
  attr: string | null,
  actor: string | null,
): EntityChange[] {
  return changes.filter(
    (change) =>
      (!attr || change.attr === attr) &&
      (!actor || change.provenance.actor === actor),
  );
}

/** The raw value of a kept instance, for a spreadsheet. */
function rawValue(instance: Record<string, unknown>): string {
  const value =
    instance.value ??
    instance.object ??
    instance.languageMap ??
    instance.vocab ??
    instance.json;
  if (value === undefined || value === null) return "";
  return typeof value === "object" ? JSON.stringify(value) : String(value);
}

/** The shown changes as CSV: the columns of the timeline, raw values, times in UTC as kept. */
export function changesCsv(changes: EntityChange[]): string {
  const header = [
    "writtenAt",
    "attribute",
    "datasetId",
    "before",
    "after",
    "actor",
    "actorKind",
    "organization",
    "channel",
    "reason",
  ];
  const rows = changes.map((change) =>
    [
      change.at,
      change.attr,
      change.datasetId ?? "",
      rawValue(change.before),
      rawValue(change.after),
      change.provenance.actor ?? "",
      change.provenance.actorKind ?? "",
      change.provenance.organization ?? "",
      change.provenance.channel ?? "",
      change.provenance.reason ?? "",
    ]
      .map(csvCell)
      .join(","),
  );
  return [header.join(","), ...rows].join("\n");
}
