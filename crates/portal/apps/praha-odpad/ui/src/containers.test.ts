import { describe, expect, it } from "vitest";
import { toRichRow } from "@joinedcontext/sdk";
import { containerOf, highestTenth, sorted, toCsv, totals } from "./containers";
import { CONTAINERS, NOW } from "./fixtures/odpad";

const containers = CONTAINERS.map((entity) => containerOf(toRichRow(entity), NOW));
const byCode = (code: string) => containers.find((c) => c.code === code)!;

describe("a container", () => {
  it("reads its fill, the age of its reading and its isle, and keeps a missing reading missing", () => {
    expect(byCode("0001-PAP")).toMatchObject({ kind: "paper", fill: 0.95, isle: expect.stringContaining(":i1") });
    expect(byCode("0001-PAP").ageHours).toBeCloseTo(2);
    expect(byCode("0006-PAP")).toMatchObject({ fill: null, measuredAt: null, ageHours: null, isle: null });
  });

  it("never takes an out-of-range fill for a reading, nor a future time for a negative age", () => {
    const bad = containerOf(toRichRow({ id: "urn:ngsi-ld:WasteContainer:x:y:z", type: "WasteContainer", fillingLevel: { type: "Property", value: 1.7 }, dateModified: { type: "Property", value: "2030-01-01T00:00:00Z" } }), NOW);
    expect(bad.fill).toBeNull();
    expect(bad.ageHours).toBe(0);
  });
});

describe("the city's figures", () => {
  it("take the mean over valid readings and count the unread", () => {
    const sum = totals(containers);
    expect(sum.containers).toBe(12);
    expect(sum.unread).toBe(1);
    expect(sum.meanFill).toBeCloseTo((0.95 + 0.4 + 0.7 + 0.2 + 0.55 + 0.1 + 0.88 + 0.3 + 0.45 + 0.6 + 0.5) / 11);
  });
});

describe("the highest tenth", () => {
  it("marks ceil(n/10) of the fullest and the longest unread, and nothing for fewer than ten", () => {
    const fullFrom = highestTenth(containers.map((c) => c.fill))!;
    expect(containers.filter((c) => c.fill !== null && c.fill >= fullFrom).map((c) => c.code)).toEqual(["0001-PAP", "0004-PAP"]);
    const unreadFrom = highestTenth(containers.map((c) => c.ageHours))!;
    expect(containers.filter((c) => c.ageHours !== null && c.ageHours >= unreadFrom).map((c) => c.code)).toEqual(["0002-GLS", "0005-GLS"]);
    expect(highestTenth([0.1, 0.2])).toBeNull();
  });
});

describe("sorting and the CSV", () => {
  it("puts a missing reading last whichever the direction", () => {
    expect(sorted(containers, "fill", false)[0].code).toBe("0001-PAP");
    expect(sorted(containers, "fill", true).at(-1)?.fill).toBeNull();
  });

  it("writes the fill as a percentage, the kind and the isle in words, and no formula", () => {
    const csv = toCsv([byCode("=0006-X"), byCode("0001-PAP")], ["a"], { plastic: "plast", paper: "papír" }, (id) => (id?.endsWith(":i1") ? "Vinohradská 12" : null));
    const [, formula, paper] = csv.trimEnd().split("\r\n");
    expect(formula.startsWith("'=0006-X,plast,50,")).toBe(true);
    expect(paper).toMatch(/^0001-PAP,papír,95,.*,Vinohradská 12$/);
  });
});

describe("the table's order and its export", () => {
  it("orders by any column either way and keeps a missing value last both ways", () => {
    for (const key of ["fill", "ageHours"] as const) {
      for (const ascending of [true, false]) expect(sorted(containers, key, ascending).at(-1)?.code).toBe("0006-PAP");
    }
    const up = sorted(containers, "code", true).map((c) => c.code);
    expect(sorted(containers, "code", false).map((c) => c.code)).toEqual([...up].reverse());
    const unnamed = { ...byCode("0001-PAP"), code: null };
    expect(sorted([unnamed, byCode("0002-PAP")], "code", true).at(-1)).toBe(unnamed);
    const twoMissing = [{ ...byCode("0006-PAP") }, { ...byCode("0006-PAP"), id: "other" }];
    expect(sorted(twoMissing, "fill", true)).toHaveLength(2);
  });

  it("writes what a spreadsheet would run as text, quotes separators, and leaves nothing as empty", () => {
    const words = { paper: "papír" };
    const csv = toCsv(
      [byCode("=0006-X"), { ...byCode("0001-PAP"), code: 'A "quoted", code' }, { ...byCode("0006-PAP"), kind: null }, { ...byCode("0002-GLS") }],
      ["a", "b", "c", "d", "e"],
      words,
      (id) => (id ? "Isle, one" : null),
    );
    const lines = csv.trimEnd().split("\r\n");
    expect(lines[1].startsWith("'=0006-X,plastic,50,")).toBe(true);
    expect(lines[2].startsWith('"A ""quoted"", code",papír,95,')).toBe(true);
    expect(lines[2].endsWith('"Isle, one"')).toBe(true);
    expect(lines[3]).toBe("0006-PAP,,,,");
    expect(lines[4]).toContain("colouredGlass");
  });
});
