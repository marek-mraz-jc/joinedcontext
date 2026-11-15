import { render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ChartCard } from "./ChartCard";

const chart = { setOption: vi.fn(), resize: vi.fn(), dispose: vi.fn() };
vi.mock("echarts", () => ({ init: vi.fn(() => chart) }));

describe("a chart card", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.clearAllMocks();
  });

  it("draws its option, follows its box's size, and lets the chart go when it leaves", () => {
    let resized: () => void = () => undefined;
    const observe = vi.fn();
    const disconnect = vi.fn();
    vi.stubGlobal(
      "ResizeObserver",
      class {
        constructor(callback: () => void) {
          resized = callback;
        }
        observe = observe;
        disconnect = disconnect;
      },
    );
    const { unmount } = render(<ChartCard title="History" option={{ series: [] }} height={200} loading={false} empty="Nothing" />);
    expect(screen.getByRole("img", { name: "History" })).toBeInTheDocument();
    expect(chart.setOption).toHaveBeenCalledWith({ series: [] }, true);
    expect(observe).toHaveBeenCalled();
    resized();
    expect(chart.resize).toHaveBeenCalled();
    unmount();
    expect(disconnect).toHaveBeenCalled();
    expect(chart.dispose).toHaveBeenCalled();
  });

  it("says it is loading, or that there is nothing to chart", () => {
    const { rerender } = render(<ChartCard title="History" option={{}} height={200} loading empty="Nothing" />);
    expect(screen.getByRole("status")).toBeInTheDocument();
    rerender(<ChartCard title="History" option={null} height={200} loading={false} empty="Nothing to chart" />);
    expect(screen.getByText("Nothing to chart")).toBeInTheDocument();
  });
});
