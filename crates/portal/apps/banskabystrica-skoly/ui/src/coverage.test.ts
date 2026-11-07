import { describe, expect, it } from "vitest";
import { toRichRow } from "@joinedcontext/sdk";
import { schoolOf, sorted, tenth, toCsv, totals, withShared } from "./coverage";
import { SCHOOLS } from "./fixtures/skoly";

const schools = SCHOOLS.map((entity) => schoolOf(toRichRow(entity), "sk"));
const byId = (suffix: string) => schools.find((one) => one.id.endsWith(suffix))!;

describe("a school's ratios", () => {
  it("come only from the counts the school published", () => {
    expect(byId("zs-moyzesova").pupilsPerTeacher).toBeCloseTo(12.8);
    expect(byId("zs-moyzesova").budgetPerPupil).toBe(5000);
    expect(byId("sos-bez-udajov")).toMatchObject({ pupils: null, teachers: null, pupilsPerTeacher: null, budgetPerPupil: null });
  });

  it("are never a division by zero", () => {
    const empty = schoolOf(toRichRow({ id: "urn:ngsi-ld:Organization:x:y:z", type: "Organization", pupilCount: { type: "Property", value: 0 }, teachingStaff: { type: "Property", value: 0 } }), "sk");
    expect(empty.pupilsPerTeacher).toBeNull();
    expect(empty.budgetPerPupil).toBeNull();
  });
});

describe("the city's totals", () => {
  it("take the city ratio as all pupils over all teachers, and count what is incomplete", () => {
    const sum = totals(schools);
    const both = schools.filter((one) => one.pupils !== null && one.teachers !== null);
    const expected = both.reduce((a, one) => a + one.pupils!, 0) / both.reduce((a, one) => a + one.teachers!, 0);
    expect(sum.pupilsPerTeacher).toBeCloseTo(expected);
    expect(sum.schools).toBe(12);
    expect(sum.incomplete).toBe(1);
  });
});

describe("the city's tenth", () => {
  it("is the value the highest or lowest tenth starts at, and nothing for fewer than ten schools", () => {
    const ratios = schools.map((one) => one.pupilsPerTeacher);
    const high = tenth(ratios, "high")!;
    // Eleven schools publish both counts: a tenth of them is two (ceil(11 / 10)).
    expect(ratios.filter((r) => r !== null && r >= high)).toHaveLength(2);
    const budgets = schools.map((one) => one.budgetPerPupil);
    const low = tenth(budgets, "low")!;
    expect(budgets.filter((b) => b !== null && b <= low)).toHaveLength(2);
    // Exactly ten known values: a tenth is one.
    const ten = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
    expect(tenth(ten, "high")).toBe(10);
    expect(tenth(ten, "low")).toBe(1);
    expect(tenth(ratios.slice(0, 9), "high")).toBeNull();
  });
});

describe("sorting", () => {
  it("puts a missing value last whichever the direction", () => {
    expect(sorted(schools, "pupils", true).at(-1)?.id).toMatch(/sos-bez-udajov/);
    expect(sorted(schools, "pupils", false).at(-1)?.id).toMatch(/sos-bez-udajov/);
    expect(sorted(schools, "pupils", false)[0].pupils).toBe(690);
  });
});

describe("the CSV", () => {
  it("quotes what needs quoting and never lets a cell run as a formula", () => {
    const csv = toCsv([byId("zs-vzorec"), byId("sos-bez-udajov")], ["Škola", "Adresa", "a", "b", "c", "d", "e", "f", "g"]);
    const [header, formula, missing] = csv.trimEnd().split("\r\n");
    expect(header).toBe("Škola,Adresa,a,b,c,d,e,f,g");
    expect(formula.startsWith(`"'=HYPERLINK(""http://example.org""), Námestie 1"`)).toBe(true);
    expect(missing).toBe("Stredná odborná škola, Tajovského 30,Tajovského 30, Banská Bystrica,,,,,,,".replace("Stredná odborná škola, Tajovského 30", '"Stredná odborná škola, Tajovského 30"').replace("Tajovského 30, Banská Bystrica", '"Tajovského 30, Banská Bystrica"'));
  });
});

