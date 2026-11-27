/**
 * Fuzz of the UI's parsers of outside text (T-7540, owner 2026-11-24 "fuzzy tests"): the model's
 * answer Markdown, a pasted MQTT topic filter and a pasted device key. A seeded generator builds
 * inputs from the characters each grammar reads, and each property states what must hold for every
 * input: no throw, and nothing outside the grammar's safe set comes out. A failure prints the seed
 * and the input, so it becomes a named regression test.
 */
import { describe, expect, it } from "vitest";
import JCRender from "../src/generated/assistantRender";
import type { Block, Inline } from "../src/generated/assistantRender";
import { topicProblem } from "../src/pages/mqttbrokers/brokerDraft";
import { parsePublicKey } from "../src/pages/mqttbrokers/devices";
import { EMPTY_MODEL, parseModel } from "../src/pages/models/linkml";

/** mulberry32: a small seeded generator, so every run draws the same inputs. */
function random(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function text(next: () => number, pieces: string[], max: number): string {
  const length = Math.floor(next() * max);
  let out = "";
  for (let i = 0; i < length; i += 1) out += pieces[Math.floor(next() * pieces.length)];
  return out;
}

const SEED = 7540;
const RUNS = 3000;

const MARKDOWN = [
  "*", "**", "_", "__", "`", "```", "~~~", "[", "]", "(", ")", "|", "\\|", "-", "#", "1.", "2)", " ", "\n", "\r\n",
  ",", "a", "Z", "7", "https://hel.fi/x", "javascript:alert(1)", "<script>", "&amp;", "[1]", "[2, 9]", ":--", "\u0000",
];
const INLINE_TAGS = new Set(["strong", "em", "code", "a", "cite", "br"]);
const BLOCK_TAGS = new Set(["p", "ul", "ol", "table", "pre"]);

function inlineProblem(node: Inline, known: (n: number) => boolean): string | undefined {
  if (typeof node === "string") return undefined;
  if (!INLINE_TAGS.has(node.tag)) return `inline tag ${node.tag}`;
  if (node.tag === "a" && !/^https?:\/\//.test(node.href)) return `link to ${node.href}`;
  if (node.tag === "cite") return node.numbers.length > 0 && node.numbers.every(known) ? undefined : "cite without a source";
  if (node.tag === "br") return undefined;
  for (const child of node.children) {
    const problem = inlineProblem(child, known);
    if (problem) return problem;
  }
  return undefined;
}

function blockProblem(block: Block, known: (n: number) => boolean): string | undefined {
  if (!BLOCK_TAGS.has(block.tag)) return `block tag ${block.tag}`;
  if (block.tag === "pre") {
    const code = block.children[0];
    return block.children.length === 1 && code.tag === "code" && code.children.every((c) => typeof c === "string") ? undefined : "pre without text";
  }
  const inlines: Inline[] =
    block.tag === "p"
      ? block.children
      : block.tag === "table"
        ? block.children.flatMap((row) => row.children.flatMap((cell) => cell.children))
        : block.children.flatMap((item) => item.children);
  for (const node of inlines) {
    const problem = inlineProblem(node, known);
    if (problem) return problem;
  }
  return undefined;
}

describe("fuzz: the answer's Markdown", () => {
  it("never throws and yields only the safe tags, http(s) links and citations that exist", () => {
    const next = random(SEED);
    const known = (n: number) => n === 1 || n === 2;
    for (let run = 0; run < RUNS; run += 1) {
      const input = text(next, MARKDOWN, 60);
      let blocks: Block[] = [];
      expect(() => (blocks = JCRender.markdown(input, known, () => true)), `seed ${SEED} run ${run}: ${JSON.stringify(input)}`).not.toThrow();
      for (const block of blocks) {
        expect(blockProblem(block, known), `seed ${SEED} run ${run}: ${JSON.stringify(input)}`).toBeUndefined();
      }
    }
  });
});

const TOPIC = ["a", "b", "/", "+", "#", "$", "${clientid}", "${x}", "sensors", "é", "", "//", "a+", "#a"];

describe("fuzz: a pasted MQTT topic filter", () => {
  it("accepts only filters inside jc-core's limits, under the device's own client id", () => {
    const next = random(SEED + 1);
    for (let run = 0; run < RUNS; run += 1) {
      const filter = text(next, TOPIC, 14);
      for (const kind of ["publish", "subscribe"] as const) {
        const problem = topicProblem(filter, kind);
        if (problem !== undefined) continue;
        const why = `seed ${SEED + 1} run ${run} ${kind}: ${JSON.stringify(filter)}`;
        const levels = filter.split("/");
        expect(new TextEncoder().encode(filter).length, why).toBeLessThanOrEqual(128);
        expect(levels.length, why).toBeLessThanOrEqual(8);
        expect(levels.every((level) => level !== ""), why).toBe(true);
        expect(levels.slice(0, -1).includes("#"), why).toBe(false);
        expect(levels.every((level) => !/[+#]/.test(level) || level === "+" || level === "#"), why).toBe(true);
        // The device's own subtree comes first: `${clientid}` before any wildcard.
        const own = levels.indexOf("${clientid}");
        const wild = levels.findIndex((level) => level === "+" || level === "#");
        expect(own >= 0 && (wild === -1 || own < wild), why).toBe(true);
      }
    }
  });
});

const X = "f83OJ3D2xF1Bg8vub9tLe1gHMzV76e8Tus9uPHvRVEU";
const Y = "x_FEzRu9m36HLN_tue659LNpXW6pCyStikYjKIWI5a0";
const MEMBERS: [string, unknown[]][] = [
  ["kty", ["EC", "RSA", "oct", "OKP", 1, null]],
  ["crv", ["P-256", "P-384", ""]],
  ["x", [X, X.slice(2), 5]],
  ["y", [Y, ""]],
  ["n", ["A".repeat(342), "A".repeat(10)]],
  ["e", ["AQAB", ""]],
  ["alg", ["ES256", "RS256", "HS256", "none"]],
  ["use", ["sig", "enc"]],
  ["kid", ["k1", "", "k".repeat(200), 7]],
  ["d", ["secret"]],
  ["k", ["c2VjcmV0"]],
  ["p", ["q"]],
];

const YAML = [
  "id: ", "name: ", "classes:", "slots:", "enums:", "attributes:", "is_a: ", "range: ", "mixins:", "imports:", "prefixes:",
  "permissible_values:", "required: true", "multivalued: [", "{", "}", "[", "]", "- ", ": ", "&a ", "*a", "!!str ", "? ",
  "'", '"', "#", "|", ">", "\n", "\n  ", "\n    ", "\t", "x", "Air", "7", "null", "~", "true", "\u0000", "\ufeff", "---\n", "...",
];

describe("fuzz: an uploaded or pasted LinkML model", () => {
  it("never throws: a source that does not read is the empty model, never an exception", () => {
    const next = random(SEED + 3);
    for (let run = 0; run < RUNS; run += 1) {
      const source = text(next, YAML, 60);
      let model: ReturnType<typeof parseModel> | undefined;
      expect(() => {
        model = parseModel(source);
      }, JSON.stringify(source)).not.toThrow();
      expect(Array.isArray(model?.classes), JSON.stringify(source)).toBe(true);
    }
  });

  it("reads a billion-laughs document and a non-mapping root as the empty model", () => {
    const laughs = ["a: &a [x, x]", ...Array.from({ length: 12 }, (_, i) => `${String.fromCharCode(98 + i)}: &${String.fromCharCode(98 + i)} [*${String.fromCharCode(97 + i)}, *${String.fromCharCode(97 + i)}]`)].join("\n");
    expect(parseModel(laughs)).toBe(EMPTY_MODEL);
    expect(parseModel("classes: [")).toBe(EMPTY_MODEL);
    expect(() => parseModel("- just\n- a list")).not.toThrow();
    expect(() => parseModel("42")).not.toThrow();
    expect(() => parseModel("classes: 5\nslots: x\nenums: [1, 2]")).not.toThrow();
  });
});

describe("fuzz: a pasted device key", () => {
  it("never throws, never takes a private member, and gives only P-256/ES256 or RSA/RS256 signing keys", async () => {
    const next = random(SEED + 2);
    for (let run = 0; run < 1500; run += 1) {
      const keys = Array.from({ length: 1 + Math.floor(next() * 3) }, () => {
        // A valid key with a few members changed or dropped, so both answers are drawn often.
        const key: Record<string, unknown> =
          next() < 0.5 ? { kty: "EC", crv: "P-256", x: X, y: Y } : { kty: "RSA", n: "A".repeat(342), e: "AQAB" };
        for (const [member, values] of MEMBERS) {
          const roll = next();
          if (roll < 0.08) delete key[member];
          else if (roll < 0.16) key[member] = values[Math.floor(next() * values.length)];
        }
        return key;
      });
      const input = next() < 0.5 ? JSON.stringify({ keys }) : JSON.stringify(keys[0]);
      const why = `seed ${SEED + 2} run ${run}: ${input}`;
      const parsed = await parsePublicKey(input);
      if (!parsed.ok) continue;
      for (const key of parsed.jwks.keys) {
        expect(Object.keys(key).some((member) => ["d", "p", "q", "dp", "dq", "qi", "oth", "k"].includes(member)), why).toBe(false);
        expect(key.use, why).toBe("sig");
        expect(key.kid.length, why).toBeGreaterThan(0);
        expect(key.kid.length, why).toBeLessThanOrEqual(128);
        if (key.kty === "EC") expect([key.crv, key.alg, key.x?.length, key.y?.length], why).toEqual(["P-256", "ES256", 43, 43]);
        else expect([key.kty, key.alg, (key.n ?? "").length >= 342], why).toEqual(["RSA", "RS256", true]);
      }
      expect(new Set(parsed.jwks.keys.map((key) => key.kid)).size, why).toBe(parsed.jwks.keys.length);
    }
  });
});
