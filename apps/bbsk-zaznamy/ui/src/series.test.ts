/**
 * The records as charts (T-2966): named with the office's labels, months and quarters on the time
 * axis, territories compared at the latest period, the residents by age as columns.
 */
import { describe, expect, it } from "vitest";
import { CODES } from "./labels";
import { chartsOf, datasetName, named } from "./series";

let n = 0;
function cell(
  dataSet: string,
  indicator: string,
  refPeriod: string,
  value: unknown,
  extra: { refArea?: string; dimensionKey?: string; unitText?: string } = {},
): Record<string, unknown> {
  n += 1;
  const entity: Record<string, unknown> = {
    id: `urn:ngsi-ld:KeyPerformanceIndicator:banskabystrica.sk:banskabystrica-mesto:${n}`,
    type: "KeyPerformanceIndicator",
    dataSet: { type: "Property", value: dataSet },
    indicator: { type: "Property", value: indicator },
    refArea: { type: "Property", value: extra.refArea ?? "SK0321508438" },
    refPeriod: { type: "Property", value: refPeriod },
    kpiValue: { type: "Property", value, unitCode: "C62" },
    unitText: { type: "Property", value: extra.unitText ?? "počet" },
  };
  if (extra.dimensionKey) entity.dimensionKey = { type: "Property", value: extra.dimensionKey };
  return entity;
}

const latest = (period: string) => `obdobie ${period}`;

describe("chartsOf", () => {
  it("names the indicator and the territory, and draws the periods in order", () => {
    const records = [
      cell("vh5003rr", "U03084", "2023", 4061.2, { unitText: "v tis. m3 fakturovanej vody" }),
      cell("vh5003rr", "U03084", "2021", 3900),
      cell("vh5003rr", "U03084", "2022", 3987.5),
    ];
    const [chart, ...rest] = chartsOf(records, "vh5003rr", "sk", { latest });
    expect(rest).toEqual([]);
    expect(chart).toMatchObject({
      title: "Spotreba pitnej vody - spolu",
      subtitle: "Banská Bystrica",
      unit: "v tis. m3 fakturovanej vody",
      shape: "line",
    });
    expect(chart.points.map((point) => point.at)).toEqual(["2021", "2022", "2023"]);
    expect(chartsOf(records, "vh5003rr", "en", { latest })[0].title).toBe("Consumption of drinking water in total");
  });

  it("puts the month on the time axis and keeps the other codes as separate charts", () => {
    const records = [
      cell("cr3803mr", "U_CR_0005", "2024", 120, { dimensionKey: "2.-VISIT_TOTAL" }),
      cell("cr3803mr", "U_CR_0005", "2024", 100, { dimensionKey: "1.-VISIT_TOTAL" }),
      cell("cr3803mr", "U_CR_0005", "2023", 90, { dimensionKey: "12.-VISIT_TOTAL" }),
      cell("cr3803mr", "U_CR_0005", "2024", 30, { dimensionKey: "1.-VISIT_FOR" }),
    ];
    const charts = chartsOf(records, "cr3803mr", "sk", { latest });
    expect(charts.map((chart) => chart.title)).toEqual([
      "Počet návštevníkov spolu · Návštevníci spolu",
      "Počet návštevníkov spolu · Zahraniční návštevníci",
    ]);
    expect(charts[0].points.map((point) => point.at)).toEqual(["2023-12", "2024-01", "2024-02"]);
    expect(charts.every((chart) => chart.shape === "line")).toBe(true);
  });

  it("folds a quarter into the period", () => {
    const records = [
      cell("cr3809qr", "U_CR_0002", "2024", 5, { dimensionKey: "3.Q." }),
      cell("cr3809qr", "U_CR_0002", "2024", 4, { dimensionKey: "1.Q." }),
    ];
    const [chart] = chartsOf(records, "cr3809qr", "sk", { latest });
    expect(chart.points.map((point) => point.at)).toEqual(["2024Q1", "2024Q3"]);
  });

  it("compares the territories at the latest period, largest first", () => {
    const records = [
      cell("st3004rr", "DOKONC_BYT", "2023", 400, { refArea: "SK0321" }),
      cell("st3004rr", "DOKONC_BYT", "2024", 150, { refArea: "SK0322" }),
      cell("st3004rr", "DOKONC_BYT", "2024", 520, { refArea: "SK0321" }),
    ];
    const [chart] = chartsOf(records, "st3004rr", "sk", { latest });
    // The office's own spelling, which has a non-breaking space in it.
    expect(chart).toMatchObject({ shape: "areas", subtitle: "obdobie 2024", title: CODES.st3004rr.DOKONC_BYT.sk });
    expect(chart.points.map((point) => [point.label, point.value])).toEqual([
      ["Okres Banská Bystrica", 520],
      ["Okres Banská Štiavnica", 150],
    ]);
  });

  it("draws the residents by age as columns of the latest snapshot, in the order of age", () => {
    const records = [
      ...[0, 1, 2, 10].map((age) => cell("mesto-obyvatelia-vek", "POCET", "2026-09-24", 900 + age, { dimensionKey: String(age) })),
      ...[0, 1, 2, 10].map((age) => cell("mesto-obyvatelia-vek", "POCET", "2026-09-25", 800 + age, { dimensionKey: String(age) })),
    ];
    const [chart, ...rest] = chartsOf(records, "mesto-obyvatelia-vek", "sk", {
      latest,
      keyAxis: "podľa veku",
      codeNames: { POCET: "Počet obyvateľov" },
    });
    expect(rest).toEqual([]);
    expect(chart).toMatchObject({
      shape: "keys",
      title: "Počet obyvateľov · podľa veku",
      subtitle: "Banská Bystrica · obdobie 2026-09-25",
    });
    expect(chart.points.map((point) => [point.at, point.value])).toEqual([
      ["0", 800],
      ["1", 801],
      ["2", 802],
      ["10", 810],
    ]);
  });

  it("draws nothing for no records, and leaves out a record without a number", () => {
    expect(chartsOf([], "vh5003rr", "sk", { latest })).toEqual([]);
    const records = [cell("vh5003rr", "U03084", "2023", null), cell("vh5003rr", "U03084", "2022", "12")];
    expect(chartsOf(records, "vh5003rr", "sk", { latest })).toEqual([]);
  });

  it("shows a code the lists do not hold as the code, never as nothing", () => {
    const [chart] = chartsOf([cell("xx0000rr", "UNKNOWN", "2024", 1, { refArea: "SK9999" })], "xx0000rr", "sk", { latest });
    expect(chart).toMatchObject({ title: "UNKNOWN", subtitle: "SK9999" });
  });
});

describe("named", () => {
  it("takes the reader's language, the other where only that exists, else the code", () => {
    expect(named({ sk: "Spolu", en: "Total" }, "en", "SPOLU")).toBe("Total");
    expect(named({ sk: "Spolu" }, "en", "SPOLU")).toBe("Spolu");
    expect(named(undefined, "sk", "SPOLU")).toBe("SPOLU");
  });

  it("names a cube by the office's title, or by the App's own name for its own table", () => {
    expect(datasetName("vh5003rr", "en")).not.toBe("vh5003rr");
    expect(datasetName("mesto-obyvatelia-vek", "sk", { "mesto-obyvatelia-vek": "Obyvatelia mesta podľa veku" })).toBe(
      "Obyvatelia mesta podľa veku",
    );
  });
});
