import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// The temporal audit on a space's Retention tab (T-4844): each attribute a pipeline writes, how
// the space records it, where the time of a row comes from and a verdict in words. `vite preview`
// has no Portal API behind it, so the API is answered in the browser. With SHOTS_DIR set, the
// 1440 px and 400 px screenshots of each state are written there for the task's evidence.

const PROJECT = "helsinki";
const SPACE = "helsinki";
const AUDIT = `/api/v1/projects/${PROJECT}/spaces/${SPACE}/temporal-audit`;

const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const SPACE_MANIFEST = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ContextSpace",
  metadata: { name: SPACE, namespace: PROJECT, title: { en: "Helsinki city context" } },
  spec: {
    retention: [
      { entityType: "Vehicle", current: "PT5M", history: "PT1H", attributes: { transportMode: { record: "onChange", heartbeat: "PT15M" } } },
      { entityType: "AirQualityObserved", history: "P30D" },
    ],
  },
  status: { phase: "Live" },
};

const row = (over: Record<string, unknown>) => ({
  pipeline: "hsl-hfp-vehicles",
  entityType: "Vehicle",
  attribute: "speed",
  record: "every",
  observedAt: ["mapped"],
  unitCode: ["MTS"],
  datasetId: [],
  history: "PT1H",
  lifetime: ["expiresAt in mapping"],
  verdict: "ok",
  ...over,
});

const FILLED = {
  rows: [
    row({ attribute: "location", unitCode: [] }),
    row({}),
    row({ attribute: "transportMode", record: "onChange", observedAt: ["received"], unitCode: [] }),
    row({ attribute: "vehicleType", observedAt: ["received"], unitCode: [], verdict: "duplicates-rows" }),
    row({ pipeline: "air-quality", entityType: "AirQualityObserved", attribute: "pm10", unitCode: ["GQ"], history: "P30D", lifetime: [] }),
    row({ pipeline: "air-quality", entityType: "AirQualityObserved", attribute: "dateObserved", observedAt: ["constant"], unitCode: [], history: "P30D", lifetime: [], verdict: "loses-history" }),
  ],
  unread: [],
  verdicts: { ok: 4, "loses-history": 1, "unbounded-growth": 0, "duplicates-rows": 1, "stamped-on-receive": 0 },
};

async function stubApi(page: Page, audit: { status: number; body: unknown }): Promise<void> {
  await page.route("**/{api,cs}/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) {
      return json({ subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", roles: ["portal-editor"] });
    }
    if (path === "/api/v1/projects") return json({ kind: "ProjectList", items: [{ name: PROJECT }] });
    if (path === AUDIT) return json(audit.body, audit.status);
    if (path.endsWith("/retention/deleted")) return json({ counts: [] });
    if (path.endsWith(`/spaces/${SPACE}`)) return json(SPACE_MANIFEST);
    return json(list([]));
  });
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.SHOTS_DIR;
  if (dir) await page.screenshot({ path: `${dir}/${name}.png`, fullPage: true });
}

const STATES = [
  ["filled", { status: 200, body: FILLED }],
  ["empty", { status: 200, body: { rows: [], unread: [], verdicts: {} } }],
  ["unread", { status: 200, body: { ...FILLED, unread: ["linked-events"] } }],
  ["error", { status: 503, body: { type: "about:blank", title: "Service Unavailable", status: 503, detail: "git forge is not configured" } }],
] as const;

for (const [label, width, height] of [["desktop-1440", 1440, 900], ["phone-400", 400, 860]] as const) {
  for (const [state, audit] of STATES) {
    test(`the temporal audit, ${state} (${label})`, async ({ page }) => {
      await page.setViewportSize({ width, height });
      await stubApi(page, audit);
      await page.goto(`/projects/${PROJECT}/spaces/${SPACE}?tab=retention&lang=en`);
      const heading = page.getByRole("heading", { name: "How history rows are created" });
      await expect(heading).toBeVisible();
      if (state === "filled" || state === "unread") {
        const table = page.getByRole("table", { name: "How each pipeline's writes become history rows" });
        await expect(table.getByRole("row", { name: /vehicleType/ }).getByText("Duplicates rows")).toBeVisible();
        await expect(table.getByRole("row", { name: /dateObserved/ }).getByText("A fixed time")).toBeVisible();
      }
      if (state === "unread") await expect(page.getByText(/linked-events could not be read/)).toBeVisible();
      if (state === "empty") await expect(page.getByText("No pipeline you can read writes this space")).toBeVisible();
      if (state === "error") await expect(page.getByText(/git forge is not configured/)).toBeVisible();
      // Nothing wider than the screen: the page body never scrolls sideways (UI-27).
      const sideways = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
      expect(sideways).toBeLessThanOrEqual(0);
      expect(await axeViolations(page)).toEqual([]);
      await heading.scrollIntoViewIfNeeded();
      await shot(page, `temporal-audit-${state}-${label}`);
    });
  }
}
