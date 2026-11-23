import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { csvRecords, readSource } from "../src/components/dashboards/vegaLiteSource";

vi.mock("../src/components/panels/VegaChart", async (actual) => ({
  ...(await actual<typeof import("../src/components/panels/VegaChart")>()),
  // jsdom has no canvas: the stub shows what the chart was given.
  VegaChart: ({ spec, label }: { spec: Record<string, unknown>; label: string }) => (
    <div role="img" aria-label={label} data-rows={JSON.stringify((spec.datasets as { source: unknown }).source)} />
  ),
}));

const { VegaLiteWidget } = await import("../src/components/dashboards/VegaLiteWidget");

const SPEC = {
  title: "PM10 by district",
  data: { name: "source" },
  mark: "bar",
  encoding: { x: { field: "district", type: "nominal" }, y: { field: "pm10", type: "quantitative" } },
};
const json = (value: unknown) => new Response(JSON.stringify(value), { headers: { "Content-Type": "application/json" } });

describe("the vega-lite widget", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("asks again when Retry is pressed after a failed read, and draws the rows it then gets", async () => {
    let healthy = false;
    const fetcher = vi.fn(() => Promise.resolve(healthy ? json({ value: [{ district: "Ružinov", pm10: 31 }] }) : new Response("", { status: 503 })));
    vi.stubGlobal("fetch", fetcher);
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <VegaLiteWidget slug="air-ops" source={{ source: "odata", entitySet: "Air" }} spec={SPEC} fallbackTitle="x" />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    const retry = await screen.findByRole("button", { name: /retry/i });
    expect(fetcher).toHaveBeenCalledTimes(1);
    healthy = true;
    await userEvent.click(retry);
    const chart = await screen.findByRole("img", { name: "PM10 by district" });
    expect(fetcher).toHaveBeenCalledTimes(2);
    expect(JSON.parse(chart.getAttribute("data-rows") ?? "[]")).toEqual([{ district: "Ružinov", pm10: 31 }]);
  });
});

describe("the rows of a source", () => {
  const NOW = Date.parse("2026-11-11T12:00:00Z");

  it("reads a CSV whose last line has no line break", () => {
    expect(csvRecords("district,pm10\nRužinov,31\nPetržalka,18")).toEqual([
      { district: "Ružinov", pm10: "31" },
      { district: "Petržalka", pm10: "18" },
    ]);
  });

  it("flattens a temporal answer to one row per instance and says when more than the cap was there", async () => {
    const fetcher = vi.fn(() =>
      Promise.resolve(json([{ id: "urn:ngsi-ld:AirQualityObserved:1", pm10: { values: [[31, "2026-11-11T10:00:00Z"], [28, "2026-11-11T11:00:00Z"]] } }])),
    );
    const got = await readSource("air-ops", { source: "temporal", type: "AirQualityObserved", attrs: ["pm10"], last: "P1D" }, fetcher as unknown as typeof fetch, NOW);
    expect(got).toEqual({
      rows: [
        { id: "urn:ngsi-ld:AirQualityObserved:1", attribute: "pm10", value: 31, observedAt: "2026-11-11T10:00:00Z" },
        { id: "urn:ngsi-ld:AirQualityObserved:1", attribute: "pm10", value: 28, observedAt: "2026-11-11T11:00:00Z" },
      ],
      cut: false,
    });
    const [url, init] = fetcher.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toContain("/api/endpoint/air-ops/ngsi-ld/v1/temporal/entities?");
    expect(init.headers).toEqual({ Accept: "application/json" });
  });

  it("follows an OGC next link on the Endpoint, and stops the chart on one that leaves it", async () => {
    const page = (next?: string) => json({ features: [{ id: "a", properties: { pm10: 1 } }], links: next ? [{ rel: "self", href: "/x" }, { rel: "next", href: next }] : [] });
    const fetcher = vi
      .fn()
      .mockResolvedValueOnce(page("/api/endpoint/air-ops/ogc/features/collections/Air/items?offset=1"))
      .mockResolvedValueOnce(page());
    const got = await readSource("air-ops", { source: "ogc", collection: "Air" }, fetcher as unknown as typeof fetch, NOW);
    expect(got.rows).toEqual([{ id: "a", pm10: 1 }, { id: "a", pm10: 1 }]);
    expect(got.cut).toBe(false);
    expect(fetcher.mock.calls[1][0]).toBe("/api/endpoint/air-ops/ogc/features/collections/Air/items?offset=1");

    const leaving = vi.fn().mockResolvedValue(page("https://elsewhere.example/steal"));
    await expect(readSource("air-ops", { source: "ogc", collection: "Air" }, leaving as unknown as typeof fetch, NOW)).rejects.toThrow("the next page is not on the Endpoint");
  });
});
