import { renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { useIsPhone } from "../src/phone";

describe("useIsPhone", () => {
  const before = window.matchMedia;
  afterEach(() => {
    window.matchMedia = before;
  });

  it("follows the narrow media query, and is false where the browser has none", () => {
    window.matchMedia = ((query: string) => ({ matches: query.includes("max-width"), addEventListener() {}, removeEventListener() {} })) as unknown as typeof window.matchMedia;
    expect(renderHook(() => useIsPhone()).result.current).toBe(true);
    window.matchMedia = undefined as unknown as typeof window.matchMedia;
    expect(renderHook(() => useIsPhone()).result.current).toBe(false);
  });
});
