// T-7589: the builder's panel form, filter rows, variable source, failed reads, refusals and the narrow screen.
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import en from "../src/locales/en.json";
import i18n from "../src/i18n";
import { BuilderCanvas, FiltersEditor, PanelEditor, VariablesEditor } from "../src/pages/grafana/DashboardBuilder";
import type { Board, BoardPanel, BoardVariable } from "../src/pages/grafana/builderModel";
import { jsonResponse, renderRoute } from "./pageHarness";

const BUILDER = "/api/v1/projects/helsinki/grafana/builder";
const UID = "jcd-0123456789abcdef";
const START = { mayEdit: true, pending: [], refreshes: ["", "1m", "5m"], units: ["", "none", "percent"], colors: ["green", "red", "yellow"] };
const OPTIONS = {
  endpoints: [
    { name: "bikes", title: "Bike stations", slug: "bikeslug", public: true, usable: true },
    { name: "parking", title: "Parking", slug: "parkslug", public: true, usable: true },
  ],
  types: ["BikeHireDockingStation"],
  attributes: [
    { name: "availableBikeNumber", type: "number", kinds: ["timeseries", "stat", "table"] },
    { name: "status", type: "string", kinds: ["table"] },
    { name: "location", type: "geo", kinds: ["table", "map"] },
  ],
};
const PANEL: BoardPanel = {
  id: 1,
  kind: "timeseries",
  title: "Free bikes",
  description: "Sensors",
  grid: { x: 0, y: 0, w: 12, h: 8 },
  endpoint: "bikes",
  type: "BikeHireDockingStation",
  attribute: "availableBikeNumber",
  aggregation: "none",
  interval: "auto",
};
const VARIABLE: BoardVariable = { name: "state", label: "State", endpoint: "bikes", type: "BikeHireDockingStation", attribute: "status", includeAll: true };
const SAVED = { uid: UID, base: "blob-1", board: { title: "Bikes", from: "now-6h", to: "now", refresh: "", variables: [], panels: [PANEL] } };

function problem(status: number, detail: string, errors?: { path: string; message: string }[]): Response {
  return new Response(JSON.stringify({ type: "about:blank", title: "Refused", status, detail, ...(errors ? { errors } : {}) }), {
    status,
    headers: { "Content-Type": "application/problem+json" },
  });
}

