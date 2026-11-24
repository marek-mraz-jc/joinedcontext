/** T-7589: coverage for DashboardBuilder components and editor flows. */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import en from "../src/locales/en.json";
import i18n from "../src/i18n";
import { BoardSettings, BuilderCanvas, DisplayEditor, VariablesEditor } from "../src/pages/grafana/DashboardBuilder";
import type { Board, BoardPanel } from "../src/pages/grafana/builderModel";
import { jsonResponse, renderRoute } from "./pageHarness";

const BUILDER = "/api/v1/projects/helsinki/grafana/builder";
const UID = "jcd-0123456789abcdef";
const START = {
  mayEdit: true,
  pending: [],
  refreshes: ["", "1m", "5m"],
  units: ["", "none", "percent"],
  colors: ["green", "red", "yellow"],
};
const OPTIONS = {
  endpoints: [{ name: "bikes", title: "Bike stations", slug: "bikeslug", public: true, usable: true }],
  types: ["BikeHireDockingStation"],
  attributes: [
    { name: "availableBikeNumber", type: "number", kinds: ["timeseries", "stat", "table"] },
    { name: "status", type: "string", kinds: ["table"] },
    { name: "location", type: "geo", kinds: ["table", "map"] },
  ],
};
const PANEL: BoardPanel = {
  id: 1,
  kind: "stat",
  title: "Free bikes",
  description: "Sensors",
  grid: { x: 0, y: 0, w: 12, h: 8 },
  endpoint: "bikes",
  type: "BikeHireDockingStation",
  attribute: "availableBikeNumber",
  aggregation: "none",
  interval: "auto",
};
const SAVED = {
  uid: UID,
  base: "blob-1",
  board: { title: "Bikes", from: "now-6h", to: "now", refresh: "", variables: [], panels: [PANEL] },
};

