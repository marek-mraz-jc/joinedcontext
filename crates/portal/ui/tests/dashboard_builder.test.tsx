/**
 * The dashboard builder: a reader meets no edit control, a builder adds a panel, picks what it
 * reads from the server's lists, previews it, and saves choices only. A refusal names its field
 * in the form's words, a conflict offers the saved version, and a saved dashboard shows that it
 * waits for approval.
 */
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ReactNode } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { jsonResponse, renderRoute } from "./pageHarness";
import { boardOf, drawable, emptyBoard, fieldId, fieldWords, missing, newPanel, refusals, withKind } from "../src/pages/grafana/builderModel";
import i18n from "../src/i18n";
import { DisplayEditor, KindOptions, PositionEditor, SaveProblem, TextEditor } from "../src/pages/grafana/DashboardBuilder";
import { ApiError } from "../src/api/client";

const BUILDER = "/api/v1/projects/helsinki/grafana/builder";
const UID = "jcd-0123456789abcdef";
const START = { mayEdit: true, pending: [], refreshes: ["", "1m", "5m"], units: ["", "none", "percent"], colors: ["green", "red"] };
const OPTIONS = {
  endpoints: [{ name: "bikes", title: "Bike stations", slug: "bikeslug", public: true, usable: true }],
  types: ["BikeHireDockingStation"],
  attributes: [
    { name: "availableBikeNumber", kinds: ["timeseries", "stat", "table"] },
    { name: "status", kinds: ["table"] },
    { name: "location", kinds: ["table", "map"] },
  ],
};
const PANEL = {
  id: 1,
  kind: "stat",
  title: "Free bikes",
  grid: { x: 0, y: 0, w: 12, h: 8 },
  endpoint: "bikes",
  type: "BikeHireDockingStation",
  attribute: "availableBikeNumber",
  aggregation: "none",
  interval: "auto",
};
const SAVED = { uid: UID, base: "blob-1", board: { title: "Bikes", from: "now-6h", to: "now", refresh: "", variables: [], panels: [PANEL] } };

function problem(status: number, detail: string, errors?: { path: string; message: string }[]): Response {
  return new Response(JSON.stringify({ type: "about:blank", title: "Refused", status, detail, ...(errors ? { errors } : {}) }), {
    status,
    headers: { "Content-Type": "application/problem+json" },
  });
}

function answering(overrides: Record<string, (request: Request) => Response | undefined> = {}) {
  return (path: string, request: Request) => {
    const key = `${request.method} ${path}`;
    if (overrides[key]) return overrides[key](request);
    if (key === `GET ${BUILDER}`) return jsonResponse(START);
    if (key === `GET ${BUILDER}/${UID}`) return jsonResponse(SAVED);
    if (path.endsWith("/grafana/options")) return jsonResponse(OPTIONS);
    return undefined;
  };
}

/** The bodies the page sent to `path` with `method`. */
async function sentTo(fetchMock: ReturnType<typeof vi.fn>, method: string, path: string): Promise<unknown[]> {
  const out: unknown[] = [];
  for (const [input] of fetchMock.mock.calls) {
    const request = input as Request;
    if (request.method === method && new URL(request.url).pathname === path) out.push(JSON.parse(await request.clone().text()));
  }
  return out;
}

afterEach(() => vi.unstubAllGlobals());

describe("the builder's model", () => {
  const t = (key: string, values?: Record<string, unknown>) => i18n.t(key, values);

  it("names a refused field by the panel's title and the field's label", () => {
    const board = boardOf({ ...SAVED.board, panels: [PANEL] } as never);
    expect(fieldWords("panels[0].endpoint", board, t)).toBe("Panel “Free bikes”, Endpoint");
    expect(fieldWords("panels[0].filters[1].value", board, t)).toBe("Panel “Free bikes”, Filter: Value");
    expect(fieldWords("variables[3].name", board, t)).toBe("Variable unnamed variable, Name");
    expect(fieldWords("title", board, t)).toBe("Title");
  });

  it("reads the server's refusal lines, a bare path taking the detail", () => {
    expect(refusals(["panels[0].kind: a stat cannot draw status"])).toEqual([{ path: "panels[0].kind", message: "a stat cannot draw status" }]);
    expect(refusals(["panels[0].endpoint"], "not yours")).toEqual([{ path: "panels[0].endpoint", message: "not yours" }]);
    expect(refusals(undefined)).toEqual([]);
  });

  it("offers numbers to a chart and anything but a place or a link to a table", () => {
    expect(drawable("stat", OPTIONS.attributes as never).map((a) => a.name)).toEqual(["availableBikeNumber"]);
    expect(drawable("table", OPTIONS.attributes as never).map((a) => a.name)).toEqual(["availableBikeNumber", "status", "location"]);
  });

  it("drops the aggregation of a table and asks one of a chart", () => {
    const panel = newPanel(emptyBoard(), "stat", "One");
    expect(withKind({ ...panel, aggregation: "max", interval: "PT1H" }, "table")).toMatchObject({ kind: "table", aggregation: "none", interval: "auto" });
    expect(withKind(panel, "timeseries").aggregation).toBe("avg");
    expect(missing(panel)).toEqual(["builder.need.endpoint", "builder.need.type", "builder.need.attribute"]);
    expect(missing({ ...PANEL, filters: [{ attribute: "status", op: "eq", value: " " }] } as never)).toEqual(["builder.need.filter"]);
  });
});

