import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { pageFindings } from "../tests/pageChecks";
import { axeViolations } from "./axe";

// The same shape as the other journeys: the built bundle, the API answered from the browser.
// The live tail is an EventSource, so the stream route is fulfilled with a body that holds one
// event and then stays open, which is what the Portal sends.

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

const LOADED = {
  time: "2026-09-16T09:00:00Z",
  project: "helsinki",
  space: "mobility",
  kind: "config.applied",
  source: "reconciler",
  summary: "The repository at 1a2b3c4 is live: 42 manifests.",
  severity: "info",
  details: { object: "spaces/mobility" },
};

const FORWARDED = {
  time: "2026-09-16T09:05:00Z",
  project: "helsinki",
  space: "mobility",
  kind: "federation.forward",
  source: "broker",
  summary: "A query was forwarded to the hub and answered in 84 ms.",
  severity: "info",
  details: { object: "endpoints/hub-read", count: 1 },
};

// Two kinds the Portal writes past the ingest door (T-4968): the walker found the raw key
// "knowledge.file.uploaded" in the feed on dev.
const UPLOADED = {
  time: "2026-09-16T09:10:00Z",
  project: "helsinki",
  kind: "knowledge.file.uploaded",
  source: "portal",
  summary: "jana.kovacova uploaded rules.pdf to city-rules",
  severity: "info",
  details: { actor: "jana.kovacova", source: "city-rules", name: "rules.pdf", bytes: 48213 },
};

const ASKED = {
  time: "2026-09-16T09:12:00Z",
  project: "helsinki",
  kind: "access-request",
  source: "portal",
  summary: "jana.kovacova asked for access: pipeline helsinki/air on endpoint hub/air-read",
  severity: "info",
  details: { actor: "jana.kovacova", action: "asked for access" },
};

// T-5177: a failed publication keeps the broker's problem body behind a closed disclosure; the
// walker read that hidden body as raw JSON on dev's helsinki activity.
const FAILED_REASON =
  'the dataset is updated, its sheet was not refreshed: GET https://portal.example/api/endpoint/x/file.csv: 503 {"type":"https://joinedcontext.com/errors/broker-failure","status":503}';
const FAILED = {
  time: "2026-09-16T09:11:00Z",
  project: "helsinki",
  kind: "catalogue.published",
  source: "ckan",
  summary: `Endpoint helsinki-transport was not published: ${FAILED_REASON}`,
  severity: "error",
  details: { reason: FAILED_REASON, object: "endpoints/helsinki-transport" },
};

// T-5613: a rejection names the rejecter and keeps their reason in the row, not behind the
// technical disclosure.
const REJECTED = {
  time: "2026-09-16T09:13:00Z",
  project: "helsinki",
  kind: "change.rejected",
  source: "portal",
  summary:
    "Change chg-0000001d rejected: create Endpoint public-air, by eva.novakova@banskabystrica.sk: The maintenance notes are private",
  severity: "info",
  correlationId: "chg-0000001d",
  details: {
    change: "chg-0000001d",
    resourceKind: "Endpoint",
    name: "public-air",
    operation: "create",
    rejectedBy: "eva.novakova@banskabystrica.sk",
    reason: "The maintenance notes are private",
    object: "endpoints/public-air",
  },
};

const NAMESPACES = Object.keys(
  JSON.parse(readFileSync(join(dirname(fileURLToPath(import.meta.url)), "../src/locales/en.json"), "utf8")) as object,
);

async function stubApi(
  page: Page,
  { items = [LOADED], live = FORWARDED }: { items?: object[]; live?: { kind: string } } = {},
): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    const url = new URL(route.request().url());
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });

    if (url.pathname.endsWith("/activity/stream")) {
      // One event, then the connection stays open the way the Portal's SSE does.
      return route.fulfill({
        status: 200,
        contentType: "text/event-stream",
        headers: { "cache-control": "no-cache" },
        body: `event: ${live.kind}\ndata: ${JSON.stringify(live)}\n\n`,
      });
    }
    if (url.pathname.endsWith("/activity")) {
      return json({
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "ActivityList",
        items,
      });
    }
    if (url.pathname.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (url.pathname === "/api/v1/projects") {
      return json(PROJECTS);
    }
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
}

