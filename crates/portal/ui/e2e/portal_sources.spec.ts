import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// T-6876 (ODP-15): a portal's Sources and Datasets tabs on stubbed routes. Every state at 1440
// and 400 px (empty, filled, error, refusal), and one journey that adds a dcat-ap source, tests it,
// runs it, reads its run log and hides a dataset, each write a checked proposal.

const HOSTS = { items: [{ organization: "bbsk", hosts: ["doprava.bbsk.sk"], opendata: "https://opendata.bbsk.sk" }] };

const KOSICE = {
  name: "kosice",
  type: "dcat-ap",
  url: "https://data.kosice.sk/catalog.ttl",
  allowedHosts: ["data.kosice.sk"],
  schedule: "23 3 * * *",
  budget: { records: 5000, bytes: 50000000 },
};
const OWN = { name: "own", type: "endpoints", projects: ["doprava"] };

function portal(extra: Record<string, unknown>) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Portal",
    metadata: { name: "bbsk-mobility", namespace: "org" },
    spec: { organizationRef: "bbsk", path: "mobility", title: { sk: "Doprava", en: "Mobility" }, languages: ["sk", "en"], ...extra },
  };
}

const RUNS = [
  { id: "r3", state: "failed", startedAt: "2026-11-14T10:02:00Z", finishedAt: "2026-11-14T10:02:40Z", created: 0, updated: 0, unchanged: 0, withdrawn: 0, skipped: 0, error: "over budget" },
  { id: "r2", state: "succeeded", startedAt: "2026-11-14T03:23:00Z", finishedAt: "2026-11-14T03:24:10Z", created: 2, updated: 5, unchanged: 41, withdrawn: 1, skipped: 3, error: null },
  { id: "r1", state: "succeeded", startedAt: "2026-11-13T03:23:00Z", finishedAt: "2026-11-13T03:24:02Z", created: 46, updated: 0, unchanged: 0, withdrawn: 0, skipped: 3, error: null },
];

const DATASETS = {
  total: 4,
  items: [
    { name: "ke-ovzdusie", title: "Kvalita ovzdušia Košice", source: "kosice", hidden: false },
    { name: "ke-skoly", title: "Školy a školské zariadenia", source: "kosice", hidden: true },
    { name: "ke-mhd-zastavky", title: "Zastávky MHD", source: "kosice", hidden: false },
    { name: "doprava-sensors", title: "Dopravné senzory", source: "endpoints", hidden: false },
  ],
};

const CHANGE = {
  metadata: { name: "chg-1a2b3c4d", namespace: "org" },
  id: "c-1",
  title: "Propose Portal bbsk-mobility",
  lane: "Yellow",
  author: "jana",
  createdAt: "2026-11-14T10:00:00Z",
  status: { phase: "AwaitingApproval" },
  resources: [{ kind: "Portal", name: "bbsk-mobility", namespace: "org", operation: "update" }],
};

interface Stub {
  spec: Record<string, unknown>;
  /** What a test answers, in order; past the list a 429. */
  tests?: string[];
  runs?: unknown[];
  /** The status a run now answers: 202, 409 or 503. */
  start?: number;
  datasets?: { total: number; items: unknown[] } | number;
}

const problem = (status: number, detail: string) => ({ type: "about:blank", title: "Refused", status, detail });