describe("the builder's new kinds", () => {
  const t = (key: string, values?: Record<string, unknown>) => i18n.t(key, values);
  const panel = newPanel(emptyBoard(), "stat", "One");

  it("gives each kind its own choices, a scale to gauges only, and a text panel nothing it reads", () => {
    const bars = withKind({ ...PANEL, min: 0, max: 40 } as never, "bargauge");
    expect(bars).toMatchObject({ kind: "bargauge", min: 0, max: 40, bargauge: { displayMode: "gradient", orientation: "auto" } });
    const pie = withKind(bars, "piechart");
    expect(pie).toMatchObject({ kind: "piechart", piechart: { pieType: "pie" } });
    expect(pie).not.toHaveProperty("bargauge");
    expect(pie.min).toBeUndefined();
    const chart = withKind({ ...pie, aggregation: "max", interval: "PT1H" }, "barchart");
    expect(chart).toMatchObject({ kind: "barchart", aggregation: "none", interval: "auto", barchart: { xField: "" } });
    expect(missing(chart)).toEqual(["builder.need.xField"]);
    const words = withKind({ ...chart, unit: "percent", filters: [{ attribute: "status", op: "eq", value: "open" }] }, "text");
    expect(Object.keys(words).sort()).toEqual(["grid", "id", "kind", "text", "title"]);
    expect(missing(words)).toEqual(["builder.need.content"]);
    expect(missing({ ...words, text: { content: "## About" } })).toEqual([]);
    expect(drawable("text", OPTIONS.attributes as never)).toEqual([]);
    expect(drawable("piechart", OPTIONS.attributes as never).map((a) => a.name)).toEqual(["availableBikeNumber"]);
    expect(panel.kind).toBe("stat");
  });

  it("names and finds each new field a refusal names", () => {
    const board = boardOf({ ...SAVED.board, panels: [{ ...PANEL, kind: "barchart", barchart: { xField: "status" } }] } as never);
    expect(fieldWords("panels[0].barchart.xField", board, t)).toBe("Panel “Free bikes”, Bars named by");
    expect(fieldWords("panels[0].max", board, t)).toBe("Panel “Free bikes”, Maximum");
    expect(fieldWords("panels[0].text.content", board, t)).toBe("Panel “Free bikes”, Text");
    expect(fieldWords("panels[0].bargauge", board, t)).toBe("Panel “Free bikes”, Bar style");
    expect(fieldId("panels[0].barchart.xField", board)).toBe("builder-panel-1-xField");
    expect(fieldId("panels[0].bargauge.orientation", board)).toBe("builder-panel-1-orientation");
    expect(fieldId("panels[0].piechart", board)).toBe("builder-panel-1-pieType");
    expect(fieldId("panels[0].text.content", board)).toBe("builder-panel-1-content");
    expect(fieldId("panels[0].min", board)).toBe("builder-panel-1-min");
    expect(fieldId("panels[0].kind", board)).toBe("builder-panel-1-kind");
  });

  it("offers a bar gauge's style and direction, a pie's shape, and text attributes to name bars", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => jsonResponse(OPTIONS)));
    const onChange = vi.fn();
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}>{children}</QueryClientProvider>;
    const { rerender } = render(<KindOptions project="helsinki" panel={withKind(PANEL as never, "bargauge")} onChange={onChange} />, { wrapper });
    fireEvent.change(screen.getByLabelText("Bar style"), { target: { value: "lcd" } });
    expect(onChange).toHaveBeenLastCalledWith(expect.objectContaining({ bargauge: { displayMode: "lcd", orientation: "auto" } }));
    fireEvent.change(screen.getByLabelText("Direction"), { target: { value: "vertical" } });
    expect(onChange).toHaveBeenLastCalledWith(expect.objectContaining({ bargauge: { displayMode: "gradient", orientation: "vertical" } }));
    rerender(<KindOptions project="helsinki" panel={withKind(PANEL as never, "piechart")} onChange={onChange} />);
    fireEvent.change(screen.getByLabelText("Shape"), { target: { value: "donut" } });
    expect(onChange).toHaveBeenLastCalledWith(expect.objectContaining({ piechart: { pieType: "donut" } }));
    rerender(<KindOptions project="helsinki" panel={withKind(PANEL as never, "barchart")} onChange={onChange} />);
    const names = screen.getByLabelText(/^Bars named by/);
    await waitFor(() => expect(within(names).getByRole("option", { name: "status" })).toBeInTheDocument());
    // Only text names a bar: the number and the place are not offered.
    expect(within(names).queryByRole("option", { name: "availableBikeNumber" })).toBeNull();
    expect(within(names).queryByRole("option", { name: "location" })).toBeNull();
    fireEvent.change(names, { target: { value: "status" } });
    expect(onChange).toHaveBeenLastCalledWith(expect.objectContaining({ barchart: { xField: "status" } }));
  });

  it("takes a gauge's scale as numbers, empty as none, and a text panel's words up to the cap", () => {
    const onChange = vi.fn();
    const gauge = withKind(PANEL as never, "gauge");
    const { rerender } = render(<DisplayEditor panel={gauge} start={START as never} onChange={onChange} />);
    fireEvent.change(screen.getByLabelText("Maximum"), { target: { value: "40" } });
    expect(onChange).toHaveBeenLastCalledWith(expect.objectContaining({ max: 40 }));
    rerender(<DisplayEditor panel={{ ...gauge, max: 40 }} start={START as never} onChange={onChange} />);
    fireEvent.change(screen.getByLabelText("Maximum"), { target: { value: "" } });
    expect(onChange.mock.lastCall?.[0].max).toBeUndefined();
    rerender(<DisplayEditor panel={withKind(PANEL as never, "stat")} start={START as never} onChange={onChange} />);
    expect(screen.queryByLabelText("Maximum")).toBeNull();
    rerender(<TextEditor panel={withKind(PANEL as never, "text")} onChange={onChange} />);
    const text = screen.getByLabelText(/^Text/);
    expect(text).toHaveAttribute("maxlength", "4000");
    fireEvent.change(text, { target: { value: "## About" } });
    expect(onChange).toHaveBeenLastCalledWith(expect.objectContaining({ text: { content: "## About" } }));
  });
});

