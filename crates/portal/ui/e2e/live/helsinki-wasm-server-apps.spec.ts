/**
 * The Helsinki apps with a server component on the shared WASM host, on dev (T-3346..T-3350,
 * ADR-N-044): each is published as `kind: wasm` with its shard, answers its main question with live
 * data for a signed-in steward, and keeps what it saves in its own schema across a reload. Every
 * record a run writes carries the run's stamp and is deleted in `finally`, so dev keeps nothing.
 *
 * Every wait is on the thing the step needs, never on the network going idle (T-2452).
 */
import { expect, test } from "@playwright/test";
import type { Page, Response } from "@playwright/test";
import { STEWARD, goSignedIn, signIn } from "./portal";

const PROJECT = "helsinki";
const APPS_DOMAIN = process.env.APPS_DOMAIN ?? "apps.dev.joinedcontext.com";

/** Each App answers on its own origin; `/apps/<app>/` on the Portal host only 308s there. */
const appOrigin = (app: string) => `https://${app}.${APPS_DOMAIN}/`;

/** An answer of the App's own API, whichever base the App's bundle calls it under (`/api/` or `/apps/<app>/api/`). */
function isAppApi(response: Response, app: string, path: string) {
  const url = new URL(response.url());
  return url.host === `${app}.${APPS_DOMAIN}` && url.pathname.endsWith(`/api/${path}`);
}

test.setTimeout(300_000);

/** The App is a server WASM App, placed on a shard, built into a component by its digest. */
async function expectPublishedAsWasm(page: Page, app: string) {
  const answer = await page.request.get(`/api/v1/projects/${PROJECT}/apps/${app}`);
  expect(answer.status(), `${app} is in the project`).toBe(200);
  const manifest = (await answer.json()) as { spec: { kind?: string }; status?: { shard?: number; build?: { component?: string } } };
  expect(manifest.spec.kind).toBe("wasm");
  // AP-149, AP-157: the shard is the decimal number the host's JC_WASM_SHARD holds.
  expect(Number.isInteger(manifest.status?.shard) && (manifest.status?.shard ?? -1) >= 0, `${app} has its shard`).toBe(true);
  expect(manifest.status?.build?.component, `${app} runs a component by digest`).toMatch(/^sha256:[0-9a-f]{64}$/);
}

