// covers (T-2137, the module gate in gate_modules.test.ts): src/components/dashboards/vegaLiteSource.ts
// and src/components/dashboards/VegaLiteWidget.tsx. A `vega-lite` dashboard widget (T-6786): the
// source of its Endpoint it reads, the rows it fills into the spec, and its loading, empty,
// failed and drawn states.
import type { ReactNode } from "react";
import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { SOURCE_MAX_ROWS, csvRecords, readSource, seconds, sourceUrl, temporalRows } from "../src/components/dashboards/vegaLiteSource";

vi.mock("../src/components/panels/VegaChart", async (actual) => ({
  ...(await actual<typeof import("../src/components/panels/VegaChart")>()),
  // jsdom has no canvas: the stub shows what the chart was given.
  VegaChart: ({ spec, label }: { spec: Record<string, unknown>; label: string }) => (
    <div role="img" aria-label={label} data-rows={JSON.stringify((spec.datasets as { source: unknown }).source)} />
  ),
}));

const { VegaLiteWidget, withRows } = await import("../src/components/dashboards/VegaLiteWidget");

const W = en.dashboards.widget;
const NOW = Date.parse("2026-11-11T12:00:00Z");
const SPEC = {
  $schema: "https://vega.github.io/schema/vega-lite/v5.json",
  title: "PM10 by district",
  data: { name: "source" },
  mark: "bar",
  encoding: { x: { field: "district", type: "nominal" }, y: { field: "pm10", type: "quantitative" } },
};

function answering(body: (url: URL) => Response) {
  const asked: URL[] = [];
  const fetcher = vi.fn(async (input: RequestInfo | URL) => {
    const url = new URL(input instanceof Request ? input.url : String(input), window.location.origin);
    asked.push(url);
    return body(url);
  });
  vi.stubGlobal("fetch", fetcher);
  return { asked, fetcher };
}

const json = (value: unknown) => new Response(JSON.stringify(value), { headers: { "Content-Type": "application/json" } });

const show = (node: ReactNode) =>
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>{node}</I18nextProvider>
    </QueryClientProvider>,
  );

describe("a vega-lite widget's look (T-6886)", () => {
  it("draws in the theme file's config for the scheme, under the author's own config", async () => {
    const file = (await import("../../../shared/jc-chart/src/theme.json")).default;
    const { chartTheme } = await import("../src/components/panels/VegaChart");
    for (const mode of ["light", "dark"] as const) {
      const config = withRows({ mark: "bar", config: { axis: { labelColor: "#000000" } } }, [], chartTheme(mode)).config as Record<string, unknown>;
      expect(config.range).toEqual(file[mode].range);
      expect(config.font).toBe(file[mode].font);
      expect(config.background).toBeNull();
      // The author's config wins where it speaks.
      expect(config.axis).toEqual({ labelColor: "#000000" });
    }
  });
});

