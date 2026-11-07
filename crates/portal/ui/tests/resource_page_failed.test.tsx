/**
 * T-2834: a resource page that cannot read its resource keeps its heading, its purpose line and a
 * way back, and offers Retry only where asking again can help (UI-01, UI-15, UI-16). A 404 and a
 * 403 answer the same the second time; a 503 may not.
 */
import { cleanup, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import en from "../src/locales/en.json";
import { problem, renderRoute } from "./pageHarness";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const P = "/projects/helsinki";
const MISSING = new Set(["air", "air-v2", "chg-0a1b2c3d", "p-1", "stewards"]);

/** Each page, with the way back it offers. */
const PAGES: [string, string][] = [
  [`${P}/endpoints/air`, en.endpoints.page.back],
  [`${P}/spaces/air`, en.spaces.inside.back],
  [`${P}/approvals/chg-0a1b2c3d`, en.approvals.back],
  [`${P}/workspaces/air-v2/try-it`, en.workspaces.compare.back],
  [`${P}/workspaces/air-v2/bring-back`, en.workspaces.compare.back],
  [`${P}/apps/air`, en.apps.back],
  [`${P}/apps/air/open`, en.apps.back],
  ["/organization/people/p-1", en.organization.people.back],
  ["/organization/groups/stewards", en.access.groupPage.back],
];

function failing(status: number) {
  return (path: string) => {
    const last = decodeURIComponent(path.split("/").pop() ?? "");
    return MISSING.has(last) || path.endsWith("/agent-runs") ? problem(status, "The store answered no.") : undefined;
  };
}

async function settled(): Promise<HTMLElement> {
  const alert = await screen.findByRole("alert", {}, { timeout: 4000 });
  const headings = screen.getAllByRole("heading", { level: 1 });
  expect(headings).toHaveLength(1);
  const purpose = headings[0].nextElementSibling;
  expect(purpose?.tagName).toBe("P");
  expect((purpose?.textContent ?? "").length).toBeGreaterThan(10);
  return alert;
}

/** The page's own way back; the shell's navigation may carry a link of the same words. */
function back(label: string): HTMLElement {
  const own = [
    ...screen.queryAllByRole("link", { name: label }),
    ...screen.queryAllByRole("button", { name: label }),
  ].filter((control) => !control.closest("nav"));
  expect(own).toHaveLength(1);
  return own[0];
}

describe("a resource page that cannot read its resource", () => {
  it.each(PAGES)("%s: a 404 keeps the heading and the way back, and offers no Retry", async (path, label) => {
    await renderRoute({ path, answer: failing(404) });
    const alert = await settled();
    expect(back(label)).toBeInTheDocument();
    expect(within(alert).queryByRole("button", { name: en.app.error.retry })).toBeNull();
  });

  it.each(PAGES)("%s: a 503 says so and offers Retry, with the way back still there", async (path, label) => {
    await renderRoute({ path, answer: failing(503) });
    const alert = await settled();
    expect(within(alert).getByText(/The store answered no\./)).toBeInTheDocument();
    expect(within(alert).getByRole("button", { name: en.app.error.retry })).toBeInTheDocument();
    expect(back(label)).toBeInTheDocument();
  });

  it("a model page whose list fails keeps its heading; a model nobody holds says so under it", async () => {
    await renderRoute({
      path: `${P}/models/air`,
      answer: (path) => (path.endsWith("/datamodels") ? problem(503, "The store answered no.") : undefined),
    });
    const alert = await settled();
    expect(within(alert).getByRole("button", { name: en.app.error.retry })).toBeInTheDocument();
    expect(back(en.models.page.back)).toBeInTheDocument();

    cleanup();
    await renderRoute({ path: `${P}/models/air` });
    await waitFor(() => expect(screen.getByText(en.models.page.notFoundLead)).toBeInTheDocument());
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(back(en.models.page.back)).toBeInTheDocument();
  });
});
