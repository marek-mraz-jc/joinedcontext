import { afterEach, describe, expect, it, vi } from "vitest";
import { act, renderHook } from "@testing-library/react";
import { currentTokens } from "@joinedcontext/sdk";
import { DARK, LIGHT, prefersDark, startTokens, useScheme } from "./theme";

/** A system colour scheme the test can switch, as `matchMedia` reports it. */
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

afterEach(() => vi.unstubAllGlobals());

describe("light and dark", () => {
  it("starts in the scheme the system asks for", () => {
    system(true);
    expect(prefersDark()).toBe(true);
    expect(startTokens()).toBe(DARK);
    system(false);
    expect(startTokens()).toBe(LIGHT);
  });

  it("follows the system when it switches, and stops listening when gone", () => {
    const scheme = system(false);
    const { result, unmount } = renderHook(() => useScheme());
    expect(result.current).toBe("light");
    act(() => scheme.switchTo(true));
    expect(result.current).toBe("dark");
    expect(currentTokens().color.surface).toBe(DARK.color.surface);
    act(() => scheme.switchTo(false));
    expect(result.current).toBe("light");
    unmount();
    expect(scheme.listeners.size).toBe(0);
  });

  it("stays light where the browser cannot say", () => {
    vi.stubGlobal("matchMedia", undefined);
    expect(prefersDark()).toBe(false);
    const { result } = renderHook(() => useScheme());
    expect(result.current).toBe("light");
  });
});