describe("a school's odd shapes", () => {
  const one = (attrs: Record<string, unknown>, locale = "en") => schoolOf(toRichRow({ id: "urn:ngsi-ld:Organization:x", type: "Organization", ...attrs }), locale);
  it("reads the Smart Data Models PostalAddress as one line (T-4329)", () => {
    const value = { streetAddress: "Havranské 9", postalCode: "97590", addressLocality: "Banská Bystrica", addressCountry: "SK" };
    expect(one({ address: { type: "Property", value } }).address).toBe("Havranské 9, 97590 Banská Bystrica");
  });

  it("reads a name in the reader's language, else Slovak, else the one written, and the first of several values", () => {
    expect(one({ name: { type: "LanguageProperty", languageMap: { sk: "Škola", en: "Organization" } } }).name).toBe("Organization");
    expect(one({ name: { type: "LanguageProperty", languageMap: { sk: "Škola" } } }).name).toBe("Škola");
    expect(one({ name: { type: "LanguageProperty", languageMap: { hu: "Iskola" } } }).name).toBe("Iskola");
    expect(one({ name: [{ type: "Property", value: "Prvá", datasetId: "urn:a" }, { type: "Property", value: "Druhá", datasetId: "urn:b" }] }).name).toBe("Prvá");
  });

  it("keeps two missing values side by side when sorting", () => {
    const bare = [one({}), one({})];
    expect(sorted(bare, "pupils", true)).toHaveLength(2);
  });
});


// T-3577: the national school map repeats one organization's staff and budget on each of its
// schools (54 city kindergartens each publish 225 teachers, 153.8 other staff, 780 546 €), so a
// sum counted the same 225 teachers 54 times and the city read 3.5 pupils per teacher.
describe("staff and budget several schools publish identically", () => {
  const kindergarten = (id: string, pupils: number | null) =>
    schoolOf(
      toRichRow({
        id: `urn:ngsi-ld:Organization:x:y:${id}`,
        type: "Organization",
        ...(pupils === null ? {} : { pupilCount: { type: "Property", value: pupils } }),
        teachingStaff: { type: "Property", value: 225 },
        nonTeachingStaff: { type: "Property", value: 153.8 },
        annualBudget: { type: "Property", value: 780546 },
      }),
      "sk",
    );
  const own = schoolOf(
    toRichRow({ id: "urn:ngsi-ld:Organization:x:y:zs", type: "Organization", pupilCount: { type: "Property", value: 300 }, teachingStaff: { type: "Property", value: 20 }, annualBudget: { type: "Property", value: 900000 } }),
    "sk",
  );
  const marked = withShared([kindergarten("a", 96), kindergarten("b", 88), kindergarten("c", null), own]);

  it("are one organization's, so no school gets a ratio of its own from them", () => {
    expect(marked.slice(0, 3).map((one) => [one.sharedWith, one.pupilsPerTeacher, one.budgetPerPupil])).toEqual([
      [3, null, null],
      [3, null, null],
      [3, null, null],
    ]);
    expect(marked[3]).toMatchObject({ sharedWith: 1, pupilsPerTeacher: 15 });
  });

  it("count once in the city's totals, against the pupils of the schools that share them", () => {
    const sum = totals(marked);
    expect(sum.teachers).toBe(245);
    expect(sum.pupils).toBe(484);
    expect(sum.pupilsPerTeacher).toBeCloseTo(484 / 245);
  });

  it("leave a zero or a missing count alone: nothing is shared by not publishing", () => {
    const zero = (id: string) => schoolOf(toRichRow({ id: `urn:ngsi-ld:Organization:x:y:${id}`, type: "Organization", teachingStaff: { type: "Property", value: 0 } }), "sk");
    expect(withShared([zero("a"), zero("b")]).map((one) => one.sharedWith)).toEqual([1, 1]);
  });
});
