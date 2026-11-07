/**
 * A form's write, the one thing the kit sends besides a read (AP-61…AP-63). The inputs come
 * from the endpoint's schema when the Portal inlined one; the write is a PATCH of attributes or
 * a POST of a new entity through the app's endpoint and nothing else. In the sandboxed preview
 * the frame has no session, so the write leaves as a message to the page that framed it and
 * comes back as its answer; a published app on the platform origin sends it itself.
 */
import type { Cell, Column, RelationshipObject } from "./ngsi";
import { isRelationshipObject } from "./ngsi";
import { enumOptions } from "./enums";
import type { EnumOption } from "./enums";
import { bridgeTransport, originTransport } from "./sdk/transport";

/** One attribute as the endpoint's schema describes it: a JSON Schema property, trimmed. */
export interface FieldSchema {
  /** A draft-07 type, or a list of them: Model Tools writes `["number", "null"]` for an optional slot. */
  type?: string | string[];
  /** What a person reads as the attribute's label, where the schema gives one. */
  title?: string;
  /** The attribute's description, which the published schema carries for every slot. */
  description?: string;
  /** The NGSI-LD kind Model Tools annotates every property with (DM-05). */
  "x-ngsi-ld-kind"?: string;
  enum?: string[];
  /** A LinkML enum as `gen-json-schema` writes it: a reference, or one inside `anyOf`/`allOf` (UI-86). */
  $ref?: string;
  anyOf?: unknown[];
  oneOf?: unknown[];
  allOf?: unknown[];
  minimum?: number;
  maximum?: number;
  pattern?: string;
  format?: string;
  /** The UN/CEFACT unit Model Tools annotates a quantity with: `ucefact:GQ` among its mappings (DM-06). */
  "x-unit"?: { exactMappings?: string[] };
  /** A stored relationship end and the class it points at (DM-64, T-2739). */
  "x-ngsi-ld-relationship"?: { target?: string };
}

/** One entity type: its properties and which are required. */
export interface TypeSchema {
  properties?: Record<string, FieldSchema>;
  required?: string[];
}

export type Schema = Record<string, TypeSchema>;

export type Input = "number" | "text" | "select" | "date" | "checkbox" | "geo" | "language" | "relation";

export interface Field {
  name: string;
  input: Input;
  /** The permissible values, with the title a person reads for each where the model has one. */
  options?: EnumOption[];
  min?: number;
  max?: number;
  pattern?: string;
  required: boolean;
  /** The UN/CEFACT code a number is measured in, when the model says (DM-06). */
  unit?: string;
  /** A relationship end's target class; the field picks entities of it (UI-84). */
  target?: string;
  /** A relationship end that holds several targets. */
  many?: boolean;
}

/**
 * The input one attribute gets: the schema decides when it names the attribute, the rows otherwise.
 * `defs` resolve a `$ref` (the merged `Schema` holds the enums beside the types), and `language`
 * picks the title of each permissible value (UI-86).
 */
export function fieldOf(name: string, schema: TypeSchema | undefined, kind: Column, defs?: Record<string, unknown>, language?: string): Field {
  const property = schema?.properties?.[name];
  const required = schema?.required?.includes(name) ?? false;
  if (!property) {
    return { name, input: kind === "number" ? "number" : kind === "geo" ? "geo" : kind === "date" ? "date" : "text", required };
  }
  // A relationship end is picked, never typed, and written as a Relationship (DM-64).
  const target = property["x-ngsi-ld-relationship"]?.target;
  if (typeof target === "string" && target !== "") {
    const many = Array.isArray(property.type) ? property.type.includes("array") : property.type === "array";
    return { name, input: "relation", target, many, required };
  }
  const options = enumOptions(property, defs, language);
  if (options) {
    return { name, input: "select", options, required };
  }
  const types = Array.isArray(property.type) ? property.type : property.type ? [property.type] : [];
  const ngsiKind = property["x-ngsi-ld-kind"];
  if (types.includes("number") || types.includes("integer")) {
    const unit = unitCodeOf(property);
    return { name, input: "number", min: property.minimum, max: property.maximum, required, ...(unit ? { unit } : {}) };
  }
  if (types.includes("boolean")) {
    return { name, input: "checkbox", required };
  }
  // A LanguageProperty is an object in the schema too, and its row cell is one language of it:
  // written back as that text it would replace every other language (SDK-07).
  if (ngsiKind === "LanguageProperty") {
    return { name, input: "language", required };
  }
  if (ngsiKind === "GeoProperty" || (ngsiKind === undefined && types.includes("object")) || kind === "geo") {
    return { name, input: "geo", required };
  }
  if (property.format === "date-time" || property.format === "date" || kind === "date") {
    return { name, input: "date", required };
  }
  return { name, input: "text", pattern: property.pattern, required };
}

