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

/**
 * A value transform (G-18): a menu item of the mapper, each one fixed Bloblang template. The
 * person picks them; nothing typed here becomes code, only string literals (F-R2UI4-007).
 */
export type Transform =
  | { op: "trim" }
  | { op: "lower" }
  | { op: "upper" }
  | { op: "decimalComma" }
  | { op: "date"; format: string }
  | { op: "lookup"; table: Record<string, string> }
  | { op: "default"; value: string };

/** The date formats the `date` transform offers, as `ts_strptime` reads them. */
export const DATE_FORMATS = ["%d.%m.%Y", "%d/%m/%Y", "%m/%d/%Y", "%Y-%m-%d", "%d.%m.%Y %H:%M", "%d/%m/%Y %H:%M", "%Y-%m-%d %H:%M:%S"] as const;

/** The coordinate systems the runner's `geo_transform` knows (ekiben geo.rs), WGS 84 first. */
export const CRS_CODES = [
  "EPSG:4326",
  "EPSG:4258",
  "EPSG:3857",
  "EPSG:25829",
  "EPSG:25830",
  "EPSG:25831",
  "EPSG:25832",
  "EPSG:25833",
  "EPSG:25834",
  "EPSG:3879",
  "EPSG:3067",
  "EPSG:5514",
  "EPSG:2056",
  "EPSG:27700",
] as const;

/** Where one attribute's value comes from. */
export type Source =
  | { field: string; transforms?: Transform[] }
  | { expression: string }
  | { value: string | number | boolean }
  | { longitude: string; latitude: string; crs?: string }
  /** A Relationship to the entity of `type` in the same space whose id comes from `field`. */
  | { ref: { type: string; field: string } }
  /** A LanguageProperty: one field per language tag. */
  | { languages: Record<string, string> }
  /** A GeoProperty from one field holding GeoJSON or WKT, in `crs` when not WGS 84. */
  | { geometry: string; format: "geojson" | "wkt"; crs?: string }
  /** Several fields joined into one text, the empty ones skipped. */
  | { concat: string[]; separator: string };