test("bike-rebalancing saves a plan on the server, and it is there after a reload with its route sheet", async ({ browser }) => {
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/apps?lang=en`);
  const page = steward.page;
  const app = appOrigin("bike-rebalancing");
  const operator = `e2e T-3346 ${Date.now()}`;
  const api = `${app}api/plans`;
  try {
    await expectPublishedAsWasm(page, "bike-rebalancing");
    await goSignedIn(page, STEWARD, `${app}?lang=en`, (opened) => opened.getByRole("heading", { level: 1, name: "City bike rebalancing planner" }));
    // The main question, answered in the browser from the live counts.
    await expect(page.getByRole("list", { name: "Route" }).getByRole("listitem").first()).toBeVisible({ timeout: 120_000 });

    await page.getByLabel("Van or crew").fill(operator);
    await page.getByRole("button", { name: "Save this plan" }).click();
    // The page holds several status regions (levels, saved plans, loading): pick the one with the answer.
    await expect(page.getByRole("status").filter({ hasText: new RegExp(`Plan saved: \\d+ stops for ${operator}`) })).toBeVisible({ timeout: 60_000 });

    await page.reload({ waitUntil: "load" });
    const saved = page.getByRole("list", { name: "Saved plans" }).getByRole("listitem").filter({ hasText: operator });
    await expect(saved).toHaveCount(1, { timeout: 60_000 });

    const plans = (await (await page.request.get(`${api}?operator=${encodeURIComponent(operator)}`)).json()) as { id: number }[];
    expect(plans).toHaveLength(1);
    const sheet = await page.request.get(`${api}/${plans[0].id}/sheet`);
    expect(sheet.status()).toBe(200);
    const csv = await page.request.get(((await sheet.json()) as { url: string }).url);
    expect(csv.status()).toBe(200);
    expect(await csv.text()).toMatch(/^# e2e T-3346 \d+\nstop,station,name,action,bikes,load_after,leg_km\n/);

    await saved.getByRole("button", { name: /^Mark as driven:/ }).click();
    await expect(page.getByRole("status").filter({ hasText: "The drive is recorded." })).toBeVisible();
    await saved.getByRole("button", { name: /^Delete:/ }).click();
    await page.getByRole("button", { name: "Delete for good" }).click();
    await expect(page.getByText(`No plan is saved for ${operator}.`)).toBeVisible();
  } finally {
    const left = await page.request.get(`${api}?operator=${encodeURIComponent(operator)}`);
    if (left.ok()) {
      for (const plan of (await left.json()) as { id: number }[]) await page.request.delete(`${api}/${plan.id}`);
    }
    await steward.context.close();
  }
});

test("event-day-planner shares a picked day under a link that opens it again, with its calendar file", async ({ browser, page }) => {
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/apps?lang=en`);
  try {
    await expectPublishedAsWasm(steward.page, "event-day-planner");
  } finally {
    await steward.context.close();
  }
  // A public App: an anonymous visitor shares the day.
  const app = appOrigin("event-day-planner");
  await page.goto(`${app}?lang=en`, { waitUntil: "domcontentloaded" });
  const list = page.getByRole("list", { name: "Events of the day" });
  const boxes = list.getByRole("checkbox", { disabled: false });
  await expect(boxes.first()).toBeVisible({ timeout: 120_000 });
  await boxes.first().check();
  await expect(page.getByText(/^1 chosen events in their best order\.$/)).toBeVisible({ timeout: 60_000 });
  const picked = new URL(page.url()).searchParams.get("pick");
  expect(picked).toBeTruthy();

  await page.getByRole("button", { name: "Share this day" }).click();
  const link = page.getByLabel("Link to the shared day");
  await expect(link).toHaveValue(/\?share=[a-z0-9]{12}$/, { timeout: 60_000 });
  const shared = await link.inputValue();
  const code = new URL(shared).searchParams.get("share") ?? "";
  const ics = await page.request.get(`${app}api/itineraries/${code}/ics`);
  expect(ics.status()).toBe(200);
  const file = await page.request.get(((await ics.json()) as { url: string }).url);
  expect(file.status()).toBe(200);
  expect(await file.text()).toMatch(/^BEGIN:VCALENDAR\r\n/);

  // The link in a fresh page: the same day and the same pick, the code gone from the address.
  await page.goto(shared, { waitUntil: "domcontentloaded" });
  await expect(page.getByText(/^Shared day opened: 1 events/)).toBeVisible({ timeout: 60_000 });
  await expect.poll(() => new URL(page.url()).searchParams.get("pick")).toBe(picked);
  expect(new URL(page.url()).searchParams.get("share")).toBeNull();
});

