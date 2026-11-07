/**
 * T-6516: a person draws a live Endpoint in Grafana from the Portal (T-6509). The film saves PM2.5 of
 * helsinki-air-quality as a time series at 1440 and opens it in Grafana; the phone clip shows the saved
 * row. A second test saves Number, Table and Map once each and reads every panel in Grafana, because
 * src/grafana.rs dashboard_of wrote their JSON from the plugin source and Grafana never drew it. Every
 * `t6516-` visualization is removed again. Saving needs propose on Dashboard, which the dev seed gives
 * demo.editor (helsinki-role-editor.yaml): EDITOR_PASSWORD.
 */
import { expect, test, type Page } from "@playwright/test";
import { EDITOR, csrf } from "./portal";
import { SIZES, clipOf } from "./clips";

const PROJECT = "helsinki";
const ADDRESS = `/projects/${PROJECT}/grafana?lang=en`;
const ENDPOINT = "helsinki-air-quality";
const TYPE = "AirQualityObserved";
const PREFIX = "t6516-";

test.setTimeout(600_000);
test.skip(!EDITOR.password, "the editor signs in with EDITOR_PASSWORD");
test.describe.configure({ mode: "serial" });

async function choose(page: Page, kind: string, attribute: string, title: string): Promise<void> {
  await page.getByLabel("Endpoint", { exact: true }).selectOption(ENDPOINT);
  await page.getByLabel("Type", { exact: true }).selectOption(TYPE);
  await page.getByLabel("Attribute", { exact: true }).selectOption(attribute);
  await page.getByLabel("Visualization", { exact: true }).selectOption(kind);
  await page.getByLabel("Name", { exact: true }).fill(title);
}

async function save(page: Page, title: string): Promise<string> {
  await page.getByRole("button", { name: "Save to Grafana" }).click();
  const link = page.getByRole("link", { name: `Open ${title} in Grafana` });
  await expect(link).toBeVisible({ timeout: 60_000 });
  return (await link.getAttribute("href")) ?? "";
}

/** Opens a saved dashboard; Grafana signs the person in through the same Keycloak session. */
async function openInGrafana(page: Page, href: string): Promise<void> {
  await page.goto(href, { waitUntil: "load" });
  const sso = page.getByRole("link", { name: /Sign in with/ });
  if (await sso.isVisible().catch(() => false)) await sso.click();
  await expect(page.locator("[data-viz-panel-key], [data-testid^='data-testid Panel header']").first()).toBeVisible({ timeout: 60_000 });
  await expect(page.locator(".panel-loading, [aria-label='Panel loading bar']")).toHaveCount(0, { timeout: 60_000 });
}

/** What Grafana drew: panel errors and "No data" are both a failed visualization. */
async function drawn(page: Page): Promise<{ errors: number; noData: number }> {
  return {
    errors: await page.locator("[data-testid='data-testid Panel status error']").count(),
    noData: await page.getByText("No data", { exact: true }).count(),
  };
}

async function removeOurs(page: Page): Promise<void> {
  const list = await page.request.get(`/api/v1/projects/${PROJECT}/grafana`);
  if (!list.ok()) return;
  const body = (await list.json()) as { items?: { id: string; title: string }[] };
  const headers = { "x-csrf-token": await csrf(page.context()) };
  for (const item of body.items ?? []) {
    if (item.title.startsWith(PREFIX)) await page.request.delete(`/api/v1/projects/${PROJECT}/grafana/${item.id}`, { headers });
  }
}

test("a Grafana time series from a live Endpoint, at 1440 and 400", async ({ browser }) => {
  const title = `${PREFIX}pm25 ${Date.now().toString(36)}`;
  const clip = await clipOf(browser, EDITOR, "grafana-create", SIZES[0], ADDRESS);
  const { page } = clip;
  let href = "";
  try {
    await clip.step("Grafana in the project: pick what to draw", async () => {
      await expect(page.getByRole("button", { name: "Save to Grafana" })).toBeVisible({ timeout: 60_000 });
    });
    await clip.step("Helsinki air quality, PM2.5, as a time series", async () => {
      await choose(page, "timeseries", "pm25", title);
      await expect(page.getByText("Preview: the rows you can read now")).toBeVisible({ timeout: 30_000 });
    }, 5_000);
    await clip.step("Save: the Portal writes it into the project's Grafana folder", async () => {
      href = await save(page, title);
    });
    await clip.step("Open it in Grafana: live PM2.5 by station", async () => {
      await openInGrafana(page, href);
      expect(await drawn(page)).toEqual({ errors: 0, noData: 0 });
    }, 6_000);
  } finally {
    await clip.finish().catch((failed: unknown) => console.log(String(failed)));
  }
  const phone = await clipOf(browser, EDITOR, "grafana-create", SIZES[1], ADDRESS);
  try {
    await phone.step("On a phone: the saved visualization, ready to open", async () => {
      await expect(phone.page.getByRole("link", { name: `Open ${title} in Grafana` })).toBeVisible({ timeout: 60_000 });
      await phone.page.getByRole("link", { name: `Open ${title} in Grafana` }).scrollIntoViewIfNeeded();
    });
    await phone.page.screenshot({ path: `${process.env.CLIP_DIR ?? "test-results/clips"}/page-400.png`, fullPage: true });
    await openInGrafana(phone.page, href);
    await phone.step("The chart in Grafana on a phone", async () => {
      expect(await drawn(phone.page)).toEqual({ errors: 0, noData: 0 });
    });
  } finally {
    await phone.page.goto(ADDRESS).catch(() => undefined);
    await removeOurs(phone.page).catch((failed: unknown) => console.log(String(failed)));
    await phone.finish();
  }
});

test("Number, Table and Map draw in Grafana", async ({ browser }) => {
  const clip = await clipOf(browser, EDITOR, "grafana-kinds", SIZES[0], ADDRESS);
  const { page } = clip;
  const seen: Record<string, { errors: number; noData: number }> = {};
  try {
    for (const [kind, attribute] of [["stat", "pm25"], ["table", "pm25"], ["map", "location"]] as const) {
      const title = `${PREFIX}${kind} ${Date.now().toString(36)}`;
      await page.goto(ADDRESS, { waitUntil: "load" });
      await expect(page.getByRole("button", { name: "Save to Grafana" })).toBeVisible({ timeout: 60_000 });
      await choose(page, kind, attribute, title);
      const href = await save(page, title);
      if (kind === "table") await page.screenshot({ path: `${process.env.CLIP_DIR ?? "test-results/clips"}/page-1440.png`, fullPage: true });
      await openInGrafana(page, href);
      await clip.step(`${kind} in Grafana`, async () => {
        seen[kind] = await drawn(page);
      }, 2_000);
    }
    console.log(`kinds drawn: ${JSON.stringify(seen)}`);
    for (const kind of Object.keys(seen)) expect(seen[kind], kind).toEqual({ errors: 0, noData: 0 });
  } finally {
    await page.goto(ADDRESS).catch(() => undefined);
    await removeOurs(page).catch((failed: unknown) => console.log(String(failed)));
    await clip.finish().catch((failed: unknown) => console.log(String(failed)));
  }
});