function withClient(node: React.ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>{node}</I18nextProvider>
    </QueryClientProvider>,
  );
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("the panel form", () => {
  function Form({ initial, removed }: { initial: BoardPanel; removed: () => void }) {
    const [panel, setPanel] = useState(initial);
    const board: Board = { title: "Bikes", panels: [panel], variables: [VARIABLE] };
    return (
      <PanelEditor
        project="helsinki"
        board={board}
        panel={panel}
        start={START}
        onChange={setPanel}
        onPlace={(grid) => setPanel({ ...panel, grid })}
        onRemove={removed}
      />
    );
  }

  beforeEach(() => {
    vi.stubGlobal("fetch", vi.fn(async () => jsonResponse(OPTIONS)));
  });

  it("changes the kind, the title, the description and the position", async () => {
    const user = userEvent.setup();
    withClient(<Form initial={PANEL} removed={vi.fn()} />);
    const form = await screen.findByRole("region", { name: /Panel: Free bikes/ });
    await user.click(within(form).getByRole("radio", { name: en.builder.kind.stat }));
    expect(within(form).getByRole("radio", { name: en.builder.kind.stat })).toBeChecked();
    const title = within(form).getByLabelText(new RegExp(`^${en.builder.field.panelTitle}`));
    await user.clear(title);
    await user.type(title, "Docks");
    expect(title).toHaveValue("Docks");
    const description = within(form).getByLabelText(en.builder.field.description);
    await user.clear(description);
    await user.type(description, "Per dock");
    expect(description).toHaveValue("Per dock");
    const x = within(form).getByLabelText(en.builder.grid.x);
    await user.clear(x);
    await user.type(x, "5");
    expect(x).toHaveValue(5);
  });

  it("asks for an interval only once the readings are aggregated, and drops it with the aggregation", async () => {
    const user = userEvent.setup();
    withClient(<Form initial={PANEL} removed={vi.fn()} />);
    const aggregation = await screen.findByLabelText(en.builder.field.aggregation);
    const interval = screen.getByLabelText(en.builder.field.interval);
    expect(interval).toBeDisabled();
    await user.selectOptions(aggregation, "avg");
    expect(interval).toBeEnabled();
    await user.selectOptions(interval, "PT1H");
    expect(interval).toHaveValue("PT1H");
    await user.selectOptions(aggregation, "none");
    expect(interval).toBeDisabled();
    expect(interval).toHaveValue("auto");
  });

  it("clears the type and the filters when another endpoint is chosen", async () => {
    const user = userEvent.setup();
    withClient(<Form initial={{ ...PANEL, filters: [{ attribute: "availableBikeNumber", op: "gt", value: "2" }] }} removed={vi.fn()} />);
    const endpoint = await screen.findByLabelText(new RegExp(`^${en.builder.field.endpoint}`));
    await screen.findByRole("option", { name: "Parking" });
    expect(screen.getByLabelText(en.builder.field.filterValue)).toHaveValue("2");
    await user.selectOptions(endpoint, "parking");
    expect(screen.getByLabelText(new RegExp(`^${en.builder.field.type}`))).toHaveValue("");
    expect(screen.queryByLabelText(en.builder.field.filterValue)).not.toBeInTheDocument();
  });

  it("names a bar chart's bars by a text attribute and forgets the choice with the endpoint", async () => {
    const user = userEvent.setup();
    withClient(<Form initial={{ ...PANEL, kind: "barchart", barchart: { xField: "" } }} removed={vi.fn()} />);
    const bars = await screen.findByLabelText(new RegExp(`^${en.builder.field.xField}`));
    await screen.findByRole("option", { name: "status" });
    await user.selectOptions(bars, "status");
    expect(bars).toHaveValue("status");
    await user.selectOptions(screen.getByLabelText(new RegExp(`^${en.builder.field.endpoint}`)), "parking");
    expect(screen.getByLabelText(new RegExp(`^${en.builder.field.xField}`))).toHaveValue("");
  });

  it("adds a filter, and asks before it removes the panel", async () => {
    const user = userEvent.setup();
    const removed = vi.fn();
    withClient(<Form initial={PANEL} removed={removed} />);
    await screen.findByRole("option", { name: "Parking" });
    await user.click(screen.getByRole("button", { name: en.builder.addFilter }));
    expect(screen.getByLabelText(en.builder.field.filterValue)).toHaveValue("");
    await user.click(screen.getByRole("button", { name: en.builder.removePanel }));
    expect(screen.getByText(en.builder.removePanelAsk)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: en.builder.keep }));
    expect(screen.queryByText(en.builder.removePanelAsk)).not.toBeInTheDocument();
    expect(removed).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: en.builder.removePanel }));
    await user.click(screen.getByRole("button", { name: en.builder.removePanel }));
    expect(removed).toHaveBeenCalledTimes(1);
  });
});

describe("the filter rows", () => {
  function Rows({ initial }: { initial: BoardPanel["filters"] }) {
    const [filters, setFilters] = useState(initial);
    return <FiltersEditor project="helsinki" panel={{ ...PANEL, filters }} variables={[VARIABLE]} onChange={setFilters} />;
  }

  beforeEach(() => {
    vi.stubGlobal("fetch", vi.fn(async () => jsonResponse(OPTIONS)));
  });

  it("edits the attribute, the comparison and the value of a filter, and removes it", async () => {
    const user = userEvent.setup();
    withClient(<Rows initial={[{ attribute: "availableBikeNumber", op: "eq", value: "3" }]} />);
    const attribute = await screen.findByLabelText(en.builder.field.attribute);
    await screen.findByRole("option", { name: "status" });
    await user.selectOptions(attribute, "status");
    expect(attribute).toHaveValue("status");
    // A text attribute is compared by equality only.
    expect(within(screen.getByLabelText(en.builder.field.filterOp)).queryByRole("option", { name: en.builder.op.gt })).not.toBeInTheDocument();
    await user.selectOptions(attribute, "availableBikeNumber");
    const op = screen.getByLabelText(en.builder.field.filterOp);
    await user.selectOptions(op, "gt");
    expect(op).toHaveValue("gt");
    const value = screen.getByLabelText(en.builder.field.filterValue);
    await user.clear(value);
    await user.type(value, "9");
    expect(value).toHaveValue("9");
    await user.click(screen.getByRole("button", { name: en.builder.removeFilter }));
    expect(screen.queryByLabelText(en.builder.field.filterValue)).not.toBeInTheDocument();
  });

  it("matches a filter against a variable's picks and back to a value", async () => {
    const user = userEvent.setup();
    withClient(<Rows initial={[{ attribute: "status", op: "eq", value: "open" }]} />);
    await user.click(await screen.findByRole("button", { name: en.builder.byVariable }));
    const picked = screen.getByLabelText(en.builder.field.filterVariable);
    expect(picked).toHaveValue("state");
    expect(screen.getByText(en.builder.op.eq)).toBeInTheDocument();
    await user.selectOptions(picked, "state");
    expect(picked).toHaveValue("state");
    await user.click(screen.getByRole("button", { name: en.builder.byValue }));
    expect(screen.getByLabelText(en.builder.field.filterValue)).toHaveValue("");
  });
});

