// T-5129: the explorer on a municipal website. The journey starts on the catalogue's dataset page,
// takes the code from the explorer's Embed dialog, pastes it into a plain HTML page of another site
// and checks the live table, the map and the source and licence line there, at 1440 and 400; then
// the web component variant, the refusals (a wrong attribute, a dataset of another Endpoint, the
// catalogue-only page framed elsewhere) and that no cookie or credential leaves the visitor's page.
// The WordPress page is wordpress.spec.mjs (wordpress/run.sh starts a real WordPress).
//   PLAYWRIGHT_BROWSERS_PATH=/ms-playwright npx playwright test -c components/viz/tests embed
import { test, expect } from "@playwright/test";
import { createRequire } from "node:module";
import path from "node:path";
import { shots, DOMAIN, VIZ, CATALOGUE, API, SLUGS, serve, catalogue } from "./explore-hosts.mjs";

const embed = createRequire(import.meta.url)("../charts/renderer/files/site/embed.js");
const SITE = "www.banskabystrica.example";
// A town's page: its own header, article and cookie, and the pasted code in the middle.
const townPage = (code) => `<!doctype html><html lang="sk"><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Bicykle | Mesto Banská Bystrica</title>
  <style>body{margin:0;font:16px Georgia,serif;color:#222}header{background:#7a1f2b;color:#fff;padding:12px 16px}main{max-width:960px;margin:0 auto;padding:16px}</style>
  <header><strong>Mesto Banská Bystrica</strong> · Doprava</header>
  <main><h1>Zdieľané bicykle</h1><p>Aktuálny stav staníc priamo z otvorených dát mesta.</p>
  ${code}
  <p>Kontakt: doprava@banskabystrica.example</p></main><script>document.cookie = "town_session=secret; path=/";</script>`;

async function town(context, code) {
  await context.route((url) => url.hostname === SITE, (route) => route.fulfill({ status: 200, contentType: "text/html", body: townPage(code) }));
}

const shot = (page, name) => page.screenshot({ path: path.join(shots, `embed-${name}.png`), fullPage: true });
const leaks = (requests) => requests.filter((r) => r.url.hostname !== SITE && (r.headers.cookie || r.headers.authorization)).map((r) => String(r.url));

for (const width of [1440, 400]) {
  test(`${width}: the Embed dialog's code shows the live table, map and licence on a town's page`, async ({ page, context }) => {
    const requests = await serve(context);
    await catalogue(context);
    await page.setViewportSize({ width, height: 900 });
    // A view the town wants its visitors to start from: a shared link with a filter and an order.
    const extra = "&type=BikeHireDockingStation&q=" + encodeURIComponent('status=="working"') + "&sort=availableBikeNumber;desc";
    await page.goto(`https://${CATALOGUE}/dataset/bike-stations?lang=sk&extra=${encodeURIComponent(extra)}`);
    const explorer = page.frameLocator("#explore");
    await expect(explorer.getByRole("grid").locator("tbody tr")).toHaveCount(1, { timeout: 20000 }); // the routed API answers a q with one row
    await explorer.locator("#embed-open").click();
    const dialog = explorer.locator("#embed");
    await expect(dialog).toBeVisible();
    await expect(dialog.locator("#embed-keep-hint")).toHaveText('Návštevníci začnú s týmto zobrazením: filter status=="working"; zoradené podľa availableBikeNumber (zostupne)');
    await dialog.locator("#embed-height").fill("100");
    await expect(dialog.locator("#embed-error")).toContainText("240");
    await expect(dialog.locator("#embed-copy")).toBeDisabled();
    await dialog.locator("#embed-height").fill("720");
    await expect(dialog.locator("#embed-copy")).toBeEnabled();
    await page.screenshot({ path: path.join(shots, `embed-dialog-${width}.png`), fullPage: true });
    const code = await dialog.locator("#embed-code").inputValue();
    expect(code).toMatch(new RegExp(`^<iframe src="https://${VIZ}/embed\\.html#${SLUGS.point}&amp;dataset=bike-stations&amp;type=BikeHireDockingStation&amp;lang=sk&amp;q=status%3D%3D%22working%22&amp;sort=availableBikeNumber%3Bdesc" `));
    expect(code).toContain('height="720"');
    await dialog.locator("#embed-close").click();
    await expect(explorer.locator("#embed-open")).toBeFocused();

    await town(context, code);
    await page.goto(`https://${SITE}/doprava/bicykle`);
    const widget = page.frameLocator("iframe");
    await expect(widget.getByRole("grid").locator("tbody tr")).toHaveCount(1, { timeout: 20000 }); // the routed API answers a q with one row
    await expect(widget.locator("#map-section")).toBeVisible();
    await expect(widget.locator("#embed-open")).toBeHidden();
    const source = widget.locator("#attribution");
    await expect(source).toContainText("Stanice zdieľaných bicyklov");
    await expect(source).toContainText("Mesto Banská Bystrica");
    await expect(source.getByRole("link", { name: "Creative Commons Attribution 4.0" })).toHaveAttribute("href", "https://creativecommons.org/licenses/by/4.0/");
    await expect(source.getByRole("link", { name: "Stanice zdieľaných bicyklov" })).toHaveAttribute("href", `https://${CATALOGUE}/dataset/bike-stations`);
    await page.waitForTimeout(500);
    await shot(page, `town-${width}`);
    const asked = requests.filter((r) => r.url.hostname === API && r.url.pathname.endsWith("/entities") && r.headers.accept !== "application/geo+json");
    expect(asked.some((r) => r.url.searchParams.get("q") === 'status=="working"' && r.url.searchParams.get("orderBy") === "availableBikeNumber;desc")).toBe(true);
    expect(leaks(requests)).toEqual([]);
  });
}

