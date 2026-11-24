// T-7706: fieldTypes edges: formula and e-mail words, an invalid select value, a formula problem of a non-formula, an unknown official type, a refused official slot.
import { describe, expect, it } from "vitest";
import { EMAIL_PATTERN, EMPTY_DRAFT, fieldWord, formulaProblem, problemsOf, suggestedSlots, withOfficialSlot } from "../src/components/entities/fieldTypes";
import { parseModel } from "../src/pages/models/linkml";

const SOURCE = `id: https://hel.fi/models/stations
name: stations
default_prefix: hel
prefixes:
  hel: https://hel.fi/models/
classes:
  Station:
    slots: [name, total, contact, doubled]
slots:
  name:
    range: string
  total:
    range: integer
  contact:
    range: string
    pattern: "${EMAIL_PATTERN.replace(/\\/g, "\\\\")}"
  doubled:
    range: integer
    equals_expression: "{total} * 2"
`;
const OFFICIAL = `id: https://smartdatamodels.org/Station
name: Station
classes:
  Station:
    slots: [name, capacity]
slots:
  name: { range: string }
  capacity: { range: integer }
`;
const model = parseModel(SOURCE);
const slot = (name: string) => {
  const found = model.slots.find((one) => one.name === name);
  if (!found) throw new Error(name);
  return found;
};

describe("fieldWord", () => {
  it("names a computed slot a formula and an address pattern an e-mail", () => {
    expect(fieldWord(slot("doubled"), model)).toEqual({ type: "formula" });
    expect(fieldWord(slot("contact"), model)).toEqual({ type: "email" });
    expect(fieldWord(slot("total"), model)).toEqual({ type: "integer" });
  });
});

describe("problemsOf and formulaProblem", () => {
  it("refuses a select value that is no value and says nothing of a formula for another type", () => {
    const draft = { ...EMPTY_DRAFT, name: "state", type: "select" as const, values: ["open", " padded "] };
    expect(problemsOf(draft, model, "Station").values).toBe("valueInvalid");
    expect(formulaProblem({ ...EMPTY_DRAFT, name: "plain" }, model, "Station")).toBeUndefined();
  });
});

describe("official slots", () => {
  it("offers nothing for a type the official model does not have", () => {
    expect(suggestedSlots(parseModel(OFFICIAL), model, "Nowhere")).toEqual([]);
    expect(suggestedSlots(parseModel(OFFICIAL), model, "Station").map((one) => one.name)).toEqual(["capacity"]);
  });

  it("hands back the refusal when the slot cannot be added to a class the model lacks", () => {
    const official = parseModel(OFFICIAL);
    const capacity = official.slots.find((one) => one.name === "capacity");
    if (!capacity) throw new Error("capacity");
    const applied = withOfficialSlot(SOURCE, capacity, official, model, "Nowhere");
    expect(applied.refused.length).toBeGreaterThan(0);
  });
});
