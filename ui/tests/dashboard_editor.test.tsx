/**
 * A dashboard App edited without code (T-6402): the editor offers only the attributes the server
 * listed, a computed column takes a formula, the colour rule, chart and tiles are picked from the
 * same names, Preview shows the file before and after, Propose opens the App's Change, and a
 * refusal of the server is shown beside the field it names.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { DashboardEditor } from "../src/pages/apps/DashboardEditor";
import { errorsByPath, lineDiff, readableNames, refusedField, typedValue } from "../src/pages/apps/dashboardForm";
import en from "../src/locales/en.json";
import { expectNoViolations } from "./checks";

const ROUTE = "/api/v1/projects/sharing/apps/board/dashboard";
const words = en.apps.dashboard;
const FILE = {
  grid: { source: { kind: "endpoint", slug: "bikes" }, type: "BikeHireDockingStation", columns: [{ attr: "name" }], history: { enabled: true } },
};
const offered = (overrides: Record<string, unknown> = {}) => ({
  path: "src/dashboard.json",
  config: FILE,
  entityType: "BikeHireDockingStation",
  attributes: ["name", "availableBikeNumber", "totalSlotNumber"].map((name) => ({ name, kind: "Property", required: false })),
  edit: { allowed: true },
  ...overrides,
});

type Reply = { status: number; body: unknown };

function renderEditor(read: Reply, write: (body: { config: Record<string, unknown>; dryRun: boolean }) => Reply) {
  const sent: { config: Record<string, unknown>; dryRun: boolean }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input instanceof Request ? input : new Request(new URL(String(input), window.location.origin));
      const url = new URL(request.url);
      const reply = ({ status, body }: Reply) =>
        new Response(JSON.stringify(body), { status, headers: { "Content-Type": status < 300 ? "application/json" : "application/problem+json" } });
      if (url.pathname === ROUTE && request.method === "GET") return reply(read);
      if (url.pathname === ROUTE && request.method === "POST") {
        const body = (await request.json()) as { config: Record<string, unknown>; dryRun: boolean };
        sent.push(body);
        return reply(write(body));
      }
      return reply({ status: 404, body: { title: "Not Found", status: 404 } });
    }),
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const view = render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <DashboardEditor project="sharing" name="board" />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return { sent, view };
}

describe("the dashboard editor", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("edits a column, a computed column, a filter, a colour rule, a chart and a tile, previews and proposes the Change", async () => {
    const user = userEvent.setup();
    const { sent, view } = renderEditor({ status: 200, body: offered() }, (body) =>
      body.dryRun
        ? { status: 200, body: { before: '{\n  "grid": {}\n}\n', after: '{\n  "grid": {},\n  "kpis": []\n}\n' } }
        : { status: 202, body: { before: "", after: "", change: { apiVersion: "v1", kind: "Change", metadata: { name: "chg-0a1b2c3d", namespace: "sharing" }, status: {} } } },
    );
    // Every attribute dropdown holds the server's three names and nothing else.
    const first = await screen.findByLabelText(words.attribute, { selector: "#col-0-attr" });
    expect(within(first).getAllByRole("option").map((o) => o.textContent)).toEqual([words.choose, "name", "availableBikeNumber", "totalSlotNumber"]);
    expect(screen.getByText(words.attributeHint)).toBeInTheDocument();

    await user.type(screen.getByLabelText(words.label, { selector: "#col-0-label" }), "Station");
    await user.click(screen.getByRole("button", { name: words.addComputed }));
    await user.type(screen.getByLabelText(words.computedName), "occupancy");
    await user.click(screen.getByLabelText(words.formula));
    await user.paste("availableBikeNumber / totalSlotNumber * 100");
    await user.click(screen.getByRole("button", { name: words.addFilter }));
    await user.selectOptions(screen.getByLabelText(words.attribute, { selector: "#filter-0-attr" }), "totalSlotNumber");
    await user.selectOptions(screen.getByLabelText(words.op), ">");
    await user.type(screen.getByLabelText(words.value), "0");
    // The computed column is offered where a value is read, after the stored attributes.
    await user.selectOptions(screen.getByLabelText(words.colourAttr), "occupancy");
    await user.click(screen.getByRole("button", { name: words.addBand }));
    await user.type(screen.getByLabelText(words.below, { selector: "#band-0-below" }), "10");
    await user.type(screen.getByLabelText(words.label, { selector: "#band-0-label" }), "Almost empty");
    await user.selectOptions(screen.getByLabelText(words.chartX), "name");
    await user.selectOptions(screen.getByLabelText(words.chartY), "availableBikeNumber");
    await user.click(screen.getByRole("button", { name: words.addKpi }));
    await user.type(screen.getByLabelText(words.label, { selector: "#kpi-0-label" }), "Empty stations");
    await user.click(screen.getByLabelText(words.where));
    await user.paste("availableBikeNumber == 0");

    expect(screen.getByRole("button", { name: words.propose })).toHaveAttribute("aria-disabled", "true");
    await user.click(screen.getByRole("button", { name: words.preview }));
    expect(await screen.findByText(words.previewTitle.replace("{path}", "src/dashboard.json"))).toBeInTheDocument();
    expect(sent[0].dryRun).toBe(true);
    expect(sent[0].config).toEqual({
      grid: {
        ...FILE.grid,
        columns: [{ attr: "name", label: "Station" }, { attr: "occupancy", formula: "availableBikeNumber / totalSlotNumber * 100" }],
      },
      filters: [{ attr: "totalSlotNumber", op: ">", value: 0 }],
      colour: { attr: "occupancy", bands: [{ below: 10, tone: "danger", label: "Almost empty" }, { tone: "success", label: words.defaultBand }] },
      chart: { agg: "sum", x: "name", y: "availableBikeNumber" },
      kpis: [{ label: "Empty stations", agg: "count", where: "availableBikeNumber == 0" }],
    });
    await expectNoViolations(view.container);

    await user.click(screen.getByRole("button", { name: words.propose }));
    await waitFor(() => expect(sent).toHaveLength(2));
    expect(sent[1]).toEqual({ ...sent[0], dryRun: false });
    expect(await screen.findByText(/chg-0a1b2c3d/)).toBeInTheDocument();
  });

  it("shows each refusal of the server beside the field it names", async () => {
    const user = userEvent.setup();
    renderEditor({ status: 200, body: offered() }, () => ({
      status: 422,
      body: {
        title: "Unprocessable",
        status: 422,
        detail: "refused",
        errors: [{ path: "config/grid/columns/0/attr", message: '"name" is no attribute of this type you may read' }],
      },
    }));
    await screen.findByLabelText(words.attribute, { selector: "#col-0-attr" });
    await user.click(screen.getByRole("button", { name: words.preview }));
    expect(await screen.findByText('"name" is no attribute of this type you may read')).toBeInTheDocument();
    expect(screen.getByLabelText(words.attribute, { selector: "#col-0-attr" })).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByText(words.refused)).toBeInTheDocument();
    expect(screen.getByText(`Columns, row 1, Attribute: "name" is no attribute of this type you may read`)).toBeInTheDocument();
    (document.activeElement as HTMLElement | null)?.blur();
    await user.click(screen.getByRole("button", { name: words.goTo }));
    expect(screen.getByLabelText(words.attribute, { selector: "#col-0-attr" })).toHaveFocus();
  });

  it("says why when the App cannot be edited here", async () => {
    renderEditor({ status: 200, body: offered({ config: null, edit: { allowed: false, reason: "Editing the dashboard needs propose on App in project sharing" } }) }, () => ({ status: 500, body: {} }));
    expect(await screen.findByText(words.unavailable)).toBeInTheDocument();
    expect(screen.getByText("Editing the dashboard needs propose on App in project sharing")).toBeInTheDocument();
  });
});

describe("the dashboard form", () => {
  it("reads the server's refusals by the path they name", () => {
    const by = errorsByPath(["config/grid/columns/0/formula: no call", "config: too big", "plain"]);
    expect(by.get("/grid/columns/0/formula")).toEqual(["no call"]);
    expect(by.get("")).toEqual(["too big", "plain"]);
  });
  it("names the form field each refusal path points at", () => {
    expect(refusedField("/kpis/0/where")).toEqual({ id: "kpi-0-where", section: "kpis", row: 1, field: "where" });
    expect(refusedField("/grid/columns/2/formula")).toEqual({ id: "col-2-formula", section: "columns", row: 3, field: "formula" });
    expect(refusedField("/filters/1")).toEqual({ id: undefined, section: "filters", row: 2, field: undefined });
    expect(refusedField("/chart/y")?.id).toBe("chart-y");
    expect(refusedField("/grid/source")).toBeUndefined();
  });
  it("types a filter value, offers computed columns after the attributes and diffs line by line", () => {
    expect([typedValue("0"), typedValue("true"), typedValue("working"), typedValue("")]).toEqual([0, true, "working", ""]);
    expect(readableNames(["a"], { grid: { columns: [{ attr: "a" }, { attr: "pct", formula: "a" }] } })).toEqual(["a", "pct"]);
    expect(lineDiff("a\nb\nc", "a\nx\nc").map((l) => `${l.sign}${l.text}`)).toEqual([" a", "+x", "-b", " c"]);
  });
});
