/** Light and dark (T-3333): the tokens follow the reader's system, and a switch applies the other set. */
import { act, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { DARK, LIGHT, prefersDark, startTokens, useScheme } from "./theme";

function system(dark: boolean) {
  const listeners: Array<() => void> = [];
  const media = {
    matches: dark,
    addEventListener: (_: string, listener: () => void) => listeners.push(listener),
    removeEventListener: (_: string, listener: () => void) => listeners.splice(listeners.indexOf(listener), 1),
  };
  vi.stubGlobal("matchMedia", () => media);
  return {
    switchTo(next: boolean) {
      media.matches = next;
      for (const listener of [...listeners]) listener();
    },
    listeners,
  };
}

function Scheme() {
  return <p>{useScheme()}</p>;
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the colour scheme", () => {
  it("starts in the system's scheme and follows its switch, applying the other tokens", () => {
    const os = system(true);
    expect(prefersDark()).toBe(true);
    expect(startTokens()).toBe(DARK);
    const { unmount } = render(<Scheme />);
    expect(screen.getByText("dark")).toBeInTheDocument();
    act(() => os.switchTo(false));
    expect(screen.getByText("light")).toBeInTheDocument();
    expect(document.documentElement.style.getPropertyValue("--jc-color-surface")).toBe(LIGHT.color.surface);
    act(() => os.switchTo(true));
    expect(document.documentElement.style.getPropertyValue("--jc-color-surface")).toBe(DARK.color.surface);
    unmount();
    expect(os.listeners).toHaveLength(0);
  });

  it("is light where the browser cannot say", () => {
    vi.stubGlobal("matchMedia", undefined);
    expect(prefersDark()).toBe(false);
    expect(startTokens()).toBe(LIGHT);
    render(<Scheme />);
    expect(screen.getByText("light")).toBeInTheDocument();
  });
});
