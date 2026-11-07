/**
 * The three sample applications on dev, each built from its own repository on the forge (T-2599,
 * AP-14, AP-70, AP-71, AP-75, AP-77, AP-78, SDK-23).
 *
 * The forge bootstrap pushes `apps/helsinki-{bikes,events,alerts}` into `helsinki_<name>` and
 * commits each App pinned to that commit; the lane builds it. Here a person opens each one: the
 * bikes after the edge login, the events without any login (it is public), the alerts with a
 * role of the application. The viewer reads the alerts and their server summary and is refused a
 * write at the gateway; the steward adds an alert through the form and removes it again, so dev
 * keeps nothing. Each catalog card says which commit it serves and links the source, and the
 * viewer may clone it.
 *
 * Every wait is on the thing the step needs, never on the network going idle (T-2452).
 */
import { expect, test } from "@playwright/test";
import type { BrowserContext, Page } from "@playwright/test";
import { STEWARD, VIEWER, csrf, goSignedIn, signIn } from "./portal";

const PROJECT = "helsinki";
/** The origin the apps are served on; the Portal's own host sends `/apps/*` there with a 308. */
const APPS_URL = process.env.APPS_URL ?? "https://dev.joinedcontext.com";
const APPS = ["helsinki-bikes", "helsinki-events", "helsinki-alerts"];
/** The one record the steward writes, a fixed id so a run that died leaves one to find. */
const LOCAL_ID = "t2599-journey";

test.setTimeout(900_000);

/** The double-submit token of the apps origin, which the index set there (AP-84). */
async function appsCsrf(context: BrowserContext): Promise<string> {
  const cookie = (await context.cookies(APPS_URL)).find((each) => each.name === "__Host-jc_csrf");
  if (!cookie) throw new Error("the app's index set no __Host-jc_csrf cookie on the apps origin");
  return cookie.value;
}

/** The `#jc-config` the host served with the index, as the page holds it. */
interface Served {
  slug: string;
  orgDomain: string;
  space: string;
  user?: { roles?: string[] } | null;
}

async function servedConfig(page: Page): Promise<Served> {
  const text = await page.locator("#jc-config").textContent();
  return JSON.parse(text ?? "{}") as Served;
}

// AP-75, AP-77, AP-78: every sample app is served from its own repository's commit, and the
// catalog says which one and links it.
test("each sample application is built from its own repository and the catalog says which commit", async ({
  browser,
}) => {
  const viewer = await signIn(browser, VIEWER, `/projects/${PROJECT}/apps?lang=en`);
  try {
    for (const app of APPS) {
      const answer = await viewer.page.request.get(`/api/v1/projects/${PROJECT}/apps/${app}`);
      expect(answer.status(), `${app} is in the project`).toBe(200);
      const manifest = (await answer.json()) as {
        spec: { source?: { git?: { url?: string; ref?: string } } };
        status?: { build?: { commit?: string }; sourceUrl?: string };
      };
      expect(manifest.spec.source?.git?.url, `${app} names its own repository`).toContain(`/${PROJECT}_${app}.git`);
      expect(manifest.spec.source?.git?.ref, `${app} is pinned to a commit`).toMatch(/^[0-9a-f]{40}$/);
      expect(manifest.status?.build?.commit, `${app} is built at the commit it names`).toBe(manifest.spec.source?.git?.ref);

      const served = new RegExp(`^Served ${(manifest.status?.build?.commit ?? "").slice(0, 7)}`);
      await expect(viewer.page.getByText(served).first(), `${app}'s card says the commit it serves`).toBeVisible();

      // The viewer reads the source (AP-78): the forge answers the clone's first request.
      const refs = await viewer.page.request.get(
        `${(manifest.spec.source?.git?.url ?? "").replace(/\.git$/, "")}/info/refs?service=git-upload-pack`,
      );
      expect(refs.status(), `the viewer may clone ${app}`).toBeLessThan(400);
    }
  } finally {
    await viewer.context.close();
  }
});

