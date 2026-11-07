/**
 * Formula fields (DM-80, Architecture/08 §3, T-3133): a slot's LinkML `equals_expression` over
 * other slots of its own class, compiled to the Bloblang the runner already evaluates. The compiled
 * text is built from the parsed tokens alone, so nothing but the class's own slot values, numbers,
 * text and the listed operators and functions can reach it: no other class or space, no
 * environment variable, no Bloblang of the author's own.
 */
import type { ResourceProposal } from "../../api/manifest";

export type Formula =
  | { kind: "number"; value: number }
  | { kind: "text"; value: string }
  | { kind: "ref"; name: string }
  | { kind: "unary"; op: "-" | "not"; arg: Formula }
  | { kind: "binary"; op: BinaryOp; left: Formula; right: Formula }
  | { kind: "call"; fn: FunctionName; args: Formula[] };

type BinaryOp = "+" | "-" | "*" | "/" | "%" | "==" | "!=" | "<" | "<=" | ">" | ">=" | "and" | "or";
type FunctionName = "abs" | "round" | "min" | "max";

const ARITY: Record<FunctionName, number> = { abs: 1, round: 1, min: 2, max: 2 };

/** The longest expression a slot carries. */
export const MAX_EXPRESSION = 1000;

/** What is wrong with an expression, and where. */
export class FormulaError extends Error {
  readonly at: number;
  constructor(message: string, at: number) {
    super(message);
    this.name = "FormulaError";
    this.at = at;
  }
}

/** A name a reference may hold: a LinkML slot name a Bloblang variable can carry. */
const NAME = /^[A-Za-z_][A-Za-z0-9_]*$/;

type Token =
  | { t: "num"; v: number; at: number }
  | { t: "str"; v: string; at: number }
  | { t: "ref"; v: string; at: number }
  | { t: "word"; v: string; at: number }
  | { t: "op"; v: string; at: number }
  | { t: "end"; at: number };

function tokens(text: string): Token[] {
  const out: Token[] = [];
  let at = 0;
  while (at < text.length) {
    const rest = text.slice(at);
    const space = /^\s+/.exec(rest);
    if (space) {
      at += space[0].length;
      continue;
    }
    const number = /^\d+(\.\d+)?/.exec(rest);
    if (number) {
      const value = Number(number[0]);
      if (!Number.isFinite(value) || value >= 1e15) throw new FormulaError(`${number[0]} is too large`, at);
      out.push({ t: "num", v: value, at });
      at += number[0].length;
      continue;
    }
    const quote = rest[0];
    if (quote === "'" || quote === '"') {
      const end = rest.indexOf(quote, 1);
      if (end < 0) throw new FormulaError("a text is not closed", at);
      out.push({ t: "str", v: rest.slice(1, end), at });
      at += end + 1;
      continue;
    }
    if (rest[0] === "{") {
      const end = rest.indexOf("}");
      const name = end < 0 ? "" : rest.slice(1, end).trim();
      if (!NAME.test(name)) throw new FormulaError("a reference is a slot name in braces, such as {capacity}", at);
      out.push({ t: "ref", v: name, at });
      at += end + 1;
      continue;
    }
    const word = /^[A-Za-z_]+/.exec(rest);
    if (word) {
      out.push({ t: "word", v: word[0], at });
      at += word[0].length;
      continue;
    }
    const op = /^(==|!=|<=|>=|[-+*/%<>(),])/.exec(rest);
    if (op) {
      out.push({ t: "op", v: op[0], at });
      at += op[0].length;
      continue;
    }
    throw new FormulaError(`\`${rest[0]}\` is not part of a formula`, at);
  }
  out.push({ t: "end", at });
  return out;
}

