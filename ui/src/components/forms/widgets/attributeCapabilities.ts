import { DATA_WRITE_OPERATIONS, expandOperations } from "../../endpoints/operationGroups";

/**
 * A Policy grant's per-attribute capabilities (ADR-N-066, DM-147): what `information[].attributes`
 * holds in jc-core. A field left unset takes the `*` row's, then the grant's own behaviour.
 */
export interface AttributeCapabilities {
  read?: boolean;
  history?: boolean;
  versions?: VersionsCapability[];
  metadata?: MetadataMember[];
  write?: WriteCapability[];
}

export const VERSIONS_CAPABILITIES = ["read", "keep"] as const;
export type VersionsCapability = (typeof VERSIONS_CAPABILITIES)[number];

/** jc-core `MetadataMember`, in its serde names. */
export const METADATA_MEMBERS = [
  "observedAt",
  "unitCode",
  "createdAt",
  "modifiedAt",
  "modifiedBy.organization",
  "modifiedBy.actorKind",
  "modifiedBy.actor",
  "provenance",
  "reason",
] as const;
export type MetadataMember = (typeof METADATA_MEMBERS)[number];

export const WRITE_CAPABILITIES = ["update", "append", "delete"] as const;
export type WriteCapability = (typeof WRITE_CAPABILITIES)[number];

export type Capabilities = Record<string, AttributeCapabilities>;
export type CapabilityField = keyof AttributeCapabilities;

/** The members a public grant never passes: an actor is a person's `sub` (DM-140, DM-152). */
export const NEVER_PUBLIC: readonly MetadataMember[] = ["modifiedBy.actor", "provenance"];

/** What the grant around one entry allows, which decides which cells can be set at all (DM-148). */
export interface GrantLimits {
  temporal: boolean;
  write: boolean;
  readVersions: boolean;
  public: boolean;
}

interface GrantShape {
  operations?: unknown;
  actions?: unknown;
  assignee?: { kind?: unknown; id?: unknown };
}

const strings = (value: unknown): string[] =>
  Array.isArray(value) ? value.filter((v): v is string => typeof v === "string") : [];

/**
 * The limits jc-core's `validate_capabilities` checks: history needs `queryTemporal` or
 * `retrieveTemporal` named on its own (no group holds them), versions read the action
 * `readVersions`, keep and write a write operation, and a public grant neither writes nor keeps.
 */
export function grantLimits(grant: GrantShape | undefined): GrantLimits {
  const named = strings(grant?.operations);
  const operations = expandOperations(named);
  return {
    temporal: named.includes("queryTemporal") || named.includes("retrieveTemporal"),
    write: operations.some((op) => DATA_WRITE_OPERATIONS.has(op)),
    readVersions: strings(grant?.actions).includes("readVersions"),
    public: grant?.assignee?.kind === "role" && grant.assignee.id === "public",
  };
}

/** Why a column cannot be set under `limits`, as a locale key; `undefined` when it can. */
export function columnBlocked(field: CapabilityField, limits: GrantLimits): string | undefined {
  if (field === "history" && !limits.temporal) return "policies.capabilities.needsTemporal";
  if (field === "write" && limits.public) return "policies.capabilities.neverPublicWrite";
  if (field === "write" && !limits.write) return "policies.capabilities.needsWrite";
  return undefined;
}

/** Why one value of a list column cannot be ticked under `limits`, as a locale key. */
export function valueBlocked(field: CapabilityField, value: string, limits: GrantLimits): string | undefined {
  if (field === "versions" && value === "read" && !limits.readVersions) return "policies.capabilities.needsReadVersions";
  if (field === "versions" && value === "keep" && limits.public) return "policies.capabilities.neverPublicKeep";
  if (field === "versions" && value === "keep" && !limits.write) return "policies.capabilities.needsWrite";
  if (field === "metadata" && limits.public && (NEVER_PUBLIC as readonly string[]).includes(value)) {
    return "policies.capabilities.neverPublicActor";
  }
  return columnBlocked(field, limits);
}

/**
 * The rows of the matrix: `*` first, then the attributes the entry lists, or every attribute the
 * model declares for its types when it lists none, then any key the manifest already holds.
 */
export function matrixRows(listed: readonly string[], declared: readonly string[], held: readonly string[]): string[] {
  const named = listed.length > 0 ? listed : declared;
  return ["*", ...new Set([...named, ...held].filter((name) => name !== "*"))];
}

/**
 * `caps` with `field` of `attribute` set to `value`; `undefined` clears it, an attribute left with
 * nothing set leaves the block, and an empty block is `undefined` so the manifest carries none.
 */
export function withCapability<F extends CapabilityField>(
  caps: Capabilities | undefined,
  attribute: string,
  field: F,
  value: AttributeCapabilities[F] | undefined,
): Capabilities | undefined {
  const row: AttributeCapabilities = { ...(caps?.[attribute] ?? {}) };
  if (value === undefined) delete row[field];
  else row[field] = value;
  const next: Capabilities = { ...(caps ?? {}) };
  if (Object.keys(row).length === 0) delete next[attribute];
  else next[attribute] = row;
  return Object.keys(next).length === 0 ? undefined : next;
}

/** `list` with `value` added or removed, in the order the vocabulary gives. */
export function toggled<T extends string>(vocabulary: readonly T[], list: readonly T[] | undefined, value: T, on: boolean): T[] {
  const set = new Set(list ?? []);
  if (on) set.add(value);
  else set.delete(value);
  return vocabulary.filter((item) => set.has(item));
}
