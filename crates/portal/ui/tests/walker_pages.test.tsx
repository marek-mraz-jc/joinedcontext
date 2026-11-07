/**
 * T-2730: every list and top-level page of the router, walked with the API mocked (UI-01, UI-15, UI-16, UI-84).
 *
 * The walk and its address book are `walkerHarness.tsx`; the three walker files split the
 * addresses so vitest walks them side by side. The live walker on dev is
 * `e2e/live/walker.spec.ts`; what is owed today is `walker.allow.json`, one task per entry.
 */
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { cleanup } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { routerPaths } from "./gates";
import { ADDRESSES, DEVELOPMENT_ONLY, addresses, allow, expectWalked } from "./walkerHarness";

const ui = resolve(dirname(fileURLToPath(import.meta.url)), "..");

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  window.localStorage.clear();
  window.sessionStorage.clear();
});

describe("the page walker's address book (T-2730)", () => {
  it("names at least one address for every route the router renders", () => {
    const paths = routerPaths(readFileSync(join(ui, "src/router.tsx"), "utf8")).filter((path) => !DEVELOPMENT_ONLY.includes(path));
    expect(paths.filter((path) => !(path in ADDRESSES)), "add the new route's addresses to ADDRESSES").toEqual([]);
    expect(Object.keys(ADDRESSES).filter((path) => !paths.includes(path)), "the router has no such route any more").toEqual([]);
  });

  it("excuses a finding only with an open task and a reason", () => {
    for (const entry of allow.excused) {
      expect(entry.task, `${entry.route} ${entry.check}`).toMatch(/^T-\d{4}$/);
      expect(entry.why.length, `${entry.route} ${entry.check} says why`).toBeGreaterThan(20);
      expect(Object.values(ADDRESSES).flat(), `${entry.route} is an address the walker opens`).toContain(entry.route);
    }
  });

  it("walks every address exactly once across the three files", () => {
    const walked = [...addresses("pages"), ...addresses("forms"), ...addresses("details")].sort();
    expect(walked).toEqual(Object.values(ADDRESSES).flat().sort());
  });
});

describe("every pages page, as the walker finds it (T-2730)", () => {
  it.each(addresses("pages"))("%s", expectWalked);
});
