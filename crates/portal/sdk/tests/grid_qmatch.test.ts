import { describe, expect, it } from "vitest";
import { boundedPattern, matchesQ, parseQ } from "../src/grid/qmatch";
import { toRichRow } from "../src/grid/model";

const row = toRichRow({
  id: "urn:ngsi-ld:BikeHireDockingStation:hel:001",
  type: "BikeHireDockingStation",
  availableBikeNumber: { type: "Property", value: 0 },
  status: { type: "Property", value: "outOfService" },
  name: { type: "Property", value: "Kamppi <east>" },
  dateLastReported: { type: "Property", value: "2026-10-06T10:00:00Z" },
  refStreet: { type: "Relationship", object: "urn:ngsi-ld:Street:hel:mannerheimintie" },
  address: { type: "Property", value: { streetAddress: "Kampinkuja 1" }, verified: { type: "Property", value: true } },
});

describe("matchesQ", () => {
  it("compares numbers, strings, booleans and dates", () => {
    expect(matchesQ(row, "availableBikeNumber==0")).toBe(true);
    expect(matchesQ(row, "availableBikeNumber>0")).toBe(false);
    expect(matchesQ(row, "availableBikeNumber<=0")).toBe(true);
    expect(matchesQ(row, 'status=="outOfService"')).toBe(true);
    expect(matchesQ(row, 'status!="outOfService"')).toBe(false);
    expect(matchesQ(row, 'dateLastReported<"2026-10-07"')).toBe(true);
    expect(matchesQ(row, "address.verified==true")).toBe(true);
  });

  it("takes ranges, lists, patterns, relationships and presence", () => {
    expect(matchesQ(row, "availableBikeNumber==0..2")).toBe(true);
    expect(matchesQ(row, 'status=="working","outOfService"')).toBe(true);
    expect(matchesQ(row, "status==working,outOfService")).toBe(true);
    expect(matchesQ(row, 'name~="^Kamp"')).toBe(true);
    expect(matchesQ(row, 'name!~="^Kamp"')).toBe(false);
    expect(matchesQ(row, 'refStreet=="urn:ngsi-ld:Street:hel:mannerheimintie"')).toBe(true);
    expect(matchesQ(row, "refStreet")).toBe(true);
    expect(matchesQ(row, "temperature")).toBe(false);
    // An attribute the row lacks is unequal to anything, and less than nothing.
    expect(matchesQ(row, "temperature!=3")).toBe(true);
    expect(matchesQ(row, "temperature<3")).toBe(false);
  });

  it("joins terms with ; (and) and | (or, looser), and keeps quoted separators and operators", () => {
    expect(matchesQ(row, 'availableBikeNumber==0;status=="working"')).toBe(false);
    expect(matchesQ(row, 'availableBikeNumber==5|status=="outOfService"')).toBe(true);
    expect(matchesQ(row, 'name=="Kamppi <east>"')).toBe(true);
    expect(matchesQ(toRichRow({ id: "x", type: "T", note: { type: "Property", value: "a;b|c" } }), 'note=="a;b|c"')).toBe(true);
  });

  it("evaluates nothing it does not understand, rather than guessing", () => {
    for (const q of ["", "(a==1|b==2);c==3", "==1", "a==", "a b==1", "a=1"]) {
      expect(matchesQ(row, q), q).toBeNull();
    }
    expect(parseQ("a==1")).toEqual([[{ path: ["a"], op: "==", value: "1" }]]);
    // A pattern the browser cannot compile marks nothing.
    expect(matchesQ(row, 'name~="("')).toBe(false);
    // A pattern that backtracks without end marks nothing, at once, instead of holding the page.
    const slow = toRichRow({ id: "urn:ngsi-ld:T:1", type: "T", name: { type: "Property", value: `${"a".repeat(25)}!` } }, "en");
    const started = performance.now();
    expect(matchesQ(slow, 'name~="(a+)+$"')).toBe(false);
    expect(performance.now() - started).toBeLessThan(50);
    expect(() => boundedPattern("x".repeat(257))).toThrow();
    // An optional group or a bounded count is no such pattern.
    expect(boundedPattern("^(\\d{2})?[a-z]+$").test("12ab")).toBe(true);
  });

  it("refuses a quantified group whose quantifier sits in a group nested inside it", () => {
    for (const pattern of ["((a+))+$", "(?:(a+))+$", "(a|(b+))*$", "((?<x>a*)){2,}", "((a+)?)+", "([)]|(a+))+"]) {
      expect(() => boundedPattern(pattern), pattern).toThrow();
    }
    // Quantifiers outside the quantified group, escaped or inside a class are no nesting.
    for (const pattern of ["(?:ab)+c*", "(a)+(b+)", "(\\+)+", "([+*]x)+", "(?<name>ab)?"]) {
      expect(() => boundedPattern(pattern), pattern).not.toThrow();
    }
  });
});
