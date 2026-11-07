/**
 * T-3280 — every main page answers on dev in under 2 s: from the navigation to the page's heading
 * and its first content, measured in the browser (Navigation Timing plus the moment the heading
 * shows), at a phone's and a desktop's width. The times are attached as a table for the task.
 * Read-only.
 */
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { STEWARD, goSignedIn } from "./portal";

test.setTimeout(600_000);

const PROJECT = "helsinki";
const BUDGET_MS = 2_000;

async function firstName(page: Page, plural: string): Promise<string | undefined> {
  const answer = await page.request.get(`/api/v1/projects/${PROJECT}/${plural}`);
  if (!answer.ok()) return undefined;
  return ((await answer.json()) as { items?: { metadata: { name: string } }[] }).items?.[0]?.metadata.name;
}

/** Milliseconds from the start of the navigation until the page's heading is drawn. */
async function timeToHeading(page: Page, path: string): Promise<number> {
  await page.goto(`/projects/${PROJECT}/${path}${path.includes("?") ? "&" : "?"}lang=en`, { waitUntil: "commit" });
  await page.locator("main h1").first().waitFor({ timeout: 60_000 });
  return page.evaluate(() => Math.round(performance.now()));
}

for (const viewport of [
  { width: 375, height: 812 },
  { width: 1440, height: 900 },
]) {
  test(`at ${viewport.width} px every main page answers in under 2 s`, async ({ browser }, info) => {
    const context = await browser.newContext({ viewport });
    const page = await context.newPage();
    try {
      await goSignedIn(page, STEWARD, `/projects/${PROJECT}/spaces?lang=en`);
      const space = await firstName(page, "spaces");
      const endpoint = await firstName(page, "endpoints");
      const dashboard = await firstName(page, "dashboards");
      const paths = [
        "spaces",
        ...(space ? [`spaces/${space}`] : []),
        "endpoints",
        ...(endpoint ? [`endpoints/${endpoint}`] : []),
        "explore",
        "dashboards",
        ...(dashboard ? [`dashboards?edit=${dashboard}`] : []),
        "pipelines",
        "policies",
        "models",
        "approvals",
        "assistant",
        "apps",
      ];
      const rows: string[] = [];
      const slow: string[] = [];
      for (const path of paths) {
        const ms = await timeToHeading(page, path);
        rows.push(`| ${path} | ${ms} |`);
        if (ms > BUDGET_MS) slow.push(`${path}: ${ms} ms`);
      }
      await info.attach(`page-times-${viewport.width}.md`, {
        body: ["| page | ms to heading |", "|---|---|", ...rows].join("\n"),
        contentType: "text/markdown",
      });
      expect(slow, "pages over 2 s").toEqual([]);
    } finally {
      await context.close();
    }
  });
}
