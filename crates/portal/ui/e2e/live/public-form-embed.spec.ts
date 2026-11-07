/**
 * T-3266 — a public form's embed and test run on dev (API/01 §33, EP-101, EP-102). The journey
 * opens the first public Endpoint of dev's catalogue as a form in a test run, with nobody signed
 * in: the page says a test writes nothing, offers "Send a test", and never asks who is signed in.
 * It checks what a site that embeds the form loads: `/f/embed.js` is a script, and a form whose
 * Endpoint names no site keeps `frame-ancestors 'self'` and `SAMEORIGIN`. It sends nothing.
 */
import { expect, test } from "@playwright/test";

test.setTimeout(180_000);

interface Detail {
  endpoint?: { url?: string };
}

test("a visitor's test run of a public form writes nothing, and the embed script is served", async ({ browser }) => {
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

    const framing = await page.request.get(`/f/${slug}`);
    expect(framing.status()).toBe(200);
    expect(framing.headers()["content-security-policy"]).toContain("frame-ancestors 'self';");
    expect(framing.headers()["x-frame-options"]).toBe("SAMEORIGIN");

    const script = await page.request.get("/f/embed.js");
    expect(script.status()).toBe(200);
    expect(script.headers()["content-type"]).toContain("javascript");
    expect(await script.text()).toContain("jc-form-height");

    const asked: string[] = [];
    page.on("request", (request) => asked.push(new URL(request.url()).pathname));
    await page.goto(`/f/${slug}?test=1&lang=en`, { waitUntil: "load" });
    await expect(page.getByRole("heading", { level: 1 })).toBeVisible({ timeout: 60_000 });
    const notice = page.getByText("Test run: sending checks the entry");
    const gone = page.getByText("This form is not published, or no longer.");
    await expect(notice.or(gone)).toBeVisible({ timeout: 60_000 });
    if (await notice.isVisible()) {
      await expect(page.getByRole("button", { name: "Send a test" })).toBeVisible();
    }
    // Anonymous by construction: the page never asks the Portal who is signed in.
    expect(asked).not.toContain("/api/v1/auth/me");
    await expect(page.getByRole("button", { name: "Sign in" })).toHaveCount(0);
  } finally {
    await context.close();
  }
});