// AP-14, AP-70: the bikes open after the edge login and show the stations the space holds.
test("the bikes application opens after the login with the stations of the space", async ({ browser }) => {
  const viewer = await signIn(browser, VIEWER, `/projects/${PROJECT}/apps?lang=en`);
  try {
    // The app opens on its Overview, whose counts are read from the space (T-2670); the station
    // table is the Stations page.
    await goSignedIn(viewer.page, VIEWER, `${APPS_URL}/apps/helsinki-bikes/`, (page) =>
      page.getByRole("region", { name: "Overview" }),
    );
    await viewer.page.getByRole("navigation", { name: "Pages" }).getByRole("button", { name: "Stations" }).click();
    const rows = viewer.page.getByRole("region", { name: "Stations" }).locator("table tbody tr");
    await expect(rows.first()).toBeVisible({ timeout: 120_000 });
  } finally {
    await viewer.context.close();
  }
});

// AP-14, AP-71: a public application opens with no login at all and lists the city's events.
test("the events application opens without a login and lists events", async ({ browser }) => {
  const anonymous = await browser.newContext();
  try {
    const page = await anonymous.newPage();
    await page.goto(`${APPS_URL}/apps/helsinki-events/`, { waitUntil: "load" });
    await expect(page.getByRole("heading", { level: 1, name: "Helsinki events" })).toBeVisible({ timeout: 60_000 });
    await expect(page.locator("#username"), "no login stands in front of a public app").toHaveCount(0);
    const list = page.getByRole("region", { name: "Events" });
    await expect(list.getByRole("listitem").or(list.locator("tr")).first()).toBeVisible({ timeout: 120_000 });
  } finally {
    await anonymous.close();
  }
});

// AP-92, AP-96, SDK-23: the viewer reads the alerts and their summary, is offered no write, and
// a write sent past the page is the gateway's 403.
test("a viewer of the alerts reads them and their summary and is refused a write", async ({ browser }) => {
  const viewer = await signIn(browser, VIEWER, `/projects/${PROJECT}/apps?lang=en`);
  try {
    const page = viewer.page;
    // The app opens on its overview, where the summary function's answer is drawn.
    await goSignedIn(page, VIEWER, `${APPS_URL}/apps/helsinki-alerts/`, (opened) =>
      opened.getByRole("region", { name: "Overview" }),
    );
    const overview = page.getByRole("region", { name: "Overview" });
    // One count per category, whichever the space holds today.
    await expect(overview.getByText(/^[a-zA-Z][\w -]*: \d+$/).first()).toBeVisible({ timeout: 120_000 });
    await expect(overview.getByText(/Alerts stewards added/), "the steward's tile is the steward's").toHaveCount(0);
    await page.getByRole("button", { name: "Alerts" }).click();
    const alerts = page.getByRole("region", { name: "Alerts" });
    await expect(alerts.locator("table tbody tr").first()).toBeVisible({ timeout: 120_000 });
    await expect(alerts.getByRole("button", { name: "New alert" })).toHaveCount(0);
    const config = await servedConfig(page);
    expect(config.user?.roles ?? []).toEqual(["viewer"]);

    const csrf = await appsCsrf(viewer.context);
    const summary = await page.request.post(`${APPS_URL}/apps/helsinki-alerts/api/functions/summary`, {
      headers: { "x-csrf-token": csrf, "content-type": "application/json" },
      data: {},
    });
    expect(summary.status(), await summary.text()).toBe(200);

    const listed = await page.request.get(
      `${APPS_URL}/apps/helsinki-alerts/api/endpoint/${config.slug}/ngsi-ld/v1/entities?type=Alert&limit=1`,
    );
    expect(listed.status()).toBe(200);
    const [first] = (await listed.json()) as { id: string }[];
    expect(first?.id, "the space holds an alert to try").toMatch(/^urn:ngsi-ld:Alert:/);
    const write = await page.request.patch(
      `${APPS_URL}/apps/helsinki-alerts/api/endpoint/${config.slug}/ngsi-ld/v1/entities/${encodeURIComponent(first.id)}/attrs`,
      {
        headers: { "x-csrf-token": csrf, "content-type": "application/json" },
        data: { address: { type: "Property", value: "written by a viewer" } },
      },
    );
    expect(write.status(), "the steward's grant is the steward's alone (AP-96)").toBe(403);
  } finally {
    await viewer.context.close();
  }
});

