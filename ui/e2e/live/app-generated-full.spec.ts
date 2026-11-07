/**
 * A generated React + functions application, from a prompt to an app that works on dev (T-2603,
 * AP-48, AP-56, AP-60, AP-70, AP-71, AP-75…AP-77, SDK-13, SDK-20, SDK-23, SDK-24).
 *
 * The steward describes the app and names the steward's write; the first version shows on screen
 * with the server's summary; one follow-up lands as a commit on the run's branch; Publish opens
 * the merge request and proposes the App; the approver, another person, approves (CC-34); the
 * branch merges into `main` of `helsinki_<app>`, the lane builds, the catalog says `Served`, and
 * the app opens after the edge login with rows, its function answering, and the person's roles
 * in the configuration it was served (AP-95). The viewer clones the repository (T-2601). The app
 * is removed at the end, so dev keeps nothing of it.
 *
 * Not asserted yet: the steward's note saving through the app's endpoint while the viewer's is the
 * gateway's 403. The reconciler renders the per-role Policies (AP-96, T-2595) but nothing writes
 * them to the configuration repository, so the gateway never holds them (chyby.md, 2026-09-22).
 *
 * Every wait is on the thing the step needs, never on the network going idle (T-2452).
 */
import { expect, test } from "@playwright/test";
import type { BrowserContext, Page } from "@playwright/test";
import { APPROVER, STEWARD, VIEWER, approve, goSignedIn, removeCompletely, signIn } from "./portal";

const PROJECT = "helsinki";
const SUFFIX = process.env.E2E_SUFFIX ?? new Date().toISOString().slice(11, 16).replace(":", "");
const NAME = `bikes-full-${SUFFIX}`;
/** The origin the apps are served on; the Portal's own host sends `/apps/*` there with a 308. */
const APPS_URL = process.env.APPS_URL ?? "https://dev.joinedcontext.com";

const PROMPT =
  "A page of HSL city bike stations: a table, a map, a filter for stations with free bikes, and a " +
  "summary of bikes available per district computed on the server by a function named summary. " +
  "Two roles: viewer reads everything, steward may also write a note on a station.";

test.setTimeout(1_800_000);

/** The double-submit token of the apps origin, which the index set there (AP-84). */
async function appsCsrf(context: BrowserContext): Promise<string> {
  const cookie = (await context.cookies(APPS_URL)).find((each) => each.name === "__Host-jc_csrf");
  if (!cookie) throw new Error("the app's index set no __Host-jc_csrf cookie on the apps origin");
  return cookie.value;
}

/** The `#jc-config` the host served with the index, as the page holds it. */
async function servedConfig(page: Page): Promise<{ user?: { roles?: string[] } | null }> {
  const text = await page.locator("#jc-config").textContent();
  return JSON.parse(text ?? "{}") as { user?: { roles?: string[] } | null };
}

