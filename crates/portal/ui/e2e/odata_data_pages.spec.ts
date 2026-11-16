import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { expect, test, type Locator, type Page, type Route } from "@playwright/test";
import { axeViolations } from "./axe";
import { odataBody } from "../tests/odata";

// T-7073, T-7074, T-7075: the data pages, the pipelines and the shared parts read and write their
// manifests through the Organization's OData service (api/odata.ts listManifests, proposeManifest,
// proposeDelete) and call the registry's operations bound to the project (jc_syncsource_*). The
// journeys answer the service's own addresses at 1440 and 400 px: filled, empty, failed and
// refused, and the network capture proves no REST manifest or sync door is asked.

const SHOTS = process.env.SHOTS_DIR ?? "test-results/T-7073";
mkdirSync(SHOTS, { recursive: true });

const API = "joinedcontext.com/v1alpha1";
const KINDS = ["SyncSource", "Pipeline", "App", "ContextSpace", "Endpoint"];

const row = (kind: string, name: string, spec: unknown, status: unknown = null, apiVersion = API) => ({
  "@odata.etag": `W/"${name}-1"`,
  name,
  kind,
  apiVersion,
  metadata: { name, namespace: "helsinki" },
  spec,
  status,
});

const SOURCE = row("SyncSource", "regional-models", {
  source: { git: { url: "https://git.example.org/udp/models.git", ref: "main", path: "models" } },
  schedule: { interval: "30m" },
  mode: "mirror",
});
const STATUS = {
  project: "helsinki",
  name: "regional-models",
  phase: "Synced",
  observedRevision: "4f2a9c1",
  lastRunAt: 1757000000,
  mergeRequest: null,
  lastError: null,
  paused: false,
  durable: true,
};
const PIPELINE = row(
  "Pipeline",
  "air-feed",
  { source: { http: { url: "https://data.example.org/air.json" } }, target: { contextSpaceRef: "air" } },
  { phase: "Live" },
  "joinedcontext.com/v1alpha2",
);

/** Every REST door these pages read or wrote before T-7073; none may be asked any more. */
const OLD_DOORS =
  /^\/api\/v1\/projects\/helsinki\/(?:(?:syncsources|pipelines|apps|spaces|endpoints)(?:\/[^/]+)?|syncsources\/[^/]+\/(?:status|sync|pause|detach))$/;

type Answer = { status: number; body?: unknown };
const NONE: Answer = { status: 200, body: { value: [] } };

interface Asked {
  method: string;
  path: string;
  body: unknown;
  ifMatch?: string;
}

/** The service answered by `reads` (by decoded path) and `operations` (by name), the shell as REST; every request recorded. */
/** What the person holds in the project: an editor of these kinds, or (a refusal) none of them. */
type Grants = "editor" | "none";

async function stub(
  page: Page,
  reads: Record<string, Answer>,
  operations: Record<string, Answer> = {},
  grants: Grants = "editor",
): Promise<Asked[]> {
  const asked: Asked[] = [];
  await page.route("**/api/**", async (route: Route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = decodeURIComponent(url.pathname);
    asked.push({ method: request.method(), path, body: request.postDataJSON() as unknown, ifMatch: await request.headerValue("if-match") ?? undefined });
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    const operation = /JoinedContext\.Portal\.(\w+)/.exec(path)?.[1];
    if (operation && operations[operation]) {
      const answer = operations[operation];
      const ok = answer.status < 300;
      return route.fulfill({ status: answer.status, contentType: "application/json", body: ok ? odataBody(answer.body) : JSON.stringify(answer.body) });
    }
    if (url.searchParams.get("$select")?.includes("spec") || reads[path]) {
      const answer = reads[path] ?? NONE;
      // The edge's reference of a failure, which the failure says to quote.
      return route.fulfill({
        status: answer.status,
        contentType: "application/json",
        headers: answer.status >= 500 ? { "x-request-id": "req-7073" } : {},
        body: JSON.stringify(answer.body),
      });
    }
    if (path === "/api/v1/projects('helsinki')") return json({ "@odata.etag": 'W/"helsinki-7"', name: "helsinki" });
    if (path.endsWith("/auth/me")) return json({ subject: "s1", username: "admin", name: "Ada Admin", email: "admin@example.org", roles: [] });
    if (path.endsWith("/permissions/me")) {
      const rule = { kinds: KINDS, verbs: ["read", "propose"] };
      return json({ project: "helsinki", grants: grants === "editor" ? [{ role: "editor", binding: "editors", scope: "project", rule }] : [] });
    }
    if (path === "/api/v1/projects") return json({ apiVersion: API, kind: "ProjectList", items: [{ name: "helsinki" }] });
    if (path === "/api/v1/preferences") return json({ firstRunDismissed: true });
    return json({ apiVersion: API, kind: "List", items: [] });
  });
  return asked;
}

