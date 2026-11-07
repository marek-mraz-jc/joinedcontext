/**
 * The filter row as a query (T-1429; UI-66, EP-07): what each operator asks the endpoint, and what
 * a value cannot do on the way there.
 *
 * The endpoint does the filtering, so every case here is about the string that leaves the browser.
 * A value is data: it may contain a quote, a backslash, a semicolon, a pipe or a bracket, and none
 * of those may turn into another term, another clause or a wildcard.
 */
import { describe, expect, it } from "vitest";
import {
  andQ,
  complete,
  opsForKind,
  queryFromFilters,
  quotePattern,
  term,
  valuesNeeded,
} from "../src/grid/filters";
import type { ColumnFilter, FilterColumn, FilterOp } from "../src/grid/filters";

const NAME: FilterColumn = { key: "name", attr: "name", meta: null, kind: "text" };
const BIKES: FilterColumn = { key: "availableBikeNumber", attr: "availableBikeNumber", kind: "number" };
const SEEN: FilterColumn = {
  key: "temperature:observedAt",
  attr: "temperature",
  meta: "observedAt",
  kind: "date",
};
const DEVICE: FilterColumn = { key: "refDevice", attr: "refDevice", kind: "relationship" };
const ID: FilterColumn = { key: "id", attr: null, kind: "id" };
const COLUMNS = [ID, NAME, BIKES, SEEN, DEVICE];

/** The query of one filter on one column, which is what most cases are about. */
function askedBy(column: FilterColumn, filter: ColumnFilter) {
  return queryFromFilters([column], { [column.key]: filter });
}

describe("the grid's filter row as an NGSI-LD query", () => {
  it("asks what each operator means, in the query language the endpoint answers", () => {
    expect(askedBy(NAME, { op: "contains", value: "Kamppi" }).q).toBe('name~="Kamppi"');
    expect(askedBy(NAME, { op: "equals", value: "Kamppi" }).q).toBe('name=="Kamppi"');
    expect(askedBy(NAME, { op: "notEquals", value: "Kamppi" }).q).toBe('name!="Kamppi"');
    // Existence, not an empty string: an attribute a subset of the entities carries at all.
    expect(askedBy(NAME, { op: "empty", value: "" }).q).toBe("!name");
    expect(askedBy(NAME, { op: "present", value: "" }).q).toBe("name");
    // A number and a timestamp go in bare; everything else is quoted.
    expect(askedBy(BIKES, { op: "gt", value: "5" }).q).toBe("availableBikeNumber>5");
    expect(askedBy(BIKES, { op: "lte", value: "-2.5" }).q).toBe("availableBikeNumber<=-2.5");
    expect(askedBy(BIKES, { op: "between", value: "1", value2: "9" }).q).toBe(
      "availableBikeNumber>=1;availableBikeNumber<=9",
    );
    expect(askedBy(DEVICE, { op: "equals", value: "urn:ngsi-ld:Device:hel:001" }).q).toBe(
      'refDevice=="urn:ngsi-ld:Device:hel:001"',
    );
  });

  it("filters a metadata column on the attribute's own member", () => {
    expect(term("temperature", "observedAt")).toBe("temperature.observedAt");
    expect(askedBy(SEEN, { op: "gte", value: "2026-09-19" }).q).toBe("temperature.observedAt>=2026-09-19");
    expect(askedBy(SEEN, { op: "between", value: "2026-09-01", value2: "2026-09-19T12:00:00Z" }).q).toBe(
      "temperature.observedAt>=2026-09-01;temperature.observedAt<=2026-09-19T12:00:00Z",
    );
  });

  it("joins two filters with the language's own 'and'", () => {
    const asked = queryFromFilters(COLUMNS, {
      availableBikeNumber: { op: "gt", value: "5" },
      "temperature:observedAt": { op: "gte", value: "2026-09-19" },
    });
    expect(asked.q).toBe("availableBikeNumber>5;temperature.observedAt>=2026-09-19");
  });

  it("asks the id column by pattern, and an exact id anchored", () => {
    expect(askedBy(ID, { op: "pattern", value: "helsinki:0" })).toEqual({
      q: undefined,
      idPattern: "helsinki:0",
    });
    expect(askedBy(ID, { op: "equals", value: "urn:ngsi-ld:Bike:hel:1" }).idPattern).toBe(
      "^urn:ngsi-ld:Bike:hel:1$",
    );
    // The id never becomes a `q` term: it is a parameter of its own.
    expect(askedBy(ID, { op: "pattern", value: "x" }).q).toBeUndefined();
  });

  it("lets no value break out of its own term", () => {
    // A quote and a backslash are escaped, so the literal ends where it should.
    expect(askedBy(NAME, { op: "equals", value: 'say "hi"' }).q).toBe('name=="say \\"hi\\""');
    expect(askedBy(NAME, { op: "equals", value: "back\\slash" }).q).toBe('name=="back\\\\slash"');
    // A semicolon and a pipe are inside the quotes, so they are part of the value, not another
    // term (`;`) and not another clause (`|`).
    const both = askedBy(NAME, { op: "equals", value: "a;b|c" }).q ?? "";
    expect(both).toBe('name=="a;b|c"');
    // One term: everything after the operator is one quoted literal, so the `;` and the `|` are
    // characters of the value and not the language's "and" and "or".
    expect(both).toMatch(/^name=="[^"]*"$/);
    // A pattern escapes what a regular expression would read as syntax, so a typed `.` is a dot
    // and a typed `(` is a bracket instead of a 400.
    expect(askedBy(NAME, { op: "contains", value: "a.b*c(d)" }).q).toBe('name~="a\\.b\\*c\\(d\\)"');
    expect(quotePattern("^$.|?*+()[]{}/\\")).toBe(
      "\\^\\$\\.\\|\\?\\*\\+\\(\\)\\[\\]\\{\\}\\/\\\\",
    );
    // And an id pattern is escaped the same way, because it is a regular expression too.
    expect(askedBy(ID, { op: "pattern", value: "hel.*" }).idPattern).toBe("hel\\.\\*");
  });

  it("quotes a text column's value even when it reads as a number (T-2436)", () => {
    // `refPeriod` holds "2023" and "2025Q2"; a period asked for as the number 2023 matches no
    // string, and the grid would show an empty page as if the endpoint had answered it.
    const period: FilterColumn = { key: "refPeriod", attr: "refPeriod", meta: null, kind: "text" };
    expect(askedBy(period, { op: "equals", value: "2023" }).q).toBe('refPeriod=="2023"');
    expect(askedBy(period, { op: "notEquals", value: "2023" }).q).toBe('refPeriod!="2023"');
    // A number column still asks for a number, and a date column for a timestamp.
    expect(askedBy(BIKES, { op: "gte", value: "5" }).q).toBe("availableBikeNumber>=5");
    expect(askedBy(BIKES, { op: "between", value: "5", value2: "9" }).q).toBe(
      "availableBikeNumber>=5;availableBikeNumber<=9",
    );
  });

  it("keeps a Unicode value as it was typed", () => {
    expect(askedBy(NAME, { op: "equals", value: "Töölö" }).q).toBe('name=="Töölö"');
    expect(askedBy(NAME, { op: "contains", value: "Kalasatama 東" }).q).toBe('name~="Kalasatama 東"');
  });

  it("asks for nothing while a filter is incomplete, and nothing once it is cleared", () => {
    expect(valuesNeeded("empty")).toBe(0);
    expect(valuesNeeded("contains")).toBe(1);
    expect(valuesNeeded("between")).toBe(2);
    expect(complete({ op: "contains", value: "  " })).toBe(false);
    expect(complete({ op: "between", value: "1" })).toBe(false);
    expect(complete({ op: "between", value: "1", value2: "2" })).toBe(true);
    expect(complete({ op: "empty", value: "" })).toBe(true);
    // A row being typed leaves the query alone, and a row removed leaves nothing behind.
    expect(queryFromFilters(COLUMNS, { name: { op: "contains", value: "" } })).toEqual({
      q: undefined,
      idPattern: undefined,
    });
    expect(queryFromFilters(COLUMNS, {})).toEqual({ q: undefined, idPattern: undefined });
  });

  it("offers only the operators a column's content can answer", () => {
    expect(opsForKind("text")).toContain("contains");
    expect(opsForKind("number")).toContain("between");
    expect(opsForKind("number")).not.toContain("contains");
    expect(opsForKind("relationship")).toEqual(["equals", "notEquals", "empty", "present"]);
    expect(opsForKind("id")).toEqual(["pattern", "startsWith", "equals"]);
    // A geometry is asked for with a geo query, which is the map's, not a typed value's.
    expect(opsForKind("geo")).toEqual([]);
    expect(opsForKind("none")).toEqual([]);
  });

  it("keeps a preset and the row both true", () => {
    expect(andQ("scope==\"north\"", "availableBikeNumber>5")).toBe(
      'scope=="north";availableBikeNumber>5',
    );
    expect(andQ(undefined, "a>1")).toBe("a>1");
    expect(andQ("a>1", undefined)).toBe("a>1");
    expect(andQ(" ", undefined)).toBeUndefined();
  });
});

