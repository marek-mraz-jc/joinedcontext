import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// T-3273: the inbox, answered from the browser like the approvals journey: one change waits, its
// diff opens in the list, and Approve decides it without leaving the page.

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-approver"],
};

const PROJECTS = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ProjectList",
  items: [{ name: "helsinki" }],
};

const PROPOSAL = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ChangeProposal",
  metadata: { name: "chg-1a2b3c4d", namespace: "helsinki" },
  summary: {
    key: "change.summary.update",
    params: { kind: "Endpoint", name: "air-quality", fields: 2 },
  },
  author: { name: "Marek Mráz", email: "marek@banskabystrica.sk" },
  createdAt: "2026-03-03T12:00:00Z",
  status: {
    lane: "yellow",
    phase: "PendingApproval",
    plan: { update: 1 },
    mergeRequest: "https://gitea.example/city/city-config/pulls/7",
  },
  planFields: [
    { path: "spec.audience", from: "public", to: "internal" },
    { path: "spec.credentials.token", from: "[REDACTED]", to: "[REDACTED]" },
  ],
};

const APPROVED = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: PROPOSAL.metadata,
  status: { lane: "yellow", phase: "Deploying", plan: { update: 1 } },
};

async function stubApi(page: Page, forgeDown?: string): Promise<{ approvals: string[] }> {
  const approvals: string[] = [];
  await page.route("**/api/v1/**", async (route) => {
    const url = new URL(route.request().url());
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });

    if (url.pathname.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (url.pathname.endsWith("/approve")) {
      approvals.push(url.pathname);
      return json(APPROVED, 202);
    }
    if (url.pathname === "/api/v1/projects") {
      return json(PROJECTS);
    }
    if (forgeDown && url.pathname === `/api/v1/projects/${forgeDown}/changes`) {
      return route.fulfill({
        status: 503,
        contentType: "application/problem+json",
        body: JSON.stringify({
          type: "about:blank",
          title: "Service Unavailable",
          status: 503,
          detail: "The Git repository did not answer the Portal. Try again in a minute; if it lasts, ask the operator to check the forge and the Portal's forge token.",
        }),
      });
    }
    if (url.pathname.endsWith("/changes")) {
      return json({
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "ChangeList",
        items: [PROPOSAL],
      });
    }
    if (url.pathname === "/api/v1/notifications") {
      return json({ unread: 0, items: [] });
    }
    if (url.pathname.endsWith(`/changes/${PROPOSAL.metadata.name}`)) {
      return json(PROPOSAL);
    }
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
  return { approvals };
}

test.describe("the inbox", () => {
  test("an approver decides a waiting change from the inbox, with its diff, without opening another page", async ({ page }) => {
    const { approvals } = await stubApi(page);
    await page.goto("/projects/helsinki/inbox?lang=en");
    await expect(page.getByRole("heading", { level: 1, name: "Inbox" })).toBeVisible();
    const waiting = page.getByTestId("inbox-decisions").getByRole("listitem");
    await expect(waiting).toHaveCount(1);
    await waiting.getByRole("button", { name: "Show what it changes" }).click();
    await expect(waiting.getByRole("cell", { name: "internal", exact: true })).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);
    await waiting.getByRole("button", { name: "Approve" }).click();
    await expect(waiting.getByRole("status")).toHaveText("Approved: it is being deployed.");
    expect(approvals).toEqual(["/api/v1/projects/helsinki/changes/chg-1a2b3c4d/approve"]);
    await expect(page).toHaveURL(/\/projects\/helsinki\/inbox/);
  });

  test("a project whose changes did not load says so, and the inbox still lists the other projects' (T-4966)", async ({ page }, testInfo) => {
    await stubApi(page, "praha");
    await page.route("**/api/v1/projects", (route) =>
      route.fulfill({ contentType: "application/json", body: JSON.stringify({ ...PROJECTS, items: [{ name: "helsinki" }, { name: "praha" }] }) }),
    );
    for (const width of [1440, 400]) {
      await page.setViewportSize({ width, height: 900 });
      await page.goto("/projects/helsinki/inbox?lang=en");
      const failed = page.getByRole("alert");
      await expect(failed).toContainText("The changes of praha did not load");
      await expect(failed).toContainText("ask the operator to check the forge");
      await expect(page.getByText("Nothing waits for your decision.")).toBeHidden();
      await expect(page.getByTestId("inbox-decisions").getByText("proposed by Marek Mráz in helsinki")).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await page.screenshot({ path: testInfo.outputPath(`inbox-forge-down-${width}.png`), fullPage: true });
    }
  });

  test("connector events reach the inbox and the bell with their own page, names only (T-5979)", async ({ page }, testInfo) => {
    await stubApi(page);
    const event = (id: number, kind: string, link: string, urn = "chg-1a2b3c4d") => ({
      id, kind, project: "helsinki", space: "", urn, commentId: null, author: "marek@hel.fi", authorName: "Marek Mráz",
      excerpt: "doprava-in-claude", link, createdAt: "2026-10-28T20:00:00Z", read: false,
    });
    await page.route("**/api/v1/notifications", (route) =>
      route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({
          unread: 3,
          items: [
            event(3, "change.opened", "/projects/helsinki/approvals/chg-1a2b3c4d"),
            event(2, "connector.pending", "/ai-connectors", "c-7f3k"),
            event(1, "connector.expiring", "/ai-connectors", "c-2m9q"),
          ],
        }),
      }),
    );
    for (const width of [1440, 400]) {
      await page.setViewportSize({ width, height: 900 });
      await page.goto("/projects/helsinki/inbox?lang=en");
      const events = page.getByTestId("inbox-connector-events");
      await expect(events.getByRole("link")).toHaveCount(3);
      await expect(events.getByRole("link", { name: /opened change chg-1a2b3c4d in helsinki through connector doprava-in-claude/ })).toHaveAttribute(
        "href",
        "/projects/helsinki/approvals/chg-1a2b3c4d",
      );
      expect(await axeViolations(page)).toEqual([]);
      await page.screenshot({ path: testInfo.outputPath(`inbox-connector-events-${width}.png`), fullPage: true });
      await page.getByRole("button", { name: /Notifications, \d+ unread/ }).click();
      await expect(page.getByRole("menuitem", { name: /asks for connector doprava-in-claude above the organization ceiling/ })).toBeVisible();
      await page.screenshot({ path: testInfo.outputPath(`bell-connector-events-${width}.png`) });
      await page.keyboard.press("Escape");
    }
  });
});