describe("the variables' source", () => {
  it("lists the endpoints and sets the one chosen for a variable", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => jsonResponse(OPTIONS)));
    function Vars() {
      const [board, setBoard] = useState<Board>({ title: "Bikes", panels: [], variables: [{ ...VARIABLE, endpoint: "", type: "", attribute: "" }] });
      return <VariablesEditor project="helsinki" board={board} onChange={setBoard} />;
    }
    const user = userEvent.setup();
    withClient(<Vars />);
    const endpoint = await screen.findByLabelText(new RegExp(`^${en.builder.field.endpoint}`));
    await screen.findByRole("option", { name: "Parking" });
    await user.selectOptions(endpoint, "parking");
    expect(endpoint).toHaveValue("parking");
  });
});

describe("the builder page, failures and refusals", () => {
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

  it("says the builder could not start and starts it on Retry", async () => {
    let tries = 0;
    await renderRoute({
      path: "/projects/helsinki/grafana/new",
      answer: answering({
        [`GET ${BUILDER}`]: () => {
          tries += 1;
          return tries === 1 ? problem(500, "builder down") : jsonResponse(START);
        },
      }),
    });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: en.app.error.retry }));
    expect(await screen.findByText(en.builder.emptyTitle)).toBeInTheDocument();
    expect(tries).toBe(2);
  });

  it("says a saved dashboard could not be read and reads it again on Retry", async () => {
    let tries = 0;
    await renderRoute({
      path: `/projects/helsinki/grafana/dashboards/${UID}/edit`,
      answer: answering({
        [`GET ${BUILDER}/${UID}`]: () => {
          tries += 1;
          return tries === 1 ? problem(500, "store down") : jsonResponse(SAVED);
        },
      }),
    });
    const user = userEvent.setup();
    expect(await screen.findByRole("heading", { level: 1, name: en.builder.editTitle })).toBeInTheDocument();
    await user.click(await screen.findByRole("button", { name: en.app.error.retry }));
    expect(await screen.findByRole("button", { name: new RegExp("Free bikes") })).toBeInTheDocument();
    expect(tries).toBe(2);
  });

  it("holds the board once the change it proposed is listed as waiting", async () => {
    let proposed = false;
    await renderRoute({
      path: `/projects/helsinki/grafana/dashboards/${UID}/edit`,
      answer: answering({
        [`PUT ${BUILDER}/${UID}`]: () => {
          proposed = true;
          return jsonResponse({ uid: UID, change: { metadata: { name: "chg-00000009" } } }, 202);
        },
        [`GET ${BUILDER}`]: () => jsonResponse({ ...START, pending: proposed ? [{ uid: UID, title: "Bikes", change: "chg-00000009", url: "https://forge/pulls/9" }] : [] }),
      }),
    });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: en.builder.save }));
    expect(await screen.findByText(en.builder.pendingTitle)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: en.builder.openChange })).toHaveAttribute("href", "/projects/helsinki/approvals/chg-00000009");
  });

  it("sends the position typed into the selected panel", async () => {
    const view = await renderRoute({
      path: `/projects/helsinki/grafana/dashboards/${UID}/edit`,
      answer: answering({ [`PUT ${BUILDER}/${UID}`]: () => jsonResponse({ uid: UID, change: { metadata: { name: "chg-00000009" } } }, 202) }),
    });
    const user = userEvent.setup();
    const x = await screen.findByLabelText(en.builder.grid.x);
    await user.clear(x);
    await user.type(x, "6");
    await user.click(screen.getByRole("button", { name: en.builder.save }));
    await waitFor(() => expect(view.calls()).toContain(`PUT ${BUILDER}/${UID}`));
    const put = view.fetchMock.mock.calls.map((call) => call[0] as Request).find((request) => request.method === "PUT");
    const body = JSON.parse(await put!.clone().text()) as { board: { panels: { grid: { x: number } }[] } };
    expect(body.board.panels[0].grid.x).toBe(6);
  });

  it("takes the person to the board field a refusal names", async () => {
    await renderRoute({
      path: `/projects/helsinki/grafana/dashboards/${UID}/edit`,
      answer: answering({ [`PUT ${BUILDER}/${UID}`]: () => problem(422, "refused", [{ path: "from", message: "not a range" }]) }),
    });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: en.builder.save }));
    const refused = (await screen.findByText(en.builder.saveFailedTitle)).closest("[role]") as HTMLElement;
    expect(within(refused).getByText(/not a range/)).toBeInTheDocument();
    await user.click(within(refused).getByRole("button"));
    await waitFor(() => expect(document.activeElement?.id).toBe("builder-from"));
  });

  it("opens the saved version again after a conflict", async () => {
    let reads = 0;
    await renderRoute({
      path: `/projects/helsinki/grafana/dashboards/${UID}/edit`,
      answer: answering({
        [`GET ${BUILDER}/${UID}`]: () => {
          reads += 1;
          return jsonResponse(SAVED);
        },
        [`PUT ${BUILDER}/${UID}`]: () => problem(409, "changed"),
      }),
    });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: en.builder.save }));
    await user.click(await screen.findByRole("button", { name: en.builder.reload }));
    await waitFor(() => expect(reads).toBe(2));
    await waitFor(() => expect(screen.queryByText(en.builder.conflictTitle)).not.toBeInTheDocument());
  });

  it("scrolls to the panel's form on a narrow screen when a panel is chosen", async () => {
    const scrolled = vi.fn();
    vi.stubGlobal("matchMedia", (query: string) => ({
      matches: query.includes("1279px"),
      media: query,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    }));
    Object.defineProperty(HTMLElement.prototype, "scrollIntoView", { configurable: true, writable: true, value: scrolled });
    await renderRoute({ path: `/projects/helsinki/grafana/dashboards/${UID}/edit`, answer: answering() });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: new RegExp("Free bikes") }));
    await waitFor(() => expect(scrolled).toHaveBeenCalled());
    expect(scrolled.mock.contexts.some((node) => (node as HTMLElement).id === "builder-panel-1-heading")).toBe(true);
  });
});

describe("the wide canvas", () => {
  it("selects a panel from its card and drops no panel when the drag is cancelled", async () => {
    vi.spyOn(HTMLDivElement.prototype, "clientWidth", "get").mockReturnValue(960);
    const onSelect = vi.fn();
    const onPlace = vi.fn();
    const second: BoardPanel = { ...PANEL, id: 2, title: "Free docks", grid: { x: 12, y: 0, w: 12, h: 8 } };
    const { container } = withClient(<BuilderCanvas panels={[PANEL, second]} selected={1} onSelect={onSelect} onPlace={onPlace} />);
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: new RegExp("Free docks") }));
    expect(onSelect).toHaveBeenCalledWith(2);
    const canvas = screen.getByRole("group", { name: en.builder.canvas });
    const handle = container.querySelector("[data-drag-handle]") as HTMLElement;
    fireEvent.pointerDown(handle, { clientX: 10, clientY: 10, pointerId: 1 });
    fireEvent.pointerMove(canvas, { clientX: 200, clientY: 200 });
    fireEvent.pointerCancel(canvas);
    fireEvent.pointerUp(canvas);
    expect(onPlace).not.toHaveBeenCalled();
  });
});
