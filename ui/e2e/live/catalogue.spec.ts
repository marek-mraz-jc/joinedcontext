/**
 * T-2726 — the public catalogue on dev (EP-81, EP-82): every public dataset of the installation,
 * read without signing in, under the publisher of the project that published it.
 *
 * dev publishes endpoints of more than one project (`helsinki`, the region's `bbsk` and the
 * city's `banskabystrica`, T-2455) to its CKAN, one CKAN organization per project (T-2407). A
 * visitor with no session opens `/catalogue`, finds more than one publisher, filters by one and
 * opens a dataset: its page names the same publisher and lists its resources. The API the page
 * reads is asked with no cookie at all, so what it lists is what a citizen sees.
 */
import { expect, test } from "@playwright/test";
import { STEWARD, signIn } from "./portal";

test.setTimeout(180_000);

interface Page {
  total: number;
  datasets: { name: string; title: string; publisher?: { name: string; title: string } }[];
  facets: { publisher: { value: string; label: string; count: number }[] };
}

test("a visitor with no session finds each project's datasets under its own publisher and opens one", async ({ browser }) => {
  const context = await browser.newContext();
  const page = await context.newPage();
  try {
    const answer = await page.request.get("/api/v1/catalogue");
    expect(answer.ok(), "the catalogue answers a caller with no session").toBe(true);
    expect(answer.headers()["cache-control"]).toContain("public");
    const catalogue = (await answer.json()) as Page;
    expect(catalogue.total, "dev publishes at least one dataset").toBeGreaterThan(0);
    const publishers = catalogue.facets.publisher.map((p) => p.value).sort();
    expect(publishers.length, `more than one project publishes (${publishers.join(", ")})`).toBeGreaterThan(1);

    await page.goto("/catalogue?lang=en", { waitUntil: "load" });
    await expect(page.getByRole("heading", { level: 1, name: "Catalogue" })).toBeVisible({ timeout: 60_000 });
    await expect(page.getByRole("button", { name: "Sign in" })).toBeVisible();

    const publisher = catalogue.facets.publisher[0];
    await page.getByRole("complementary", { name: "Filters" }).getByRole("checkbox", { name: new RegExp(publisher.label) }).check();
    await expect(page).toHaveURL(/publisher=/);
    const results = page.getByRole("region", { name: "Datasets" });
    await expect(results.getByRole("listitem")).toHaveCount(Math.min(publisher.count, 20), { timeout: 30_000 });

    await results.getByRole("heading", { level: 2 }).first().getByRole("link").click();
    await expect(page).toHaveURL(/\/catalogue\/[^/?]+$/);
    await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
    await expect(page.getByText(publisher.label).first()).toBeVisible();
    await expect(page.getByRole("heading", { name: "Resources" })).toBeVisible();
  } finally {
    await context.close();
  }
});

test("a signed-in steward reaches the catalogue from the sidebar", async ({ browser }) => {
  const { context, page } = await signIn(browser, STEWARD, "/?lang=en");
  try {
    await page.getByRole("navigation", { name: "Main navigation" }).getByRole("link", { name: "Catalogue" }).click();
    await expect(page).toHaveURL(/\/catalogue$/);
    await expect(page.getByRole("heading", { level: 1, name: "Catalogue" })).toBeVisible({ timeout: 60_000 });
    await expect(page.getByRole("button", { name: "Sign in" })).toHaveCount(0);

    // A dataset's address opens inside the Portal's shell for a person who is signed in.
    const listed = (await (await page.request.get("/api/v1/catalogue")).json()) as Page;
    const first = listed.datasets[0];
    await page.goto(`/catalogue/${first.name}?lang=en`, { waitUntil: "load" });
    await expect(page.getByRole("heading", { level: 1, name: first.title })).toBeVisible({ timeout: 60_000 });
    await expect(page.getByRole("navigation", { name: "Main navigation" })).toBeVisible();
  } finally {
    await context.close();
  }
});