describe("DashboardBuilder components coverage pass 2", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("handles BoardSettings description, time range from and refresh select changes", () => {
    const onChange = vi.fn();
    const board: Board = {
      title: "Bikes",
      description: "Old description",
      from: "now-6h",
      to: "now",
      refresh: "1m",
      panels: [],
      variables: [],
    };

    render(
      <I18nextProvider i18n={i18n}>
        <BoardSettings board={board} start={START} onChange={onChange} />
      </I18nextProvider>,
    );

    const desc = screen.getByLabelText(en.builder.field.description);
    fireEvent.change(desc, { target: { value: "New description" } });
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ description: "New description" }));

    const from = screen.getByLabelText(en.builder.field.from);
    fireEvent.change(from, { target: { value: "now-1h" } });
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ from: "now-1h", to: "now" }));

    const refresh = screen.getByLabelText(en.builder.field.refresh);
    fireEvent.change(refresh, { target: { value: "5m" } });
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ refresh: "5m" }));
  });

  it("handles VariablesEditor adding, editing and removing variables", async () => {
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const onChange = vi.fn();
    const board: Board = {
      title: "Bikes",
      panels: [],
      variables: [
        { name: "v1", label: "Var 1", endpoint: "bikes", type: "BikeHireDockingStation", attribute: "status", includeAll: true },
      ],
    };

    vi.stubGlobal("fetch", vi.fn(async () => jsonResponse(OPTIONS)));

    render(
      <QueryClientProvider client={queryClient}>
        <I18nextProvider i18n={i18n}>
          <VariablesEditor project="helsinki" board={board} onChange={onChange} />
        </I18nextProvider>
      </QueryClientProvider>,
    );

    const nameInput = screen.getByLabelText(en.builder.field.variableName);
    fireEvent.change(nameInput, { target: { value: "v1_renamed" } });
    expect(onChange).toHaveBeenCalledWith(
      expect.objectContaining({
        variables: [expect.objectContaining({ name: "v1_renamed" })],
      }),
    );

    const labelInput = screen.getByLabelText(en.builder.field.variableLabel);
    fireEvent.change(labelInput, { target: { value: "New Label" } });
    expect(onChange).toHaveBeenCalledWith(
      expect.objectContaining({
        variables: [expect.objectContaining({ label: "New Label" })],
      }),
    );

    const includeAll = screen.getByRole("checkbox", { name: en.builder.includeAll });
    fireEvent.click(includeAll);
    expect(onChange).toHaveBeenCalledWith(
      expect.objectContaining({
        variables: [expect.objectContaining({ includeAll: false })],
      }),
    );

    const addBtn = screen.getByRole("button", { name: en.builder.addVariable });
    fireEvent.click(addBtn);
    expect(onChange).toHaveBeenCalledWith(
      expect.objectContaining({
        variables: expect.arrayContaining([expect.objectContaining({ name: "var2" })]),
      }),
    );

    const removeBtn = screen.getByRole("button", { name: en.builder.removeVariable.replace("{name}", "v1") });
    fireEvent.click(removeBtn);
    expect(onChange).toHaveBeenCalledWith(
      expect.objectContaining({
        variables: [],
      }),
    );
  });

  it("handles DisplayEditor unit, decimals, thresholds base and steps", () => {
    const onChange = vi.fn();
    const panel: BoardPanel = {
      ...PANEL,
      kind: "gauge",
      unit: "none",
      decimals: 2,
      min: 0,
      max: 100,
      thresholds: {
        base: "green",
        steps: [{ value: 80, color: "red" }],
      },
    };

    render(
      <I18nextProvider i18n={i18n}>
        <DisplayEditor panel={panel} start={START} onChange={onChange} />
      </I18nextProvider>,
    );

    const unitSelect = screen.getByLabelText(en.builder.field.unit);
    fireEvent.change(unitSelect, { target: { value: "percent" } });
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ unit: "percent" }));

    const decSelect = screen.getByLabelText(en.builder.field.decimals);
    fireEvent.change(decSelect, { target: { value: "1" } });
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ decimals: 1 }));
    fireEvent.change(decSelect, { target: { value: "" } });
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ decimals: undefined }));

    const minInput = screen.getByLabelText(en.builder.field.min);
    fireEvent.change(minInput, { target: { value: "10" } });
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ min: 10 }));

    const baseColorSelect = screen.getByRole("combobox", { name: en.builder.baseColour });
    fireEvent.change(baseColorSelect, { target: { value: "yellow" } });
    expect(onChange).toHaveBeenCalledWith(
      expect.objectContaining({
        thresholds: expect.objectContaining({ base: "yellow" }),
      }),
    );

    const stepValInput = screen.getByRole("spinbutton", { name: en.builder.stepFrom });
    fireEvent.change(stepValInput, { target: { value: "85" } });
    expect(onChange).toHaveBeenCalledWith(
      expect.objectContaining({
        thresholds: expect.objectContaining({ steps: [expect.objectContaining({ value: 85 })] }),
      }),
    );

    const stepColorSelect = screen.getByRole("combobox", { name: en.builder.stepColour });
    fireEvent.change(stepColorSelect, { target: { value: "yellow" } });
    expect(onChange).toHaveBeenCalled();

    const addStepBtn = screen.getByRole("button", { name: en.builder.addStep });
    fireEvent.click(addStepBtn);
    expect(onChange).toHaveBeenCalledWith(
      expect.objectContaining({
        thresholds: expect.objectContaining({
          steps: expect.arrayContaining([expect.objectContaining({ value: 90 })]),
        }),
      }),
    );

    const removeStepBtn = screen.getByRole("button", { name: en.builder.removeStep });
    fireEvent.click(removeStepBtn);
    expect(onChange).toHaveBeenCalledWith(
      expect.objectContaining({
        thresholds: expect.objectContaining({ steps: [] }),
      }),
    );

    const useThresholds = screen.getByRole("checkbox", { name: en.builder.useThresholds });
    fireEvent.click(useThresholds);
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ thresholds: undefined }));
  });

  it("handles BuilderCanvas when ResizeObserver is undefined", () => {
    vi.stubGlobal("ResizeObserver", undefined);

    const { container } = render(
      <I18nextProvider i18n={i18n}>
        <BuilderCanvas panels={[PANEL]} selected={1} onSelect={vi.fn()} onPlace={vi.fn()} />
      </I18nextProvider>,
    );
    expect(container.querySelector("[role='group']")).toBeInTheDocument();

  });

  it("handles panel removal inside DashboardBuilderPage and Canvas card click", async () => {
    const user = userEvent.setup();
    await renderRoute({
      path: `/projects/helsinki/grafana/dashboards/${UID}/edit`,
      answer: (path, request) => {
        if (path === BUILDER && request.method === "GET") return jsonResponse(START);
        if (path === `${BUILDER}/${UID}` && request.method === "GET") return jsonResponse(SAVED);
        if (path.endsWith("/grafana/options")) return jsonResponse(OPTIONS);
        return undefined;
      },
    });

    const cardBtn = await screen.findByRole("button", { name: /Free bikes/i });
    await user.click(cardBtn);

    const removeBtn = screen.getByRole("button", { name: en.builder.removePanel });
    await user.click(removeBtn);

    const confirmBtn = screen.getByRole("button", { name: en.builder.removePanel });
    await user.click(confirmBtn);

    expect(screen.getByText(en.builder.emptyTitle)).toBeInTheDocument();
  });
});
