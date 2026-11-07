import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// The Retention tab's history opt-in and "Rows of history" (T-5058, DM-132): a type no rule names
// keeps no history, a type that keeps history says which rows the broker writes, and an owner
// picks the mode with a hint and an example. The server checks the value again on the dry run.
// `vite preview` has no Portal API behind it, so the API is answered in the browser. With
// SHOTS_DIR set, the 1440 px and 400 px screenshots of each state are written there.

const PROJECT = "helsinki";
const SPACE = "helsinki";
const BASE = `/api/v1/projects/${PROJECT}/spaces/${SPACE}`;

const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const SPACE_MANIFEST = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ContextSpace",
  metadata: { name: SPACE, namespace: PROJECT, title: { en: "Helsinki city context" } },
  spec: {
    retention: [
      {
        entityType: "BikeHireDockingStation",
        history: "P30D",
        record: "onChange",
        heartbeat: "PT1H",
        attributes: { dateModified: { record: "never" } },
      },
      { entityType: "AirQualityObserved", history: "P30D" },
      { entityType: "Vehicle", current: "PT5M", history: "PT5M" },
    ],
  },
  status: { phase: "Live" },
};
const OWNER_GRANT = { rule: { kinds: ["ContextSpace"], verbs: ["read", "propose", "approve"] } };

type Sent = { method: string; search: string; body: unknown };

async function stubApi(page: Page, refuse: boolean): Promise<Sent[]> {
  const sent: Sent[] = [];
  await page.route("**/{api,cs}/**", async (route) => {
    const url = new URL(route.request().url());
    const path = url.pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) {
      return json({ subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", roles: ["portal-approver"] });
    }
    if (path.endsWith("/permissions/me")) return json({ project: PROJECT, bootstrap: false, grants: [OWNER_GRANT] });
    if (path === "/api/v1/projects") return json({ kind: "ProjectList", items: [{ name: PROJECT }] });
    if (path === `${BASE}/retention/deleted`) return json({ counts: [{ entityType: "Vehicle", kind: "history", count: 1204 }] });
    if (path === `${BASE}/temporal-audit`) return json({ rows: [], unread: [], verdicts: {} });
    if (path === BASE && route.request().method() === "PUT") {
      sent.push({ method: "PUT", search: url.search, body: route.request().postDataJSON() });
      if (refuse) {
        return json(
          {
            type: "about:blank",
            title: "Unprocessable Entity",
            status: 422,
            detail: "spec.retention[0].record: record is every, observed, never or onChange (DM-108, DM-132)",
          },
          422,
        );
      }
      return json({ ...SPACE_MANIFEST, status: {} });
    }
    if (path === BASE) return json(SPACE_MANIFEST);
    return json(list([]));
  });
  return sent;
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.SHOTS_DIR;
  if (dir) await page.screenshot({ path: `${dir}/${name}.png`, fullPage: true });
}

async function open(page: Page, width: number, height: number, refuse = false): Promise<Sent[]> {
  await page.setViewportSize({ width, height });
  const sent = await stubApi(page, refuse);
  await page.goto(`/projects/${PROJECT}/spaces/${SPACE}?tab=retention&lang=en`);
  await expect(page.getByRole("table", { name: "How long each type's data is kept" })).toBeVisible();
  return sent;
}

async function noSideways(page: Page): Promise<void> {
  const sideways = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
  expect(sideways).toBeLessThanOrEqual(0);
  expect(await axeViolations(page)).toEqual([]);
}

for (const [label, width, height] of [["desktop-1440", 1440, 900], ["phone-400", 400, 860]] as const) {
  test(`the rows of history each type writes (${label})`, async ({ page }) => {
    await open(page, width, height);
    const table = page.getByRole("table", { name: "How long each type's data is kept" });
    const bikes = table.getByRole("row", { name: /BikeHireDockingStation/ });
    await expect(bikes.getByText("Only changes")).toBeVisible();
    await expect(bikes.getByText("a row at least every hour")).toBeVisible();
    await expect(table.getByRole("row", { name: /AirQualityObserved/ }).getByText("Newer observations")).toBeVisible();
    await noSideways(page);
    await table.scrollIntoViewIfNeeded();
    await shot(page, `retention-record-filled-${label}`);
  });

  test(`an owner picks the rows of history and proposes them (${label})`, async ({ page }) => {
    const sent = await open(page, width, height);
    await page.getByRole("button", { name: "Change retention" }).click();
    const air = page.getByRole("combobox", { name: "Rows of history of AirQualityObserved" });
    await expect(air).toHaveValue("observed");
    await expect(page.getByText(/Rows of history: Newer observations writes a row only when observedAt is newer/)).toBeVisible();
    await air.selectOption("every");
    await page.getByRole("combobox", { name: "Row at least every, for BikeHireDockingStation" }).selectOption("PT15M");
    await noSideways(page);
    await shot(page, `retention-record-editing-${label}`);
    await page.getByRole("button", { name: "Propose", exact: true }).click();
    await expect(page.getByText("Proposed as a change: it applies once it is approved.")).toBeVisible();
    const put = sent.find((one) => one.search === "");
    expect((put?.body as { spec: { retention: unknown } }).spec.retention).toEqual([
      { entityType: "AirQualityObserved", history: "P30D", record: "every" },
      {
        entityType: "BikeHireDockingStation",
        history: "P30D",
        record: "onChange",
        heartbeat: "PT15M",
        attributes: { dateModified: { record: "never" } },
      },
      { entityType: "Vehicle", current: "PT5M", history: "PT5M" },
    ]);
  });

  test(`a value the server refuses is named in the tab (${label})`, async ({ page }) => {
    await open(page, width, height, true);
    await page.getByRole("button", { name: "Change retention" }).click();
    await page.getByRole("combobox", { name: "Rows of history of AirQualityObserved" }).selectOption("every");
    await page.getByRole("button", { name: "Propose", exact: true }).click();
    await expect(page.getByText(/spec\.retention\[0\]\.record: record is every, observed, never or onChange/)).toBeVisible();
    await noSideways(page);
    await shot(page, `retention-record-refused-${label}`);
  });
}