/** An expression parsed into its tree; a `FormulaError` names the first thing that is not one. */
export function parseFormula(text: string): Formula {
  if (text.trim() === "") throw new FormulaError("the formula is empty", 0);
  if (text.length > MAX_EXPRESSION) throw new FormulaError(`a formula is at most ${MAX_EXPRESSION} characters`, MAX_EXPRESSION);
  const list = tokens(text);
  let at = 0;
  const peek = () => list[at];
  const next = () => list[at++];
  const isOp = (v: string) => {
    const token = peek();
    return (token.t === "op" || token.t === "word") && token.v === v;
  };
  const expect = (v: string) => {
    if (!isOp(v)) throw new FormulaError(`\`${v}\` expected`, peek().at);
    next();
  };

  const or = (): Formula => {
    let left = and();
    while (isOp("or")) {
      next();
      left = { kind: "binary", op: "or", left, right: and() };
    }
    return left;
  };
  const and = (): Formula => {
    let left = not();
    while (isOp("and")) {
      next();
      left = { kind: "binary", op: "and", left, right: not() };
    }
    return left;
  };
  const not = (): Formula => {
    if (isOp("not")) {
      next();
      return { kind: "unary", op: "not", arg: not() };
    }
    return comparison();
  };
  const comparison = (): Formula => {
    const left = additive();
    for (const op of ["==", "!=", "<=", ">=", "<", ">"] as const) {
      if (isOp(op)) {
        next();
        return { kind: "binary", op, left, right: additive() };
      }
    }
    return left;
  };
  const additive = (): Formula => {
    let left = multiplicative();
    while (isOp("+") || isOp("-")) {
      const op = (next() as { v: "+" | "-" }).v;
      left = { kind: "binary", op, left, right: multiplicative() };
    }
    return left;
  };
  const multiplicative = (): Formula => {
    let left = unary();
    while (isOp("*") || isOp("/") || isOp("%")) {
      const op = (next() as { v: "*" | "/" | "%" }).v;
      left = { kind: "binary", op, left, right: unary() };
    }
    return left;
  };
  const unary = (): Formula => {
    if (isOp("-")) {
      next();
      return { kind: "unary", op: "-", arg: unary() };
    }
    return primary();
  };
  const primary = (): Formula => {
    const token = next();
    if (token.t === "num") return { kind: "number", value: token.v };
    if (token.t === "str") return { kind: "text", value: token.v };
    if (token.t === "ref") return { kind: "ref", name: token.v };
    if (token.t === "op" && token.v === "(") {
      const inner = or();
      expect(")");
      return inner;
    }
    if (token.t === "word" && token.v in ARITY) {
      const fn = token.v as FunctionName;
      expect("(");
      const args: Formula[] = [];
      if (!isOp(")")) {
        args.push(or());
        while (isOp(",")) {
          next();
          args.push(or());
        }
      }
      expect(")");
      if (args.length !== ARITY[fn]) throw new FormulaError(`${fn} takes ${ARITY[fn]} value${ARITY[fn] === 1 ? "" : "s"}`, token.at);
      return { kind: "call", fn, args };
    }
    if (token.t === "end") throw new FormulaError("the formula ends too early", token.at);
    if (token.t === "word") throw new FormulaError(`\`${token.v}\` is not a function of a formula: use abs, round, min or max`, token.at);
    throw new FormulaError(`\`${token.t === "op" ? token.v : ""}\` is not expected here`, token.at);
  };

  const tree = or();
  if (peek().t !== "end") throw new FormulaError("something follows the formula", peek().at);
  return tree;
}

/** The slots a formula reads, each once, in order. */
export function refsOf(formula: Formula): string[] {
  const found: string[] = [];
  const walk = (node: Formula) => {
    if (node.kind === "ref") {
      if (!found.includes(node.name)) found.push(node.name);
    } else if (node.kind === "unary") walk(node.arg);
    else if (node.kind === "binary") {
      walk(node.left);
      walk(node.right);
    } else if (node.kind === "call") node.args.forEach(walk);
  };
  walk(formula);
  return found;
}