/** The service's answer to a kind the person reads nothing of (PF-50): the list is hidden, not named. */
const hidden = (plural: string): Answer => refusal(404, "resource-not-found", `plural '${plural}' not found in project 'helsinki'`);
const failure: Answer = { status: 500, body: { error: { code: "internal-error", message: "the change store did not answer", type: "https://joinedcontext.com/errors/internal-error", title: "Error", status: 500 } } };
const HIDDEN = "This list is not open to you in helsinki";

const refusal = (status: number, code: string, message: string): Answer => ({
  status,
  body: { error: { code, message, type: `https://joinedcontext.com/errors/${code}`, title: "Error", status } },
});

async function shoot(page: Page, name: string, width: number, at: Locator): Promise<void> {
  expect(await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth), "nothing scrolls sideways").toBe(0);
  await at.scrollIntoViewIfNeeded();
  await page.screenshot({ path: join(SHOTS, `${name}-${width}.png`) });
}

const APP = {
  ...row("App", "city-bikes", { endpointRef: "bikes", visibility: "project" }, { phase: "Live" }),
  metadata: { name: "city-bikes", namespace: "helsinki", title: "City bikes" },
};
const APPS = "/api/v1/projects('helsinki')/apps";
const SOURCES = "/api/v1/projects('helsinki')/syncsources";
const PIPELINES = "/api/v1/projects('helsinki')/pipelines";