// AP-09, AP-96: the steward adds an alert through the form and removes it, which is the write the
// viewer was refused.
test("a steward of the alerts adds one through the form and removes it", async ({ browser }) => {
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/apps?lang=en`);
  const page = steward.page;
  page.on("dialog", (dialog) => void dialog.accept());
  try {
    await goSignedIn(page, STEWARD, `${APPS_URL}/apps/helsinki-alerts/`, (opened) =>
      opened.getByRole("region", { name: "Overview" }),
    );
    // SDK-23: the summary counts what stewards added, for a steward alone.
    await expect(page.getByRole("region", { name: "Overview" }).getByText(/^Alerts stewards added: \d+/)).toBeVisible({
      timeout: 120_000,
    });
    await page.getByRole("button", { name: "Alerts" }).click();
    const alerts = page.getByRole("region", { name: "Alerts" });
    expect((await servedConfig(page)).user?.roles ?? []).toContain("steward");

    await alerts.getByRole("button", { name: "New alert" }).click();
    const form = alerts.getByRole("form", { name: "New alert" });
    await form.getByLabel("Local id").fill(LOCAL_ID);
    await form.getByLabel("address", { exact: true }).fill("Senaatintori, Helsinki (T-2599 journey)");
    await form.getByLabel("category", { exact: true }).fill("event");
    await form.getByRole("button", { name: "Save" }).click();
    await expect(form).toHaveCount(0);

    const row = alerts.getByRole("table").getByText("Senaatintori, Helsinki (T-2599 journey)");
    await expect(row).toBeVisible({ timeout: 60_000 });
    await row.click();
    await alerts.getByRole("button", { name: "Delete" }).click();
    await expect(row).toHaveCount(0, { timeout: 60_000 });
  } finally {
    // A run that died between the save and the delete leaves the one fixed record: remove it.
    const config = await servedConfig(page).catch(() => null);
    if (config?.slug) {
      await page.request.delete(
        `${APPS_URL}/apps/helsinki-alerts/api/endpoint/${config.slug}/ngsi-ld/v1/entities/${encodeURIComponent(`urn:ngsi-ld:Alert:${config.orgDomain}:${config.space}:${LOCAL_ID}`)}`,
        { headers: { "x-csrf-token": await appsCsrf(steward.context).catch(() => "") } },
      );
    }
    await steward.context.close();
  }
});

// AP-124 (T-2940): the rename of old App shapes is a step a person takes on the Apps page. Nothing
// is proposed here: a viewer is refused before anything is read, and the steward's call answers 409
// while no App is written in an old shape; with one left, the notice names it and is only read.
test("the rename of old App shapes is offered only while one is left, and a viewer may not propose it", async ({
  browser,
}) => {
  const rename = `/api/v1/projects/${PROJECT}/apps/rename-shapes`;
  const viewer = await signIn(browser, VIEWER, `/projects/${PROJECT}/apps?lang=en`);
  try {
    const refused = await viewer.page.request.post(rename, { headers: { "x-csrf-token": await csrf(viewer.context) } });
    expect(refused.status(), "a viewer may not propose an App").toBe(403);
  } finally {
    await viewer.context.close();
  }
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/apps?lang=en`);
  try {
    const { page, context } = steward;
    await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
    const offer = page.getByRole("button", { name: "Propose the rename" });
    if ((await offer.count()) === 0) {
      const nothing = await page.request.post(rename, { headers: { "x-csrf-token": await csrf(context) } });
      expect(nothing.status(), "no App carries an old shape, so there is nothing to propose").toBe(409);
    } else {
      await expect(page.getByText(/ becomes ui/).first()).toBeVisible();
    }
  } finally {
    await steward.context.close();
  }
});

