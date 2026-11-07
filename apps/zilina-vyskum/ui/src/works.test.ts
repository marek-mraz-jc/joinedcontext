import { describe, expect, it } from "vitest";
import { toRichRow } from "@joinedcontext/sdk";
import { byNewest, countBy, narrowed, perYear, safeUrl, seriesOf, workOf } from "./works";
import { WORKS } from "./fixtures/works";

const works = WORKS.map((entity) => workOf(toRichRow(entity as Record<string, unknown>)));

describe("workOf", () => {
  it("reads a work's title, kind, year, collection, licence and handle", () => {
    const first = works.find((work) => work.id.endsWith(":hdluniza-936"));
    expect(first).toMatchObject({
      title: "Accuracy of digital terrain model on forest roads using airborne LIDAR and UAV point clouds",
      language: "sk",
      kind: "Conference paper",
      year: 2023,
      licence: "http://creativecommons.org/licenses/by/4.0/",
      url: "http://drepo.uniza.sk/handle/hdluniza/936",
    });
  });

  it("keeps a title of no stated language without one", () => {
    const entity = { ...WORKS[0], name: { type: "LanguageProperty", languageMap: { "@none": "Titul" } } };
    expect(workOf(toRichRow(entity as Record<string, unknown>))).toMatchObject({ title: "Titul", language: null });
  });

  it("never opens a link that is not http or https", () => {
    expect(safeUrl("javascript:alert(1)")).toBeNull();
    expect(safeUrl("not a url")).toBeNull();
  });
});

describe("seriesOf", () => {
  it("names the journal an issue belongs to", () => {
    expect(seriesOf("Krízový manažment - Ročník 24.; Číslo 2/2025")).toBe("Krízový manažment");
    expect(seriesOf("Práce a štúdie - Vydanie 17")).toBe("Práce a štúdie");
    expect(seriesOf("Súčasné problémy v koľajových vozidlách – PRORAIL 2023 Diel II.")).toBe("Súčasné problémy v koľajových vozidlách");
    expect(seriesOf("Komunikácie")).toBe("Komunikácie");
    expect(seriesOf(null)).toBeNull();
  });
});

describe("counts", () => {
  it("count the recorded works by kind as the pipeline wrote them", () => {
    expect(countBy(works, (work) => work.kind)).toEqual([
      { name: "Article", count: 102 },
      { name: "Conference paper", count: 72 },
      { name: "Book of proceedings", count: 33 },
      { name: "Working Paper", count: 25 },
      { name: "Journal", count: 19 },
    ]);
  });

  it("give every year from the first to the last, a year without a work as zero", () => {
    const years = perYear(works);
    expect(years[0].year).toBe(Math.min(...works.flatMap((w) => (w.year === null ? [] : [w.year]))));
    expect(years.find((entry) => entry.year === 2023)?.count).toBe(65);
    expect(years.map((entry) => entry.year)).toEqual(years.map((_, index) => years[0].year + index));
    expect(perYear([])).toEqual([]);
  });
});

describe("narrowed", () => {
  it("finds every word in the title or collection without diacritics, within the kind and year", () => {
    const found = narrowed(works, { search: "prace a studie", kind: null, year: null });
    expect(found.length).toBeGreaterThan(0);
    expect(found.every((work) => work.series === "Práce a štúdie")).toBe(true);
    const articles2023 = narrowed(works, { search: "", kind: "Article", year: 2023 });
    expect(articles2023.every((work) => work.kind === "Article" && work.year === 2023)).toBe(true);
    expect(narrowed(works, { search: "nic take neexistuje", kind: null, year: null })).toEqual([]);
  });

  it("orders the newest first", () => {
    const sorted = [...works].sort(byNewest);
    expect(sorted[0].year).toBe(Math.max(...works.map((w) => w.year ?? 0)));
  });
});

describe("what the repository writes otherwise", () => {
  const P = (value: unknown) => ({ type: "Property", value });
  const work = (attrs: Record<string, unknown>) => workOf(toRichRow({ id: "urn:ngsi-ld:Article:x", type: "Article", ...attrs }));

  it("keeps what a work does not carry as null, and reads the first of several values", () => {
    expect(work({})).toMatchObject({ title: null, language: null, kind: null, year: null, collection: null, series: null, licence: null, url: null });
    expect(work({ name: { type: "LanguageProperty", languageMap: { sk: "  ", en: " Title " } } })).toMatchObject({ title: "Title", language: "en" });
    expect(work({ yearPublished: P(2023.5), workType: P("   ") })).toMatchObject({ year: null, kind: null });
    expect(work({ workType: [P("Article"), P("Journal")] }).kind).toBe("Article");
  });

  it("opens https and http only", () => {
    expect(safeUrl("https://drepo.uniza.sk/handle/1")).toBe("https://drepo.uniza.sk/handle/1");
    expect(safeUrl(null)).toBeNull();
    expect(safeUrl("")).toBeNull();
  });

  it("names no journal for a collection that is only its issue", () => {
    expect(seriesOf(" - Vydanie 17")).toBeNull();
    expect(seriesOf("")).toBeNull();
  });

  it("counts no work without a value, and orders a tie by name", () => {
    const a = work({ workType: P("B") });
    const b = work({ workType: P("A") });
    expect(countBy([a, b, work({})], (w) => w.kind)).toEqual([
      { name: "A", count: 1 },
      { name: "B", count: 1 },
    ]);
  });

  it("orders a work without a year last, and one without a title first among its year", () => {
    const untitled = work({ yearPublished: P(2020) });
    const titled = work({ yearPublished: P(2020), name: { type: "LanguageProperty", languageMap: { sk: "A" } } });
    const undated = work({ name: { type: "LanguageProperty", languageMap: { sk: "B" } } });
    expect([undated, titled, untitled].sort(byNewest)).toEqual([untitled, titled, undated]);
    expect(byNewest(undated, undated)).toBe(0);
    expect(narrowed([untitled, undated], { search: "b", kind: null, year: null })).toEqual([undated]);
  });
});
