/**
 * The output node's mapper (T-3224): a record's fields mapped onto the attributes an entity type
 * has in the target space's data model (API/01 §36), and the Bloblang compute step that builds
 * the normalized NGSI-LD entity from them. The generated step is what the runner executes and the
 * pipeline test checks (PL-43, PL-59); the map rides in its first line, so the editor can show it
 * again, and only while the step is still exactly what that map generates: a step edited by hand
 * is the author's and the mapper leaves it alone.
 */
import type { components } from "../../api/schema";
import type { LifetimeMode } from "./lifetime";

export type Attribute = components["schemas"]["Attribute"];

/** Where one attribute's value comes from. */
export type Source =
  | { field: string }
  | { expression: string }
  | { value: string | number | boolean }
  | { longitude: string; latitude: string };

export interface MapperState {
  type: string;
  /** The field the entity's local id comes from. */
  idField?: string;
  attributes: Record<string, Source>;
  /**
   * The `expiresAt` a T-4822 step wrote itself. Read back only to move it into the pipeline's
   * `spec.lifetime` (T-4839, PL-80); the mapper never writes it again.
   */
  expires?: Expires;
}

/**
 * An entity's `expiresAt` as the T-4822 header held it: `field` a record field holding a time,
 * absent for the time the record is written; `plus` an ISO 8601 duration.
 */
export interface Expires {
  field?: string;
  plus?: string;
}

/** The longest `plus` a mapper writes: past it the space's own rules are the right tool. */
const LONGEST_PLUS = 3650 * 86_400;

/** Seconds of a `plus` the mapper writes: whole minutes up to 3650 days, else `undefined`. */
export function plusSeconds(plus: string | undefined): number | undefined {
  if (plus === undefined) return 0;
  const match = /^P(?:(\d+)D)?(?:T(?:(\d+)H)?(?:(\d+)M)?)?$/.exec(plus);
  if (!match || plus === "P" || plus.endsWith("T")) return undefined;
  const [, d, h, m] = match.map((part) => Number(part ?? 0));
  const total = d * 86_400 + h * 3600 + m * 60;
  return total <= LONGEST_PLUS ? total : undefined;
}

/**
 * The T-4822 header's `expires` as the entity lifetime it means (T-4839): the write time plus a
 * while is `rolling`; a record field is `from` the date-time attribute the map fills from it.
 * `undefined` when no date-time attribute holds that field: the person then picks one.
 */
export function migrated(expires: Expires, state: MapperState, attributes: Attribute[]): LifetimeMode | undefined {
  const plus = plusSeconds(expires.plus) ? expires.plus : undefined;
  if (expires.field === undefined) return { rolling: plus ?? "PT1M" };
  const slot = attributes.find((attribute) => {
    const source = state.attributes[attribute.name];
    return attribute.format === "date-time" && source !== undefined && "field" in source && source.field === expires.field;
  });
  if (!slot) return undefined;
  return plus ? { from: slot.name, plus } : { from: slot.name };
}

/** The record field a lifetime's `from` attribute is filled from, when the map fills it so. */
export function fromField(lifetime: LifetimeMode | undefined, state: MapperState): string | undefined {
  const source = lifetime?.from ? state.attributes[lifetime.from] : undefined;
  return source && "field" in source ? source.field : undefined;
}

/**
 * The two lines that write `expiresAt`: the base time in Unix seconds, `null` when the record
 * lacks the field (the entity then has no own expiry), plus the while, in UTC. A `plus` the
 * mapper does not write is left out rather than written wrong.
 */
function expiresLines(expires: Expires): string[] {
  const after = plusSeconds(expires.plus) ?? 0;
  const base = expires.field
    ? `if ${fieldPath(expires.field)} != null && ${fieldPath(expires.field)} != "" { ${fieldPath(expires.field)}.ts_unix() } else { null }`
    : "now().ts_unix()";
  return [
    `let expires = ${base}`,
    `root.expiresAt = if $expires != null { ($expires + ${after}).ts_format("2006-01-02T15:04:05Z", "UTC") } else { deleted() }`,
  ];
}