// AP-171: an App's earlier builds are listed with the one it serves; restoring that one again is
// refused, and so is any restore by a viewer. Opening the merge request of an earlier build is the
// route's own test (app_build_tests), so dev's sample repositories keep no restore branch.
test("a steward reads an App's earlier builds, the build it serves is not restored again, and a viewer may not restore", async ({
  browser,
}) => {
  const app = "helsinki-bikes";
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/apps?lang=en`);
  let served = "";
  try {
    const { page, context } = steward;
    const answer = await page.request.get(`/api/v1/projects/${PROJECT}/apps/${app}/builds`);
    expect(answer.status(), "the steward reads the builds").toBe(200);
    const listed = (await answer.json()) as { builds: { commit: string; current: boolean }[]; dataNote: string };
    expect(listed.dataNote, "Restore says what it does not roll back").not.toBe("");
    const current = listed.builds.find((build) => build.current);
    expect(current, `${app}'s build at status.build.commit is listed as the current one`).toBeDefined();
    served = current?.commit ?? "";
    const again = await page.request.post(`/api/v1/projects/${PROJECT}/apps/${app}/restore`, {
      headers: { "x-csrf-token": await csrf(context) },
      data: { commit: served },
    });
    expect(again.status(), "the build the App serves is not restored again").toBe(409);
  } finally {
    await steward.context.close();
  }
  const viewer = await signIn(browser, VIEWER, `/projects/${PROJECT}/apps?lang=en`);
  try {
    const refused = await viewer.page.request.post(`/api/v1/projects/${PROJECT}/apps/${app}/restore`, {
      headers: { "x-csrf-token": await csrf(viewer.context) },
      data: { commit: served },
    });
    expect(refused.status(), "a viewer may not restore an App").toBe(403);
  } finally {
    await viewer.context.close();
  }
});

// T-6402: the dashboard editor of an App reads its src/dashboard.json and the attributes it may
// offer; a sample App keeps no such file, so Edit is not offered and a proposal is refused with
// the reason, and a viewer may not propose an edit at all. The edit itself, committed and
// proposed as a Change, is the route's own test (app_dashboard_tests).
test("a steward reads an App's dashboard file, an App without one is not edited, and a viewer may not propose an edit", async ({
  browser,
}) => {
  // DASHBOARD_APP names an App that keeps the file (T-6429), for the branch of this test that reads it.
  const app = process.env.DASHBOARD_APP ?? "helsinki-bikes";
  const route = `/api/v1/projects/${PROJECT}/apps/${app}/dashboard`;
  const edit = { config: { grid: { source: { kind: "endpoint", slug: "bikes" }, type: "BikeHireDockingStation" } }, dryRun: true };
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/apps/${app}?lang=en`);
  try {
    const { page, context } = steward;
    const answer = await page.request.get(route);
    expect(answer.status(), "the steward reads the dashboard").toBe(200);
    const read = (await answer.json()) as { path: string; config: unknown; edit: { allowed: boolean; reason?: string } };
    expect(read.path).toBe("src/dashboard.json");
    if (read.config === null) {
      expect(read.edit.allowed, "an App without the file is not edited").toBe(false);
      expect(read.edit.reason ?? "").toContain("src/dashboard.json");
      const proposed = await page.request.post(route, { headers: { "x-csrf-token": await csrf(context) }, data: edit });
      expect(proposed.status(), "nothing to edit").toBe(409);
    }
    await page.getByText("Edit the dashboard").click();
    await expect(page.getByText(read.config === null ? "This dashboard cannot be edited here" : "Columns").first()).toBeVisible();
  } finally {
    await steward.context.close();
  }
  const viewer = await signIn(browser, VIEWER, `/projects/${PROJECT}/apps?lang=en`);
  try {
    const refused = await viewer.page.request.post(route, { headers: { "x-csrf-token": await csrf(viewer.context) }, data: edit });
    expect(refused.status(), "a viewer may not propose a dashboard edit").toBe(403);
  } finally {
    await viewer.context.close();
  }
});
