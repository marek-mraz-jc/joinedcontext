/**
 * T-2730: every create and edit form at its own address, walked with the API mocked (UI-01, UI-15, UI-16, UI-84).
 *
 * The walk and its address book are `walkerHarness.tsx`; the three walker files split the
 * addresses so vitest walks them side by side. The live walker on dev is
 * `e2e/live/walker.spec.ts`; what is owed today is `walker.allow.json`, one task per entry.
 */
import { cleanup } from "@testing-library/react";
import { afterEach, describe, it, vi } from "vitest";
import { addresses, expectWalked } from "./walkerHarness";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  window.localStorage.clear();
  window.sessionStorage.clear();
});

describe("every forms page, as the walker finds it (T-2730)", () => {
  it.each(addresses("forms"))("%s", expectWalked);
});
