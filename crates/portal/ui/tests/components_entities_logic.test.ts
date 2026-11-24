/**
 * The pure parts of the entities folder that the page tests leave open: the evaluator's operators,
 * the formula parser's refusal of a stray operator, a field's enum name colliding with one the
 * model has, an official slot copied with and without its enum, the kept grid layout read back
 * from a hand-edited store, and the gateway reads of filters.ts and historyApi.ts on refusals.
 */
import { afterEach, describe, expect, it, vi } from "vitest";
import { evaluate, FormulaError, parseFormula } from "../src/components/entities/formula";
import { EMPTY_DRAFT, fieldOperations, withOfficialSlot } from "../src/components/entities/fieldTypes";
import { parseModel } from "../src/pages/models/linkml";
import { readLayout, writeLayout } from "../src/components/entities/viewLayout";
import { deleteEntity, fetchEntities, fetchEntity } from "../src/components/entities/filters";
import { readHistory } from "../src/components/entities/history/historyApi";

afterEach(() => {
  vi.restoreAllMocks();
  window.localStorage.clear();
});

describe("evaluate", () => {
  const run = (text: string, values: Record<string, unknown> = {}) => evaluate(parseFormula(text), values);

  it("adds, subtracts and compares numbers, and joins two texts", () => {
    expect(run("{a} + {b}", { a: 2, b: 3 })).toBe(5);
    expect(run("{a} - {b}", { a: 2, b: 3 })).toBe(-1);
    expect(run("{a} < {b}", { a: 2, b: 3 })).toBe(true);
    expect(run("{a} <= {b}", { a: 3, b: 3 })).toBe(true);
    expect(run("{a} > {b}", { a: 2, b: 3 })).toBe(false);
    expect(run("{a} >= {b}", { a: 2, b: 3 })).toBe(false);
    expect(run("{a} + {b}", { a: "x", b: "y" })).toBe("xy");
  });

  it("gives no value for a modulo by zero", () => {
    expect(run("{a} % {b}", { a: 4, b: 0 })).toBeNull();
    expect(run("{a} % {b}", { a: 7, b: 4 })).toBe(3);
  });
});

describe("parseFormula", () => {
  it("names the operator that stands where a value must", () => {
    expect(() => parseFormula("* 2")).toThrow(FormulaError);
    expect(() => parseFormula("* 2")).toThrow("`*` is not expected here");
  });
});

const MODEL = `id: https://hel.fi/models/stations
name: stations
default_prefix: hel
prefixes:
  hel: https://hel.fi/models/
enums:
  StationState:
    permissible_values:
      open: {}
classes:
  Station:
    slots: [name]
slots:
  name:
    range: string
`;

describe("parseModel on a source that does not parse (T-7588)", () => {
  it("answers the empty model for an alias bomb instead of throwing", () => {
    const bomb = `id: https://example.org/m\nname: m\na: &a [x]\nclasses: [${Array.from({ length: 120 }, () => "*a").join(", ")}]\n`;
    expect(() => parseModel(bomb)).not.toThrow();
    expect(parseModel(bomb).classes).toEqual([]);
  });
});

describe("fieldOperations", () => {
  it("numbers an enum name the model already has with a trailing underscore", () => {
    const model = parseModel(MODEL);
    const ops = fieldOperations({ ...EMPTY_DRAFT, name: "state", type: "select", values: ["a", "b"] }, model, "Station");
    expect(ops.slice(0, 3)).toEqual([
      { op: "addEnum", name: "StationState_" },
      { op: "addEnumValue", enum: "StationState_", value: "a" },
      { op: "addEnumValue", enum: "StationState_", value: "b" },
    ]);
  });
});

const OFFICIAL = parseModel(`id: https://smartdatamodels.org/Station
name: Station
enums:
  StatusEnum:
    permissible_values:
      working: { description: In use }
      broken: {}
classes:
  Station:
    slots: [status]
slots:
  status:
    range: StatusEnum
    multivalued: true
    description: Where the station stands
`);

