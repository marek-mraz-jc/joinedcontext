/**
 * The control a grid cell edits a manifest field with, read off the kind's jc-core JSON Schema
 * (T-6115): an enum is a dropdown, an array of enums a set of checkboxes, a reference to another
 * manifest a picker of what the person may read, a boolean a switch, a number a number input, a
 * duration or any other string a text input. The schema decides, so a kind that grows a value
 * grows its dropdown without a page edit.
 */
import { REFERENCE_PICKERS } from "../../schemas/pickers";

type Schema = Record<string, unknown>;

export interface EnumChoice {
  value: string;
  /** The schema's own sentence on the value, shown as its hint. */
  description?: string;
}

export type FieldWidget =
  | { kind: "enum"; options: EnumChoice[]; nullable: boolean }
  | { kind: "multiEnum"; options: EnumChoice[] }
  | { kind: "reference"; plural: string }
  /** `absent`: what an unset switch means, read off the schema's default or its own words. */
  | { kind: "boolean"; absent: boolean }
  | { kind: "number"; integer: boolean; minimum?: number }
  | { kind: "duration" }
  | { kind: "dateTime" }
  | { kind: "text" };

export interface ResolvedField {
  widget: FieldWidget;
  /** The schema's description of the field, the cell panel's hint. */
  description?: string;
}

