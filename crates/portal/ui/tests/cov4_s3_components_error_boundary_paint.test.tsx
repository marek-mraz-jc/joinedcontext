// T-7706 slice 3: reloadForChunk with no storage (ErrorBoundary.tsx) and Tinted with no colour (paint.tsx).
import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { reloadForChunk } from "../src/components/ErrorBoundary";
import { Tinted } from "../src/components/panels/paint";

describe("reloadForChunk without storage", () => {
  it("does not reload when the tab has no sessionStorage", () => {
    const spy = vi.spyOn(window, "sessionStorage", "get").mockImplementation(() => {
      throw new Error("blocked");
    });
    const reload = vi.fn();
    expect(reloadForChunk(undefined, reload)).toBe(false);
    expect(reload).not.toHaveBeenCalled();
    spy.mockRestore();
  });
});

describe("Tinted without a colour", () => {
  it("renders the plain text in a span with the class and no style", () => {
    render(<Tinted className="plain">hello</Tinted>);
    const span = screen.getByText("hello");
    expect(span).toHaveClass("plain");
    expect(span.getAttribute("style")).toBeNull();
  });
});
