import { describe, expect, it } from "vitest";
import { columnLabel, displayOrder, readable } from "../src/components/entities/displayOrder";

const slots = [
  { name: "status" },
  { name: "location" },
  { name: "kpiValue", rank: 2 },
  { name: "name", rank: 1 },
  { name: "secret" },
  { name: "calculationPeriod" },
];

describe("the grid's columns as the model orders and names them (DM-98, DM-100, DM-102)", () => {
  it("ranked first, then the class's slots as written, system attributes last, unlisted after", () => {
    const granted = ["location", "status", "calculationPeriod", "name", "kpiValue", "zeta", "description", "alpha"];
    expect(displayOrder(slots, granted)).toEqual(["name", "kpiValue", "status", "calculationPeriod", "location", "description", "alpha", "zeta"]);
  });

  it("never lists a slot the caller's answer does not carry, however the model ranks it", () => {
    const order = displayOrder(slots, ["status", "name"]);
    expect(order).toEqual(["name", "status"]);
    expect(order).not.toContain("secret");
    expect(order).not.toContain("kpiValue");
    expect(displayOrder(slots, [])).toEqual([]);
  });

  it("a type without a model is ordered by the name heuristic alone", () => {
    expect(displayOrder([], ["refStop", "name", "dateModified", "speed"])).toEqual(["name", "refStop", "speed", "dateModified"]);
  });

  it("labels by annotations.label in the language, then the model's own, then the title, then the readable name", () => {
    const slot = { label: { fi: "Tila", "": "Stav" }, title: { en: "Status" } };
    expect(columnLabel(slot, "status", "fi-FI")).toBe("Tila");
    expect(columnLabel(slot, "status", "de")).toBe("Stav");
    expect(columnLabel({ title: { en: "Status" } }, "status", "de")).toBe("Status");
    expect(columnLabel(undefined, "ngsi-ld:availableBikeNumber", "en")).toBe("Available bike number");
    expect(readable("route_color")).toBe("Route color");
    expect(readable("https://smartdatamodels.org/dataModel.Transportation/vocab#routeColor")).toBe("Route color");
  });
});