for (const width of [1440, 400]) {
  test.describe(`at ${width} px`, () => {
    test.use(width < 640 ? { hasTouch: true, isMobile: true } : {});
    test.beforeEach(async ({ page }) => page.setViewportSize({ width, height: 900 }));

    test("sync sources: the list, its status and a pause go through the service", async ({ page }) => {
      const asked = await stub(
        page,
        { [SOURCES]: { status: 200, body: { value: [SOURCE] } } },
        {
          jc_syncsource_status: { status: 200, body: STATUS },
          jc_syncsource_pause: { status: 200, body: { ...STATUS, paused: true } },
        },
      );
      await page.goto("/projects/helsinki/syncsources?lang=en");
      const card = page.getByText("regional-models").first();
      await expect(card).toBeVisible();
      await expect(page.getByText("Every 30 minutes").first()).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await shoot(page, "syncsources-filled", width, card);
      await page.getByRole("button", { name: "Pause" }).first().click();
      await expect.poll(() => asked.find((a) => a.path.endsWith("jc_syncsource_pause"))?.body).toEqual({ name: "regional-models", paused: true });
      expect(asked.filter((a) => OLD_DOORS.test(a.path))).toEqual([]);
      expect(asked.some((a) => a.path.endsWith("jc_syncsource_status(name=@name)"))).toBe(true);
    });

    test("sync sources: an empty project says what to add", async ({ page }) => {
      await stub(page, { [SOURCES]: NONE });
      await page.goto("/projects/helsinki/syncsources?lang=en");
      const empty = page.getByText("This project syncs from nothing yet.").first();
      await expect(empty).toBeVisible();
      await shoot(page, "syncsources-empty", width, empty);
    });

    test("sync sources: a failed read says so and offers a retry", async ({ page }) => {
      await stub(page, { [SOURCES]: failure });
      await page.goto("/projects/helsinki/syncsources?lang=en");
      const failed = page.getByText("req-7073").first();
      await expect(failed).toBeVisible();
      await shoot(page, "syncsources-error", width, failed);
    });

    test("sync sources: a refused read shows the service's refusal", async ({ page }) => {
      await stub(page, { [SOURCES]: hidden("syncsources") }, {}, "none");
      await page.goto("/projects/helsinki/syncsources?lang=en");
      const refused = page.getByText(HIDDEN).first();
      await expect(refused).toBeVisible();
      await expect(page.getByRole("button", { name: "Add source" }).first()).toBeDisabled();
      await shoot(page, "syncsources-refused", width, refused);
    });

    test("pipelines: the list reads the service's rows with their apiVersion", async ({ page }) => {
      const asked = await stub(page, { [PIPELINES]: { status: 200, body: { value: [PIPELINE] } } });
      await page.goto("/projects/helsinki/pipelines?lang=en");
      const named = page.getByText("air-feed").first();
      await expect(named).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await shoot(page, "pipelines-filled", width, named);
      expect(asked.filter((a) => OLD_DOORS.test(a.path))).toEqual([]);
      expect(asked.some((a) => a.path === PIPELINES)).toBe(true);
    });

    test("pipelines: an empty project and a refused read", async ({ page }) => {
      await stub(page, { [PIPELINES]: NONE });
      await page.goto("/projects/helsinki/pipelines?lang=en");
      const main = page.getByRole("main");
      await expect(main).toBeVisible();
      await shoot(page, "pipelines-empty", width, main);
      await page.unrouteAll({ behavior: "ignoreErrors" });
      await stub(page, { [PIPELINES]: hidden("pipelines") }, {}, "none");
      await page.reload();
      const refused = page.getByText(HIDDEN).first();
      await expect(refused).toBeVisible();
      await expect(page.getByRole("button", { name: "New pipeline" }).first()).toBeDisabled();
      await shoot(page, "pipelines-refused", width, refused);
      await page.unrouteAll({ behavior: "ignoreErrors" });
      await stub(page, { [PIPELINES]: failure });
      await page.reload();
      const failed = page.getByText("req-7073").first();
      await expect(failed).toBeVisible();
      await shoot(page, "pipelines-error", width, failed);
    });

    test("apps: the catalogue lists the service's App rows, and says what to do when there are none", async ({ page }) => {
      const asked = await stub(page, { [APPS]: { status: 200, body: { value: [APP] } } });
      await page.goto("/projects/helsinki/apps?lang=en");
      const named = page.getByText("City bikes").first();
      await expect(named).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await shoot(page, "apps-filled", width, named);
      expect(asked.filter((a) => OLD_DOORS.test(a.path))).toEqual([]);
      await page.unrouteAll({ behavior: "ignoreErrors" });
      await stub(page, { [APPS]: NONE });
      await page.reload();
      const empty = page.getByText("No applications yet").first();
      await expect(empty).toBeVisible();
      await shoot(page, "apps-empty", width, empty);
    });

    test("apps: a refused catalogue shows the service's refusal", async ({ page }) => {
      await stub(page, { [APPS]: hidden("apps") }, {}, "none");
      await page.goto("/projects/helsinki/apps?lang=en");
      const refused = page.getByText(HIDDEN).first();
      await expect(refused).toBeVisible();
      await shoot(page, "apps-refused", width, refused);
      await page.unrouteAll({ behavior: "ignoreErrors" });
      await stub(page, { [APPS]: failure });
      await page.reload();
      const failed = page.getByText("req-7073").first();
      await expect(failed).toBeVisible();
      await shoot(page, "apps-error", width, failed);
    });
  });
}
