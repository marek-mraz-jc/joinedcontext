// T-5129: the Embed codes in a real WordPress 6.8 page ("Custom HTML" block), started and filled by
// wordpress/run.sh, which sets WP_URL. WordPress itself answers; viz, the API, the tiles and the
// catalogue are routed as in embed.spec.mjs. Shots: test-results/explore/embed-wordpress-*.png.
import { test, expect } from "@playwright/test";
import path from "node:path";
import { shots, DOMAIN, VIZ, API, CATALOGUE, serve, catalogue } from "./explore-hosts.mjs";

const WP = new URL(process.env.WP_URL);

async function hosts(context) {
  const requests = await serve(context);
  await catalogue(context);
  // WordPress and its own assets come from the container; nothing else leaves the test.
  await context.route((url) => url.host === WP.host, (route) => route.continue());
  return requests;
}

for (const width of [1440, 400]) {
  test(`${width}: the frame code in a WordPress page shows the live table, map and licence`, async ({ page, context }) => {
    const requests = await hosts(context);
    await page.setViewportSize({ width, height: 900 });
    await page.goto(new URL("/bicykle/", WP).href);
    await expect(page.locator("h1")).toContainText("Zdieľané bicykle");
    // WordPress kept the snippet as pasted: the sandbox and the address are intact.
    const frame = page.locator(`iframe[src^="https://${VIZ}/embed.html#"]`);
    await expect(frame).toHaveAttribute("sandbox", "allow-scripts allow-same-origin allow-popups allow-popups-to-escape-sandbox");
    const widget = page.frameLocator(`iframe[src^="https://${VIZ}/embed.html#"]`);
    await expect(widget.getByRole("grid").locator("tbody tr")).toHaveCount(3, { timeout: 20000 });
    await expect(widget.locator("#map-section")).toBeVisible();
    await expect(widget.locator("#attribution")).toContainText("Zdroj: Stanice zdieľaných bicyklov · Mesto Banská Bystrica · Licencia: Creative Commons Attribution 4.0");
    await frame.scrollIntoViewIfNeeded();
    await page.waitForTimeout(500);
    await page.screenshot({ path: path.join(shots, `embed-wordpress-${width}.png`), fullPage: true });
    const off = requests.filter((r) => ![VIZ, API, CATALOGUE, `tiles.${DOMAIN}`, WP.hostname].includes(r.url.hostname));
    expect(off.map((r) => String(r.url))).toEqual([]);
    expect(requests.filter((r) => r.url.hostname !== WP.hostname && (r.headers.cookie || r.headers.authorization))).toEqual([]);
  });
}

test("the web component code in a WordPress page grows to the table", async ({ page, context }) => {
  await hosts(context);
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(new URL("/bicykle-tabulka/", WP).href);
  const widget = page.frameLocator("jc-data-explorer iframe");
  await expect(widget.getByRole("grid").locator("tbody tr")).toHaveCount(3, { timeout: 20000 });
  await expect(widget.locator("#map-section")).toBeHidden();
  await expect(widget.locator("#attribution")).toContainText("Licencia: Creative Commons Attribution 4.0");
  await page.waitForTimeout(500);
  await page.screenshot({ path: path.join(shots, "embed-wordpress-component-1440.png"), fullPage: true });
});
