/**
 * T-3103, T-3172 — a published form on dev (API/01 §33): the page at `/f/{slug}` reads a public
 * Endpoint's schema with no session. The journey opens the link of the first public Endpoint of
 * dev's catalogue and finds the page's heading and either the form, with its public-data notice,
 * or the line that says it is not a published form. It sends nothing: an entry would be public
 * data written into dev.
 */
import { expect, test } from "@playwright/test";

test.setTimeout(180_000);

interface Detail {
  endpoint?: { url?: string };
}

test("a visitor opens a public Endpoint as a form link and is told what sending means", async ({ browser }) => {
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

    await page.goto(`/f/${slug}?lang=en`, { waitUntil: "load" });
    await expect(page.getByRole("heading", { level: 1 })).toBeVisible({ timeout: 60_000 });
    await expect(
      page.getByText("Your answers become public data of this space").or(page.getByText("This form is not published, or no longer.")),
    ).toBeVisible({ timeout: 60_000 });
  } finally {
    await context.close();
  }
});