describe("the source a vega-lite widget reads", () => {
  it("reads each kind of source under the Endpoint's own path, never elsewhere", () => {
    expect(sourceUrl("air-ops", { source: "csv", type: "AirQualityObserved" }, NOW)).toBe("/api/endpoint/air-ops/file.csv?type=AirQualityObserved");
    expect(sourceUrl("air-ops", { source: "ogc", collection: "AirQualityObserved", filter: "pm10>40" }, NOW)).toBe(
      "/api/endpoint/air-ops/ogc/features/collections/AirQualityObserved/items?limit=1000&filter=pm10%3E40",
    );
    expect(sourceUrl("air-ops", { source: "odata", entitySet: "AirQualityObserved", $select: "pm10" }, NOW)).toBe(
      "/api/endpoint/air-ops/odata/v4/AirQualityObserved?%24select=pm10",
    );
    const temporal = new URL(sourceUrl("air-ops", { source: "temporal", type: "AirQualityObserved", attrs: ["pm10"], last: "P1D", aggrMethod: "avg", aggrPeriod: "PT1H" }, NOW) ?? "", "https://x");
    expect(temporal.searchParams.get("timeAt")).toBe("2026-11-10T12:00:00Z");
    expect(temporal.searchParams.get("format")).toBe("aggregatedValues");
    // A name that could leave the path, or a window without a length, reads nothing.
    expect(sourceUrl("air-ops", { source: "ogc", collection: "../../admin" }, NOW)).toBeNull();
    expect(sourceUrl("air-ops", { source: "temporal", type: "A", attrs: ["pm10"], last: "P1M" }, NOW)).toBeNull();
    expect(seconds("P1DT2H")).toBe(93_600);
    expect(seconds("PT")).toBeNull();
  });

  it("flattens a temporal answer into one row per reading", () => {
    const body = [{ id: "urn:a", pm10: { values: [[12, "2026-11-11T10:00:00Z"]] } }, { id: "urn:b", pm10: [{ datasetId: "d", values: [[3, "2026-11-11T11:00:00Z"]] }] }];
    expect(temporalRows(body, ["pm10"], false)).toEqual([
      { id: "urn:a", attribute: "pm10", value: 12, observedAt: "2026-11-11T10:00:00Z" },
      { id: "urn:b", attribute: "pm10", value: 3, observedAt: "2026-11-11T11:00:00Z" },
    ]);
    expect(temporalRows([{ id: "urn:a", pm10: { avg: [[5, "2026-11-11T10:00:00Z"]] } }], ["pm10"], true)).toEqual([
      { id: "urn:a", attribute: "pm10", method: "avg", value: 5, observedAt: "2026-11-11T10:00:00Z" },
    ]);
  });

  it("follows next pages on the Endpoint, refuses a next link that leaves it, and says when it was cut", async () => {
    const page = (n: number, next?: string) => json({ value: Array.from({ length: 1000 }, (_, i) => ({ n: n * 1000 + i })), "@odata.nextLink": next });
    const { asked } = answering((url) => {
      const n = Number(url.searchParams.get("p") ?? 0);
      return page(n, n < 2 ? `/api/endpoint/air-ops/odata/v4/Air?p=${n + 1}` : "https://elsewhere.example/odata/v4/Air?p=3");
    });
    await expect(readSource("air-ops", { source: "odata", entitySet: "Air" })).rejects.toThrow("not on the Endpoint");
    expect(asked.map((u) => u.searchParams.get("p"))).toEqual([null, "1", "2"]);
    expect(asked.every((u) => u.origin === window.location.origin)).toBe(true);

    answering(() => page(0, "/api/endpoint/air-ops/odata/v4/Air?again=1"));
    const capped = await readSource("air-ops", { source: "odata", entitySet: "Air" });
    expect(capped.rows).toHaveLength(SOURCE_MAX_ROWS);
    expect(capped.cut).toBe(true);
  });

  it("reads a CSV's records, quoted fields and CRLF included, up to the row cap", () => {
    expect(csvRecords('name,note\r\n"Kamppi, Helsinki","said ""hi"""\r\nTöölö,\r\n')).toEqual([
      { name: "Kamppi, Helsinki", note: 'said "hi"' },
      { name: "Töölö", note: "" },
    ]);
    expect(csvRecords("a\nmulti\n\n")).toEqual([{ a: "multi" }]);
    expect(csvRecords("")).toEqual([]);
    expect(csvRecords("n\n1\n2\n3\n", 2)).toEqual([{ n: "1" }, { n: "2" }]);
  });

  it("fills the rows into the dataset `source` and keeps the spec's own data reference", () => {
    const drawn = withRows(SPEC, [{ district: "Staré Mesto", pm10: 12 }]);
    expect(drawn.data).toEqual({ name: "source" });
    expect(drawn.datasets).toEqual({ source: [{ district: "Staré Mesto", pm10: 12 }] });
    expect(drawn.width).toBe("container");
    expect(drawn.height).toBe(240);
    expect(withRows({ ...SPEC, height: 120 }, []).height).toBe(120);
    expect(drawn.title).toBeUndefined();
    expect((drawn.config as { font: string }).font).toBeTruthy();
    expect((withRows({ ...SPEC, config: { font: "Serif" } }, []).config as { font: string }).font).toBe("Serif");
  });
});

describe("the vega-lite widget", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("shows a skeleton while it reads, then the chart under the spec's title", async () => {
    answering(() => json({ type: "FeatureCollection", features: [{ id: "a", properties: { district: "Ružinov", pm10: 31 } }], links: [] }));
    show(<VegaLiteWidget slug="air-ops" source={{ source: "ogc", collection: "AirQualityObserved" }} spec={SPEC} fallbackTitle="fallback" />);
    expect(screen.getByRole("status", { name: en.app.loading })).toBeInTheDocument();
    const chart = await screen.findByRole("img", { name: "PM10 by district" });
    expect(JSON.parse(chart.getAttribute("data-rows") ?? "[]")).toEqual([{ id: "a", district: "Ružinov", pm10: 31 }]);
    expect(screen.getByText("PM10 by district").tagName).toBe("FIGCAPTION");
  });

  it("parses a CSV source into rows without evaluating code, which the Portal's CSP refuses", async () => {
    // T-6791: d3-dsv built each CSV row with `new Function`; under `script-src` without
    // 'unsafe-eval' every CSV chart failed. A Function that throws stands in for the CSP here.
    vi.stubGlobal(
      "Function",
      class {
        constructor() {
          throw new EvalError("Refused to evaluate a string as JavaScript (CSP)");
        }
      },
    );
    answering(() => new Response("district,pm10\nRužinov,31\nPetržalka,18\n", { headers: { "Content-Type": "text/csv" } }));
    show(<VegaLiteWidget slug="air-ops" source={{ source: "csv", type: "AirQualityObserved" }} spec={SPEC} fallbackTitle="fallback" />);
    const chart = await screen.findByRole("img", { name: "PM10 by district" });
    expect(JSON.parse(chart.getAttribute("data-rows") ?? "[]")).toEqual([
      { district: "Ružinov", pm10: 31 },
      { district: "Petržalka", pm10: 18 },
    ]);
  });

  it("says what to do when the source holds no rows, and why a read failed", async () => {
    answering(() => json({ value: [] }));
    const { unmount } = show(<VegaLiteWidget slug="air-ops" source={{ source: "odata", entitySet: "Air" }} spec={{ ...SPEC, title: undefined }} fallbackTitle="Chart of Air" />);
    expect(await screen.findByText(W.vegaNoRows)).toBeInTheDocument();
    expect(screen.getByText(W.vegaNoRowsTitle)).toBeInTheDocument();
    expect(screen.getByText("Chart of Air")).toBeInTheDocument();
    unmount();

    answering(() => new Response("", { status: 403, statusText: "Forbidden" }));
    show(<VegaLiteWidget slug="air-ops" source={{ source: "odata", entitySet: "Air" }} spec={SPEC} fallbackTitle="x" />);
    expect(await screen.findByRole("button", { name: /retry/i })).toBeInTheDocument();
  });
});
