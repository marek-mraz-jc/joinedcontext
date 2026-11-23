/** ErrorBoundary: the fallbacks under a broken i18n, crypto and storage, retry on a new page, reload (T-7585). */
import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { ErrorBoundary, errorReference, mayReloadForChunk } from "../src/components/ErrorBoundary";

afterEach(() => {
  vi.restoreAllMocks();
});

function Thrower({ fail }: { fail: boolean }) {
  if (fail) throw new Error("drawing failed");
  return <p>page is fine</p>;
}

describe("the crash panel", () => {
  it("falls back to English when the translations themselves fail", () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    vi.spyOn(i18n, "t").mockImplementation(() => {
      throw new Error("i18n down");
    });
    render(
      <ErrorBoundary>
        <Thrower fail />
      </ErrorBoundary>,
    );
    expect(screen.getByRole("alert")).toHaveTextContent("This page stopped");
    expect(screen.getByRole("button", { name: "Load the page again" })).toBeInTheDocument();
  });

  it("reloads the page from the button", () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const reload = vi.fn();
    vi.spyOn(window, "location", "get").mockReturnValue({ ...window.location, reload });
    render(
      <ErrorBoundary>
        <Thrower fail />
      </ErrorBoundary>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Load the page again" }));
    expect(reload).toHaveBeenCalledOnce();
  });

  it("clears itself when the page it guards is another one", () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const { rerender } = render(
      <ErrorBoundary resetKey="/a">
        <Thrower fail />
      </ErrorBoundary>,
    );
    expect(screen.getByRole("alert")).toBeInTheDocument();
    rerender(
      <ErrorBoundary resetKey="/b">
        <Thrower fail={false} />
      </ErrorBoundary>,
    );
    expect(screen.getByText("page is fine")).toBeInTheDocument();
  });
});

describe("the reference and the tab's storage", () => {
  it("is a time-based reference when crypto cannot make one", () => {
    vi.spyOn(crypto, "randomUUID").mockImplementation(() => {
      throw new Error("no crypto");
    });
    expect(errorReference()).toMatch(/^[0-9a-z]+$/);
  });

  it("does not reload for a chunk when the tab's storage is closed to the page", () => {
    vi.spyOn(window, "sessionStorage", "get").mockImplementation(() => {
      throw new Error("blocked");
    });
    expect(mayReloadForChunk(new TypeError("Failed to fetch dynamically imported module: /a.js"))).toBe(false);
  });
});
