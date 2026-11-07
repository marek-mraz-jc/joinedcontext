import { describe, expect, it } from "vitest";
import { parse } from "yaml";
import { withRange } from "../src/pages/spaces/ModelReview";
import { spaceNameOf, tableNameOf } from "../src/pages/spaces/csvUpload";
import { withPlace } from "../src/pages/spaces/DraftedModel";

describe("withRange (T-6456)", () => {
  it("changes a shared slot's range and keeps the rest of the model", () => {
    const out = withRange("id: https://x/m\nclasses:\n  T:\n    slots: [a]\nslots:\n  a: { range: integer, title: { en: A } }\n", "T", "a", "string");
    expect(parse(out)).toMatchObject({ id: "https://x/m", slots: { a: { range: "string", title: { en: "A" } } } });
  });

  it("changes an attribute written on the class", () => {
    const out = withRange("classes:\n  T:\n    attributes:\n      a:\n        range: float\n", "T", "a", "date");
    expect(parse(out)).toEqual({ classes: { T: { attributes: { a: { range: "date" } } } } });
  });
});

describe("spaceNameOf (T-6456)", () => {
  it("makes a DNS-1123 label of a file name", () => {
    expect(spaceNameOf("Kultúrne miesta BBSK.xlsx")).toBe("kulturne-miesta-bbsk");
    expect(spaceNameOf("__.csv")).toBe("");
  });
});

describe("tableNameOf and withPlace (T-6567)", () => {
  it("names a table after its own sheet, a default sheet after the file", () => {
    expect(tableNameOf("priklad.xlsx", "Detské ihriská")).toBe("Detské ihriská");
    expect(spaceNameOf(tableNameOf("priklad.xlsx", "Detské ihriská"))).toBe("detske-ihriska");
    expect(tableNameOf("garages.xlsx", "Sheet1")).toBe("garages");
    expect(tableNameOf("garages.xlsx", "Hárok1")).toBe("garages");
    expect(tableNameOf("garages.csv", "garages")).toBe("garages");
    expect(spaceNameOf("Ul. 9. mája")).toBe("ul-9-maja");
  });

  it("draws the position a latitude and a longitude column fill, and nothing without them", () => {
    const model = "imports: [linkml:types, ngsi-ld-core]\nclasses:\n  T:\n    is_a: Entity\n    slots: [name]\nslots:\n  name: { range: string }\n";
    const geo = { columns: ["name", "Zemepisná šírka", "Zemepisná dĺžka"], rows: [] };
    expect(parse(withPlace(model, "T", geo))).toMatchObject({ classes: { T: { slots: ["name", "location"] } }, slots: { location: { range: "GeoProperty" } } });
    expect(withPlace(model, "T", { columns: ["name"], rows: [] })).toBe(model);
  });
});
