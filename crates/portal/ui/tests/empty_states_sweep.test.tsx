/**
 * T-3246: every list page of a project, holding nothing, says what belongs there and offers the
 * first action: an empty state with a title, one sentence and an action, never a blank table.
 */
import { screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { renderRoute } from "./pageHarness";

const PAGES = [
  "approvals",
  "workspaces",
  "models",
  "knowledge",
  "mcp",
  "shared",
  "spaces",
  "endpoints",
  "pipelines",
  "policies",
  "subscriptions",
  "csrs",
  "datasources",
  "dashboards",
  "apps",
  "syncsources",
  "roles",
  "mappings",
  "settings/members",
  "settings/roles",
  "settings/service-accounts",
];

describe("an empty project list", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it.each(PAGES)("/projects/helsinki/%s says what belongs there and offers the first step", async (page) => {
    await renderRoute({ path: `/projects/helsinki/${page}` });
    const empties = await waitFor(
      () => {
        const found = [...document.querySelectorAll<HTMLElement>("[data-empty-state]")];
        expect(found.length, "an empty state").toBeGreaterThan(0);
        return found;
      },
      { timeout: 5_000 },
    );
    for (const empty of empties) {
      const title = empty.querySelector("p, h1, h2")?.textContent ?? "";
      const sentence = empty.querySelector("p + p, h1 + p, h2 + p");
      expect(sentence?.textContent?.trim(), `"${title}" says what belongs there`).toBeTruthy();
      expect(empty.querySelector("a, button"), `"${title}" offers the first step`).not.toBeNull();
    }
    expect(screen.queryAllByRole("alert")).toEqual([]);
  });
});