describe("a column whose type the caller does not know", () => {
  const untyped = (value: string, op: FilterOp = "gt") =>
    queryFromFilters([{ key: "a", attr: "a", kind: "untyped" }], { a: { op, value } }).q;

  it("judges the value by its shape, so a number stays a number", () => {
    // `pm10>"50"` compares a number against a string and matches nothing: the endpoint page
    // has attribute names and no model, and claiming `text` there broke every numeric
    // condition an endpoint publishes (T-2448).
    expect(untyped("50")).toBe("a>50");
    expect(untyped("50.5")).toBe("a>50.5");
    expect(untyped("2026-09-20T00:00:00Z", "gte")).toBe("a>=2026-09-20T00:00:00Z");
  });

  it("still quotes what is not a number, a boolean or a timestamp", () => {
    expect(untyped("brezno", "equals")).toBe('a=="brezno"');
  });

  it("does not take the shape rule away from a column that names its type", () => {
    const q = queryFromFilters(
      [{ key: "a", attr: "a", kind: "text" }],
      { a: { op: "equals", value: "2023" } },
    ).q;
    expect(q, "a period is a string, and a text column says so").toBe('a=="2023"');
  });
});

describe("pageNumbers: a DataTables pager (T-5196)", () => {
  it("shows the first, the last and the pages beside the current one, with gaps", async () => {
    const { pageNumbers } = await import("../src/grid/EntityGrid");
    expect(pageNumbers(1, 1)).toEqual([1]);
    expect(pageNumbers(1, 12)).toEqual([1, 2, null, 12]);
    expect(pageNumbers(6, 12)).toEqual([1, null, 5, 6, 7, null, 12]);
    expect(pageNumbers(12, 12)).toEqual([1, null, 11, 12]);
    expect(pageNumbers(3, 4)).toEqual([1, 2, 3, 4]);
  });
});
