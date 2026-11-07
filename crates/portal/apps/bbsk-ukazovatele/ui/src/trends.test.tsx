/**
 * The trend lines (T-2934): the region's yearly rows, read through the public endpoint of
 * bbsk-kraj, drawn on each region card that has two years or more and on no other.
 * `fixtures/bbsk-kraj.json` is what the pipelines obyvatelstvo and emisie wrote from the cubes
 * recorded for T-2307, cut to the attributes the read asks for.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { JcProvider, toRichRow } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import App from "./App";
import region from "./fixtures/bbsk-kpi.json";
import city from "./fixtures/banskabystrica-kpi.json";
import raw from "./fixtures/bbsk-kraj.json";
import { LOCALES } from "./locales";
import { linePoints, SERIES_QUERY, seriesKey, seriesOf } from "./trends";

const s = LOCALES.sk;
const ENDPOINTS = [
  { name: "bbsk-kpi", slug: "region", space: "bbsk-kpi", types: ["KeyPerformanceIndicator"] },
  { name: "mesto-kpi", slug: "city", space: "banskabystrica-kpi", types: ["KeyPerformanceIndicator"] },
  { name: "bbsk-kraj-verejne", slug: "kraj", space: "bbsk-kraj", types: ["KeyPerformanceIndicator"] },
];

type Row = (typeof raw)[number];
const area = (row: Row) => row.refArea.value;
const cube = (row: Row) => row.dataSet.value;

/** The app over the three endpoints; `rows` is what bbsk-kraj answers, or `refused` for a 403. */
function show(rows: unknown[] | "refused", endpoints = ENDPOINTS) {
  const asked: URL[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (path: string) => {
      const url = new URL(path, "http://portal.test");
      const slug = /\/api\/endpoint\/([^/]+)\//.exec(url.pathname)?.[1];
      const json = (body: unknown) => new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
      if (slug === "region") return json(region);
      if (slug === "city") return json(city);
      if (slug === "kraj") {
        asked.push(url);
        if (rows === "refused") {
          return new Response(JSON.stringify({ title: "Forbidden", status: 403, detail: "no policy" }), {
            status: 403,
            headers: { "content-type": "application/problem+json" },
          });
        }
        const offset = Number(url.searchParams.get("offset") ?? 0);
        return json(rows.slice(offset, offset + Number(url.searchParams.get("limit") ?? 1000)));
      }
      throw new Error(`no stub for ${path}`);
    }),
  );
  const client = stubClient(undefined, {
    slug: "region",
    orgDomain: "bbsk.sk",
    space: "bbsk-kpi",
    transport: "origin",
    appName: "bbsk-ukazovatele",
    language: "sk",
    endpoints,
  });
  render(
    <JcProvider client={client}>
      <App />
    </JcProvider>,
  );
  return asked;
}

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

const regionSection = () => screen.getByRole("region", { name: s.body.bbsk });
const cardOf = (key: string, territory: string) =>
  within(within(regionSection()).getByRole("region", { name: s.indicator[key]?.title ?? key })).getByRole("article", {
    name: s.territory[territory] ?? territory,
  });

describe("seriesOf", () => {
  it("gives each indicator and territory its published years, oldest first", () => {
    const series = seriesOf(raw.map((row) => toRichRow(row)));
    const kraj = series.get(seriesKey("obyvatelstvo-stav", "kraj")) ?? [];
    expect(kraj.length).toBe(raw.filter((row) => cube(row) === "om7102rr" && area(row) === "SK032").length);
    expect(kraj.map((point) => point.period)).toEqual([...kraj.map((point) => point.period)].sort());
    const emissions = series.get(seriesKey("emisie-tuhe-km2", "okres-brezno")) ?? [];
    expect(emissions.map((point) => point.period)).toEqual(["2019", "2020", "2021", "2022", "2023"]);
    expect(series.size).toBe(28);
  });

  it("leaves out a row of another cell, territory or a value that is no number", () => {
    const [row] = raw.filter((r) => cube(r) === "zp3803rs");
    const other = (patch: Record<string, unknown>) => toRichRow({ ...row, ...patch });
    expect(seriesOf([other({ dimensionKey: { type: "Property", value: "2" } })]).size).toBe(0);
    expect(seriesOf([other({ refArea: { type: "Property", value: "SK010" } })]).size).toBe(0);
    expect(seriesOf([other({ kpiValue: { type: "Property", value: "n/a" } })]).size).toBe(0);
    expect(seriesOf([other({ refPeriod: { type: "Property", value: "2023Q1" } })]).size).toBe(0);
    expect(seriesOf([other({ refArea: undefined })]).size).toBe(0);
    expect(seriesOf([other({ type: "PointOfInterest" })]).size).toBe(0);
    // Several values of one attribute: the first is the one read.
    const twice = other({ kpiValue: [{ type: "Property", value: 4, datasetId: "urn:a" }, { type: "Property", value: 9, datasetId: "urn:b" }] });
    expect([...seriesOf([twice]).values()][0]).toEqual([{ period: String(row.refPeriod.value), value: 4 }]);
  });

  it("draws a line only through two points or more, a flat one through the middle", () => {
    expect(linePoints([{ period: "2020", value: 1 }], 100, 20)).toBeNull();
    expect(linePoints([{ period: "2020", value: 1 }, { period: "2021", value: 3 }], 100, 20)).toBe("0.0,20.0 100.0,0.0");
    expect(linePoints([{ period: "2020", value: 2 }, { period: "2021", value: 2 }], 100, 20)).toBe("0.0,10.0 100.0,10.0");
  });
});

describe("the region's cards", () => {
  it("draw one line per indicator with at least two years, and none on the city's", async () => {
    const asked = show(raw);
    await waitFor(() => expect(regionSection().querySelectorAll(".trend polyline").length).toBe(28));
    expect(asked[0].searchParams.get("q")).toBe(SERIES_QUERY);
    const kraj = cardOf("obyvatelstvo-stav", "kraj");
    expect(within(kraj).getByText(/^Vývoj 1993 – \d{4}: od /)).toBeInTheDocument();
    expect(screen.getByRole("region", { name: s.body.banskabystrica }).querySelectorAll(".trend").length).toBe(0);
  });

  it("draw no line where a territory has one year", async () => {
    const rows = raw.filter((row) => !(cube(row) === "zp3803rs" && area(row) === "SK0327" && row.refPeriod.value !== "2023"));
    show(rows);
    await waitFor(() => expect(regionSection().querySelectorAll(".trend polyline").length).toBe(27));
    expect(cardOf("emisie-tuhe-km2", "okres-poltar").querySelector(".trend")).toBeNull();
  });

  it("keep their one window when the app has no endpoint of the raw rows, or the read fails", async () => {
    show(raw, ENDPOINTS.slice(0, 2));
    await screen.findAllByRole("article");
    expect(document.querySelectorAll(".trend").length).toBe(0);
  });

  it("say so when the read of the raw rows is refused", async () => {
    show("refused");
    expect(await screen.findByText(new RegExp(s.trendUnavailable))).toHaveTextContent("no policy");
    await screen.findAllByRole("article");
    expect(document.querySelectorAll(".trend").length).toBe(0);
  });
});
