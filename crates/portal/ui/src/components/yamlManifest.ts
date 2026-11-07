/**
 * What the YAML view of a manifest knows about its text (T-5052): which field a position names,
 * on which line a field stands, what the kind's schema says a field is for, and which lines a
 * parse error or a refusal points at. No editor here, so it is tested without one.
 */
import { LineCounter, isMap, isPair, isScalar, isSeq, parseDocument } from "yaml";
import type { Node } from "yaml";

/** A message at a 1-based line, as the editor marks it. */
export interface LineMark {
  line: number;
  message: string;
}

type Path = (string | number)[];

/** The path of the key or value at `offset` (0-based) in `text`, or null where there is none. */
export function pathAt(text: string, offset: number): Path | null {
  const doc = parseDocument(text);
  return walk(doc.contents, offset, []);
}

function walk(node: unknown, offset: number, path: Path): Path | null {
  if (isMap(node)) {
    for (const pair of node.items) {
      if (!isPair(pair) || !isScalar(pair.key) || !pair.key.range) {
        continue;
      }
      const key = String(pair.key.value);
      const end = (pair.value as Node | null)?.range?.[2] ?? pair.key.range[2];
      if (offset >= pair.key.range[0] && offset <= end) {
        const inner = offset > pair.key.range[1] ? walk(pair.value, offset, [...path, key]) : null;
        return inner ?? [...path, key];
      }
    }
  }
  if (isSeq(node)) {
    for (const [i, item] of node.items.entries()) {
      const range = (item as Node | null)?.range;
      if (range && offset >= range[0] && offset <= range[2]) {
        return walk(item, offset, [...path, i]) ?? [...path, i];
      }
    }
  }
  return null;
}

/** The 1-based line of the field at `path`, or null when the text holds no such field. */
export function lineOf(text: string, path: Path): number | null {
  const lineCounter = new LineCounter();
  const doc = parseDocument(text, { lineCounter });
  let node: unknown = doc.contents;
  let line: number | null = null;
  for (const step of path) {
    const pair = isMap(node) ? node.items.find((p) => isScalar(p.key) && String(p.key.value) === String(step)) : null;
    const item = isSeq(node) && typeof step === "number" ? node.items[step] : null;
    const start = pair && isScalar(pair.key) ? pair.key.range?.[0] : (item as Node | null)?.range?.[0];
    if (start === undefined) {
      return line;
    }
    line = lineCounter.linePos(start).line;
    node = pair ? pair.value : item;
  }
  return line;
}

type Schema = Record<string, unknown>;

/** What the kind's JSON Schema says the field at `path` is for, or null when it says nothing. */
export function describe(schema: Schema | null | undefined, path: Path): string | null {
  let node = resolve(schema, schema);
  for (const step of path) {
    if (!node) {
      return null;
    }
    node = resolve(schema, child(schema, node, step));
  }
  return node && typeof node.description === "string" ? node.description : null;
}

function child(root: Schema | null | undefined, node: Schema, step: string | number): Schema | undefined {
  if (typeof step === "number") {
    return node.items as Schema | undefined;
  }
  const props = node.properties as Record<string, Schema> | undefined;
  if (props?.[step]) {
    return props[step];
  }
  for (const key of ["allOf", "anyOf", "oneOf"]) {
    for (const branch of (node[key] as Schema[] | undefined) ?? []) {
      const found = child(root, resolve(root, branch) ?? {}, step);
      if (found) {
        return found;
      }
    }
  }
  return undefined;
}

/** A `$ref` to the schema's own definitions, followed. */
function resolve(root: Schema | null | undefined, node: Schema | null | undefined): Schema | null {
  const ref = node?.$ref;
  if (typeof ref !== "string" || !root) {
    return node ?? null;
  }
  const name = ref.split("/").pop() ?? "";
  const defs = (root.definitions ?? root.$defs) as Record<string, Schema> | undefined;
  return defs?.[name] ?? null;
}

/** Each YAML parse error of `text` at its 1-based line, first line of its message only. */
export function parseErrorsOf(text: string): LineMark[] {
  const lineCounter = new LineCounter();
  return parseDocument(text, { lineCounter }).errors.map((e) => ({
    line: e.linePos?.[0]?.line ?? 1,
    // The line is the mark's own; yaml's "… at line 3, column 9:" tail would say it twice.
    message: (e.message.split("\n")[0] ?? e.message).replace(/ at line \d+, column \d+:?$/, ""),
  }));
}

/**
 * The marks of a manifest's text: each YAML parse error at its line, and each refusal that names
 * a field path (`spec.rules: …`) at that field's line. A refusal naming no field it finds is
 * marked on line 1, so it is still read out beside the text.
 */
export function marksOf(text: string, refusals: readonly string[]): LineMark[] {
  const parse = parseErrorsOf(text);
  const refused = refusals.map((message) => {
    const field = /^`?((?:metadata|spec)(?:\.[\w-]+|\[\d+\])*)`?:?\s/.exec(message)?.[1];
    const path = field
      ? field
          .replace(/\[(\d+)\]/g, ".$1")
          .split(".")
          .map((p) => (/^\d+$/.test(p) ? Number(p) : p))
      : null;
    return { line: (path && lineOf(text, path)) ?? 1, message };
  });
  return [...parse, ...refused];
}