/** The first line of a generated step: the map, so the editor can read it back. */
export const HEADER = "# jc-mapper ";

/** One member name as a Bloblang path segment: a plain name as it is, any other one quoted. */
export function pathSegment(field: string): string {
  return /^[A-Za-z_][A-Za-z0-9_]*$/.test(field) ? field : JSON.stringify(field);
}

/** A record field as a Bloblang path. */
export function fieldPath(field: string): string {
  return `this.${pathSegment(field)}`;
}

/** The member of an NGSI-LD attribute that carries its value, by kind. */
const MEMBER: Record<string, string> = {
  Property: "value",
  Relationship: "object",
  GeoProperty: "value",
  LanguageProperty: "languageMap",
  VocabProperty: "vocab",
};

/** A source as one Bloblang expression, coerced to the slot's type: a CSV field is text. */
function valueOf(source: Source, attribute: Attribute): string {
  // A missing or empty field is `null`, so the attribute is left out; a present value the type
  // cannot read is an error, and the record is refused with it rather than written wrong.
  const present = (path: string) => `${path} != null && ${path} != ""`;
  if ("field" in source) {
    const path = fieldPath(source.field);
    if (attribute.kind !== "Property" || attribute.values) return path;
    const coerced: Record<string, string> = {
      number: `${path}.number()`,
      integer: `${path}.number().round().int64()`,
      boolean: `${path}.bool()`,
      string: `${path}.string()`,
    };
    const read = attribute.valueType ? coerced[attribute.valueType] : undefined;
    return read ? `if ${present(path)} { ${read} } else { null }` : path;
  }
  if ("expression" in source) return `(${source.expression})`;
  if ("value" in source) return JSON.stringify(source.value);
  const longitude = fieldPath(source.longitude);
  const latitude = fieldPath(source.latitude);
  return `if ${present(longitude)} && ${present(latitude)} { { "type": "Point", "coordinates": [ ${longitude}.number(), ${latitude}.number() ] } } else { null }`;
}

/**
 * The compute step for `state` over `attributes`, writing into `space`. An attribute the map
 * names but the model does not has no kind to be written as and is left out, as is one mapped to
 * nothing; a value that is missing on a record leaves the attribute out of that entity, since
 * NGSI-LD has no null attribute.
 */
export function generate(state: MapperState, attributes: Attribute[], space: string): string {
  const lines = [
    `${HEADER}${JSON.stringify(state)}`,
    `let domain = env("JC_ORG_DOMAIN")`,
    "root = {}",
  ];
  if (state.idField) {
    lines.push(
      `root.id = "urn:ngsi-ld:%v:%v:%v:%v".format(${JSON.stringify(state.type)}, $domain, ${JSON.stringify(space)}, ${fieldPath(state.idField)}.string().re_replace_all("[^A-Za-z0-9._~-]", "-"))`,
    );
  }
  lines.push(`root.type = ${JSON.stringify(state.type)}`);
  attributes.forEach((attribute, index) => {
    const source = state.attributes[attribute.name];
    if (!source) return;
    const member = MEMBER[attribute.kind] ?? "value";
    const unit = attribute.kind === "Property" && attribute.unit?.code ? `, "unitCode": ${JSON.stringify(attribute.unit.code)}` : "";
    const name = /^[A-Za-z_][A-Za-z0-9_]*$/.test(attribute.name) ? attribute.name : JSON.stringify(attribute.name);
    lines.push(`let a${index} = ${valueOf(source, attribute)}`);
    lines.push(
      `root.${name} = if $a${index} != null { { "type": ${JSON.stringify(attribute.kind)}, ${JSON.stringify(member)}: $a${index}${unit} } } else { deleted() }`,
    );
  });
  if (state.expires) lines.push(...expiresLines(state.expires));
  return `${lines.join("\n")}\n`;
}

/**
 * The map a step was generated from, when it was and still is exactly that: `undefined` for a
 * step written or edited by hand, which the editor then shows as code.
 */