test.describe("activity", () => {
  test("an arriving forward reaches the tail and links to its object", async ({ page }) => {
    await stubApi(page);

    await page.goto("/?lang=en");
    await page
      .getByRole("navigation", { name: "Main navigation" })
      .getByRole("link", { name: "Activity" })
      .click();

    await expect(page).toHaveURL(/\/projects\/helsinki\/activity$/);
    await expect(page.getByText("What happened in the last hour")).toBeVisible();
    await expect(page.getByRole("cell", { name: LOADED.summary })).toBeVisible();

    // The tail: the event the stream pushed is on top within two seconds of the page opening.
    await expect(page.getByRole("cell", { name: FORWARDED.summary })).toBeVisible({
      timeout: 2000,
    });
    const rows = page.getByRole("row");
    await expect(rows.nth(1)).toContainText(FORWARDED.summary);
    await expect(page.getByRole("link", { name: "endpoints/hub-read" })).toHaveAttribute(
      "href",
      "/projects/helsinki/endpoints/hub-read",
    );

    // An info event interrupts nobody: the live region stays empty until an error arrives.
    await expect(page.getByTestId("activity-announce")).toHaveText("");
  });

  test("a filter narrows what the page asks for", async ({ page }) => {
    const asked: string[] = [];
    await stubApi(page);
    page.on("request", (request) => {
      if (request.url().includes("/activity?")) {
        asked.push(request.url());
      }
    });

    await page.goto("/projects/helsinki/activity?lang=en");
    await expect(page.getByRole("table")).toBeVisible();
    await page.getByLabel("Severity").selectOption("error");

    await expect
      .poll(() => asked.some((url) => url.includes("severity=error")))
      .toBe(true);
  });

  // T-5608: a link to the page with `?kind=mcp.tool` (the MCP door's own pointer at its calls)
  // opens it filtered to that kind, and asks the server for that kind alone.
  for (const width of [1440, 400]) {
    test(`a kind in the address opens the feed filtered at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      const asked: string[] = [];
      await stubApi(page);
      page.on("request", (request) => {
        if (request.url().includes("/activity?")) asked.push(request.url());
      });

      await page.goto("/projects/helsinki/activity?kind=mcp.tool&lang=en");
      await expect(page.getByLabel("Kind")).toHaveValue("mcp.tool");
      await expect.poll(() => asked.some((url) => url.includes("kind=mcp.tool"))).toBe(true);
      if (process.env.SHOTS_DIR) await page.screenshot({ path: `${process.env.SHOTS_DIR}/activity-kind-from-address-${width}.png`, fullPage: true });
    });
  }

  test("the activity page has no axe violations", async ({ page }) => {
    await stubApi(page);

    await page.goto("/projects/helsinki/activity?lang=en");
    await expect(page.getByRole("table")).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);
  });

  for (const width of [1440, 400]) {
    test(`kinds the Portal writes read as words at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, { items: [REJECTED, UPLOADED, FAILED], live: ASKED });

      await page.goto("/projects/helsinki/activity?lang=en");
      await expect(page.getByRole("cell", { name: UPLOADED.summary })).toBeVisible();
      // The live tail listens per kind: an access-request event reaches it as it happens.
      await expect(page.getByRole("cell", { name: ASKED.summary })).toBeVisible({ timeout: 2000 });
      const row = (summary: string) => page.getByRole("row").filter({ hasText: summary });
      await expect(row(UPLOADED.summary)).toContainText("File added to a knowledge source");
      await expect(row(ASKED.summary)).toContainText("Access request");
      await expect(row(REJECTED.summary)).toContainText("Change rejected");
      await expect(row(REJECTED.summary).getByRole("link", { name: "chg-0000001d" })).toHaveAttribute(
        "href",
        "/projects/helsinki/approvals/chg-0000001d",
      );
      await expect(page.getByLabel("Kind").locator("option", { hasText: "Change rejected" })).toHaveCount(1);
      await expect(page.getByLabel("Kind").locator("option", { hasText: "File added to a knowledge source" })).toHaveCount(1);
      await expect(row("Endpoint helsinki-transport was not published.")).toContainText("Technical details");

      // The live walker's own checks (tests/pageChecks.ts), raw-i18n-key among them.
      expect(await page.evaluate(pageFindings, { namespaces: NAMESPACES, layout: true })).toEqual([]);
      if (process.env.SHOTS_DIR) await page.screenshot({ path: `${process.env.SHOTS_DIR}/activity-kinds-${width}.png`, fullPage: true });
    });
  }
});
