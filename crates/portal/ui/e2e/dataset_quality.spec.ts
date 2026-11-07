import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// T-5112: a dataset's quality score in the public catalogue, as a visitor with no session reads it:
// the badge with its number and word, the date, and each part with its reason. The public form holds
// counts and dates only; no fix and no name reach it.

const ENDPOINT = "http://127.0.0.1/api/endpoint/k7m2qz4tv6xh3n5jb2ryd3wcfa/";

const SCORED = {
  score: 72,
  measuredAt: "2026-10-22T08:04:00Z",
  parts: [
    { part: "freshness", score: 0, measuredAt: "2026-10-22T08:04:00Z", reason: { code: "late", args: { late: "1", total: "1", newest: "2026-10-21T02:50:00Z" }, text: "1 of 1 feeds are late; the newest observation is from 2026-10-21T02:50:00Z" } },
    { part: "completeness", score: 97, measuredAt: "2026-10-22T03:00:00Z", reason: { code: "complete", args: { complete: "1", fields: "2" }, text: "1 of 2 required fields are filled in every entity" } },
    { part: "validity", score: 95, measuredAt: "2026-10-22T03:00:00Z", reason: { code: "valid", args: { valid: "190", checked: "200" }, text: "190 of 200 entities follow the model" } },
    { part: "links", score: 100, measuredAt: "2026-10-22T08:04:00Z", reason: { code: "linksUp", args: { up: "2", total: "2" }, text: "2 of 2 access links answer" } },
  ],
};

// T-6565: the Endpoint's own type is on schedule; and a feed that is late with no reading yet.
// the badge is the mean of the four parts, as the server computes it
const freshnessOf = (reason: unknown, score: number) => ({
  ...SCORED,
  score: Math.floor((score + 97 + 95 + 100) / 4),
  parts: [{ part: "freshness", score, measuredAt: "2026-10-22T08:04:00Z", reason }, ...SCORED.parts.slice(1)],
});
const FRESH = freshnessOf({ code: "onTime", args: { fresh: "1", total: "1" }, text: "1 of 1 feeds are on schedule" }, 100);
const UNREAD = freshnessOf({ code: "lateUnread", args: { late: "1", total: "1" }, text: "1 of 1 feeds are late; none has delivered an observation yet" }, 0);

const UNMEASURED = {
  parts: ["freshness", "completeness", "validity", "links"].map((part) => ({
    part,
    reason: { code: "notMeasured", args: {}, text: "not measured yet" },
  })),
};

const detail = (quality: unknown) => ({
  name: "bbsk-kpi",
  title: "Ukazovatele kraja",
  notes: "Ukazovatele Banskobystrického samosprávneho kraja.",
  keywords: [],
  themes: [],
  spatial: [],
  catalogueUrl: "https://data.example.org/dataset/bbsk-kpi",
  resources: [{ name: "CSV", format: "CSV", url: `${ENDPOINT}file.csv` }],
  endpoint: { url: ENDPOINT, representations: ["ngsi-ld"] },
  quality,
});

async function stubApi(page: Page, quality: unknown): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    const url = new URL(route.request().url());
    const json = (body: unknown, status = 200) =>
      route.fulfill({
        status,
        contentType: status >= 400 ? "application/problem+json" : "application/json",
        body: JSON.stringify(body),
      });
    if (url.pathname.endsWith("/auth/me")) return json({ title: "Unauthorized", status: 401 }, 401);
    if (url.pathname === "/api/v1/catalogue/datasets/bbsk-kpi") return json(detail(quality));
    return json({ title: "Not Found", status: 404, detail: "not stubbed" }, 404);
  });
}

test.describe("a dataset's quality score", () => {
  for (const width of [1440, 400]) {
    test(`the badge, the date and the four parts read at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, SCORED);
      await page.goto("/catalogue/bbsk-kpi?lang=en");
      const card = page.getByRole("region", { name: "Data quality" });
      await expect(card).toBeVisible();
      await expect(card.getByTestId("quality-badge")).toHaveText("72 / 100 · fair");
      await expect(card.getByText(/^Measured /)).toBeVisible();
      await expect(card.getByRole("meter", { name: "Freshness" })).toHaveAttribute("value", "0");
      await expect(card.getByRole("meter", { name: "Validity" })).toHaveAttribute("value", "95");
      await expect(card.getByText("190 of 200 entities follow the model")).toBeVisible();
      await expect(card.getByText("To fix:")).toHaveCount(0);
      const sideways = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
      expect(sideways, "the page scrolls sideways").toBeLessThanOrEqual(0);
      expect(await axeViolations(page)).toEqual([]);
      if (process.env.SHOTS_DIR) await card.screenshot({ path: `${process.env.SHOTS_DIR}/quality-filled-${width}.png` });
    });

    for (const [state, quality, said] of [
      ["fresh", FRESH, "1 of 1 feeds are on schedule"],
      ["unread", UNREAD, "1 of 1 feeds are late; none has delivered an observation yet"],
    ] as const) {
      test(`freshness reads the ${state} feed in words at ${width} px`, async ({ page }) => {
        await page.setViewportSize({ width, height: 900 });
        await stubApi(page, quality);
        await page.goto("/catalogue/bbsk-kpi?lang=en");
        const card = page.getByRole("region", { name: "Data quality" });
        await expect(card.getByText(said)).toBeVisible();
        await expect(card.getByText(/never/)).toHaveCount(0);
        expect(await axeViolations(page)).toEqual([]);
        if (process.env.SHOTS_DIR) await card.screenshot({ path: `${process.env.SHOTS_DIR}/quality-${state}-${width}.png` });
      });
    }

    test(`a dataset nothing measured yet says so at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stubApi(page, UNMEASURED);
      await page.goto("/catalogue/bbsk-kpi?lang=sk");
      const card = page.getByRole("region", { name: "Kvalita údajov" });
      await expect(card.getByTestId("quality-badge")).toHaveText("nenamerané");
      await expect(card.getByText(/^Zatiaľ nenamerané/)).toBeVisible();
      if (process.env.SHOTS_DIR) await card.screenshot({ path: `${process.env.SHOTS_DIR}/quality-empty-${width}.png` });
    });
  }
});
