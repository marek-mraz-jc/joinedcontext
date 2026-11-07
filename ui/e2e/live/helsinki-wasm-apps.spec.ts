/**
 * The Helsinki apps that compute in WebAssembly, on dev (T-3331..T-3333, AP-142): each opened by an
 * anonymous visitor, as a public App is, answers its question on arrival with live data; its module
 * comes from its own bundle on the App's own origin (`<app>.apps.<domain>`, where `/apps/<app>/`
 * sends a visitor) as `application/wasm`, and the page runs under a CSP that allows
 * `'wasm-unsafe-eval'` and nothing wider. The time to the first answer is printed for the task.
 * Read only.
 */
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

const APPS_URL = process.env.APPS_URL ?? "https://dev.joinedcontext.com";

test.setTimeout(240_000);

/** Opens an App anonymously and watches its module and its CSP on the way. */
async function open(page: Page, app: string, search: string) {
  const wasm: { type: string; url: string }[] = [];
  page.on("response", (response) => {
    if (response.url().endsWith(".wasm")) wasm.push({ type: response.headers()["content-type"] ?? "", url: response.url() });
  });
  const started = Date.now();
  const response = await page.goto(`${APPS_URL}/apps/${app}/${search}`, { waitUntil: "domcontentloaded" });
  const csp = response?.headers()["content-security-policy"] ?? "";
  return { wasm, csp, started };
}

/** The next Monday to Friday after today, `YYYY-MM-DD` in Helsinki time. */
function nextWeekday(): string {
  const today = new Date().toLocaleDateString("sv-SE", { timeZone: "Europe/Helsinki" });
  const day = new Date(`${today}T12:00:00Z`);
  do {
    day.setUTCDate(day.getUTCDate() + 1);
  } while ([0, 6].includes(day.getUTCDay()));
  return day.toISOString().slice(0, 10);
}

function expectWasmPolicy(page: Page, csp: string, wasm: { type: string; url: string }[], app: string) {
  const origin = new URL(page.url()).origin;
  expect(new URL(origin).hostname, "the App's own origin").toMatch(new RegExp(`^${app}\\.apps\\.`));
  expect(csp, "the static host's CSP").toContain("'wasm-unsafe-eval'");
  expect(csp.split(/[\s;]+/), "no eval of JavaScript").not.toContain("'unsafe-eval'");
  expect(wasm.length, "the module was loaded").toBeGreaterThan(0);
  for (const each of wasm) {
    expect(each.type).toContain("application/wasm");
    expect(new URL(each.url).origin, "the module of the App's own bundle").toBe(origin);
  }
}

test("alerts-heatmap answers where and when with the live alerts", async ({ page }) => {
  const { wasm, csp, started } = await open(page, "alerts-heatmap", "?lang=en");
  await expect(page.locator(".app-summary")).toHaveText(/^[\d,]+ alerts from \d{1,2} \w{3} \d{4} to \d{1,2} \w{3} \d{4}\./, { timeout: 120_000 });
  console.log(`alerts-heatmap: first answer after ${Date.now() - started} ms`);
  await expect(page.getByTestId("jc-map").locator("canvas")).toHaveCount(1);
  // The repeat places, or the App's own line that the live alerts hold none (T-6192).
  await expect(
    page.getByRole("list", { name: "Places alerts keep coming back to" }).or(page.getByText(/^No place repeats: nowhere holds three alerts within \d+ metres\.$/)),
  ).toBeVisible();
  expectWasmPolicy(page, csp, wasm, "alerts-heatmap");
});

test("event-day-planner plans a day of the live events", async ({ page }) => {
  // The next weekday: today's suggested day starts from now and takes events of up to three hours,
  // so on a weekend afternoon the live feed can leave it empty with all-day events only (T-6192).
  const { wasm, csp, started } = await open(page, "event-day-planner", `?lang=en&day=${nextWeekday()}`);
  const plan = page.getByRole("region", { name: "My day" }).getByRole("list", { name: "The plan" });
  await expect(plan.getByRole("listitem").first()).toBeVisible({ timeout: 120_000 });
  console.log(`event-day-planner: first answer after ${Date.now() - started} ms`);
  await expect(page.getByText(/A suggested day/)).toBeVisible();
  await expect(page.getByRole("list", { name: "Events of the day" }).getByRole("listitem").first()).toBeVisible();
  expectWasmPolicy(page, csp, wasm, "event-day-planner");
});

test("air-weather-explorer says how the weather moves the live air", async ({ page }) => {
  const { wasm, csp, started } = await open(page, "air-weather-explorer", "?lang=en&days=7");
  const answer = page.getByRole("region", { name: "The answer" });
  await expect(answer).toHaveText(/(rises|falls) as .* rises|hardly moves with|Too few common hours|No measurements in the chosen period/, { timeout: 120_000 });
  console.log(`air-weather-explorer: first answer after ${Date.now() - started} ms: ${await answer.textContent()}`);
  await expect(page.getByTestId("jc-map").locator("canvas")).toHaveCount(1);
  expectWasmPolicy(page, csp, wasm, "air-weather-explorer");
});

test("kpi-forecast answers how the indicators move with their live history", async ({ page }) => {
  const { wasm, csp, started } = await open(page, "kpi-forecast", "?lang=en");
  await expect(page.locator(".app-summary")).toHaveText(/^[\d,]+ indicators over the last 30 days: [\d,]+ rising, [\d,]+ falling, [\d,]+ flat\./, { timeout: 120_000 });
  console.log(`kpi-forecast: first answer after ${Date.now() - started} ms`);
  await expect(page.getByRole("list", { name: "Indicators" }).getByRole("button").first()).toHaveAttribute("aria-pressed", "true");
  // The temporal read is the App's own grant: a refusal would show here.
  await expect(page.getByText(/history could not be read/)).toHaveCount(0);
  expectWasmPolicy(page, csp, wasm, "kpi-forecast");
});

test("transit-reach answers how far one gets from Rautatientori with the live vehicles", async ({ page }) => {
  const { wasm, csp, started } = await open(page, "transit-reach", "?lang=en");
  await expect(page.locator(".app-summary")).toHaveText(/^Reachable from this point: 10 min [\d.]+ km², 20 min [\d.]+ km², 30 min [\d.]+ km²\. Stops within 30 minutes: [\d,]+ of [\d,]+\.$/, {
    timeout: 120_000,
  });
  console.log(`transit-reach: first answer after ${Date.now() - started} ms: ${await page.locator(".app-summary").textContent()}`);
  await expect(page.getByTestId("jc-map").locator("canvas")).toHaveCount(1);
  // The temporal read is the App's own grant: a refusal would show here.
  await expect(page.getByText(/history could not be read/)).toHaveCount(0);
  expectWasmPolicy(page, csp, wasm, "transit-reach");
});