test("the web component frames the same widget and grows to its content", async ({ page, context }) => {
  const requests = await serve(context);
  await catalogue(context);
  const code = embed.snippet("component", { slug: SLUGS.point, dataset: "bike-stations", type: "BikeHireDockingStation", lang: "en", view: "table", height: 400 },
    `https://${VIZ}`, "Bike stations");
  await town(context, code);
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`https://${SITE}/`);
  const widget = page.frameLocator("jc-data-explorer iframe");
  await expect(widget.getByRole("grid").locator("tbody tr")).toHaveCount(3, { timeout: 20000 });
  await expect(widget.locator("#map-section")).toBeHidden(); // view=table
  await expect(widget.locator("#attribution")).toContainText("Licence: Creative Commons Attribution 4.0");
  const frame = page.locator("jc-data-explorer iframe");
  await expect(frame).toHaveAttribute("title", "Bike stations");
  await expect(frame).toHaveAttribute("sandbox", "allow-scripts allow-same-origin allow-popups allow-popups-to-escape-sandbox");
  await expect.poll(async () => (await frame.boundingBox()).height).toBeGreaterThan(400);
  await shot(page, "component-1440");
  expect(leaks(requests)).toEqual([]);
});

test("a wrongly set web component says what to do and frames nothing", async ({ page, context }) => {
  await serve(context);
  await town(context, `<script src="https://${VIZ}/embed.js" defer></script><jc-data-explorer endpoint="../../evil" height="640"></jc-data-explorer>`);
  await page.goto(`https://${SITE}/`);
  await expect(page.getByRole("alert")).toContainText("Copy the snippet again");
  await expect(page.locator("jc-data-explorer iframe")).toHaveCount(0);
});

test("a dataset of another Endpoint lends the embed neither its title nor its licence", async ({ page, context }) => {
  await serve(context);
  await catalogue(context);
  await town(context, embed.snippet("iframe", { slug: SLUGS.point, dataset: "other-endpoint", lang: "en" }, `https://${VIZ}`));
  await page.goto(`https://${SITE}/`);
  const source = page.frameLocator("iframe").locator("#attribution");
  await expect(source).toContainText(`opendata.${DOMAIN}`);
  await expect(source).toContainText("Licence: see the dataset");
  await expect(source).not.toContainText("Iný dataset");
});

test("only /embed.html may be framed by another site; /explore.html stays the catalogue's", async ({ page, context }) => {
  await serve(context);
  await catalogue(context);
  await town(context, `<iframe id="a" src="https://${VIZ}/explore.html#${SLUGS.point}"></iframe><iframe id="b" src="https://${VIZ}/embed.html#${SLUGS.point}"></iframe>`);
  await page.goto(`https://${SITE}/`);
  await expect(page.frameLocator("#b").getByRole("grid").locator("tbody tr")).toHaveCount(3, { timeout: 20000 });
  await expect(page.frameLocator("#a").locator("#grid")).toHaveCount(0);
});