async function stub(page: Page, options: Stub): Promise<string[]> {
  const sent: string[] = [];
  const tests = [...(options.tests ?? [])];
  await page.route(/\/api\//, async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json({ subject: "s", username: "jana", name: "Jana Admin", email: "jana@bbsk.sk", roles: [] });
    if (path.endsWith("/permissions/me")) {
      return json({ project: "org", bootstrap: false, grants: [{ rule: { kinds: ["Portal", "Organization", "RoleBinding", "Group"], verbs: ["read", "propose", "approve", "delete"] }, organization: "bbsk" }] });
    }
    if (path === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "doprava", organization: "bbsk" }, { name: "skoly", organization: "bbsk" }] });
    if (path === "/api/v1/organization/portal-hosts") return json(HOSTS);
    const base = "/api/v1/projects/org/portals/bbsk-mobility";
    if (path === `${base}/sources/kosice/test`) {
      sent.push(`POST ${path}`);
      const outcome = tests.shift();
      return outcome ? json({ outcome }) : json(problem(429, "6 source tests a minute, 3 at once; try again in 10 s"), 429);
    }
    if (path === `${base}/sources/kosice/runs` && request.method() === "POST") {
      sent.push(`POST ${path}`);
      const status = options.start ?? 202;
      return status === 202 ? route.fulfill({ status }) : json(problem(status, "a run of kosice is active"), status);
    }
    if (path === `${base}/sources/kosice/runs`) return json({ items: options.runs ?? [] });
    if (path === `${base}/datasets`) {
      return typeof options.datasets === "number"
        ? json(problem(options.datasets, "this installation serves no open data catalogue"), options.datasets)
        : json(options.datasets ?? { total: 0, items: [] });
    }
    if (path === base) {
      if (request.method() === "GET") return json(portal(options.spec));
      sent.push(`${request.method()} ${path}${url.search} ${request.postData() ?? ""}`);
      return url.searchParams.get("dryRun") ? json({ valid: true, verdict: { ok: true, findings: [] } }) : json(CHANGE, 202);
    }
    if (path.endsWith("/events")) return route.fulfill({ status: 200, contentType: "text/event-stream", body: "" });
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
  return sent;
}

async function shoot(page: Page, width: number, path: string): Promise<void> {
  // Full height in the viewport, so the sticky sidebar is not cut (knowledge/portal-ui.md).
  const height = await page.evaluate(() => document.documentElement.scrollHeight);
  await page.setViewportSize({ width, height: Math.max(height, 900) });
  await page.screenshot({ path });
  await page.setViewportSize({ width, height: 900 });
}

async function openTab(page: Page, tab: "Sources" | "Datasets"): Promise<void> {
  await page.goto("/portals/bbsk-mobility?lang=en");
  await page.getByRole("tab", { name: tab }).click();
  // A shot right after a tab click catches the old tab's underline mid-fade (knowledge/portal-ui.md).
  await page.waitForFunction(() => document.getAnimations().every((animation) => animation.playState !== "running"));
}

async function noOverflow(page: Page): Promise<void> {
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
  expect(overflow).toBeLessThanOrEqual(0);
}

