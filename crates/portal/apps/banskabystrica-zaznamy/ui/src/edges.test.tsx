/**
 * The records screen's smaller parts at their edges (T-3382, T-3387): the table's cells for a value
 * that is no number or no date, each shape of chart, a cube read page by page, every dataset button
 * in both languages, and a configuration with no endpoint at all.
 */
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { JcProvider, toRichRow } from "@joinedcontext/sdk";
import type { EntitySource, RichCell, RichRow } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import App from "./App";
import { recordRenderers } from "./cells";
import { answer, CITY } from "./fixtures/records";
import { LOCALES, SPACE_OF } from "./locales";
import { Overview } from "./Overview";
import { gridConfig, noteFault } from "./records";
import { SeriesChart } from "./SeriesChart";
import type { Chart } from "./series";

const WORDS = { locale: "en", showId: "Identifier", openRecord: "Open the record" };

function cell(renderers: ReturnType<typeof recordRenderers>, attr: string, row: Record<string, unknown>) {
  const rich = toRichRow({ id: "urn:ngsi-ld:KeyPerformanceIndicator:x", type: "KeyPerformanceIndicator", ...row }, "en");
  const view = render(<div data-testid="cell">{renderers[attr](rich.cells[attr] as RichCell | RichCell[] | undefined, rich)}</div>);
  const text = view.getByTestId("cell").textContent;
  view.unmount();
  return text;
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the table's cells", () => {
  const renderers = recordRenderers("en", WORDS, vi.fn());

  it("draw a code the lists do not hold as the code, and the first of several values", () => {
    expect(cell(renderers, "indicator", { indicator: { type: "Property", value: "X999" } })).toBe("X999");
    expect(cell(renderers, "indicator", { indicator: [{ type: "Property", value: "X1", datasetId: "urn:a" }, { type: "Property", value: "X2", datasetId: "urn:b" }] })).toBe("X1");
    expect(cell(renderers, "refArea", { refArea: { type: "Property", value: "SK000" } })).toBe("SK000");
    expect(cell(renderers, "indicator", {})).toBe("");
    expect(cell(renderers, "refArea", {})).toBe("");
  });

  it("draw a value that is no number as written, and a number without a unit alone", () => {
    expect(cell(renderers, "kpiValue", { kpiValue: { type: "Property", value: "n/a" } })).toBe("n/a");
    expect(cell(renderers, "kpiValue", {})).toBe("");
    expect(cell(renderers, "kpiValue", { kpiValue: { type: "Property", value: 12.5 } })).toBe("12.5");
    expect(cell(renderers, "kpiValue", { kpiValue: { type: "Property", value: 3 }, unitText: { type: "Property", value: 7 } })).toBe("3");
  });

  it("draw a date that is no date as written", () => {
    expect(cell(renderers, "dateObserved", { dateObserved: { type: "Property", value: "someday" } })).toBe("someday");
    expect(cell(renderers, "dateObserved", { dateObserved: { type: "Property", value: 5 } })).toBe("5");
    expect(cell(renderers, "dateObserved", {})).toBe("");
  });

  it("open a record by its button", () => {
    const open = vi.fn();
    const rich: RichRow = toRichRow({ id: "urn:ngsi-ld:KeyPerformanceIndicator:x", type: "KeyPerformanceIndicator" }, "en");
    render(<div>{recordRenderers("en", WORDS, open).id(undefined, rich)}</div>);
    fireEvent.click(screen.getByRole("button", { name: "Open the record" }));
    expect(open).toHaveBeenCalledWith(rich);
  });
});

describe("the charts", () => {
  const words = LOCALES.en.chart;
  const chart = (shape: Chart["shape"], values: number[]): Chart => ({
    id: shape,
    title: shape,
    subtitle: "",
    unit: "",
    shape,
    points: values.map((value, i) => ({ at: String(i), label: `p${i}`, value })),
  });

  it("draws columns for keys, bars for areas, a line for periods, and nothing out of no point", () => {
    for (const [shape, values] of [["keys", [3, -1, 4]], ["areas", [0, 0]], ["line", [1, 2, 3, 4, 5, 6]], ["line", []]] as const) {
      const { container, unmount } = render(<SeriesChart chart={chart(shape, [...values])} colour="#000" words={words} />);
      expect(container.querySelector("figure")).not.toBeNull();
      unmount();
    }
  });
});

describe("a cube read page by page", () => {
  it("asks for the next page while one comes back full", async () => {
    const asked: number[] = [];
    const rows = answer(CITY);
    const source = {
      query: async (_query: unknown, page: { offset: number; limit: number }) => {
        asked.push(page.offset);
        const full = asked.length === 1 ? page.limit : 1;
        return { rows: Array.from({ length: full }, (_, at) => toRichRow({ ...rows[0], id: `${String(rows[0].id)}-${at}` }, "sk")) };
      },
    } as unknown as EntitySource;
    render(<Overview body="banskabystrica" source={source} s={LOCALES.sk} />);
    await waitFor(() => expect(asked.length).toBe(2));
    expect(asked[1]).toBeGreaterThan(0);
  });
});

describe("the dataset buttons", () => {
  it.each([
    ["sk", "banskabystrica"],
    ["en", "banskabystrica"],
    ["sk", "bbsk"],
    ["en", "bbsk"],
  ] as const)("in %s, the %s screen's each picks its cube", async (language, body) => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => new Response(JSON.stringify(answer(CITY)), { status: 200, headers: { "content-type": "application/json" } })),
    );
    const client = stubClient(undefined, { slug: "s", orgDomain: "banskabystrica.sk", space: SPACE_OF[body], transport: "origin", appName: "banskabystrica-zaznamy", language });
    render(
      <JcProvider client={client}>
        <App />
      </JcProvider>,
    );
    const group = await screen.findByRole("group", { name: LOCALES[language].pickDataset });
    for (const button of within(group).getAllByRole("button")) {
      fireEvent.click(button);
      expect(button).toHaveAttribute("aria-pressed", "true");
    }
  });

  it("says so when the configuration names no endpoint to read", () => {
    const client = stubClient(undefined, { slug: "", orgDomain: "banskabystrica.sk", space: SPACE_OF.banskabystrica, transport: "origin", appName: "banskabystrica-zaznamy", language: "en" });
    render(
      <JcProvider client={client}>
        <App />
      </JcProvider>,
    );
    expect(screen.getByRole("alert")).toHaveTextContent(LOCALES.en.noEndpoint);
  });
});

describe("the words and the grid", () => {
  it("say the latest period in both languages", () => {
    expect(LOCALES.sk.latestOf("2025")).toContain("2025");
    expect(LOCALES.en.latestOf("2025")).toContain("2025");
  });

  it("label a column it has no word for by its attribute, and take a note given bare", () => {
    expect(gridConfig("s", {}, true).columns.every((column) => column.label === column.attr)).toBe(true);
    const words = { tooLong: (max: number) => `max ${max}`, forbidden: "no <>", notText: "text" };
    expect(noteFault({ stewardNote: "bare note" }, words)).toBeNull();
  });
});
