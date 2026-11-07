/**
 * T-4945: the text boxes of the Portal's own pages, found in the source, each with the label a
 * person reads beside it.
 *
 * A text box asks a person to type. Where the value names something that already exists (a
 * space, a type, an Endpoint, a role) or comes from a fixed set, typing is the wrong control:
 * the person has to know the exact name, and the server refuses the typo afterwards. The gate in
 * `gate_free_text.test.ts` reads the labels this module finds and refuses a new box whose label
 * names such a thing, unless `free_text.allow.json` says why it stays a box.
 *
 * Why the source and not a render: the same reason as the control gate (T-2136), most boxes
 * live in a dialog or a state a default render never reaches.
 */

/** One text box: where it is and what its label says, in English. */
export interface TextBox {
  file: string;
  line: number;
  /** The label, the aria-label, the placeholder or the id, the first that resolves. */
  label: string;
}

/** The text controls: the Portal's Input and Textarea, and a bare `<input>` of a text type. */
const OPEN = /<(Input|Textarea|textarea|input)\b/g;
const NOT_TEXT =
  /\btype=["{]["']?(checkbox|radio|file|number|range|color|date|datetime-local|time|hidden|password)\b/;

/** The attributes of the tag that starts at `start`, up to its closing `>` outside every brace. */
function tagAt(source: string, start: number): string {
  let depth = 0;
  for (let i = start; i < source.length; i += 1) {
    const c = source[i];
    if (c === "{") depth += 1;
    else if (c === "}") depth -= 1;
    else if (c === ">" && depth === 0 && source[i - 1] !== "=")
      return source.slice(start, i + 1);
  }
  return source.slice(start);
}

/** A string literal, or the key of a `t("…")` call, written as an attribute's value. */
function attribute(
  text: string,
  name: string,
  t: (key: string) => string | undefined,
): string | undefined {
  const plain = new RegExp(`\\b${name}="([^"]+)"`).exec(text);
  if (plain) return plain[1];
  const translated = new RegExp(
    `\\b${name}=\\{\\s*t\\(\\s*["'\`]([^"'\`]+)["'\`]`,
  ).exec(text);
  if (translated) return t(translated[1]) ?? translated[1];
  const literal = new RegExp(`\\b${name}=\\{\\s*["']([^"']+)["']\\s*\\}`).exec(
    text,
  );
  return literal?.[1];
}

/**
 * The nearest label before the box: a Field's `label=`, or a `<label>` / `<Label>` element's
 * text, within the twelve lines above. A Field wraps its control, so its label comes first.
 */
function labelAbove(
  before: string,
  t: (key: string) => string | undefined,
): string | undefined {
  const window = before.split("\n").slice(-12).join("\n");
  const candidates = [
    // A ternary label (`label={own ? t("a") : t("b")}`) reads as its first branch.
    ...window.matchAll(/\blabel=\{\s*(?:[\w.!]+\s*\?\s*)?t\(\s*["'`]([^"'`]+)["'`]/g),
    ...window.matchAll(/\blabel="([^"]+)"/g),
    ...window.matchAll(
      /<(?:label|Label)\b[^>]*>\s*\{\s*t\(\s*["'`]([^"'`]+)["'`]/g,
    ),
    ...window.matchAll(/<(?:label|Label)\b[^>]*>([^<{]+)</g),
  ].sort((a, b) => (a.index ?? 0) - (b.index ?? 0));
  const last = candidates.at(-1);
  if (!last) return undefined;
  return last[0].includes("t(") ? (t(last[1]) ?? last[1]) : last[1].trim();
}

/** Every text box of one module. `t` resolves an i18n key to its English text. */
export function textBoxesIn(
  file: string,
  source: string,
  t: (key: string) => string | undefined,
): TextBox[] {
  const boxes: TextBox[] = [];
  for (const match of source.matchAll(OPEN)) {
    const start = match.index ?? 0;
    const tag = tagAt(source, start);
    if (NOT_TEXT.test(tag)) continue;
    const before = source.slice(0, start);
    const label =
      attribute(tag, "aria-label", t) ??
      labelAbove(before, t) ??
      attribute(tag, "placeholder", t) ??
      attribute(tag, "id", t) ??
      "(no label)";
    boxes.push({
      file,
      line: before.split("\n").length,
      label: label.replace(/\s+/g, " ").trim(),
    });
  }
  return boxes;
}

/**
 * Words that name something a person picks rather than types: an object of the platform (REF)
 * or a fixed set (ENUM). Matched on the label, whole words, case-insensitive.
 */
export const PICKED =
  /\b(context spaces?|spaces?|endpoints?|data ?sources?|pipelines?|projects?|people|person|users?|members?|groups?|roles?|data ?models?|models?|brokers?|apps?|polic(?:y|ies)|views?|licen[cs]es?|themes?|entity types?|types?|attributes?|kinds?|service ?accounts?|organi[sz]ations?|instances?|catalogu?es?|mappings?|units?|languages?|formats?|scopes?|keywords?|tags?|categor(?:y|ies)|publishers?|assistants?|deployments?)\b/i;

/** What the allow-list says about one label of one module. */
export interface Allowed {
  /** FREE stays a box; REF, ENUM and SUGGEST are waiting for the task named in `task`. */
  class: "FREE" | "REF" | "ENUM" | "SUGGEST";
  reason: string;
  task?: string;
  /** REF only: the server rule that decides which values a person may pick (owner 2026-10-21). */
  rule?: string;
}

/** The key of a box in the allow-list: its module and its label. */
export const keyOf = (box: Pick<TextBox, "file" | "label">): string =>
  `${box.file} :: ${box.label}`;

/**
 * Whether the gate has to hear why a box stays a box: its label names a picked thing, or the
 * gate cannot read the label at all (a key built at run time, or none), so it cannot tell.
 */
export const needsReason = (box: Pick<TextBox, "label">): boolean =>
  PICKED.test(box.label) ||
  box.label.includes("${") ||
  box.label === "(no label)";

/** The boxes whose label names a picked thing and that the allow-list does not explain. */
export function unexplained(
  boxes: readonly TextBox[],
  allowed: Readonly<Record<string, Allowed>>,
): TextBox[] {
  return boxes.filter(
    (box) => needsReason(box) && !Object.hasOwn(allowed, keyOf(box)),
  );
}

/** Allow-list entries no box matches any more: the field became a picker, the entry has to go. */
export function stale(
  boxes: readonly TextBox[],
  allowed: Readonly<Record<string, Allowed>>,
): string[] {
  const keys = new Set(boxes.map(keyOf));
  return Object.keys(allowed)
    .filter((key) => !keys.has(key))
    .sort();
}
