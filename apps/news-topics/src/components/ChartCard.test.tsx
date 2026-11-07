import { render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ProblemError } from "@joinedcontext/sdk";
import { ChartCard } from "./ChartCard";

const charts: Array<{ setOption: ReturnType<typeof vi.fn>; resize: ReturnType<typeof vi.fn>; dispose: ReturnType<typeof vi.fn> }> = [];
vi.mock("echarts", () => ({
  init: vi.fn(() => {
    const chart = { setOption: vi.fn(), resize: vi.fn(), dispose: vi.fn() };
    charts.push(chart);
    return chart;
  }),
}));

describe("ChartCard", () => {
  afterEach(() => {
    charts.length = 0;
    vi.unstubAllGlobals();
  });

  it("says what went wrong, that it is reading, or that there is nothing to chart, in place of the chart", () => {
    const view = render(<ChartCard title="Share" option={{}} error={new ProblemError(503, { title: "Unavailable" })} />);
    // A one-word title is a code, not a sentence: the SDK says what failed instead (T-4200).
    expect(screen.getByRole("alert")).toHaveTextContent("The data could not be read. Try again later.");
    view.rerender(<ChartCard title="Share" option={{}} loading />);
    expect(screen.getByRole("status")).toBeInTheDocument();
    view.rerender(<ChartCard title="Share" option={null} />);
    expect(screen.getByText("Nothing to chart yet.")).toBeInTheDocument();
    view.rerender(<ChartCard title="Share" option={null} empty="No week to show." />);
    expect(screen.getByText("No week to show.")).toBeInTheDocument();
    expect(charts).toEqual([]);
  });

  it("draws the option, follows the size of its box, and lets the chart go with it", () => {
    let resized: () => void = () => undefined;
    vi.stubGlobal(
      "ResizeObserver",
      class {
        constructor(callback: () => void) {
          resized = callback;
        }
        observe() {}
        disconnect() {}
      },
    );
    const view = render(<ChartCard title="Share" option={{ series: [] }} />);
    expect(screen.getByRole("img", { name: "Share" })).toBeInTheDocument();
    expect(charts[0].setOption).toHaveBeenCalledWith({ series: [] }, true);
    resized();
    expect(charts[0].resize).toHaveBeenCalledTimes(1);
    view.unmount();
    expect(charts[0].dispose).toHaveBeenCalled();
  });

  it("draws without a ResizeObserver too", () => {
    vi.stubGlobal("ResizeObserver", undefined);
    render(<ChartCard title="Share" option={{ series: [] }} />);
    expect(charts[0].setOption).toHaveBeenCalled();
  });
});
