/**
 * The machine-checkable half of the UI writing rules (T-3847, docs User-Guide/ui-writing.md).
 *
 * Pure functions over locale strings and source text, so `gate_human_text.test.ts` can hold the
 * Portal to them and prove each rule on a fixture that must fail. What a machine cannot judge
 * (is this plain language, is the example helpful) is the walk's, recorded in
 * `human_ui_walk.json`.
 */

/** Words that say nothing about what happens or what went wrong (rule 2, rule 4). */
const GENERIC = /^(submit|ok|yes|error|invalid|failed)[.!]?$/i;

/** A string's rule breaks, by rule name. Empty when it reads as a person would write it. */
export function textProblems(key: string, text: string): string[] {
  const problems: string[] = [];
  if (GENERIC.test(text.trim())) problems.push("generic");
  if (/urn:/i.test(text)) problems.push("urn");
  if (
    /\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b/i.test(
      text,
    )
  )
    problems.push("uuid");
  // ICU arguments (`{name}`) are the code's, not the reader's; what is left is what they read.
  const read = text.replace(/\{\s*\w+\s*\}/g, "");
  if (/\b[A-Z][a-z]+[A-Z][a-z]+[A-Za-z]*\b/.test(read)) problems.push("kind");
  if (/\b(HTTP\s*)?[45]\d\d\b/.test(read)) problems.push("http");
  if (/[{[]\s*"|"\s*:/.test(read)) problems.push("json");
  if (isError(key) && !hasNextStep(read)) problems.push("next-step");
  return problems;
}

/** An error message, by where it lives: a key named for an error, a failure or a problem. */
export function isError(key: string): boolean {
  return key
    .split(".")
    .some(
      (segment) =>
        /^(error|errors|failed|problem|problems)$/i.test(segment) ||
        /(Failed|Error)$/.test(segment),
    );
}

/**
 * Whether a message says what to do next (rule 4): a second sentence, or an instruction the
 * reader can follow. A bare "Could not save the endpoint." says neither.
 */
export function hasNextStep(text: string): boolean {
  const sentences = text
    .split(/[.!?](?:\s|$)/)
    .filter((s) => s.trim().length > 0);
  if (sentences.length >= 2) return true;
  return /\b(try|check|ask|use|open|sign in|reload|contact|choose|pick|enter|fix|add|remove|wait|retry|change|correct|select|type|give|answer|verify|store|send|load|turn|split|go|quote|copy|report)\b/i.test(
    text,
  );
}

/** Every leaf of a locale as `a.b.c` → string. */
export function leaves(node: unknown, prefix = ""): Record<string, string> {
  if (typeof node === "string") return { [prefix]: node };
  if (typeof node !== "object" || node === null) return {};
  return Object.assign(
    {},
    ...Object.entries(node as Record<string, unknown>).map(([key, value]) =>
      leaves(value, prefix ? `${prefix}.${key}` : key),
    ),
  );
}

/** Keys one locale has and the other lacks, either way. */
export function missingKeys(
  reference: Record<string, string>,
  other: Record<string, string>,
): string[] {
  return [
    ...Object.keys(reference).filter((key) => !(key in other)),
    ...Object.keys(other).filter((key) => !(key in reference)),
  ].sort();
}

const CONTROL_TAG =
  /<(Button|button|Input|input|Select|select|Textarea|textarea|Switch|Checkbox|RadioGroup)(?![\w.])((?:[^<>]|<[^<>]*>)*?)\/?>/g;

/**
 * Controls a module can disable without saying why (rule 7): an opening tag of a control with a
 * `disabled` prop and none of `title`, `aria-describedby` or `disabledReason`. Counted per
 * module from the source, as the control gate counts controls.
 */
export function silentDisabled(source: string): number {
  let count = 0;
  // A comparison (`count > 0`, `zoom <= min`) or an arrow (`() => go()`) inside a prop is not
  // the edge of a tag.
  const tags = source.replace(/ [<>]=? (?=\S)/g, " cmp ").replace(/=>/g, "=)");
  // Each control's own opening tag, so a control inside another component's props (a dialog's
  // footer) is read as itself.
  for (const [, , props] of tags.matchAll(CONTROL_TAG)) {
    if (!/\bdisabled\b/.test(props)) continue;
    if (/\b(title|aria-describedby|disabledReason)=/.test(props)) continue;
    count += 1;
  }
  return count;
}