test("air-weather-explorer answers from the hours its server keeps, and a saved comparison opens again from its link", async ({ browser, page }) => {
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/apps?lang=en`);
  try {
    await expectPublishedAsWasm(steward.page, "air-weather-explorer");
  } finally {
    await steward.context.close();
  }
  // A public App: an anonymous visitor.
  const app = appOrigin("air-weather-explorer");
  const series = page.waitForResponse((response) => isAppApi(response, "air-weather-explorer", "series"), { timeout: 120_000 });
  await page.goto(`${app}?lang=en&days=3&window=6`, { waitUntil: "domcontentloaded" });
  expect((await series).status()).toBe(200);
  const answer = page.getByRole("region", { name: "The answer" });
  await expect(answer).toHaveText(/(rises|falls) as .* rises|hardly moves with|Too few common hours|No measurements in the chosen period/, { timeout: 120_000 });

  const exported = page.waitForResponse((response) => isAppApi(response, "air-weather-explorer", "exports"));
  await page.getByRole("button", { name: "Export the hours (CSV)" }).click();
  const csv = await page.request.get(((await (await exported).json()) as { url: string }).url);
  expect(csv.status()).toBe(200);
  expect(await csv.text()).toMatch(/^# air quality station: urn:ngsi-ld:AirQualityObserved:/);

  await page.goto(`${app}?lang=en&days=3&window=6`, { waitUntil: "domcontentloaded" });
  await page.getByLabel("Name of the comparison (optional)").fill(`e2e T-3348 ${Date.now()}`);
  await page.getByRole("button", { name: "Save the comparison" }).click();
  const link = page.getByLabel("Link to the comparison");
  await expect(link).toHaveValue(/\?compare=[a-z0-9]{12}$/, { timeout: 60_000 });
  const shared = await link.inputValue();

  await page.goto(shared, { waitUntil: "domcontentloaded" });
  await expect(page.getByText(/^Saved comparison opened: e2e T-3348 \d+\.$/)).toBeVisible({ timeout: 60_000 });
  await expect.poll(() => new URL(page.url()).searchParams.get("window")).toBe("6");
  expect(new URL(page.url()).searchParams.get("compare")).toBeNull();
});

test("transit-reach keeps the areas from a start stop on the server, the same for a second visitor", async ({ browser, page }) => {
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/apps?lang=en`);
  try {
    await expectPublishedAsWasm(steward.page, "transit-reach");
  } finally {
    await steward.context.close();
  }
  const app = appOrigin("transit-reach");
  await page.goto(`${app}?lang=en`, { waitUntil: "domcontentloaded" });
  await expect(page.locator(".app-summary")).toHaveText(/^Reachable from this point/, { timeout: 120_000 });
  const reached = page.getByRole("list", { name: /^Stops within 30 minutes/ });
  await reached.getByRole("button", { name: /: start from here$/ }).first().click();
  await expect(page.getByText(/^Start stop: /)).toBeVisible();

  // The App asks for the areas, and when the server has not read HSL's network yet (409) it reads it
  // and asks again; then it sends the tab to the file, which drops the answer's body from the page.
  // So the answer is read as it passes, unchanged.
  type Reach = { url: string; stop: string; bands: { minutes: number }[] };
  let resolveKept: (answer: Reach) => void = () => undefined;
  const kept = new Promise<Reach>((resolve) => (resolveKept = resolve));
  await page.route(/\/api\/reach\?stop=/, async (route) => {
    const response = await route.fetch();
    if (response.status() === 200) resolveKept((await response.json()) as Reach);
    await route.fulfill({ response });
  });
  await page.getByRole("button", { name: "Download the areas from the start stop (GeoJSON)" }).click();
  const answer = await kept;
  expect(answer.bands.map((b) => b.minutes)).toEqual([10, 20, 30]);
  const file = await page.request.get(answer.url);
  expect(file.status()).toBe(200);
  expect(await file.text()).toContain('"type":"FeatureCollection"');

  // The same stop again: the server answers from what it kept for this version of the network.
  const again = await page.request.get(`${app}api/reach?stop=${encodeURIComponent(answer.stop)}`);
  expect(again.status()).toBe(200);
  expect(((await again.json()) as { cached: boolean }).cached).toBe(true);
});

test("kpi-forecast has today's forecasts recorded on its server, and a month's report downloads", async ({ browser, page }) => {
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/apps?lang=en`);
  try {
    await expectPublishedAsWasm(steward.page, "kpi-forecast");
  } finally {
    await steward.context.close();
  }
  const app = appOrigin("kpi-forecast");
  const recorded = page.waitForResponse((response) => isAppApi(response, "kpi-forecast", "forecasts") && response.request().method() === "POST", { timeout: 120_000 });
  await page.goto(`${app}?lang=en&days=30`, { waitUntil: "domcontentloaded" });
  expect([200, 201]).toContain((await recorded).status());
  await expect(page.locator(".app-summary")).toHaveText(/^[\d,]+ indicators over the last 30 days/, { timeout: 120_000 });
  await expect(page.getByRole("region", { name: "Earlier forecasts against what came" })).toBeVisible();

  // Recorded once a day: a second ask the same day records nothing.
  const again = await page.request.post(`${app}api/forecasts`, { data: { days: 30 } });
  expect(again.status()).toBe(200);
  expect(((await again.json()) as { already: boolean }).already).toBe(true);

  const report = page.waitForResponse((response) => isAppApi(response, "kpi-forecast", "reports"));
  await page.getByRole("button", { name: "Download forecasts against readings (CSV)" }).click();
  const csv = await page.request.get(((await (await report).json()) as { url: string }).url);
  expect(csv.status()).toBe(200);
  expect(await csv.text()).toContain("kpi,made_on,period_days,for_utc,expected,low_95,high_95,measured,within_95");
});