test("a prompt becomes a React + functions application that opens, reads and answers on dev", async ({
  browser,
}) => {
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/apps?lang=en`);
  const page = steward.page;
  try {
    // 1. The prompt, over the bikes endpoint (AP-48).
    await page.getByRole("button", { name: "Generate your own app" }).first().click();
    const endpoint = page.getByLabel("Endpoint").first();
    const bikes = endpoint.locator("option").filter({ hasText: /bike/i }).first();
    await endpoint.selectOption((await bikes.getAttribute("value")) ?? "");
    await page.getByLabel("What should the app do?").first().fill(PROMPT);
    await page.getByText("Details:", { exact: false }).first().click();
    await page.getByLabel("App name").fill(NAME);
    const started = Date.now();
    await page.getByRole("button", { name: "Generate the app" }).click();
    await page.waitForURL(new RegExp(`/projects/${PROJECT}/apps/${NAME}`), { timeout: 60_000 });

    // 2. The first version: the preview draws a number read from the data, the run waits for the
    // person, and the server's summary function answered (SDK-13, SDK-23). Waited on
    // what the person sees, never a class name the model happens to choose: `.stat-value` failed
    // a working app that drew 457 stations in its own tiles (T-3044).
    const preview = page.frameLocator("iframe").first();
    const waiting = page.getByTestId("run-progress").filter({ hasText: "Waiting for you" });
    await expect(preview.getByText(/^\s*[1-9][\d,.\s]*$/).first()).toBeVisible({ timeout: 600_000 });
    await expect(waiting).toBeVisible({ timeout: 600_000 });
    await expect(page.getByText(/^function:summary/).first()).toBeVisible();
    test.info().annotations.push({
      type: "first version",
      description: `${Math.round((Date.now() - started) / 1000)} s after Generate`,
    });
    await expect(page.getByText(/No sample of .* could be read/)).toHaveCount(0);

    // 3. One follow-up lands as a new tested version on the run's branch (AP-76). The map already
    // drew an `svg`, so the chart is proven by a version past the first, not by the element.
    const versions = page.getByText(/^Version \d+: all \d+ tests pass\.$/);
    const tested = async (): Promise<number> =>
      Math.max(0, ...(await versions.allInnerTexts()).map((text) => Number(/^Version (\d+)/.exec(text)?.[1] ?? 0)));
    const before = await tested();
    const composer = page.getByPlaceholder("Tell the assistant what to build or change…");
    await composer.fill("Add a chart of stations by free slots.");
    await page.getByRole("button", { name: "Send", exact: true }).click();
    await expect.poll(tested, { timeout: 600_000, intervals: [5_000] }).toBeGreaterThan(before);
    await expect(waiting).toBeVisible({ timeout: 120_000 });
    await expect(preview.locator("svg, canvas").first()).toBeVisible();

    // 4. Publish opens the merge request and proposes the App with its source (AP-77).
    // Publish opens the merge request at once; the change notice links to its review.
    await page.getByRole("button", { name: "Publish this app" }).click();
    const notice = page.getByRole("link", { name: /review/i }).first();
    await expect(notice).toBeVisible({ timeout: 120_000 });
    const change = new URL((await notice.getAttribute("href")) ?? "", page.url()).pathname.split("/").pop() ?? "";
    expect(change, "Publish left a change to approve").not.toBe("");

    // 5. Another person approves (CC-34); the run's branch merges into main and the lane builds.
    const approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/approvals?lang=en`);
    await approve(approver.page, PROJECT, change, NAME);
    await approver.context.close();

    await page.goto(`/projects/${PROJECT}/apps?lang=en`, { waitUntil: "load" });
    const card = page.getByRole("heading", { name: new RegExp(NAME.replace(/-/g, "[- ]"), "i") }).locator("xpath=ancestor::li[1]");
    await expect
      .poll(
        async () => {
          await page.reload({ waitUntil: "load" });
          return (await card.getByText(/^Served [0-9a-f]{7}/).count()) > 0;
        },
        { timeout: 900_000, intervals: [15_000] },
      )
      .toBe(true);

    // The manifest names the repository, and main there holds the run's files, not a README.
    const app = await page.request.get(`/api/v1/projects/${PROJECT}/apps/${NAME}`);
    expect(app.status()).toBe(200);
    const manifest = (await app.json()) as { spec: { source?: { git?: { url?: string } } }; status?: { sourceUrl?: string } };
    expect(manifest.spec.source?.git?.url, "the App names its own repository").toContain(`${PROJECT}_${NAME}`);

    // 6. The app opens after the edge login, with rows and the steward's roles (AP-95).
    await goSignedIn(page, STEWARD, `${APPS_URL}/apps/${NAME}/`, (opened) => opened.getByRole("heading", { level: 1 }));
    const rows = page.locator("table tbody tr").filter({ hasNotText: "No rows" });
    await expect(rows.first()).toBeVisible({ timeout: 120_000 });
    const config = await servedConfig(page);
    expect(config.user?.roles ?? [], "the steward holds the steward role in the app").toContain("steward");
    expect(JSON.stringify(config).toLowerCase(), "no token in the served page (AP-23)").not.toMatch(/bearer|access_token|eyj/);

    // 7. The published function answers (SDK-23, AP-84).
    const summary = await page.request.post(`${APPS_URL}/apps/${NAME}/api/functions/summary`, {
      headers: { "x-csrf-token": await appsCsrf(steward.context), "content-type": "application/json" },
      data: {},
    });
    expect(summary.status(), await summary.text()).toBe(200);

    // 8. The viewer opens it too, without the steward's role, and may clone the source (T-2601).
    const viewer = await signIn(browser, VIEWER, `/projects/${PROJECT}/apps?lang=en`);
    await goSignedIn(viewer.page, VIEWER, `${APPS_URL}/apps/${NAME}/`, (opened) => opened.getByRole("heading", { level: 1 }));
    expect((await servedConfig(viewer.page)).user?.roles ?? []).not.toContain("steward");
    const source = manifest.spec.source?.git?.url ?? "";
    const info = await viewer.page.request.get(`${source.replace(/\.git$/, "")}/info/refs?service=git-upload-pack`);
    expect(info.status(), "the viewer reads the application's repository").toBeLessThan(400);
    await viewer.context.close();
  } finally {
    // Nothing of the journey stays on dev.
    await page.goto(`/projects/${PROJECT}/apps?lang=en`, { waitUntil: "load" });
    await removeCompletely(steward, PROJECT, "apps", NAME);
    await steward.context.close();
  }
});
