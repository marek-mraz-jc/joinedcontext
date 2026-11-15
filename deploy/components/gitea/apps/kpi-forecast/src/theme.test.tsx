import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { DARK, LIGHT, SERIES, prefersDark, seriesColours, startTokens, useScheme } from "./theme";

/** A system that prefers dark or light, and can switch while the page is open. */
function system(dark: boolean) {
  const listeners = new Set<() => void>();
  const media = {
    matches: dark,
    addEventListener: (_: string, listener: () => void) => listeners.add(listener),
    removeEventListener: (_: string, listener: () => void) => listeners.delete(listener),
  };
  vi.stubGlobal("matchMedia", () => media);
  return {
    switchTo(next: boolean) {
      media.matches = next;
      for (const listener of listeners) listener();
    },
    listeners,
  };
}

describe("light and dark", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("follows the system's preference for the tokens and the chart's colours", () => {
    system(true);
    expect(prefersDark()).toBe(true);
    expect(startTokens()).toBe(DARK);
    expect(seriesColours()).toBe(SERIES.dark);
    system(false);
    expect(startTokens()).toBe(LIGHT);
    expect(seriesColours()).toBe(SERIES.light);
  });

  it("is light where the browser cannot tell", () => {
    vi.stubGlobal("matchMedia", undefined);
    expect(prefersDark()).toBe(false);
    const { result } = renderHook(() => useScheme());
    expect(result.current).toBe("light");
  });

  it("switches with the system, applies the other tokens, and stops listening when gone", () => {
    const sys = system(false);
    const { result, unmount } = renderHook(() => useScheme());
    expect(result.current).toBe("light");
    act(() => sys.switchTo(true));
    expect(result.current).toBe("dark");
    expect(document.documentElement.style.getPropertyValue("--jc-color-surface")).toBe(DARK.color.surface);
    act(() => sys.switchTo(false));
    expect(document.documentElement.style.getPropertyValue("--jc-color-surface")).toBe(LIGHT.color.surface);
    unmount();
    expect(sys.listeners.size).toBe(0);
  });
});
