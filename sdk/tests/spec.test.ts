import { describe, expect, it } from "vitest";
import example from "../spec.example.json";
import { parseSpec, sourceOf } from "../src/spec";


describe("parseSpec", () => {
  it("accepts the example", () => {
    const parsed = parseSpec(example);
    expect(parsed.errors).toEqual([]);
    expect(parsed.spec?.title).toBe("Helsinki city bikes");
  });
  it("names every problem at once, with its path", () => {
    const parsed = parseSpec({
      title: "",
      sources: [{ name: "s", type: "T", attrs: ["a"] }, { name: "s", type: "T", attrs: [] }],
      filters: [{ kind: "select", attr: "zzz" }, { kind: "nope" }],
      views: [
        { kind: "chart", type: "bar", x: "a", y: "b" },
        { kind: "table", source: "ghost", columns: [] },
        { kind: "stats", items: [{ label: "n", agg: "median" }] },
      ],
    });
    expect(parsed.spec).toBeNull();
    expect(parsed.errors).toEqual([
      "title: must be a non-empty string",
      "sources[1].name: 's' is used twice",
      "sources[1].attrs: must list at least one attribute",
      "filters[0].attr: 'zzz' is not among the source's attributes",
      "filters[1].kind: must be one of search, select, range",
      "views[0].y: 'b' is not among the source's attributes",
      "views[1].source: 'ghost' is not a source",
      "views[1].columns: must list at least one column",
      "views[2].items[0].agg: must be one of count, sum, avg, min, max",
    ]);
  });
  it("refuses what is not an object and an empty spec", () => {
    expect(parseSpec("x").errors).toEqual(["spec: must be an object"]);
    expect(parseSpec({}).errors).toEqual([
      "title: must be a non-empty string",
      "sources: must list at least one entity type",
      "views: must list at least one view",
    ]);
  });
  it("resolves a view's source to the first one when it names none", () => {
    const parsed = parseSpec(example);
    expect(parsed.spec && sourceOf(parsed.spec, { source: undefined }).name).toBe("stations");
  });

  it("checks a form's fields and a view's page", () => {
    const bad = { ...example, views: [{ kind: "form", fields: ["ghost"], page: "" }] };
    expect(parseSpec(bad).errors).toEqual(["views[0].page: must be a non-empty string", "views[0].fields[0]: 'ghost' is not among the source's attributes"]);
    expect(parseSpec({ ...example, views: [{ kind: "form", page: "Edit" }] }).errors).toEqual([]);
  });
});
