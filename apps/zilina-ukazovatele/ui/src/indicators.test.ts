import { describe, expect, it } from "vitest";
import { toRichRow } from "@joinedcontext/sdk";
import { toIndicator, windowOf } from "./indicators";
import { INDICATORS } from "./fixtures/kpi";

const row = (name: string) => structuredClone(INDICATORS.find((e) => e.name.value === name)) as (typeof INDICATORS)[number];

describe("toIndicator", () => {
  it("reads the value, its unit, its window, its formula and the run", () => {
    expect(toIndicator(toRichRow(row("obyvatelstvo-stav-mesto")))).toMatchObject({
      key: "obyvatelstvo-stav",
      value: 79617,
      unitAsDefined: true,
      period: { start: "2026-04-01T00:00:00Z", end: "2026-06-30T23:59:59Z" },
      updatedAt: "2026-10-06T19:00:00Z",
    });
    expect(toIndicator(toRichRow(row("celkovy-prirastok-mesto")))?.value).toBe(-384);
  });

  it("keeps not measured missing, never a zero", () => {
    const entity = row("uchadzaci-mesto");
    (entity as { currentValue: unknown }).currentValue = { type: "Property", value: "not measured" };
    expect(toIndicator(toRichRow(entity))).toMatchObject({ value: null, unitCode: null });
  });

  it("refuses a number it could not place", () => {
    const elsewhere = row("uchadzaci-mesto");
    elsewhere.id = elsewhere.id.replace(":zilina-kpi:", ":bbsk-kpi:");
    expect(toIndicator(toRichRow(elsewhere))).toBeNull();
    const renamed = row("uchadzaci-mesto");
    renamed.name.value = "obyvatelstvo-stav-mesto";
    expect(toIndicator(toRichRow(renamed))).toBeNull();
    const unknown = row("uchadzaci-mesto");
    unknown.id = unknown.id.replace("uchadzaci-mesto", "nieco-mesto");
    unknown.name.value = "nieco-mesto";
    expect(toIndicator(toRichRow(unknown))).toBeNull();
  });

  it("says when a value is not in the unit the indicator is defined in", () => {
    const entity = row("priemerny-vek-mesto");
    entity.currentValue.unitCode = "C62";
    expect(toIndicator(toRichRow(entity))).toMatchObject({ unitAsDefined: false, unitCode: "C62" });
  });
});

describe("what a pipeline writes otherwise", () => {
  const P = (value: unknown) => ({ type: "Property", value });
  const base = () => row("uchadzaci-mesto") as unknown as Record<string, unknown>;

  it("reads a name written as a relationship, and a run time that is plain text", () => {
    const entity = base();
    entity.name = { type: "Relationship", object: "uchadzaci-mesto" };
    entity.updatedAt = P("2026-10-06T19:00:00Z");
    expect(toIndicator(toRichRow(entity as never))).toMatchObject({ key: "uchadzaci", updatedAt: "2026-10-06T19:00:00Z" });
  });

  it("leaves out a window, a formula and a run time it cannot read", () => {
    const entity = base();
    entity.calculationPeriod = P({ start: "2026-04-01" });
    delete entity.calculationFormula;
    entity.updatedAt = P({ "@type": "DateTime", "@value": 42 });
    expect(toIndicator(toRichRow(entity as never))).toMatchObject({ period: null, formula: "", updatedAt: null });
    entity.updatedAt = P(null);
    expect(toIndicator(toRichRow(entity as never))?.updatedAt).toBeNull();
    entity.currentValue = { type: "Property", value: Number.POSITIVE_INFINITY, unitCode: "C62" };
    expect(toIndicator(toRichRow(entity as never))).toMatchObject({ value: null, unitCode: null, unitAsDefined: false });
    entity.currentValue = P(12);
    expect(toIndicator(toRichRow(entity as never))).toMatchObject({ value: 12, unitCode: null, unitAsDefined: false });
  });

  it("refuses another type, an id of another shape, and a name of no territory or none at all", () => {
    const of = (change: (entity: Record<string, unknown>) => void) => {
      const entity = base();
      change(entity);
      return toIndicator(toRichRow(entity as never));
    };
    expect(of((e) => (e.type = "Indicator"))).toBeNull();
    expect(of((e) => (e.id = "urn:ngsi-ld:KeyPerformanceIndicator:uchadzaci-mesto"))).toBeNull();
    expect(of((e) => (e.id = String(e.id).replace(/^urn/, "urx")))).toBeNull();
    expect(of((e) => (e.id = String(e.id).replace("ngsi-ld", "ngsi")))).toBeNull();
    expect(of((e) => delete e.name)).toBeNull();
    expect(of((e) => {
      e.id = String(e.id).replace("uchadzaci-mesto", "uchadzaci-kraj");
      e.name = P("uchadzaci-kraj");
    })).toBeNull();
  });
});

describe("windowOf", () => {
  it("names a quarter, a year, and anything else by its days", () => {
    expect(windowOf({ start: "2026-04-01T00:00:00Z", end: "2026-06-30T23:59:59Z" })).toEqual({ kind: "quarter", label: "Q2 2026" });
    expect(windowOf({ start: "2025-01-01T00:00:00Z", end: "2025-12-31T23:59:59Z" })).toEqual({ kind: "year", label: "2025" });
    expect(windowOf({ start: "0001-01-01T00:00:00Z", end: "2026-10-06T19:00:00Z" }).kind).toBe("days");
    // A quarter's start with another quarter's end is two days, not a quarter.
    expect(windowOf({ start: "2026-04-01T00:00:00Z", end: "2026-09-30T23:59:59Z" })).toEqual({ kind: "days", label: "2026-04-01 – 2026-09-30" });
  });
});
