/**
 * A computed column's formula (T-6384): arithmetic and comparison over a row's attributes, such as
 * `availableBikeNumber / totalSlotNumber * 100` or `freeSlotNumber == 0`. The text is parsed into
 * a tree and the tree is walked; nothing is ever handed to `eval`, `Function` or the DOM, so a
 * formula an App or a person writes can read attribute values and nothing else.
 *
 * The language: numbers (`12`, `0.5`), `true`/`false`, attribute names (`[A-Za-z_][A-Za-z0-9_]*`),
 * unary `-` and `!`, `* / %`, `+ -`, `< <= > >=`, `== !=`, `&&`, `||`, a conditional
 * `a ? b : c`, and parentheses, in the usual precedence. Anything else (a string, a call, a member
 * access, an assignment) is refused with the position it starts at.
 *
 * A value is a number or a boolean. An attribute that is not set, or holds anything else, reads as
 * `null`, and `null` carries through: a station without `totalSlotNumber` gets an empty cell, not
 * a 0. Division by zero is `null` too.
 */

import type { RichCell, RichRow } from "./model";

export type FormulaValue = number | boolean | null;

export type Formula =
  | { op: "num"; value: number }
  | { op: "bool"; value: boolean }
  | { op: "attr"; name: string }
  | { op: "neg" | "not"; arg: Formula }
  | { op: "+" | "-" | "*" | "/" | "%" | "<" | "<=" | ">" | ">=" | "==" | "!=" | "&&" | "||"; left: Formula; right: Formula }
  | { op: "?"; test: Formula; then: Formula; else: Formula };

export type FormulaParse =
  | { ok: true; formula: Formula; attrs: string[] }
  | { ok: false; error: string; at: number };

/** The longest formula text read; a column's formula is a line, not a program. */
export const MAX_FORMULA_CHARS = 500;
/** How deep parentheses and operators may nest before the parse stops. */
const MAX_DEPTH = 32;

type Token = { kind: "num"; value: number; at: number } | { kind: "name"; value: string; at: number } | { kind: "op"; value: string; at: number };

const OPERATORS = ["<=", ">=", "==", "!=", "&&", "||", "+", "-", "*", "/", "%", "<", ">", "!", "(", ")", "?", ":"];

class Refused extends Error {
  constructor(
    message: string,
    readonly at: number,
  ) {
    super(message);
  }
}

function tokens(text: string): Token[] {
  const out: Token[] = [];
  let i = 0;
  while (i < text.length) {
    const char = text[i];
    if (char === " " || char === "\t") {
      i++;
      continue;
    }
    const number = /^(\d+(\.\d+)?|\.\d+)/.exec(text.slice(i));
    if (number) {
      out.push({ kind: "num", value: Number(number[0]), at: i });
      i += number[0].length;
      continue;
    }
    const name = /^[A-Za-z_][A-Za-z0-9_]*/.exec(text.slice(i));
    if (name) {
      out.push({ kind: "name", value: name[0], at: i });
      i += name[0].length;
      continue;
    }
    const op = OPERATORS.find((each) => text.startsWith(each, i));
    if (!op) {
      const what = char === '"' || char === "'" || char === "`" ? "text in quotes" : `"${char}"`;
      throw new Refused(`${what} is not part of a formula; use attribute names, numbers and + - * / % < > == != && || ! ?:`, i);
    }
    out.push({ kind: "op", value: op, at: i });
    i += op.length;
  }
  return out;
}

const BINARY: string[][] = [["||"], ["&&"], ["==", "!="], ["<", "<=", ">", ">="], ["+", "-"], ["*", "/", "%"]];

/**
 * The formula `text` as a tree and the attributes it reads, or why it is refused and where. With
 * `known`, an attribute name outside the list is refused too, so a typo shows at once instead of
 * as a column of empty cells.
 */
