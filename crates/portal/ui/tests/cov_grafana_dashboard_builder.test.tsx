
import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "../src/api/client";
import i18n from "../src/i18n";
import { BuilderCanvas, SaveProblem } from "../src/pages/grafana/DashboardBuilder";
import type { Board, BoardPanel, PanelKind } from "../src/pages/grafana/builderModel";

const SAMPLE_PANEL: BoardPanel = {
  id: 1,
  kind: "timeseries",
  title: "Bike count",
  description: "Sensors at stations",
  grid: { x: 0, y: 0, w: 12, h: 8 },
  endpoint: "bikes",
  type: "BikeHireDockingStation",
  attribute: "availableBikeNumber",
  aggregation: "avg",
  interval: "auto",
  filters: [{ attribute: "status", op: "eq", value: "open" }],
};

const SAMPLE_BOARD: Board = {
  title: "Bikes",
  description: "Bike station metrics",
  from: "now-6h",
  to: "now",
  refresh: "1m",
  variables: [
    {
      name: "statusVar",
      label: "Status filter",
      endpoint: "bikes",
      type: "BikeHireDockingStation",
      attribute: "status",
      includeAll: true,
    },
  ],
  panels: [SAMPLE_PANEL],
};

describe("DashboardBuilder subcomponents coverage", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("covers BuilderCanvas wide and phone layouts, dragging, resizing, and KindSketches", () => {
    let resizeCb: (() => void) | null = null;
    vi.stubGlobal(
      "ResizeObserver",
      class {
        constructor(cb: () => void) {
          resizeCb = cb;
        }
        observe() {}
        disconnect() {}
      },
    );

    const onSelect = vi.fn();
    const onPlace = vi.fn();

    const kinds: PanelKind[] = ["timeseries", "stat", "gauge", "bargauge", "piechart", "barchart", "text", "table"];
    const panels: BoardPanel[] = kinds.map((kind, idx) => ({
      ...SAMPLE_PANEL,
      id: idx + 1,
      kind,
      title: `Panel ${kind}`,
      grid: { x: (idx % 2) * 12, y: Math.floor(idx / 2) * 8, w: 12, h: 8 },
    }));

    // Wide layout: width >= 640
    vi.spyOn(HTMLDivElement.prototype, "clientWidth", "get").mockReturnValue(800);

    const { container, rerender } = render(
      <BuilderCanvas panels={panels} selected={1} onSelect={onSelect} onPlace={onPlace} />,
    );

    // Lines 304-306: ResizeObserver callback
    (resizeCb as (() => void) | null)?.();

    // Lines 324, 436-441: move drag handle pointer down
    const dragHandles = container.querySelectorAll("[data-drag-handle]");
    expect(dragHandles.length).toBeGreaterThan(0);
    const canvas = container.querySelector("[role='group']");
    expect(canvas).not.toBeNull();

    fireEvent.pointerDown(dragHandles[0], { clientX: 10, clientY: 10, pointerId: 1 });
    // Line 453: pointer move
    fireEvent.pointerMove(canvas!, { clientX: 60, clientY: 60 });
    // Lines 455-456: pointer up
    fireEvent.pointerUp(canvas!);
    expect(onPlace).toHaveBeenCalled();

    // Lines 339, 458: resize handle pointer down and cancel
    const resizeHandles = container.querySelectorAll("[data-resize-handle]");
    expect(resizeHandles.length).toBeGreaterThan(0);
    fireEvent.pointerDown(resizeHandles[0], { clientX: 10, clientY: 10, pointerId: 1 });
    fireEvent.pointerCancel(canvas!);

    // Lines 478-484: Phone / narrow layout: width < 640
    vi.spyOn(HTMLDivElement.prototype, "clientWidth", "get").mockReturnValue(320);
    rerender(<BuilderCanvas panels={panels} selected={1} onSelect={onSelect} onPlace={onPlace} />);
    const cardButtons = screen.getAllByRole("button", { pressed: true });
    expect(cardButtons.length).toBeGreaterThan(0);
  });

  it("covers SaveProblem field link button clicking onGo", () => {
    const onGo = vi.fn();
    const error = new ApiError(422, "Refusal", {
      type: "https://joinedcontext.com/problems/invalid",
      title: "Refusal",
      status: 422,
      errors: [
        { path: "panels[0].title", message: "panel title is invalid" },
        { path: "title", message: "board title is missing" },
      ],
    });

    render(<SaveProblem error={error} board={SAMPLE_BOARD} onGo={onGo} />);
    const linkBtn = screen.getByRole("button", { name: /Panel “Bike count”, Title:/ });
    fireEvent.click(linkBtn);
    expect(onGo).toHaveBeenCalledWith("panels[0].title");
  });
});