for (const width of [1440, 400]) {
  test.describe(`at ${width} px`, () => {
    test.beforeEach(async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
    });

    test("sources: empty, the add form, filled with its run log, error and refusal", async ({ page }, info) => {
      await stub(page, { spec: {} });
      await openTab(page, "Sources");
      await expect(page.getByText("No source yet")).toBeVisible();
      await shoot(page, width, info.outputPath(`sources-empty-${width}.png`));
      expect(await axeViolations(page)).toEqual([]);

      await page.getByRole("button", { name: "Add a source" }).click();
      await page.getByLabel(/^Type/).selectOption("dcat-ap");
      await expect(page.getByLabel(/^Address/)).toBeVisible();
      await shoot(page, width, info.outputPath(`sources-add-${width}.png`));
      expect(await axeViolations(page)).toEqual([]);
      await noOverflow(page);

      await page.unrouteAll();
      await stub(page, { spec: { sources: [KOSICE, OWN] }, tests: ["ok", "unreachable"], runs: RUNS });
      await openTab(page, "Sources");
      await page.getByRole("button", { name: "Test connection of kosice" }).click();
      await expect(page.getByText("The source answered in the expected format.")).toBeVisible();
      await page.getByRole("button", { name: "Last runs" }).click();
      await expect(page.getByRole("table", { name: "Last runs of kosice" })).toBeVisible();
      await shoot(page, width, info.outputPath(`sources-filled-${width}.png`));
      expect(await axeViolations(page)).toEqual([]);
      await noOverflow(page);

      await page.getByRole("button", { name: "Test connection of kosice" }).click();
      await expect(page.getByText(/The source did not answer/)).toBeVisible();
      await expect(page.getByText(/The run stopped at the source's budget/)).toBeVisible();
      await shoot(page, width, info.outputPath(`sources-error-${width}.png`));

      await page.getByRole("button", { name: "Test connection of kosice" }).click();
      await expect(page.getByText(/Too many tests in a short time/)).toBeVisible();
      await shoot(page, width, info.outputPath(`sources-refusal-${width}.png`));
    });

    test("datasets: empty, filled, and no catalogue", async ({ page }, info) => {
      await stub(page, { spec: { sources: [KOSICE] } });
      await openTab(page, "Datasets");
      await expect(page.getByText("No dataset yet")).toBeVisible();
      await shoot(page, width, info.outputPath(`datasets-empty-${width}.png`));

      await page.unrouteAll();
      await stub(page, { spec: { sources: [KOSICE, OWN], hidden: ["ke-skoly"] }, datasets: DATASETS });
      await openTab(page, "Datasets");
      await expect(page.getByRole("table", { name: "Datasets of the portal" })).toBeVisible();
      await shoot(page, width, info.outputPath(`datasets-filled-${width}.png`));
      expect(await axeViolations(page)).toEqual([]);
      await noOverflow(page);

      await page.unrouteAll();
      await stub(page, { spec: { sources: [KOSICE] }, datasets: 503 });
      await openTab(page, "Datasets");
      await expect(page.getByText(/serves no open data catalogue yet/)).toBeVisible();
      await shoot(page, width, info.outputPath(`datasets-refusal-${width}.png`));
    });
  });
}

test("add, test, run and hide: each write a checked proposal on the portal's own routes", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  const sent = await stub(page, { spec: {}, tests: ["refused host"], runs: RUNS, datasets: DATASETS });
  await openTab(page, "Sources");
  await page.getByRole("button", { name: "Add a source" }).click();
  await page.getByLabel(/^Type/).selectOption("dcat-ap");
  await page.getByLabel(/^Name/).fill("kosice");
  await page.getByLabel(/^Address/).fill("https://data.kosice.sk/catalog.ttl");
  await page.getByRole("button", { name: "Propose the source" }).click();
  await expect(page.getByText("Proposed as a change:")).toBeVisible();
  const puts = sent.filter((line) => line.startsWith("PUT"));
  expect(puts).toHaveLength(2);
  expect(puts[0]).toContain("dryRun=All");
  expect(JSON.parse(puts[1].slice(puts[1].indexOf("{"))).spec.sources).toEqual([KOSICE]);

  await page.unrouteAll();
  const after = await stub(page, { spec: { sources: [KOSICE] }, tests: ["refused host"], runs: RUNS, datasets: DATASETS });
  await openTab(page, "Sources");
  await page.getByRole("button", { name: "Test connection of kosice" }).click();
  await expect(page.getByText(/The address is refused/)).toBeVisible();
  await page.getByRole("button", { name: "Run kosice now" }).click();
  await expect(page.getByText(/The run started/)).toBeVisible();
  await expect(page.getByRole("table", { name: "Last runs of kosice" })).toBeVisible();
  await page.getByRole("tab", { name: "Datasets" }).click();
  await page.getByRole("button", { name: "Hide Kvalita ovzdušia Košice" }).click();
  await expect(page.getByText("Proposed as a change:")).toBeVisible();
  const hide = after.filter((line) => line.startsWith("PUT")).at(-1) ?? "";
  expect(JSON.parse(hide.slice(hide.indexOf("{"))).spec.hidden).toEqual(["ke-ovzdusie"]);
  expect(after.filter((line) => line.startsWith("POST"))).toEqual([
    "POST /api/v1/projects/org/portals/bbsk-mobility/sources/kosice/test",
    "POST /api/v1/projects/org/portals/bbsk-mobility/sources/kosice/runs",
  ]);
});