describe("withOfficialSlot", () => {
  it("copies the enum, its values and the multiplicity with the slot", () => {
    const slot = OFFICIAL.slots.find((s) => s.name === "status")!;
    const result = withOfficialSlot(MODEL, slot, OFFICIAL, parseModel(MODEL), "Station");
    expect(result.refused).toEqual([]);
    const after = parseModel(result.source);
    expect(after.enums.find((e) => e.name === "StatusEnum")?.permissible_values.map((v) => [v.name, v.description])).toEqual([
      ["working", "In use"],
      ["broken", undefined],
    ]);
    expect(after.slots.find((s) => s.name === "status")).toMatchObject({ range: "StatusEnum", multivalued: true, description: "Where the station stands" });
  });

  it("does not add an enum the model has already", () => {
    const has = MODEL.replace("StationState:", "StatusEnum:").replace("open: {}", "fine: {}");
    const slot = OFFICIAL.slots.find((s) => s.name === "status")!;
    const result = withOfficialSlot(has, slot, OFFICIAL, parseModel(has), "Station");
    expect(result.refused).toEqual([]);
    expect(parseModel(result.source).enums.find((e) => e.name === "StatusEnum")?.permissible_values.map((v) => v.name)).toEqual(["fine"]);
  });
});

describe("readLayout", () => {
  const KEY = "jc.space-columns.p.s.T";

  it("keeps a width inside 40 to 2000 and a label of up to 80 letters, and drops the rest", () => {
    window.localStorage.setItem(
      KEY,
      JSON.stringify({ hidden: ["a"], order: ["b", "a"], width: { a: 120, b: 10, c: "wide", d: 5000 }, labels: { a: "Name", b: 7, c: "x".repeat(81) } }),
    );
    expect(readLayout("p", "s", "T")).toEqual({ hidden: ["a"], order: ["b", "a"], width: { a: 120 }, labels: { a: "Name" } });
  });

  it("reads nothing for no type, no entry, text that is no JSON or a store that throws", () => {
    expect(readLayout("p", "s", "")).toBeNull();
    expect(readLayout("p", "s", "T")).toBeNull();
    window.localStorage.setItem(KEY, "{not json");
    expect(readLayout("p", "s", "T")).toBeNull();
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    expect(readLayout("p", "s", "T")).toBeNull();
  });

  it("writes what it reads back", () => {
    writeLayout("p", "s", "T", { hidden: ["a"], width: { a: 200 }, order: ["a"], labels: { a: "A" } });
    expect(readLayout("p", "s", "T")).toEqual({ hidden: ["a"], order: ["a"], width: { a: 200 }, labels: { a: "A" } });
  });
});

function stubFetch(answer: (request: Request) => Response) {
  const seen: Request[] = [];
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input) => {
    seen.push(input as Request);
    return answer(input as Request);
  });
  return seen;
}

describe("fetchEntities", () => {
  it("asks for no rows without a type, and sends the scope and the id pattern trimmed", async () => {
    const seen = stubFetch(() => new Response("[]", { status: 200 }));
    expect(await fetchEntities("bikes", {}, { limit: 10 })).toEqual({ rows: [] });
    expect(seen).toHaveLength(0);
    await fetchEntities("bikes", { type: "Station", scopeQ: " /city/# ", idPattern: " ^urn:.*7$ " }, { limit: 10 });
    const url = new URL(seen[0].url);
    expect(url.searchParams.get("scopeQ")).toBe("/city/#");
    expect(url.searchParams.get("idPattern")).toBe("^urn:.*7$");
    expect(url.searchParams.get("type")).toBe("Station");
  });
});

describe("fetchEntity", () => {
  it("refuses an answer that is no entity", async () => {
    stubFetch(() => new Response(JSON.stringify({ name: "x" }), { status: 200 }));
    await expect(fetchEntity("bikes", "urn:ngsi-ld:Station:1")).rejects.toMatchObject({ status: 502, message: "Not an entity" });
  });
});

describe("deleteEntity", () => {
  it("falls back to the status line when the refusal is no JSON", async () => {
    const seen = stubFetch(() => new Response("<html>nope</html>", { status: 403, statusText: "Forbidden" }));
    await expect(deleteEntity("bikes", "urn:ngsi-ld:Station:1", "wrong site")).rejects.toMatchObject({ status: 403, message: "Forbidden" });
    expect(seen[0].method).toBe("DELETE");
    expect(seen[0].headers.get("JC-Reason")).toBe("wrong%20site");
  });
});

describe("readHistory", () => {
  it("throws the gateway's own sentence, or the status when it sent none", async () => {
    stubFetch(() => new Response(JSON.stringify({ title: "Bad Gateway", detail: "broker down" }), { status: 502 }));
    await expect(readHistory("bikes", "urn:ngsi-ld:Station:1", "level")).rejects.toThrow("broker down");
    vi.restoreAllMocks();
    stubFetch(() => new Response("oops", { status: 500 }));
    await expect(readHistory("bikes", "urn:ngsi-ld:Station:1", "level")).rejects.toThrow("500");
  });
});
