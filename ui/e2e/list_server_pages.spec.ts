import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { axeViolations } from "./axe";

// T-6175: a kind's list reads its rows from the server a page at a time (API/13 section 2):
// `limit` and `cursor` page it, `q` searches it and `count=true` gives the footer its total.
// `vite preview` has no Portal API behind it, so the browser answers with a server that pages,
// searches and counts the way the Portal does; the clicks and the rendering are the real ones.

const API = "joinedcontext.com/v1alpha1";
const PROJECT = "helsinki";
const SHOTS = process.env.SHOTS_DIR;

const BLUEPRINTS = Array.from({ length: 120 }, (_, index) => {
  const name = `air-feed-${String(index).padStart(3, "0")}`;
  return {
    apiVersion: API,
    kind: "Blueprint",
    metadata: { name, namespace: PROJECT, title: { en: `Air quality feed ${index}` } },
    spec: { version: "1.0.0", category: "ingestion", riskClass: index % 3 === 0 ? "red" : "yellow" },
    status: { phase: "Live" },
  };
});

async function stubApi(page: Page, rows = BLUEPRINTS): Promise<URL[]> {
  const asked: URL[] = [];
  await page.route("**/api/v1/**", async (route) => {
    const url = new URL(route.request().url());
    const json = (body: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    if (url.pathname.endsWith("/auth/me")) {
      return json({ subject: "b7c1e0f4", username: "aino.virtanen", name: "Aino Virtanen", roles: ["portal-approver"] });
    }
    if (url.pathname === "/api/v1/projects") return json({ apiVersion: API, kind: "ProjectList", items: [{ name: PROJECT }] });
    if (url.pathname === `/api/v1/projects/${PROJECT}/blueprints`) {
      asked.push(url);
      const q = (url.searchParams.get("q") ?? "").toLowerCase();
      const kept = rows.filter((row) => `${row.metadata.name} ${row.metadata.title.en}`.toLowerCase().includes(q));
      const from = Number(url.searchParams.get("cursor") ?? 0);
      const limit = Number(url.searchParams.get("limit") ?? kept.length);
      const next = from + limit < kept.length ? String(from + limit) : undefined;
      return json({ apiVersion: API, kind: "List", metadata: {}, items: kept.slice(from, from + limit), nextCursor: next, totalCount: kept.length });
    }
    return json({ apiVersion: API, kind: "List", items: [] });
  });
  return asked;
}

async function shot(page: Page, state: string): Promise<void> {
  if (!SHOTS) return;
  mkdirSync(SHOTS, { recursive: true });
  for (const width of [1440, 400]) {
    await page.setViewportSize({ width, height: 900 });
    await page.screenshot({ path: `${SHOTS}/${state}-${width}.png`, fullPage: true });
  }
  await page.setViewportSize({ width: 1440, height: 900 });
}

test.describe("a kind's list pages on the server", () => {
  test("a person reads the first page, asks for more and searches, and the server does each", async ({ page }) => {
    const asked = await stubApi(page);
    await page.goto(`/projects/${PROJECT}/blueprints?lang=en`);
    const table = page.getByRole("table");
    await expect(table.getByRole("row")).toHaveCount(51);
    await expect(page.getByText("Showing 50 of 120")).toBeVisible();
    expect(asked[0]?.searchParams.get("limit")).toBe("50");
    expect(asked[0]?.searchParams.get("count")).toBe("true");
    await shot(page, "first-page");

    await page.getByRole("button", { name: "Show 50 more" }).click();
    await expect(table.getByRole("row")).toHaveCount(101);
    expect(asked.at(-1)?.searchParams.get("cursor")).toBe("50");

    await page.getByRole("searchbox").fill("feed 11");
    await expect.poll(() => asked.at(-1)?.searchParams.get("q")).toBe("feed 11");
    await expect(table.getByRole("row")).toHaveCount(12);
    await shot(page, "searched");
    expect(await axeViolations(page)).toEqual([]);

    await page.getByRole("searchbox").fill("no such feed");
    await expect(page.getByText("No rows match the search and filters.")).toBeVisible();
    await shot(page, "no-match");
  });

  test("an empty kind says what would fill it", async ({ page }) => {
    await stubApi(page, []);
    await page.goto(`/projects/${PROJECT}/blueprints?lang=en`);
    await expect(page.getByText("Nothing here yet.")).toBeVisible();
    await expect(page.getByRole("table")).toHaveCount(0);
    await shot(page, "empty");
  });
});
