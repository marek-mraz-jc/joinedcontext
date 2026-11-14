/**
 * Whether a loaded row matches an NGSI-LD `q` (CIM 009 §4.9), for what is decided in the page on
 * the rows it shows, such as a view's colour rules (API/01 §30). The endpoint stays the judge of
 * what is read; this only marks rows already read.
 *
 * The subset a colour rule needs: terms `attr<op>value` with `==`, `!=`, `<`, `<=`, `>`, `>=`,
 * `~=` (a regular expression) and `!~=`, a value that is a number, a quoted string, `true`/`false`,
 * a range `a..b` or a list `a,b` (for `==` and `!=`), and a bare `attr` for "has the attribute";
 * terms joined by `;` (and) and `|` (or, binding looser). Parentheses and anything else are not
 * evaluated: the answer is `null`, never a guess.
 */
import type { RichCell, RichRow } from "./model";

const OPS = ["!~=", "~=", "==", "!=", ">=", "<=", ">", "<"] as const;
type Op = (typeof OPS)[number];

interface Term {
  path: string[];
  op?: Op;
  value?: string;
}

/** `true`/`false` for a `q` this evaluates, `null` for one it does not. */
export function matchesQ(row: RichRow, q: string): boolean | null {
  const parsed = parseQ(q);
  if (parsed === null) return null;
  return parsed.some((all) => all.every((term) => holds(row, term)));
}