describe("the builder's parts", () => {
  it("keeps a typed position inside the 24 columns", () => {
    const onPlace = vi.fn();
    render(<PositionEditor panel={PANEL as never} onPlace={onPlace} />);
    fireEvent.change(screen.getByLabelText("Width"), { target: { value: "40" } });
    expect(onPlace).toHaveBeenLastCalledWith({ x: 0, y: 0, w: 24, h: 8 });
    fireEvent.change(screen.getByLabelText("Column"), { target: { value: "20" } });
    expect(onPlace).toHaveBeenLastCalledWith({ x: 12, y: 0, w: 12, h: 8 });
  });

  it("offers the saved version only on a conflict", () => {
    const onReload = vi.fn();
    const board = boardOf(SAVED.board as never);
    const { rerender } = render(<SaveProblem error={new ApiError(409, "changed")} board={board} onReload={onReload} />);
    fireEvent.click(screen.getByRole("button", { name: "Open the saved version" }));
    expect(onReload).toHaveBeenCalledOnce();
    rerender(<SaveProblem error={new ApiError(500, "the forge did not answer")} board={board} onReload={onReload} />);
    expect(screen.queryByRole("button", { name: "Open the saved version" })).toBeNull();
    expect(screen.getByText("The dashboard was not proposed")).toBeInTheDocument();
  });
});

