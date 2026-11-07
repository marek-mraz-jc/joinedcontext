import { describe, expect, it } from "vitest";
import { toRichRow } from "@joinedcontext/sdk";
import { bridgeOf, highestTenth, sorted, toCsv, totals } from "./bridges";
import { BRIDGES } from "./fixtures/mosty";

const YEAR = 2026;
const bridges = BRIDGES.map((entity) => bridgeOf(toRichRow(entity), "sk", YEAR));
const byCode = (code: string) => bridges.find((b) => b.code === code)!;

describe("a bridge", () => {
  it("is as old as the published year makes it, and never of negative age", () => {
    expect(byCode("66-001")).toMatchObject({ yearBuilt: 1931, age: 95, length: 210, heritage: "culturalAndTechnical" });
    expect(byCode("x-2").age).toBeNull();
    expect(byCode("x-1")).toMatchObject({ yearBuilt: null, age: null, length: null, roadClass: null });
  });
});

describe("a bridge's values", () => {
  const one = (attrs: Record<string, unknown>) => bridgeOf(toRichRow({ id: "urn:ngsi-ld:Bridge:x", type: "Bridge", ...attrs }), "en", YEAR);

  it("reads a name in the reader's language, else Slovak, else the one written; a number as text; blanks as missing", () => {
    expect(one({ name: { type: "LanguageProperty", languageMap: { sk: "Most", en: "Bridge" } } }).name).toBe("Bridge");
    expect(one({ name: { type: "LanguageProperty", languageMap: { sk: "Most" } } }).name).toBe("Most");
    expect(one({ name: { type: "LanguageProperty", languageMap: { hu: "Híd" } } }).name).toBe("Híd");
    expect(one({ roadNumber: { type: "Property", value: 66 } }).roadNumber).toBe("66");
    expect(one({ structureMaterial: { type: "Property", value: "  " } }).material).toBeNull();
    expect(one({ structureMaterial: { type: "Property", value: true } }).material).toBeNull();
  });

  it("takes the first of several values, and never a negative or non-finite count", () => {
    expect(one({ spanCount: [{ type: "Property", value: 4, datasetId: "urn:a" }, { type: "Property", value: 5, datasetId: "urn:b" }] }).spans).toBe(4);
    expect(one({ spanCount: { type: "Property", value: -1 } }).spans).toBeNull();
    expect(one({ bridgedLength: { type: "Property", value: "12" } }).length).toBeNull();
  });
});

describe("the region's figures", () => {
  it("add up the lengths published, take the median age and count listed monuments and gaps", () => {
    const sum = totals(bridges);
    expect(sum.bridges).toBe(12);
    expect(sum.length).toBe(210 + 96 + 34 + 140 + 18 + 41 + 52 + 30 + 27 + 260 + 9);
    expect(sum.listed).toBe(2);
    expect(sum.incomplete).toBe(2);
    expect(sum.medianAge).toBe(54);
  });
});

describe("the highest tenth", () => {
  it("starts where the oldest tenth starts, and is nothing for fewer than ten", () => {
    const from = highestTenth(bridges.map((b) => b.age))!;
    expect(bridges.filter((b) => b.age !== null && b.age >= from).map((b) => b.code)).toEqual(["51-020"]);
    expect(highestTenth(bridges.slice(0, 5).map((b) => b.age))).toBeNull();
  });

  it("takes an odd count's middle age, and has none without a known year", () => {
    expect(totals(bridges.filter((b) => b.age !== null).slice(0, 3)).medianAge).toBe(
      [...bridges.filter((b) => b.age !== null).slice(0, 3).map((b) => b.age!)].sort((a, b) => a - b)[1],
    );
    expect(totals([byCode("x-1")]).medianAge).toBeNull();
    expect(totals([])).toMatchObject({ bridges: 0, length: 0, medianAge: null });
  });
});

describe("sorting and the CSV", () => {
  it("puts a missing value last whichever the direction", () => {
    expect(sorted(bridges, "age", false)[0].code).toBe("51-020");
    expect(sorted(bridges, "age", true).at(-1)?.age).toBeNull();
    expect(sorted(bridges, "age", false).at(-1)?.age).toBeNull();
  });

  it("writes the values in words and never lets a cell run as a formula", () => {
    const csv = toCsv([byCode("x-2"), byCode("66-001")], ["a"], { service: "účelová", culturalAndTechnical: "kultúrna a technická pamiatka" });
    const [, formula, listed] = csv.trimEnd().split("\r\n");
    expect(formula.startsWith("'=CMD(),x-2,účelová")).toBe(true);
    expect(listed).toContain("kultúrna a technická pamiatka");
  });

  it("quotes a cell that holds a separator, a quote or a line break, and rounds a number to cents", () => {
    const tricky = { ...byCode("66-001"), name: 'Most "Pod hradom", sever', manager: "SSC\nIVSC", length: 210.456 };
    const [, row] = toCsv([tricky], ["a"], {}).trimEnd().split("\r\n");
    expect(row.startsWith('"Most ""Pod hradom"", sever"')).toBe(true);
    expect(row).toContain("210.46");
  });

  it("sorts names in the region's order, both ways", () => {
    const names = sorted(bridges, "name", true).map((b) => b.name);
    expect(names.at(-1)).not.toBeNull();
    expect(sorted(bridges, "name", false).map((b) => b.name)).toEqual([...names].reverse());
  });
});