/** What a formula's value is, for the slot's LinkML range. */
export function rangeOf(formula: Formula): "float" | "string" | "boolean" {
  if (formula.kind === "text") return "string";
  if (formula.kind === "unary") return formula.op === "not" ? "boolean" : "float";
  if (formula.kind === "binary") {
    if (["==", "!=", "<", "<=", ">", ">=", "and", "or"].includes(formula.op)) return "boolean";
    if (formula.op === "+" && (rangeOf(formula.left) === "string" || rangeOf(formula.right) === "string")) return "string";
  }
  if (formula.kind === "call" && (formula.fn === "min" || formula.fn === "max")) return rangeOf(formula.args[0]);
  return "float";
}

/** A slot of the class as the checks need it. */
export interface ClassSlot {
  name: string;
  kind: string;
  /** Its formula, when it is a formula field. */
  equals_expression?: string;
}

/** The formulas of a class, checked: in the order they compute, or why each cannot. */
export interface Checked {
  /** Every formula slot, each after the formula slots it reads. */
  order: { name: string; formula: Formula }[];
  /** Why a formula slot cannot be computed, by its name. */
  problems: Record<string, string>;
}

/**
 * Every formula of a class checked together: each parses, reads only Properties of the class
 * (or other formula slots), and no formula reads itself through others; a cycle is named in full.
 */
export function checkFormulas(slots: ClassSlot[]): Checked {
  const bySlot = new Map(slots.map((slot) => [slot.name, slot]));
  const parsed = new Map<string, Formula>();
  const problems: Record<string, string> = {};
  for (const slot of slots) {
    if (slot.equals_expression === undefined) continue;
    // The name is spliced into the mapping as `v_{slot}` and `root.{slot}` (F-UI1-022).
    if (!NAME.test(slot.name)) {
      problems[slot.name] = "a formula slot's name is letters, digits and _, such as free_places";
      continue;
    }
    try {
      const formula = parseFormula(slot.equals_expression);
      for (const ref of refsOf(formula)) {
        const target = bySlot.get(ref);
        if (!target) throw new FormulaError(`{${ref}} is not a slot of this class`, 0);
        if (target.kind !== "Property") throw new FormulaError(`{${ref}} is a ${target.kind}, which a formula cannot compute with`, 0);
      }
      parsed.set(slot.name, formula);
    } catch (error) {
      problems[slot.name] = error instanceof Error ? error.message : String(error);
    }
  }
  const order: { name: string; formula: Formula }[] = [];
  const state = new Map<string, "visiting" | "done">();
  const visit = (name: string, path: string[]): boolean => {
    if (state.get(name) === "done") return true;
    if (state.get(name) === "visiting") {
      const cycle = [...path.slice(path.indexOf(name)), name];
      for (const member of cycle) problems[member] ??= `a cycle: ${cycle.join(" → ")}`;
      return false;
    }
    const formula = parsed.get(name);
    if (!formula) return problems[name] === undefined;
    state.set(name, "visiting");
    let ok = true;
    for (const ref of refsOf(formula)) {
      if (bySlot.get(ref)?.equals_expression !== undefined && !visit(ref, [...path, name])) ok = false;
    }
    state.set(name, "done");
    if (ok && problems[name] === undefined) order.push({ name, formula });
    else problems[name] ??= "it reads a formula that cannot be computed";
    return ok && problems[name] === undefined;
  };
  for (const name of parsed.keys()) visit(name, []);
  return { order, problems };
}