export function readBack(bloblang: string | undefined, attributes: Attribute[], space: string): MapperState | undefined {
  if (!bloblang?.startsWith(HEADER)) return undefined;
  const first = bloblang.slice(HEADER.length).split("\n", 1)[0];
  let state: MapperState;
  try {
    state = JSON.parse(first) as MapperState;
  } catch {
    return undefined;
  }
  if (typeof state?.type !== "string" || typeof state.attributes !== "object" || state.attributes === null) return undefined;
  return generate(state, attributes, space) === bloblang ? state : undefined;
}

const folded = (name: string) => name.normalize("NFD").replace(/\p{Diacritic}/gu, "").replace(/[^A-Za-z0-9]/g, "").toLowerCase();

/** Each attribute matched to the field of the same name, case, accents and separators aside. */
export function autoMatch(attributes: Attribute[], fields: string[], already: Record<string, Source> = {}): Record<string, Source> {
  const matched: Record<string, Source> = { ...already };
  for (const attribute of attributes) {
    if (matched[attribute.name]) continue;
    const field = fields.find((candidate) => folded(candidate) === folded(attribute.name));
    if (field !== undefined) matched[attribute.name] = { field };
  }
  return matched;
}

/** The field an id most likely comes from: `id`, then anything ending in `id`, else none. */
export function idFieldOf(fields: string[]): string | undefined {
  return fields.find((f) => folded(f) === "id") ?? fields.find((f) => folded(f).endsWith("id"));
}

export type MapperProblem =
  | { attribute: string; kind: "required" }
  | { attribute: string; kind: "notInSample"; field: string }
  | { attribute: string; kind: "notNumber"; field: string; value: string }
  | { attribute: string; kind: "notBoolean"; field: string; value: string }
  | { attribute: string; kind: "notValue"; field: string; value: string; values: string[] }
  | { attribute: ""; kind: "noId" }
  | { attribute: "expiresAt"; kind: "notTime"; field: string; value: string };

/** Unix seconds of a time as the runner's `ts_unix` reads it: RFC 3339 text or a number. */
function timeOf(value: unknown): number | undefined {
  if (typeof value === "number" && Number.isFinite(value)) return Math.trunc(value);
  if (typeof value !== "string" || !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:\d{2})$/.test(value)) return undefined;
  const ms = Date.parse(value);
  return Number.isNaN(ms) ? undefined : Math.floor(ms / 1000);
}

const isNumber = (value: unknown) =>
  typeof value === "number" || (typeof value === "string" && value.trim() !== "" && Number.isFinite(Number(value)));

/** A time field the sample lacks or cannot read as a time, named at `expiresAt`. */
function expiresProblems(timed: string | undefined, records: Record<string, unknown>[], fields: Set<string>): MapperProblem[] {
  if (timed === undefined || records.length === 0) return [];
  if (!fields.has(timed)) return [{ attribute: "expiresAt", kind: "notInSample", field: timed }];
  const value = records.map((record) => record[timed]).find((v) => v !== undefined && v !== null && v !== "");
  if (value === undefined || timeOf(value) !== undefined) return [];
  return [{ attribute: "expiresAt", kind: "notTime", field: timed, value: typeof value === "string" ? value : JSON.stringify(value) }];
}

/**
 * What would stop the entities from being written, in the sample's terms (T-3224): a required
 * attribute mapped to nothing, a field the sample does not have, a value the slot's type cannot
 * read. Each names its attribute, so the form shows it there before anything is saved.
 */