describe("the dashboard builder page", () => {
  it("shows a reader no edit control and says what to ask for", async () => {
    await renderRoute({ path: "/projects/helsinki/grafana/new", answer: answering({ [`GET ${BUILDER}`]: () => jsonResponse({ ...START, mayEdit: false }) }) });
    expect(await screen.findByText("You cannot build dashboards in this project")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Propose dashboard" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Add panel" })).toBeNull();
  });

  it("builds a panel from the lists, previews it, and saves choices only", async () => {
    const view = await renderRoute({
      path: "/projects/helsinki/grafana/new",
      answer: answering({
        [`POST ${BUILDER}/preview`]: () => jsonResponse({ panel: { id: 1, type: "stat", title: "Panel 1", gridPos: { x: 0, y: 0, w: 12, h: 8 } }, frames: [{ name: "", refId: "A", fields: [{ name: "availableBikeNumber", type: "number", values: [7] }] }] }),
        [`POST ${BUILDER}`]: () => jsonResponse({ uid: UID, change: { metadata: { name: "chg-00000009" } } }, 202),
      }),
    });
    const user = userEvent.setup();
    expect(await screen.findByText("This dashboard has no panels yet")).toBeInTheDocument();
    const save = screen.getByRole("button", { name: "Propose dashboard" });
    expect(save).toHaveAttribute("aria-disabled", "true");
    await user.type(screen.getByLabelText(/^Title/), "Bikes");
    await user.click(screen.getByRole("button", { name: "Add panel" }));
    const editor = await screen.findByRole("region", { name: "Panel: Panel 1" });
    await within(editor).findByRole("option", { name: "Bike stations" });
    await user.selectOptions(within(editor).getByLabelText("Endpoint"), "bikes");
    await waitFor(() => expect(within(editor).getByLabelText("Type")).toBeEnabled());
    await user.selectOptions(within(editor).getByLabelText("Type"), "BikeHireDockingStation");
    await waitFor(() => expect(within(editor).getByLabelText("Attribute")).toBeEnabled());
    // A time series takes numbers only: the text and the place are not offered.
    expect(within(within(editor).getByLabelText("Attribute")).queryByRole("option", { name: "status" })).toBeNull();
    await user.selectOptions(within(editor).getByLabelText("Attribute"), "availableBikeNumber");
    await user.click(within(editor).getByRole("button", { name: "Preview" }));
    await waitFor(async () => expect(await sentTo(view.fetchMock, "POST", `${BUILDER}/preview`)).toHaveLength(1));
    const [asked] = (await sentTo(view.fetchMock, "POST", `${BUILDER}/preview`)) as { board: Record<string, unknown>; panel: number }[];
    expect(asked.panel).toBe(1);
    expect(JSON.stringify(asked)).not.toMatch(/customQuery|targets|"q"/);
    await waitFor(() => expect(save).not.toHaveAttribute("aria-disabled", "true"));
    await user.click(save);
    await waitFor(async () => expect(await sentTo(view.fetchMock, "POST", BUILDER)).toHaveLength(1));
    const [body] = (await sentTo(view.fetchMock, "POST", BUILDER)) as { board: { panels: Record<string, unknown>[] } }[];
    expect(Object.keys(body)).toEqual(["board"]);
    expect(body.board.panels[0]).toMatchObject({ kind: "timeseries", endpoint: "bikes", type: "BikeHireDockingStation", attribute: "availableBikeNumber", grid: { x: 0, y: 0, w: 12, h: 8 } });
    await waitFor(() => expect(window.location.pathname).toBe(`/projects/helsinki/grafana/dashboards/${UID}/edit`));
  });

  it("names each refused field in the form's words", async () => {
    await renderRoute({
      path: `/projects/helsinki/grafana/dashboards/${UID}/edit`,
      answer: answering({ [`PUT ${BUILDER}/${UID}`]: () => problem(422, "not one you may read", [{ path: "panels[0].endpoint", message: "choose one from the list" }]) }),
    });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Propose dashboard" }));
    expect(await screen.findByText("The dashboard was not proposed")).toBeInTheDocument();
    expect(screen.getByText("Panel “Free bikes”, Endpoint:")).toBeInTheDocument();
    expect(screen.queryByText(/panels\[0\]/)).toBeNull();
  });

  it("offers the saved version after a conflict and sends the revision it started from", async () => {
    const view = await renderRoute({
      path: `/projects/helsinki/grafana/dashboards/${UID}/edit`,
      answer: answering({ [`PUT ${BUILDER}/${UID}`]: () => problem(409, "the dashboard changed since you opened it; open it again to edit the saved version") }),
    });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Propose dashboard" }));
    expect(await screen.findByText("This dashboard changed since you opened it")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Open the saved version" })).toBeInTheDocument();
    const [body] = (await sentTo(view.fetchMock, "PUT", `${BUILDER}/${UID}`)) as { base: string }[];
    expect(body.base).toBe("blob-1");
  });

  it("holds the save while a change of the dashboard waits for approval", async () => {
    await renderRoute({
      path: `/projects/helsinki/grafana/dashboards/${UID}/edit`,
      answer: answering({ [`GET ${BUILDER}/${UID}`]: () => jsonResponse({ ...SAVED, pending: { uid: UID, title: "Bikes", change: "chg-00000009", url: "https://forge/pulls/9" } }) }),
    });
    expect(await screen.findByText("A change of this dashboard waits for approval")).toBeInTheDocument();
    expect(screen.getByText("Waiting for approval")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Propose dashboard" })).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByRole("link", { name: "Open the change" })).toHaveAttribute("href", "/projects/helsinki/approvals/chg-00000009");
  });

  it("says why a preview failed", async () => {
    await renderRoute({
      path: `/projects/helsinki/grafana/dashboards/${UID}/edit`,
      answer: answering({ [`POST ${BUILDER}/preview`]: () => problem(403, "panels[0].endpoint: bikes is not an Endpoint of helsinki you may read") }),
    });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Preview" }));
    expect(await screen.findByText("The preview could not be read")).toBeInTheDocument();
  });
});