/** The `q` as alternatives of conjunctions, or `null` when it uses what this does not evaluate. */
export function parseQ(q: string): Term[][] | null {
  const text = q.trim();
  if (text === "" || /[()]/.test(text.replace(/"[^"]*"/g, '""'))) return null;
  const alternatives: Term[][] = [];
  for (const alternative of splitOutsideQuotes(text, "|")) {
    const terms: Term[] = [];
    for (const raw of splitOutsideQuotes(alternative, ";")) {
      const term = parseTerm(raw.trim());
      if (term === null) return null;
      terms.push(term);
    }
    alternatives.push(terms);
  }
  return alternatives;
}

function splitOutsideQuotes(text: string, separator: string): string[] {
  const parts: string[] = [];
  let current = "";
  let quoted = false;
  for (const char of text) {
    if (char === '"') quoted = !quoted;
    if (char === separator && !quoted) {
      parts.push(current);
      current = "";
    } else {
      current += char;
    }
  }
  parts.push(current);
  return parts;
}

const PATH = /^[A-Za-z_@][A-Za-z0-9_:/#.-]*$/;

function parseTerm(raw: string): Term | null {
  // The first operator character ends the path: a path holds none, a quoted value may.
  const at = raw.search(/[=!<>~]/);
  if (at < 0) return PATH.test(raw) ? { path: raw.split(".") } : null;
  const op = OPS.find((candidate) => raw.startsWith(candidate, at));
  const path = raw.slice(0, at).trim();
  const value = op ? raw.slice(at + op.length).trim() : "";
  if (!op || !PATH.test(path) || value === "") return null;
  return { path: path.split("."), op, value };
}

/** The values a path reaches in the row: an attribute's value or object, or one of its sub-attributes'. */
function valuesAt(row: RichRow, path: string[]): unknown[] {
  const [head, ...rest] = path;
  if (head === "id") return [row.id];
  let cells: (RichCell | RichCell[] | undefined)[] = [row.cells[head]];
  for (const name of rest) {
    cells = cells.flatMap((cell) => (Array.isArray(cell) ? cell : cell ? [cell] : [])).map((cell) => cell.sub?.[name]);
  }
  return cells
    .flatMap((cell) => (Array.isArray(cell) ? cell : cell ? [cell] : []))
    .flatMap((cell) => {
      const own = cell.kind === "relationship" ? cell.object : cell.value;
      return Array.isArray(own) ? own : [own];
    })
    .filter((value) => value !== undefined && value !== null);
}

/** A literal of the `q` grammar as the value it stands for. */
function literal(text: string): string | number | boolean {
  if (text.length >= 2 && text.startsWith('"') && text.endsWith('"')) return text.slice(1, -1);
  if (text === "true" || text === "false") return text === "true";
  const n = Number(text);
  return text.trim() !== "" && Number.isFinite(n) ? n : text;
}

function compare(actual: unknown, op: Op, expected: string | number | boolean): boolean {
  if (typeof expected === "number" || typeof actual === "number") {
    const a = Number(actual);
    const e = Number(expected);
    if (!Number.isFinite(a) || !Number.isFinite(e)) return op === "!=";
    return op === "==" ? a === e : op === "!=" ? a !== e : op === ">" ? a > e : op === ">=" ? a >= e : op === "<" ? a < e : a <= e;
  }
  const a = String(actual);
  const e = String(expected);
  switch (op) {
    case "==":
      return a === e;
    case "!=":
      return a !== e;
    // Strings order as text, which is how ISO dates compare too.
    case ">":
      return a > e;
    case ">=":
      return a >= e;
    case "<":
      return a < e;
    default:
      return a <= e;
  }
}

/** The longest pattern a colour rule may test, in characters. */
const MAX_PATTERN = 256;

/**
 * Whether a quantified group holds a quantifier at any depth (`(a+)+`, `((a*))*`, `(a|b?){2,}`).
 * Escapes and character classes hold none; `?` opening a group (`(?:`, `(?<name>`) is no quantifier.
 */
function nestedQuantifier(source: string): boolean {
  const groups: boolean[] = [];
  const quantifier = (at: number) => at < source.length && "*+?{".includes(source[at]);
  for (let at = 0; at < source.length; at++) {
    const char = source[at];
    if (char === "\\") {
      at++;
    } else if (char === "[") {
      for (at++; at < source.length && source[at] !== "]"; at++) if (source[at] === "\\") at++;
    } else if (char === "(") {
      groups.push(false);
      if (source[at + 1] === "?") at++;
    } else if (char === ")") {
      const holds = groups.pop() ?? false;
      const quantified = quantifier(at + 1);
      // An optional group runs once at most; a repeated one holding a quantifier backtracks.
      if (holds && quantified && source[at + 1] !== "?") return true;
      if (groups.length > 0 && (holds || quantified)) groups[groups.length - 1] = true;
    } else if (quantifier(at) && groups.length > 0) {
      groups[groups.length - 1] = true;
    }
  }
  return false;
}

/**
 * A saved view's pattern compiled for this browser, whose regular expressions backtrack: a long
 * one, or one with a quantified group holding a quantifier, could hold the page on every cell, so
 * it throws and marks nothing. The broker checks the same `q` with a linear engine.
 */
// ponytail: catches nested quantifiers, not overlapping alternations such as (a|a)+.
export function boundedPattern(source: string): RegExp {
  if (source.length > MAX_PATTERN || nestedQuantifier(source)) {
    throw new SyntaxError("pattern too costly to test in the browser");
  }
  return new RegExp(source, "u");
}

function holds(row: RichRow, term: Term): boolean {
  const values = valuesAt(row, term.path);
  if (term.op === undefined) return values.length > 0;
  const raw = term.value ?? "";
  if (term.op === "~=" || term.op === "!~=") {
    let pattern: RegExp;
    try {
      pattern = boundedPattern(String(literal(raw)));
    } catch {
      // A pattern this browser cannot compile marks nothing.
      return false;
    }
    const found = values.some((value) => pattern.test(String(value)));
    return term.op === "~=" ? found : !found;
  }
  const range = raw.includes("..") && !raw.startsWith('"') ? raw.split("..") : [];
  if ((term.op === "==" || term.op === "!=") && range.length === 2) {
    const [low, high] = range.map(literal);
    const within = values.some((value) => compare(value, ">=", low) && compare(value, "<=", high));
    return term.op === "==" ? within : !within;
  }
  const listed = splitOutsideQuotes(raw, ",");
  if ((term.op === "==" || term.op === "!=") && listed.length > 1) {
    const options = listed.map((part) => literal(part.trim()));
    const any = values.some((value) => options.some((option) => compare(value, "==", option)));
    return term.op === "==" ? any : !any;
  }
  const expected = literal(raw);
  // `!=` holds when no value equals; every other operator when some value satisfies it.
  if (term.op === "!=") return !values.some((value) => compare(value, "==", expected));
  return values.some((value) => compare(value, term.op as Op, expected));
}