function isSchema(value: unknown): value is Schema {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** The definition a `#/definitions/X` (or `#/$defs/X`) pointer names in `root`. */
function deref(root: Schema, ref: string): Schema | undefined {
  const match = /^#\/(definitions|\$defs)\/(.+)$/.exec(ref);
  if (!match) return undefined;
  const table = root[match[1]];
  const found = isSchema(table) ? table[match[2]] : undefined;
  return isSchema(found) ? found : undefined;
}

/**
 * The schema of a value with its wrappers taken off: a lone `allOf`, an `anyOf` of the value and
 * `null`, a `$ref`. The outermost description is kept, since jc-core writes the field's own
 * sentence beside the `$ref` it points at.
 */
function unwrap(root: Schema, schema: Schema, depth = 0): { schema: Schema; nullable: boolean } {
  if (depth > 16) return { schema, nullable: false };
  const allOf = schema.allOf;
  if (Array.isArray(allOf) && allOf.length === 1 && isSchema(allOf[0])) {
    const inner = unwrap(root, allOf[0], depth + 1);
    return { schema: { ...inner.schema, description: schema.description ?? inner.schema.description }, nullable: inner.nullable };
  }
  const anyOf = schema.anyOf;
  if (Array.isArray(anyOf)) {
    const real = anyOf.filter((each) => isSchema(each) && each.type !== "null");
    if (real.length === 1 && isSchema(real[0])) {
      const inner = unwrap(root, real[0], depth + 1);
      return { schema: { ...inner.schema, description: schema.description ?? inner.schema.description }, nullable: real.length < anyOf.length || inner.nullable };
    }
  }
  if (typeof schema.$ref === "string") {
    const target = deref(root, schema.$ref);
    if (target) {
      const inner = unwrap(root, target, depth + 1);
      return { schema: { ...inner.schema, description: schema.description ?? inner.schema.description }, nullable: inner.nullable };
    }
  }
  const type = schema.type;
  if (Array.isArray(type) && type.includes("null")) {
    const rest = type.filter((each) => each !== "null");
    return { schema: { ...schema, type: rest.length === 1 ? rest[0] : rest }, nullable: true };
  }
  return { schema, nullable: false };
}

/** The values an enum offers: `enum`, or a `oneOf` of single-value enums or consts, each with its sentence. */
function choices(schema: Schema): EnumChoice[] | null {
  if (Array.isArray(schema.enum)) {
    return schema.enum.filter((each): each is string => typeof each === "string").map((value) => ({ value }));
  }
  const oneOf = schema.oneOf;
  if (!Array.isArray(oneOf) || oneOf.length === 0) return null;
  const out: EnumChoice[] = [];
  for (const each of oneOf) {
    if (!isSchema(each)) return null;
    const values = Array.isArray(each.enum) ? each.enum : "const" in each ? [each.const] : null;
    if (!values) return null;
    for (const value of values) {
      if (typeof value !== "string") return null;
      out.push({ value, description: typeof each.description === "string" ? each.description : undefined });
    }
  }
  return out;
}

/** The schema of `path` (dot-separated, under `spec`) in a kind's schema, unwrapped. */
function fieldSchema(root: Schema, path: string): { schema: Schema; nullable: boolean } | null {
  let at = unwrap(root, root);
  for (const step of path.split(".")) {
    const properties = at.schema.properties;
    const next = isSchema(properties) ? properties[step] : undefined;
    if (!isSchema(next)) return null;
    at = unwrap(root, next);
  }
  return at;
}

/** A string the schema describes as a Bento or ISO duration (`15s`, `5m`, `P1D`). */
function isDuration(schema: Schema): boolean {
  if (schema.format === "duration") return true;
  const text = typeof schema.description === "string" ? schema.description : "";
  return /\bduration\b/i.test(text);
}

/**
 * The widget for `spec.<path>` of `kind`, or `null` when the schema has no such field or it is a
 * structure a single cell cannot hold (an object, a list of objects): those stay in the full form.
 */
export function fieldWidget(kind: string, schema: unknown, path: string): ResolvedField | null {
  if (!isSchema(schema)) return null;
  const found = fieldSchema(schema, `spec.${path}`);
  if (!found) return null;
  const description = typeof found.schema.description === "string" ? found.schema.description : undefined;
  const plural = REFERENCE_PICKERS[kind]?.[path]?.["ui:options"];
  if (REFERENCE_PICKERS[kind]?.[path]?.["ui:widget"] === "resourcePicker" && isSchema(plural) && typeof plural.plural === "string") {
    return { widget: { kind: "reference", plural: plural.plural }, description };
  }
  const own = choices(found.schema);
  if (own) return { widget: { kind: "enum", options: own, nullable: found.nullable }, description };
  const type = found.schema.type;
  if (type === "boolean") {
    // jc-core says "Absent means `true`" or "`false` turns X off" where it writes no default.
    const absent = found.schema.default === true || /absent means `?true|^`false` turns/i.test(description ?? "");
    return { widget: { kind: "boolean", absent }, description };
  }
  if (type === "integer" || type === "number") {
    const minimum = typeof found.schema.minimum === "number" ? found.schema.minimum : undefined;
    return { widget: { kind: "number", integer: type === "integer", minimum }, description };
  }
  if (type === "array" && isSchema(found.schema.items)) {
    const items = choices(unwrap(schema, found.schema.items).schema);
    return items ? { widget: { kind: "multiEnum", options: items }, description } : null;
  }
  if (type === "string") {
    if (found.schema.format === "date-time") return { widget: { kind: "dateTime" }, description };
    return { widget: isDuration(found.schema) ? { kind: "duration" } : { kind: "text" }, description };
  }
  return null;
}

/** A schema sentence as a hint: no requirement ids, no code ticks, no rustdoc links. */
export function hintText(description: string | undefined): string | undefined {
  if (!description) return undefined;
  const first = description.split(/(?<=\.)\s/)[0];
  return first
    // A switch's sentence opens with the literal it is about ("`false` turns ... off"): say it as the switch reads.
    .replace(/^`?true`?(?=\s)/, "On")
    .replace(/^`?false`?(?=\s)/, "Off")
    .replace(/\s*\((?:[A-Z]{2,5}-\d+|T-\d+)(?:[,;]\s*(?:[A-Z]{2,5}-\d+|T-\d+|[^)]*))*\)/g, "")
    .replace(/\[`([^`\]]+)`\]/g, "$1")
    .replace(/`/g, "")
    .trim();
}

/** The value at `spec.<path>` of a manifest; a reference reads as its name. */
export function specValue(manifest: { spec?: unknown }, path: string): unknown {
  let at: unknown = manifest.spec;
  for (const step of path.split(".")) {
    if (!isSchema(at)) return undefined;
    at = at[step];
  }
  return at;
}

/**
 * The manifest with `spec.<path>` set to `value`, every other member as it was; `undefined`
 * removes the member, as clearing an optional field means "the platform's default".
 */
export function withSpecValue<T extends { spec?: unknown }>(manifest: T, path: string, value: unknown): T {
  const steps = path.split(".");
  const write = (at: unknown, index: number): Schema => {
    const copy: Schema = isSchema(at) ? { ...at } : {};
    const step = steps[index];
    if (index === steps.length - 1) {
      if (value === undefined) delete copy[step];
      else copy[step] = value;
    } else {
      copy[step] = write(copy[step], index + 1);
    }
    return copy;
  };
  return { ...manifest, spec: write(manifest.spec, 0) };
}
