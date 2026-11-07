import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { DARK, LIGHT, prefersDark, REACH, reachColours, startTokens, useScheme } from "./theme";

/** A system that asks for `dark`, and tells its listeners when it changes. */
function system(dark: boolean) {
  const listeners: Array<() => void> = [];
  const media = {
    matches: dark,
    addEventListener: (_: string, listener: () => void) => listeners.push(listener),
    removeEventListener: vi.fn(),
  };
  vi.stubGlobal("matchMedia", () => media);
  window.matchMedia = (() => media) as unknown as typeof window.matchMedia;
  return {
    media,
    turn(next: boolean) {
      media.matches = next;
      for (const listener of listeners) listener();
    },
  };
}

afterEach(() => {
  vi.unstubAllGlobals();
  Reflect.deleteProperty(window, "matchMedia");
});

describe("the scheme", () => {
  it("is light without a system to ask, and follows the system's choice", () => {
    Reflect.deleteProperty(window, "matchMedia");
    expect(prefersDark()).toBe(false);
    expect(reachColours()).toBe(REACH.light);
    expect(startTokens()).toBe(LIGHT);
    system(true);
    expect(prefersDark()).toBe(true);
    expect(reachColours()).toBe(REACH.dark);
    expect(startTokens()).toBe(DARK);
  });

  it("changes when the system switches, and stops listening when it is gone", () => {
    const { media, turn } = system(false);
    const { result, unmount } = renderHook(() => useScheme());
    expect(result.current).toBe("light");
    act(() => turn(true));
    expect(result.current).toBe("dark");
    act(() => turn(false));
    expect(result.current).toBe("light");
    unmount();
    expect(media.removeEventListener).toHaveBeenCalled();
  });

  it("keeps light when there is no system to listen to", () => {
    Reflect.deleteProperty(window, "matchMedia");
    const { result } = renderHook(() => useScheme());
    expect(result.current).toBe("light");
  });
});
