/**
 * T-7031 — an Organization's own texts of its open data portal's pages (spec.appearance.pages).
 *
 * Publishing or reverting a page text proposes the Organization manifest, and the preview renders
 * Markdown with the catalogue's renderer; all three are for the Organization's administrators.
 * `demo.viewer` holds no propose binding at the Organization: the Portal refuses them every write
 * and the preview, so nothing is proposed in their name. The publish and revert paths are proved by
 * tests/opendata_pages_tests.rs; the editor screen is T-7041.
 */
import { expect, test } from "@playwright/test";
import { VIEWER, csrf, signIn } from "./portal";

test("a person who is no administrator of the Organization is refused the page preview, publish and revert", async ({
  browser,
}) => {
  test.skip(!VIEWER.password, "VIEWER_PASSWORD is not set");
  const { context, page } = await signIn(browser, VIEWER, "/?lang=en");
  const headers = { "x-csrf-token": await csrf(context), "content-type": "application/json" };
  const data = { markdown: "# Pripojenie\n\nText." };
  const answers = [
    await page.request.post("/api/v1/organization/opendata/pages/preview", { headers, data }),
    await page.request.put("/api/v1/organization/opendata/pages/connector/sk", { headers, data }),
    await page.request.delete("/api/v1/organization/opendata/pages/connector/sk", { headers }),
  ];
  for (const answer of answers) {
    expect(answer.status(), `${answer.url()} refuses a non-administrator`).toBe(403);
    expect(await answer.text(), `${answer.url()} says who may edit`).toContain("administrators");
  }
  await context.close();
});