/** One formula as a Bloblang expression over the variables `$v_{slot}`. */
export function toBloblang(formula: Formula): string {
  switch (formula.kind) {
    case "number":
      return String(formula.value);
    case "text":
      // The runner fills `${NAME}` from its environment in the whole stream text before Bloblang
      // reads it, and `${JC_CLIENT_SECRET}` passes the server's check as the project's own: `$`
      // goes in as Bloblang's `\u0024`, which only Bloblang turns back into `$` (T-6230).
      return JSON.stringify(formula.value).replaceAll("$", "\\u0024");
    case "ref":
      return `$v_${formula.name}`;
    case "unary":
      return formula.op === "-" ? `(0 - ${toBloblang(formula.arg)})` : `!(${toBloblang(formula.arg)})`;
    case "binary": {
      const op = formula.op === "and" ? "&&" : formula.op === "or" ? "||" : formula.op;
      return `(${toBloblang(formula.left)} ${op} ${toBloblang(formula.right)})`;
    }
    case "call": {
      const [a, b] = formula.args.map(toBloblang);
      if (formula.fn === "abs") return `(${a}).abs()`;
      if (formula.fn === "round") return `(${a}).round()`;
      return `(if ${a} ${formula.fn === "min" ? "<" : ">"} ${b} { ${a} } else { ${b} })`;
    }
  }
}

/** The pipeline's name for a class: `formulas-{class}` in lower case, a DNS-1123 label. */
export function pipelineName(type: string): string {
  // Room for the longest suffix its Policies add (`-write`), so every name stays within 63.
  return `formulas-${type.toLowerCase().replace(/[^a-z0-9-]/g, "-")}`.slice(0, 63 - "-write".length).replace(/-+$/, "");
}

/**
 * The compute of the class's formula pipeline: each entity of the page read into the variables
 * its formulas need, each formula after those it reads, and the entity written back with the
 * formula attributes alone, each carrying `computedBy` (PL-36). A formula with no value leaves its
 * attribute alone; an entity whose formulas all have none is not written.
 */
export function formulaMapping(checked: Checked, type: string): string {
  const formulas = new Set(checked.order.map((entry) => entry.name));
  const read = [...new Set(checked.order.flatMap((entry) => refsOf(entry.formula)))].filter((name) => !formulas.has(name));
  const name = pipelineName(type);
  const lines = [
    "map formula_entity {",
    `  let pipeline = "urn:ngsi-ld:Pipeline:%v:%v:${name}".format(env("JC_ORG_DOMAIN"), env("JC_SPACE"))`,
    ...read.map((slot) => `  let v_${slot} = this.${slot}.value.catch(null)`),
    ...checked.order.map(({ name: slot, formula }) => {
      const refs = refsOf(formula).map((ref) => `$v_${ref}`);
      const guard = refs.length > 0 ? `[${refs.join(", ")}].contains(null)` : "false";
      return `  let v_${slot} = if ${guard} { null } else { (${toBloblang(formula)}).catch(null) }`;
    }),
    "  root.id = this.id",
    "  root.type = this.type",
    ...checked.order.map(
      ({ name: slot }) =>
        `  root.${slot} = if $v_${slot} == null { deleted() } else { { "type": "Property", "value": $v_${slot}, "computedBy": { "type": "Property", "value": $pipeline } } }`,
    ),
    "}",
    `root = this.or([]).filter(e -> e.type == ${JSON.stringify(type)}).map_each(e -> e.apply("formula_entity")).filter(e -> e.keys().length() > 2)`,
  ];
  // Every name in the text came through `NAME` or `pipelineName`; nothing else is spliced in.
  return lines.join("\n") + "\n";
}