export function parseFormula(text: string, known?: readonly string[]): FormulaParse {
  if (text.length > MAX_FORMULA_CHARS) return { ok: false, error: `a formula holds at most ${MAX_FORMULA_CHARS} characters`, at: MAX_FORMULA_CHARS };
  let list: Token[];
  try {
    list = tokens(text);
  } catch (err) {
    return refusal(err);
  }
  if (list.length === 0) return { ok: false, error: "the formula is empty", at: 0 };
  let pos = 0;
  const attrs: string[] = [];
  const peek = () => list[pos];
  const end = () => (pos < list.length ? list[pos].at : text.length);
  const isOp = (value: string) => peek()?.kind === "op" && peek()?.value === value;

  const conditional = (depth: number): Formula => {
    if (depth > MAX_DEPTH) throw new Refused(`the formula nests deeper than ${MAX_DEPTH} levels`, end());
    const test = binary(0, depth);
    if (!isOp("?")) return test;
    pos++;
    const then = conditional(depth + 1);
    if (!isOp(":")) throw new Refused('a "?" needs its ":" and the value for the other case', end());
    pos++;
    return { op: "?", test, then, else: conditional(depth + 1) };
  };

  const binary = (level: number, depth: number): Formula => {
    if (level === BINARY.length) return unary(depth);
    let left = binary(level + 1, depth);
    for (;;) {
      const next = peek();
      if (next?.kind !== "op" || !BINARY[level].includes(next.value)) return left;
      pos++;
      const right = binary(level + 1, depth);
      left = { op: next.value as "+", left, right };
    }
  };

  const unary = (depth: number): Formula => {
    if (depth > MAX_DEPTH) throw new Refused(`the formula nests deeper than ${MAX_DEPTH} levels`, end());
    if (isOp("-")) {
      pos++;
      return { op: "neg", arg: unary(depth + 1) };
    }
    if (isOp("!")) {
      pos++;
      return { op: "not", arg: unary(depth + 1) };
    }
    return primary(depth);
  };

  const primary = (depth: number): Formula => {
    const token = peek();
    if (!token) throw new Refused("the formula ends where a value belongs", text.length);
    pos++;
    if (token.kind === "num") return { op: "num", value: token.value };
    if (token.kind === "name") {
      if (token.value === "true" || token.value === "false") return { op: "bool", value: token.value === "true" };
      if (isOp("(")) throw new Refused(`"${token.value}(" is a function, which a formula cannot use: write attribute names, numbers and + - * / only`, token.at);
      if (known && !known.includes(token.value)) throw new Refused(`"${token.value}" is no attribute of this type`, token.at);
      if (!attrs.includes(token.value)) attrs.push(token.value);
      return { op: "attr", name: token.value };
    }
    if (token.value === "(") {
      const inner = conditional(depth + 1);
      if (!isOp(")")) throw new Refused('a "(" is not closed', end());
      pos++;
      return inner;
    }
    throw new Refused(`"${token.value}" stands where a value belongs`, token.at);
  };

  try {
    const formula = conditional(0);
    if (pos < list.length) throw new Refused(`"${text.slice(list[pos].at, list[pos].at + 12)}" follows a complete formula`, list[pos].at);
    return { ok: true, formula, attrs };
  } catch (err) {
    return refusal(err);
  }
}

function refusal(err: unknown): FormulaParse {
  if (err instanceof Refused) return { ok: false, error: err.message, at: err.at };
  throw err;
}

const num = (value: FormulaValue): number | null => (typeof value === "number" ? value : null);

/**
 * The formula's value for one row; `read` gives an attribute's value. A number that is not finite
 * (a division by zero, an overflow) is `null`, so the cell stays empty rather than showing Infinity.
 */
export function evaluate(formula: Formula, read: (attr: string) => unknown): FormulaValue {
  const value = walk(formula, read);
  return typeof value === "number" && !Number.isFinite(value) ? null : value;
}

function walk(f: Formula, read: (attr: string) => unknown): FormulaValue {
  switch (f.op) {
    case "num":
    case "bool":
      return f.value;
    case "attr": {
      const value = read(f.name);
      return typeof value === "number" || typeof value === "boolean" ? value : null;
    }
    case "neg": {
      const v = num(walk(f.arg, read));
      return v === null ? null : -v;
    }
    case "not": {
      const v = walk(f.arg, read);
      return typeof v === "boolean" ? !v : null;
    }
    case "?": {
      const test = walk(f.test, read);
      if (typeof test !== "boolean") return null;
      return walk(test ? f.then : f.else, read);
    }
    case "&&":
    case "||": {
      const l = walk(f.left, read);
      const r = walk(f.right, read);
      if (typeof l !== "boolean" || typeof r !== "boolean") return null;
      return f.op === "&&" ? l && r : l || r;
    }
    case "==":
    case "!=": {
      const l = walk(f.left, read);
      const r = walk(f.right, read);
      if (l === null || r === null || typeof l !== typeof r) return null;
      return f.op === "==" ? l === r : l !== r;
    }
    default: {
      const l = num(walk(f.left, read));
      const r = num(walk(f.right, read));
      if (l === null || r === null) return null;
      switch (f.op) {
        case "+":
          return l + r;
        case "-":
          return l - r;
        case "*":
          return l * r;
        case "/":
          return r === 0 ? null : l / r;
        case "%":
          return r === 0 ? null : l % r;
        case "<":
          return l < r;
        case "<=":
          return l <= r;
        case ">":
          return l > r;
        case ">=":
          return l >= r;
      }
    }
  }
}

/** A grid column worked out from its row (T-6384): the column's name and its parsed formula. */
export interface ComputedColumn {
  attr: string;
  formula: Formula;
  reads: string[];
}

/** The computed columns of a grid configuration; `parseGridConfig` already refused a bad formula. */
export function formulasOf(columns: readonly { attr: string; formula?: string }[]): ComputedColumn[] {
  const out: ComputedColumn[] = [];
  for (const column of columns) {
    if (column.formula === undefined) continue;
    const parsed = parseFormula(column.formula);
    if (parsed.ok) out.push({ attr: column.attr, formula: parsed.formula, reads: parsed.attrs });
  }
  return out;
}

function valueOf(cell: RichCell | RichCell[] | undefined): unknown {
  const first = Array.isArray(cell) ? cell[0] : cell;
  return first?.value;
}

/** Each row with a cell per computed column; a row the formula gives `null` for has no such cell. */
export function withFormulas(rows: RichRow[], formulas: readonly ComputedColumn[]): RichRow[] {
  if (formulas.length === 0) return rows;
  return rows.map((row) => {
    const cells = { ...row.cells };
    for (const { attr, formula } of formulas) {
      // A formula reads stored attributes only, never another computed column, so the order of
      // the columns cannot change a value and no two columns can read each other in a loop.
      const value = evaluate(formula, (name) => valueOf(row.cells[name]));
      if (value === null) delete cells[attr];
      else cells[attr] = { kind: "property", value };
    }
    return { ...row, cells };
  });
}