/** One attribute of a patch as NGSI-LD writes it. */
export type Attribute = { type: "Property"; value: Cell } | { type: "Relationship"; object: string | string[] };

/** The NGSI-LD fragment of a patch: a relationship end a Relationship, every other cell a Property. */
export function attrsOf(patch: Record<string, Cell | RelationshipObject>): Record<string, Attribute> {
  return Object.fromEntries(
    Object.entries(patch).map(([name, value]): [string, Attribute] => [
      name,
      isRelationshipObject(value) ? { type: "Relationship", object: value.object } : { type: "Property", value },
    ]),
  );
}

export interface WriteResult {
  ok: boolean;
  status: number;
  /** The RFC 7807 `detail` (or `title`) when the endpoint refused. */
  detail?: string;
}

export interface Write {
  /** The entity patched; absent for a new one. */
  id?: string;
  type: string;
  /** For a new entity, its id; the patch otherwise. */
  entity?: Record<string, unknown>;
  patch?: Record<string, Cell | RelationshipObject>;
}

/** How long a preview waits for its host page to answer a write. */
export const BRIDGE_TIMEOUT_MS = 15000;

/** The request a write is: method, path under the endpoint, body. */
export function requestOf(slug: string, write: Write): { method: "PATCH" | "POST"; path: string; body: unknown } {
  const base = `/api/endpoint/${encodeURIComponent(slug)}/ngsi-ld/v1/entities`;
  if (write.id) {
    return { method: "PATCH", path: `${base}/${encodeURIComponent(write.id)}/attrs`, body: attrsOf(write.patch ?? {}) };
  }
  return { method: "POST", path: base, body: { type: write.type, ...write.entity } };
}

function detailOf(body: unknown, status: number): string | undefined {
  const problem = (typeof body === "object" && body !== null ? body : {}) as { detail?: unknown; title?: unknown };
  if (typeof problem.detail === "string" && problem.detail !== "") return problem.detail;
  if (typeof problem.title === "string" && problem.title !== "") return problem.title;
  return status >= 400 ? `The endpoint answered ${status}.` : undefined;
}

async function direct(request: ReturnType<typeof requestOf>): Promise<WriteResult> {
  // The SDK's own origin transport: the session, the csrf header and the answer's parsing are its.
  const { status, body } = await originTransport()(request);
  const ok = status >= 200 && status < 300;
  return { ok, status, detail: ok ? undefined : detailOf(body, status) };
}

/** The SDK's bridge: a `jc-request` to the page that framed the preview (SDK-18). */
const host = bridgeTransport({ timeoutMs: BRIDGE_TIMEOUT_MS });

async function viaBridge(request: ReturnType<typeof requestOf>): Promise<WriteResult> {
  const answer = await host(request);
  const ok = answer.status >= 200 && answer.status < 300;
  return { ok, status: answer.status, detail: ok ? undefined : detailOf(answer.body, answer.status) };
}

/** One write through the endpoint: by the host page in a preview, by this document otherwise. */
export function writeEntity(slug: string, write: Write, bridge: boolean): Promise<WriteResult> {
  const request = requestOf(slug, write);
  return bridge ? viaBridge(request) : direct(request).catch((err: unknown) => ({ ok: false, status: 0, detail: err instanceof Error ? err.message : String(err) }));
}

/** The UN/CEFACT code among a property's `x-unit` mappings, and the `unece:` spelling older models used. */
function unitCodeOf(property: FieldSchema): string | undefined {
  for (const mapping of property["x-unit"]?.exactMappings ?? []) {
    const code = /^(?:ucefact|unece):(.+)$/.exec(mapping)?.[1];
    if (code) return code;
  }
  return undefined;
}

/**
 * The UN/CEFACT code each quantity of a type is measured in, by attribute (DM-06): what the views
 * write beside a number, since a key-value row carries no `unitCode` of its own.
 */
export function unitsOf(schema: TypeSchema | undefined): Record<string, string> {
  const units: Record<string, string> = {};
  for (const [name, property] of Object.entries(schema?.properties ?? {})) {
    const code = unitCodeOf(property);
    if (code) units[name] = code;
  }
  return units;
}
