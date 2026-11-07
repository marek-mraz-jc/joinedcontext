// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/explore/exportView.ts.
// The explorer's reads (T-3253): a CSV field a spreadsheet would run stays text, the geometry is
// found, the filters join, and the whole view is paged, capped and cancelled the way the page says.
// The file formats moved to the space page's Export menu (T-6107).
import { describe, expect, it } from "vitest";
import { andQ, collect, csvField, geometryOf } from "../src/pages/explore/exportView";

describe("a CSV field", () => {
  it("keeps a text a spreadsheet would run as a formula as text, and a negative number a number", () => {
    expect(csvField('=HYPERLINK("http://evil")', ";", false)).toBe(`"'=HYPERLINK(""http://evil"")"`);
    expect(csvField("@SUM(A1)", ",", false)).toBe("'@SUM(A1)");
    expect(csvField("-2", ";", true)).toBe("-2");
  });

  it("quotes a field holding the separator, a quote or a line break", () => {
    expect(csvField("Kauppatori; harbour", ";", false)).toBe('"Kauppatori; harbour"');
    expect(csvField("Kauppatori; harbour", ",", false)).toBe("Kauppatori; harbour");
    expect(csvField("Rautatientori\nnorth", ",", false)).toBe('"Rautatientori\nnorth"');
  });
});

describe("the view's geometry and filter", () => {
  it("reads the location first, a normalized one by its value, and nothing when none is located", () => {
    const point = { type: "Point", coordinates: [24.95, 60.17] };
    const line = { type: "LineString", coordinates: [[0, 0], [1, 1]] };
    expect(geometryOf({ id: "a", route: line, location: { type: "GeoProperty", value: point } })).toEqual(point);
    expect(geometryOf({ id: "a", route: line })).toEqual(line);
    expect(geometryOf({ id: "a", name: "Kauppatori" })).toBeUndefined();
  });

  it("joins the page's filter and the grid's with ;, an alternative in brackets", () => {
    expect(andQ("bikes>0", undefined, " ")).toBe("bikes>0");
    expect(andQ("bikes>0", "name==\"a\"|name==\"b\"")).toBe('bikes>0;(name=="a"|name=="b")');
    expect(andQ(undefined, "")).toBeUndefined();
  });
});

describe("reading the whole view", () => {
  const pages = (total: number) => async (offset: number, limit: number) => ({
    rows: Array.from({ length: Math.max(0, Math.min(limit, total - offset)) }, (_, i) => ({ id: String(offset + i) })),
    count: total,
  });

  it("pages until the endpoint has no more, saying how far it is", async () => {
    const seen: number[] = [];
    const { entities, truncated, total } = await collect(pages(1_100), { pageSize: 500, progress: (done) => seen.push(done) });
    expect(entities).toHaveLength(1_100);
    expect(seen).toEqual([500, 1_000, 1_100]);
    expect(truncated).toBe(false);
    expect(total).toBe(1_100);
  });

  it("stops at the cap and says the rest was left out", async () => {
    const { entities, truncated } = await collect(pages(1_300), { pageSize: 500, max: 1_000 });
    expect(entities).toHaveLength(1_000);
    expect(truncated).toBe(true);
  });

  it("an empty view is no error, and a cancelled one asks no further page", async () => {
    expect((await collect(pages(0))).entities).toEqual([]);
    const controller = new AbortController();
    let asked = 0;
    const reading = collect(
      async (offset, limit) => {
        asked += 1;
        controller.abort();
        return pages(5_000)(offset, limit);
      },
      { signal: controller.signal, pageSize: 500 },
    );
    await expect(reading).rejects.toMatchObject({ name: "AbortError" });
    expect(asked).toBe(1);
  });
});
