import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";
import { manifestReadsAsRest } from "./manifestReads";

// The Data quality tab of a space through the OData service's Portal doors (T-7105): the daily
// report (portal_space_quality), the checks on write (portal_space_quality_checks) and the drift
// list (portal_drift_list), each answered by its REST door's stub as the service wraps it, at
// 1440 and 400 px: filled, empty, refused and failed. With SHOTS_DIR set, each state's screenshot
// is written there for the task's evidence.
test.beforeEach(({ page }) => manifestReadsAsRest(page));

const PROJECT = "helsinki";
const SPACE = "bikes";
const AT = `/api/v1/projects/${PROJECT}/spaces/${SPACE}`;

const list = (items: unknown[], metadata: unknown = {}) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  metadata,
  items,
});

const SPACE_MANIFEST = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ContextSpace",
  metadata: { name: SPACE, namespace: PROJECT, title: { en: "City bikes" } },
  spec: {},
  status: { phase: "Live" },
};

const REPORT = {
  observedAt: "2026-09-25T02:00:00Z",
  checked: 1200,
  invalid: 16,
  truncated: false,
  rules: [
    { rule: "sh:minCount", path: "name", count: 12, examples: ["urn:ngsi-ld:Station:hel.fi:bikes:001"] },
    { rule: "type", path: "", count: 4, examples: [] },
  ],
  freshness: [
    { pipeline: "bikes-feed", type: "BikeHireDockingStation", newest: "2026-09-25T01:58:00Z", targetSeconds: 615, state: "fresh", paused: false },
    { pipeline: "weather", type: "", newest: "2026-09-23T01:00:00Z", targetSeconds: 93600, state: "stale", paused: true },
  ],
};

const counts = (pass: number, reject = 0) => ({ pass, reject, flag: 0, quarantine: 0, passRate: pass / (pass + reject) });
const CHECKS = {
  at: "2026-09-25T08:00:00Z",
  windows: { "24h": "2026-09-24T08:00:00Z", "7d": "2026-09-18T08:00:00Z" },
  rules: [
    { type: "BikeHireDockingStation", rule: "sh:minCount", attribute: "name", "24h": counts(980, 12), "7d": counts(7000, 40), lastRejected: "2026-09-25T07:40:00Z" },
  ],
};

const DRIFTED = {
  space: SPACE,
  id: "urn:ngsi-ld:BikeHireDockingStation:hel.fi:bikes:001",
  drift: "MODIFIED",
  diff: [{ path: "availableBikeNumber.value", declared: 12, live: 4 }],
  resolutions: ["revert", "adopt"],
  source: `projects/${PROJECT}/spaces/${SPACE}/entities/seed/stations.json`,
};

type Answer = { status: number; body: unknown };
const problem = (status: number, detail: string) => ({ type: "about:blank", title: "refused", status, detail });

const STATES: [string, { quality: Answer; checks: Answer; drift: Answer }][] = [
  [
    "filled",
    {
      quality: { status: 200, body: REPORT },
      checks: { status: 200, body: CHECKS },
      drift: { status: 200, body: list([DRIFTED], { observedAt: "2026-09-25T08:00:00Z" }) },
    },
  ],
  [
    "empty",
    {
      quality: { status: 200, body: {} },
      checks: { status: 200, body: { ...CHECKS, rules: [] } },
      drift: { status: 200, body: list([]) },
    },
  ],
  [
    "refused",
    {
      quality: { status: 403, body: problem(403, "you do not read the space bikes") },
      checks: { status: 403, body: problem(403, "you do not read the space bikes") },
      drift: { status: 403, body: problem(403, "you are no member of helsinki") },
    },
  ],
  [
    "error",
    {
      quality: { status: 503, body: problem(503, "the broker is not reachable") },
      checks: { status: 503, body: problem(503, "the gateway's counters are not reachable") },
      drift: { status: 503, body: problem(503, "the drift scan is not reachable") },
    },
  ],
];

async function stubApi(page: Page, answers: { quality: Answer; checks: Answer; drift: Answer }): Promise<string[]> {
  const asked: string[] = [];
  await page.route("**/{api,cs}/**", async (route) => {
    const url = new URL(route.request().url());
    const path = url.pathname;
    asked.push(path);
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) {
      return json({ subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", roles: ["portal-editor"] });
    }
    if (path === "/api/v1/projects") return json({ kind: "ProjectList", items: [{ name: PROJECT }] });
    if (path === `${AT}/quality`) return json(answers.quality.body, answers.quality.status);
    if (path === `${AT}/quality/checks`) return json(answers.checks.body, answers.checks.status);
    if (path === `/api/v1/projects/${PROJECT}/drift`) return json(answers.drift.body, answers.drift.status);
    if (path === AT) return json(SPACE_MANIFEST);
    return json(list([]));
  });
  return asked;
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.SHOTS_DIR;
  if (dir) await page.screenshot({ path: `${dir}/${name}.png`, fullPage: true });
}

for (const [label, width, height] of [["desktop-1440", 1440, 900], ["phone-400", 400, 860]] as const) {
  for (const [state, answers] of STATES) {
    test(`the space's Data quality tab through the Portal doors, ${state} (${label})`, async ({ page }) => {
      await page.setViewportSize({ width, height });
      const asked = await stubApi(page, answers);
      const sent: string[] = [];
      page.on("request", (request) => sent.push(decodeURIComponent(new URL(request.url()).pathname)));
      await page.goto(`/projects/${PROJECT}/spaces/${SPACE}?tab=quality&lang=en`);
      const drift = page.getByRole("heading", { name: "Drift from the repository" });
      await expect(drift).toBeVisible();
      if (state === "filled") {
        await expect(page.getByText("98.7% valid · 2 rules failing · 1 pipeline stale")).toBeVisible();
        await expect(page.getByText("1 entity differs from the repository.")).toBeVisible();
      }
      if (state === "empty") {
        await expect(page.getByText("The space declares no quality rules.")).toBeVisible();
        await expect(page.getByText(/No drift scan has run yet/)).toBeVisible();
      }
      if (state === "refused") await expect(page.getByText(/you are no member of helsinki/)).toBeVisible();
      if (state === "error") await expect(page.getByText(/the drift scan is not reachable/)).toBeVisible();
      // The page asked the service's doors, never the REST doors themselves.
      for (const door of ["portal_space_quality", "portal_space_quality_checks", "portal_drift_list"]) {
        expect(sent.some((path) => path.includes(`projects('${PROJECT}')/JoinedContext.Portal.${door}`))).toBe(true);
      }
      expect(sent.filter((path) => path === `${AT}/quality` || path.endsWith("/drift"))).toEqual([]);
      expect(asked.length).toBeGreaterThan(0);
      const sideways = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
      expect(sideways).toBeLessThanOrEqual(0);
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `space-quality-${state}-${label}`);
    });
  }
}
