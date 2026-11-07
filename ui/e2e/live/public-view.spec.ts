/**
 * T-3108 — a published data view on dev (API/01 §33): the page at `/v/{slug}` reads a public
 * Endpoint with no session. dev's public catalogue names Endpoints of this installation, each a
 * public Endpoint, so the journey opens the link of the first one a visitor can reach and finds its
 * entities in a table, without signing in.
 */
import { expect, test } from "@playwright/test";

test.setTimeout(180_000);

interface Detail {
  endpoint?: { url?: string };
}

test("a visitor opens a public Endpoint as a published view and reads its entities", async ({ browser }) => {
  const context = await browser.newContext();
  const page = await context.newPage();
  try {
    const listed = (await (await page.request.get("/api/v1/catalogue")).json()) as { datasets: { name: string }[] };
    let slug: string | undefined;
    for (const dataset of listed.datasets) {
      const detail = (await (await page.request.get(`/api/v1/catalogue/datasets/${dataset.name}`)).json()) as Detail;
      slug = /\/api\/endpoint\/([^/]+)\//.exec(detail.endpoint?.url ?? "")?.[1];
      if (slug) break;
    }
    expect(slug, "dev publishes at least one public Endpoint of its own").toBeTruthy();

    await page.goto(`/v/${slug}?lang=en`, { waitUntil: "load" });
    await expect(page.getByRole("heading", { level: 1 })).toBeVisible({ timeout: 60_000 });
    await expect(page.getByRole("table")).toBeVisible({ timeout: 60_000 });
    await expect(page.getByRole("button", { name: "Sign in" })).toBeVisible();
  } finally {
    await context.close();
  }
});