export interface MapperState {
  type: string;
  /** The field the entity's local id comes from. */
  idField?: string;
  attributes: Record<string, Source>;
  /** Per attribute, the record field its `observedAt` comes from (G-18). */
  observedAt?: Record<string, string>;
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

// A missing or empty field is `null`, so the attribute is left out; a present value the type
// cannot read is an error, and the record is refused with it rather than written wrong.
const present = (path: string) => `${path} != null && ${path} != ""`;

/** The id-safe form of a local id, as the step writes an entity's own id. */
const localId = (expression: string) => `${expression}.string().re_replace_all("[^A-Za-z0-9._~-]", "-")`;

/** The time format of an attribute the `date` transform writes. */
const timeLayout = (attribute: Attribute) => (attribute.format === "date" ? "2006-01-02" : "2006-01-02T15:04:05Z");

/** One transform applied to `expression`. */
function transformed(expression: string, transform: Transform, attribute: Attribute): string {
  switch (transform.op) {
    case "trim":
      return `${expression}.string().trim()`;
    case "lower":
      return `${expression}.string().lowercase()`;
    case "upper":
      return `${expression}.string().uppercase()`;
    case "decimalComma":
      return `${expression}.string().replace_all(" ", "").replace_all(".", "").replace_all(",", ".")`;
    case "date":
      return `${expression}.string().ts_strptime(${JSON.stringify(transform.format)}).ts_format(${JSON.stringify(timeLayout(attribute))}, "UTC")`;
    case "lookup": {
      const cases = Object.entries(transform.table).map(([from, to]) => `${JSON.stringify(from)} => ${JSON.stringify(to)}`);
      return cases.length === 0 ? expression : `match ${expression}.string() { ${[...cases, "_ => this"].join(", ")} }`;
    }
    case "default":
      return expression;
  }
}

/** A typed default as a Bloblang literal of the slot's type. */
function literalOf(value: string, attribute: Attribute): string {
  if (attribute.kind === "Property" && !attribute.values) {
    if ((attribute.valueType === "number" || attribute.valueType === "integer") && isNumber(value)) return String(Number(value));
    if (attribute.valueType === "boolean" && (value === "true" || value === "false")) return value;
  }
  return JSON.stringify(value);
}

/** `expression` read as the slot's type: a CSV field is text. */
function coerced(expression: string, attribute: Attribute): string | undefined {
  if (attribute.kind !== "Property" || attribute.values) return undefined;
  const read: Record<string, string> = {
    number: `${expression}.number()`,
    integer: `${expression}.number().round().int64()`,
    boolean: `${expression}.bool()`,
    string: `${expression}.string()`,
  };
  return attribute.valueType ? read[attribute.valueType] : undefined;
}

/** A geometry, reprojected to WGS 84 when it is in another system. */
const toWgs84 = (expression: string, crs: string | undefined) =>
  crs && crs !== "EPSG:4326" ? `${expression}.geo_transform(${JSON.stringify(crs)}, "EPSG:4326")` : expression;

/** A source as one Bloblang expression, coerced to the slot's type. */
function valueOf(source: Source, attribute: Attribute, space: string): string {
  if ("field" in source) {
    const path = fieldPath(source.field);
    const steps = (source.transforms ?? []).filter((transform) => transform.op !== "default");
    const fallback = source.transforms?.find((transform) => transform.op === "default");
    if (steps.length === 0 && !fallback) {
      const read = coerced(path, attribute);
      return read ? `if ${present(path)} { ${read} } else { null }` : path;
    }
    const shaped = steps.reduce((expression, transform) => transformed(expression, transform, attribute), path);
    const otherwise = fallback?.op === "default" ? literalOf(fallback.value, attribute) : "null";
    return `if ${present(path)} { ${coerced(shaped, attribute) ?? shaped} } else { ${otherwise} }`;
  }
  if ("expression" in source) return `(${source.expression})`;
  if ("value" in source) return JSON.stringify(source.value);
  if ("ref" in source) {
    const path = fieldPath(source.ref.field);
    return `if ${present(path)} { "urn:ngsi-ld:%v:%v:%v:%v".format(${JSON.stringify(source.ref.type)}, $owner, ${JSON.stringify(space)}, ${localId(path)}) } else { null }`;
  }
  if ("languages" in source) {
    const tags = Object.entries(source.languages);
    if (tags.length === 0) return "null";
    const texts = tags.map(([tag, field]) => `${JSON.stringify(tag)}: if ${present(fieldPath(field))} { ${fieldPath(field)}.string() } else { deleted() }`);
    const any = tags.map(([, field]) => `(${present(fieldPath(field))})`).join(" || ");
    return `if ${any} { { ${texts.join(", ")} } } else { null }`;
  }
  if ("geometry" in source) {
    const path = fieldPath(source.geometry);
    const read = source.format === "wkt" ? `${path}.string().parse_wkt()` : `if ${path}.type() == "string" { ${path}.parse_json() } else { ${path} }`;
    return `if ${present(path)} { ${toWgs84(`(${read})`, source.crs)} } else { null }`;
  }
  if ("concat" in source) {
    if (source.concat.length === 0) return "null";
    const parts = source.concat.map((field) => `if ${present(fieldPath(field))} { ${fieldPath(field)}.string() } else { deleted() }`);
    return `[ ${parts.join(", ")} ].join(${JSON.stringify(source.separator)}).(joined -> if joined != "" { joined } else { null })`;
  }
  const longitude = fieldPath(source.longitude);
  const latitude = fieldPath(source.latitude);
  const point = `{ "type": "Point", "coordinates": [ ${longitude}.number(), ${latitude}.number() ] }`;
  return `if ${present(longitude)} && ${present(latitude)} { ${source.crs && source.crs !== "EPSG:4326" ? toWgs84(`(${point})`, source.crs) : point} } else { null }`;
}

/** Every record field a source reads. */
export function fieldsOf(source: Source): string[] {
  if ("field" in source) return [source.field];
  if ("ref" in source) return [source.ref.field];
  if ("languages" in source) return Object.values(source.languages);
  if ("geometry" in source) return [source.geometry];
  if ("concat" in source) return source.concat;
  if ("longitude" in source) return [source.longitude, source.latitude];
  return [];
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
    `let owner = env("JC_ORG_ID")`,
    "root = {}",
  ];
  if (state.idField) {
    lines.push(
      `root.id = "urn:ngsi-ld:%v:%v:%v:%v".format(${JSON.stringify(state.type)}, $owner, ${JSON.stringify(space)}, ${fieldPath(state.idField)}.string().re_replace_all("[^A-Za-z0-9._~-]", "-"))`,
    );
  }
  lines.push(`root.type = ${JSON.stringify(state.type)}`);
  attributes.forEach((attribute, index) => {
    const source = state.attributes[attribute.name];
    if (!source) return;
    const member = MEMBER[attribute.kind] ?? "value";
    const unit = attribute.kind === "Property" && attribute.unit?.code ? `, "unitCode": ${JSON.stringify(attribute.unit.code)}` : "";
    const name = /^[A-Za-z_][A-Za-z0-9_]*$/.test(attribute.name) ? attribute.name : JSON.stringify(attribute.name);
    const observed = state.observedAt?.[attribute.name];
    const at =
      observed === undefined
        ? ""
        : `, "observedAt": if ${present(fieldPath(observed))} { ${fieldPath(observed)}.ts_unix().ts_format("2006-01-02T15:04:05Z", "UTC") } else { deleted() }`;
    lines.push(`let a${index} = ${valueOf(source, attribute, space)}`);
    lines.push(
      `root.${name} = if $a${index} != null { { "type": ${JSON.stringify(attribute.kind)}, ${JSON.stringify(member)}: $a${index}${unit}${at} } } else { deleted() }`,
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
  | { attribute: string; kind: "notShaped"; field: string; value: string }
  | { attribute: ""; kind: "noId" }
  | { attribute: string; kind: "notTime"; field: string; value: string };

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

/** `strftime` text read with one of DATE_FORMATS, as UTC; `undefined` when it does not fit. */
function parsedDate(text: string, format: string): Date | undefined {
  const parts: string[] = [];
  const pattern = format.replace(/[.*+?^${}()|[\]\\/]/g, "\\$&").replace(/%([dmYHMS])/g, (_, part: string) => {
    parts.push(part);
    return part === "Y" ? "(\\d{4})" : "(\\d{1,2})";
  });
  const match = new RegExp(`^${pattern}$`).exec(text.trim());
  if (!match) return undefined;
  const at: Record<string, number> = { Y: 1970, m: 1, d: 1, H: 0, M: 0, S: 0 };
  parts.forEach((part, index) => (at[part] = Number(match[index + 1])));
  const date = new Date(Date.UTC(at.Y, at.m - 1, at.d, at.H, at.M, at.S));
  return date.getUTCDate() === at.d && date.getUTCMonth() === at.m - 1 ? date : undefined;
}

/**
 * A field's value after its transforms, computed here the way the generated step computes it:
 * `undefined` when a transform cannot read it (a date that does not fit the format), which the
 * runner refuses the record for.
 */
export function shapedValue(value: unknown, transforms: Transform[] | undefined, attribute: Attribute): unknown {
  let shaped = value;
  for (const transform of transforms ?? []) {
    if (shaped === undefined || shaped === null || shaped === "") break;
    const text = typeof shaped === "string" ? shaped : JSON.stringify(shaped);
    switch (transform.op) {
      case "trim":
        shaped = text.trim();
        break;
      case "lower":
        shaped = text.toLowerCase();
        break;
      case "upper":
        shaped = text.toUpperCase();
        break;
      case "decimalComma":
        shaped = text.replaceAll(" ", "").replaceAll(".", "").replaceAll(",", ".");
        break;
      case "date": {
        const date = parsedDate(text, transform.format);
        if (!date) return undefined;
        const iso = date.toISOString().replace(/\.\d{3}Z$/, "Z");
        shaped = attribute.format === "date" ? iso.slice(0, 10) : iso;
        break;
      }
      case "lookup":
        shaped = Object.hasOwn(transform.table, text) ? transform.table[text] : text;
        break;
      case "default":
        break;
    }
  }
  return shaped;
}

/** The transform `default`'s value of a field source, when it has one. */
const defaultOf = (transforms: Transform[] | undefined) => transforms?.find((transform) => transform.op === "default")?.value;

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
  for (const [name, field] of Object.entries(state.observedAt ?? {})) {
    problems.push(...expiresProblems(field, records, fields).map((problem) => ({ ...problem, attribute: name }) as MapperProblem));
  }
  for (const attribute of attributes) {
    const source = state.attributes[attribute.name];
    if (!source) {
      if (attribute.required) problems.push({ attribute: attribute.name, kind: "required" });
      continue;
    }
    const missing = records.length > 0 ? fieldsOf(source).find((field) => field !== "" && !fields.has(field)) : undefined;
    if (missing !== undefined) {
      problems.push({ attribute: attribute.name, kind: "notInSample", field: missing });
      continue;
    }
    if (!("field" in source)) continue;
    if (records.length > 0 && !fields.has(source.field)) {
      problems.push({ attribute: attribute.name, kind: "notInSample", field: source.field });
      continue;
    }
    const raw = records.map((record) => record[source.field]).find((v) => v !== undefined && v !== null && v !== "");
    if (raw === undefined || attribute.kind !== "Property") continue;
    // The checks read the value the transforms make of the field, as the runner writes it.
    const value = shapedValue(raw, source.transforms, attribute);
    if (value === undefined) {
      problems.push({ attribute: attribute.name, kind: "notShaped", field: source.field, value: typeof raw === "string" ? raw : JSON.stringify(raw) });
      continue;
    }
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
        value = shapedValue(record[source.field], source.transforms, attribute);
        if (value === undefined || value === null || value === "") value = defaultOf(source.transforms);
        if (value !== undefined && value !== null && attribute.kind === "Property" && !attribute.values) {
          if (attribute.valueType === "number" && isNumber(value)) value = Number(value);
          if (attribute.valueType === "integer" && isNumber(value)) value = Math.round(Number(value));
          if (attribute.valueType === "boolean") value = value === true || value === "true";
          if (attribute.valueType === "string") value = String(value);
        }
      } else if ("value" in source) {
        value = source.value;
      } else if ("longitude" in source && !(source.crs && source.crs !== "EPSG:4326")) {
        value = { type: "Point", coordinates: [Number(record[source.longitude]), Number(record[source.latitude])] };
      } else if ("ref" in source) {
        const key = record[source.ref.field];
        value = key === undefined || key === null || key === "" ? undefined : `urn:ngsi-ld:${source.ref.type}:${orgDomain}:${space}:${String(key).replace(/[^A-Za-z0-9._~-]/g, "-")}`;
      } else if ("languages" in source) {
        const texts = Object.entries(source.languages).filter(([, field]) => record[field] !== undefined && record[field] !== null && record[field] !== "");
        value = texts.length > 0 ? Object.fromEntries(texts.map(([tag, field]) => [tag, String(record[field])])) : undefined;
      } else if ("concat" in source) {
        value = source.concat.map((field) => record[field]).filter((part) => part !== undefined && part !== null && part !== "").map(String).join(source.separator);
      } else if ("geometry" in source && source.format === "geojson" && !(source.crs && source.crs !== "EPSG:4326")) {
        const raw = record[source.geometry];
        try {
          value = typeof raw === "string" && raw !== "" ? JSON.parse(raw) : raw;
        } catch {
          value = raw;
        }
      } else if ("geometry" in source || "longitude" in source) {
        // WKT and a reprojection are the runner's to compute: the sample's text shows instead.
        value = fieldsOf(source).map((field) => String(record[field] ?? "")).join(" ");
        if (value === "" || value === " ") value = undefined;
        else value = `(${value})`;
      } else {
        value = `(${source.expression})`;
      }
      if (value === undefined || value === null || value === "") continue;
      const member = MEMBER[attribute.kind] ?? "value";
      const observed = state.observedAt?.[attribute.name];
      const observedAt = observed === undefined ? undefined : timeOf(record[observed]);
      entity[attribute.name] = {
        type: attribute.kind,
        [member]: value,
        ...(attribute.kind === "Property" && attribute.unit?.code && typeof value === "number" ? { unitCode: attribute.unit.code } : {}),
        ...(observedAt !== undefined ? { observedAt: new Date(observedAt * 1000).toISOString().replace(/\.\d{3}Z$/, "Z") } : {}),
      };
    }
    const ends = lifetime ?? (state.expires ? migrated(state.expires, state, attributes) : undefined);
    const expiresAt = ends ? expiresAtOf(ends, state, record, now) : undefined;
    if (expiresAt !== undefined) entity.expiresAt = expiresAt;
    return entity;
  });
}