export function problemsOf(
  state: MapperState,
  attributes: Attribute[],
  records: Record<string, unknown>[],
  lifetime?: LifetimeMode,
): MapperProblem[] {
  const problems: MapperProblem[] = [];
  if (!state.idField) problems.push({ attribute: "", kind: "noId" });
  const fields = new Set(records.flatMap((record) => Object.keys(record)));
  problems.push(...expiresProblems(fromField(lifetime, state) ?? state.expires?.field, records, fields));
  for (const attribute of attributes) {
    const source = state.attributes[attribute.name];
    if (!source) {
      if (attribute.required) problems.push({ attribute: attribute.name, kind: "required" });
      continue;
    }
    if (!("field" in source)) continue;
    if (records.length > 0 && !fields.has(source.field)) {
      problems.push({ attribute: attribute.name, kind: "notInSample", field: source.field });
      continue;
    }
    const value = records.map((record) => record[source.field]).find((v) => v !== undefined && v !== null && v !== "");
    if (value === undefined || attribute.kind !== "Property") continue;
    const shown = typeof value === "string" ? value : JSON.stringify(value);
    if (attribute.values && !attribute.values.includes(String(value))) {
      problems.push({ attribute: attribute.name, kind: "notValue", field: source.field, value: shown, values: attribute.values });
    } else if ((attribute.valueType === "number" || attribute.valueType === "integer") && !isNumber(value)) {
      problems.push({ attribute: attribute.name, kind: "notNumber", field: source.field, value: shown });
    } else if (attribute.valueType === "boolean" && !["true", "false", true, false].includes(value as string | boolean)) {
      problems.push({ attribute: attribute.name, kind: "notBoolean", field: source.field, value: shown });
    }
  }
  return problems;
}

/** The `expiresAt` the lifetime step writes for `record`, as the runner formats it; none without a time. */
function expiresAtOf(lifetime: LifetimeMode, state: MapperState, record: Record<string, unknown>, now: number): string | undefined {
  let base: number | undefined;
  let span: string | undefined;
  if (lifetime.at !== undefined) {
    base = timeOf(lifetime.at);
  } else if (lifetime.from === undefined) {
    base = Math.floor(now / 1000);
    span = lifetime.rolling;
  } else {
    const field = fromField(lifetime, state);
    base = field === undefined ? undefined : timeOf(record[field]);
    span = lifetime.plus;
  }
  if (base === undefined) return undefined;
  return new Date((base + (plusSeconds(span) ?? 0)) * 1000).toISOString().replace(/\.\d{3}Z$/, "Z");
}

/**
 * Up to three of the entities the step would write from the sample, as NGSI-LD, computed the way
 * the generated step computes them. An expression is the runner's to evaluate, so it shows as
 * such; the pipeline test runs the real step.
 */
export function preview(
  state: MapperState,
  attributes: Attribute[],
  records: Record<string, unknown>[],
  space: string,
  orgDomain: string,
  now: number = Date.now(),
  lifetime?: LifetimeMode,
): Record<string, unknown>[] {
  return records.slice(0, 3).map((record) => {
    const entity: Record<string, unknown> = {};
    if (state.idField) {
      const local = String(record[state.idField] ?? "").replace(/[^A-Za-z0-9._~-]/g, "-");
      entity.id = `urn:ngsi-ld:${state.type}:${orgDomain}:${space}:${local}`;
    }
    entity.type = state.type;
    for (const attribute of attributes) {
      const source = state.attributes[attribute.name];
      if (!source) continue;
      let value: unknown;
      if ("field" in source) {
        value = record[source.field];
        if (value !== undefined && value !== null && attribute.kind === "Property" && !attribute.values) {
          if (attribute.valueType === "number" && isNumber(value)) value = Number(value);
          if (attribute.valueType === "integer" && isNumber(value)) value = Math.round(Number(value));
          if (attribute.valueType === "boolean") value = value === true || value === "true";
          if (attribute.valueType === "string") value = String(value);
        }
      } else if ("value" in source) {
        value = source.value;
      } else if ("longitude" in source) {
        value = { type: "Point", coordinates: [Number(record[source.longitude]), Number(record[source.latitude])] };
      } else {
        value = `(${source.expression})`;
      }
      if (value === undefined || value === null || value === "") continue;
      const member = MEMBER[attribute.kind] ?? "value";
      entity[attribute.name] = {
        type: attribute.kind,
        [member]: value,
        ...(attribute.kind === "Property" && attribute.unit?.code && typeof value === "number" ? { unitCode: attribute.unit.code } : {}),
      };
    }
    const ends = lifetime ?? (state.expires ? migrated(state.expires, state, attributes) : undefined);
    const expiresAt = ends ? expiresAtOf(ends, state, record, now) : undefined;
    if (expiresAt !== undefined) entity.expiresAt = expiresAt;
    return entity;
  });
}
