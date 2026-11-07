// covers (T-2137, the module gate in gate_modules.test.ts): src/components/dashboards/typeCharts.ts
// and src/components/dashboards/TypeChart.tsx. Charts over an entity type (UI-93, T-3257): what
// they count, the chart an attribute suggests, and the unit and the time zone on their axes.
import type { ReactNode } from "react";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { TemporalChart } from "../src/components/dashboards/TemporalChart";
import { TypeChart } from "../src/components/dashboards/TypeChart";
import { BINS, MAX_BARS, barsOf, binsOf, readerTimeZone, suggestChart, unitCodeOf } from "../src/components/dashboards/typeCharts";

const W = en.dashboards.widget;
const AIR = [
  { id: "a1", type: "AirQualityObserved", level: { type: "Property", value: "good" }, pm10: { type: "Property", value: 12, unitCode: "GQ" } },
  { id: "a2", type: "AirQualityObserved", level: { type: "Property", value: "moderate" }, pm10: { type: "Property", value: 48.5, unitCode: "GQ" } },
  { id: "a3", type: "AirQualityObserved", level: { type: "Property", value: "good" }, pm10: { type: "Property", value: 30, unitCode: "GQ" } },
  { id: "a4", type: "AirQualityObserved" },
];

describe("what a chart over a type counts", () => {
  it("counts each value, the most common first, leaving out entities without one", () => {
    expect(barsOf(AIR, "level")).toEqual({ bars: [{ label: "good", count: 2 }, { label: "moderate", count: 1 }], rest: 0 });
  });

  it("counts the values past the twelfth as one more bar", () => {
    const many = Array.from({ length: MAX_BARS + 3 }, (_, i) => ({ id: String(i), kind: `k${String(i).padStart(2, "0")}` }));
    const { bars, rest } = barsOf(many, "kind");
    expect(bars).toHaveLength(MAX_BARS);
    expect(rest).toBe(3);
  });

  it("spreads numbers over ten equal bins from the lowest to the highest, one value in one bin", () => {
    const bins = binsOf(AIR, "pm10");
    expect(bins).toHaveLength(BINS);
    expect(bins[0]).toMatchObject({ from: 12, count: 1 });
    expect(bins[BINS - 1]).toMatchObject({ to: 48.5, count: 1 });
    expect(bins.reduce((sum, bin) => sum + bin.count, 0)).toBe(3);
    expect(binsOf([{ id: "x", n: 5 }, { id: "y", n: 5 }], "n")).toEqual([{ from: 5, to: 5, count: 2 }]);
    expect(binsOf([{ id: "x", n: "high" }], "n")).toEqual([]);
  });

  it("reads the unit the attribute carries", () => {
    expect(unitCodeOf(AIR, "pm10")).toBe("GQ");
    expect(unitCodeOf(AIR, "level")).toBeUndefined();
  });

  it("suggests a history for an observed attribute, a histogram for a number, bars for the rest", () => {
    expect(suggestChart(undefined, [{ id: "k", value: { type: "Property", value: 3, observedAt: "2026-10-07T08:00:00Z" } }], "value")).toBe("temporal-chart");
    expect(suggestChart({ range: "float" }, [], "pm10")).toBe("histogram");
    expect(suggestChart({ values: ["good", "bad"] }, [], "level")).toBe("bar-chart");
    expect(suggestChart(undefined, AIR, "pm10")).toBe("histogram");
    expect(suggestChart(undefined, AIR, "level")).toBe("bar-chart");
  });
});

function answering(body: () => Response) {
  const asked: URL[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      asked.push(new URL(input instanceof Request ? input.url : String(input), window.location.origin));
      return body();
    }),
  );
  return asked;
}

const show = (node: ReactNode) =>
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>{node}</I18nextProvider>
    </QueryClientProvider>,
  );

describe("the chart widgets", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("reads the type with the widget's filter and says the unit on the value axis", async () => {
    const asked = answering(() => new Response(JSON.stringify(AIR), { headers: { "Content-Type": "application/json" } }));
    show(<TypeChart slug="air-ops" entityType="AirQualityObserved" property="pm10" q="pm10>0" kind="histogram" title="pm10 spread" />);
    expect(await screen.findByText("Number of entities in each range of pm10, in microgram per cubic metre (GQ).")).toBeInTheDocument();
    expect(asked[0].searchParams.get("q")).toBe("pm10>0");
    expect(asked[0].searchParams.get("attrs")).toBe("pm10");
    expect(asked[0].searchParams.get("options")).toBeNull();
  });

  it("draws bars with their numbers, and the same numbers as a list for a screen reader", async () => {
    answering(() => new Response(JSON.stringify(AIR), { headers: { "Content-Type": "application/json" } }));
    show(<TypeChart slug="air-ops" entityType="AirQualityObserved" property="level" kind="bar-chart" title="levels" />);
    expect(await screen.findByRole("img", { name: "levels" })).toBeInTheDocument();
    expect(screen.getByRole("list", { name: "level" })).toHaveTextContent("good: 2");
    expect(screen.getByText(W.barAxis.replace("{property}", "level"))).toBeInTheDocument();
  });

  it("says why a read failed and reads again on Retry", async () => {
    let calls = 0;
    answering(() => {
      calls += 1;
      return calls === 1
        ? new Response("", { status: 503, statusText: "Service Unavailable" })
        : new Response("[]", { headers: { "Content-Type": "application/json" } });
    });
    show(<TypeChart slug="air-ops" entityType="AirQualityObserved" property="level" kind="bar-chart" title="levels" />);
    await userEvent.click(await screen.findByRole("button", { name: en.app.error.retry }));
    expect(await screen.findByText(W.noValues.replace("{property}", "level"))).toBeInTheDocument();
  });

  it("states the reader's time zone and the unit under a history", async () => {
    answering(
      () =>
        new Response(
          JSON.stringify({
            id: "k1",
            kpiValue: [
              { type: "Property", value: 3, observedAt: "2026-10-06T08:00:00Z", unitCode: "P1" },
              { type: "Property", value: 5, observedAt: "2026-10-07T08:00:00Z", unitCode: "P1" },
            ],
          }),
          { headers: { "Content-Type": "application/json" } },
        ),
    );
    show(<TemporalChart slug="kpi" entityId="k1" property="kpiValue" title="kpi over time" />);
    expect(await screen.findByText(`Time in ${readerTimeZone()}; values in percent (P1).`)).toBeInTheDocument();
  });
});