/** A formula's value on one row, as the compiled Bloblang computes it: for the dialog's preview. */
export function evaluate(formula: Formula, values: Record<string, unknown>): unknown {
  const ofRef = (name: string) => {
    const value = values[name];
    return value === undefined ? null : value;
  };
  const go = (node: Formula): unknown => {
    switch (node.kind) {
      case "number":
      case "text":
        return node.value;
      case "ref":
        return ofRef(node.name);
      case "unary": {
        const value = go(node.arg);
        if (node.op === "not") {
          if (typeof value !== "boolean") throw new Error("not a yes or no");
          return !value;
        }
        if (typeof value !== "number") throw new Error("not a number");
        return -value;
      }
      case "binary": {
        const left = go(node.left);
        const right = go(node.right);
        if (node.op === "and" || node.op === "or") {
          if (typeof left !== "boolean" || typeof right !== "boolean") throw new Error("not a yes or no");
          return node.op === "and" ? left && right : left || right;
        }
        if (node.op === "==") return left === right;
        if (node.op === "!=") return left !== right;
        if (node.op === "+" && typeof left === "string" && typeof right === "string") return left + right;
        if (typeof left !== "number" || typeof right !== "number") throw new Error("not numbers");
        switch (node.op) {
          case "+":
            return left + right;
          case "-":
            return left - right;
          case "*":
            return left * right;
          case "/":
            if (right === 0) throw new Error("division by zero");
            return left / right;
          case "%":
            if (right === 0) throw new Error("division by zero");
            return left % right;
          case "<":
            return left < right;
          case "<=":
            return left <= right;
          case ">":
            return left > right;
          default:
            return left >= right;
        }
      }
      case "call": {
        const args = node.args.map(go);
        if (args.some((arg) => typeof arg !== "number")) throw new Error("not numbers");
        const [a, b] = args as number[];
        if (node.fn === "abs") return Math.abs(a);
        // Half away from zero, as Bloblang's `round` does.
        if (node.fn === "round") return Math.sign(a) * Math.round(Math.abs(a));
        return node.fn === "min" ? Math.min(a, b) : Math.max(a, b);
      }
    }
  };
  if (refsOf(formula).some((ref) => ofRef(ref) === null)) return null;
  try {
    return go(formula);
  } catch {
    return null;
  }
}

/** Where the class's formula pipeline reads and writes. */
export interface PipelineTarget {
  project: string;
  space: string;
  type: string;
  orgDomain: string;
  /** The space's Endpoint the pipeline reads and writes through: one bound to no single Policy. */
  endpoint: string;
}

/**
 * The manifests the class's formulas run by (DM-80): the scheduled derived pipeline, and the two
 * Policies that let the project's `pipelines` ServiceAccount read the slots the formulas read and
 * write the formula slots, and nothing else.
 */
export function formulaManifests(target: PipelineTarget, checked: Checked): ResourceProposal[] {
  const name = pipelineName(target.type);
  const formulas = checked.order.map((entry) => entry.name);
  const read = [...new Set([...checked.order.flatMap((entry) => refsOf(entry.formula)), ...formulas])];
  const meta = (suffix: string, title: string) => ({
    name: `${name}${suffix}`,
    namespace: target.project,
    title: { en: title },
  });
  const policy = (suffix: string, title: string, operations: string[], propertyNames: string[]): ResourceProposal => ({
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Policy",
    metadata: meta(suffix, title),
    spec: {
      contextSpaceRef: { kind: "ContextSpace", name: target.space },
      assigner: "did:web:{orgDomain}",
      assignee: { kind: "serviceAccount", id: "pipelines" },
      operations,
      information: [{ entities: [{ type: target.type }], propertyNames }],
    },
  }) as ResourceProposal;
  return [
    policy("-read", `The formula fields of ${target.type} read what they compute from`, ["queryEntity"], read),
    // The runner writes every stream as `entityOperations/upsert?options=update`, which merges the
    // attributes it names: the gateway decides it as `upsertBatch`, here narrowed to the formulas.
    // ponytail: an entity deleted between the read and the write comes back holding only its
    // formulas; a `PATCH …/attrs` output for update-attrs pipelines would close that.
    policy("-write", `The formula fields of ${target.type} are written by their pipeline`, ["upsertBatch"], formulas),
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Pipeline",
      metadata: meta("", `Formula fields of ${target.type}`),
      spec: {
        class: "scheduled",
        schedule: "*/5 * * * *",
        source: {
          endpointRef: { kind: "Endpoint", name: target.endpoint },
          query: { type: target.type, attrs: read },
        },
        targetEndpoint: `urn:ngsi-ld:Endpoint:${target.orgDomain}:${target.space}:${target.endpoint}`,
        output: { type: target.type, mode: "update-attrs" },
        compute: { kind: "bloblang", bloblang: formulaMapping(checked, target.type) },
      },
    } as ResourceProposal,
  ];
}
