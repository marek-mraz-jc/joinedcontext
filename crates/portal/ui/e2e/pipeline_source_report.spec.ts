import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

// T-4892: "Source vs space" on a snapshot pipeline's page, in a real browser. The API is answered
// in the browser, as in pipeline_lifetime.spec.ts; JC_SHOTS=<dir> also saves each state at 1440
// and 400 px for the review.

const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const id = (n: number) => `urn:ngsi-ld:Dataset:hel.fi:open:ds-${n}`;
const REPORT = "/api/v1/projects/helsinki/pipelines/registry/source-deletes";

const pipeline = (mode: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Pipeline",
  metadata: { name: "registry", namespace: "helsinki" },
  spec: {
    class: "scheduled",
    schedule: "0 3 * * *",
    source: { dataSourceRef: { name: "ckan" } },
    sourceDeletes: { mode, misses: 2, maxShare: 5 },
  },
  status: { phase: "Live" },
});

const removal = (n: number) => ({
  id: id(n),
  pipeline: "registry",
  run: "2026-10-21T03:00:00Z",
  at: "2026-10-21T03:05:00Z",
  reason: "source-snapshot: pipeline registry run 2026-10-21T03:00:00Z",
});

const HELD = {
  misses: 2,
  maxShare: 5,
  ledger: 50,
  next: [],
  due: [id(17), id(33), id(4)],
  hold: { run: "2026-10-21T03:00:00Z", reason: "share-exceeded", would: 3, ledger: 50 },
  space: "open",
  lastComplete: { run: "2026-10-21T03:00:00Z", at: "2026-10-21T03:04:00Z", read: 48 },
  orphans: 3,
  orphanIds: [id(17), id(33), id(4)],
  missing: 1,
  missingIds: ["urn:ngsi-ld:Dataset:hel.fi:open:bad"],
  removed: [],
  idsHidden: false,
};
const RELEASED = { ...HELD, ledger: 47, due: [], hold: null, orphans: 0, orphanIds: [], removed: [removal(17), removal(33), removal(4)] };
const HIDDEN = { ...HELD, hold: null, due: [], orphanIds: [], missingIds: [], idsHidden: true };

interface Stub {
  mode: string;
  report: unknown;
  status?: number;
  verbs: string[];
}

async function stubApi(page: Page, stub: Stub): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) {
      return json({ subject: "s1", username: "aino", name: "Aino Virtanen", email: "aino@hel.fi", roles: ["portal-editor"] });
    }
    if (path.endsWith("/branding")) {
      return json({ instanceName: "joinedcontext", orgDomain: "hel.fi", languages: { default: "en", offered: ["en"] } });
    }
    if (path.endsWith("/permissions/me")) {
      return json({ project: "helsinki", bootstrap: false, grants: [{ role: "r", binding: "b", rule: { kinds: ["Pipeline"], verbs: stub.verbs } }] });
    }
    if (path === REPORT && request.method() === "POST") {
      stub.report = RELEASED;
      return json({ deleting: 3 }, 202);
    }
    if (path === REPORT) {
      return stub.status
        ? json({ title: "Unavailable", status: stub.status, detail: "the pipeline's source ledger could not be read; try again" }, stub.status)
        : json(stub.report);
    }
    if (path === "/api/v1/projects") return json({ kind: "ProjectList", items: [{ name: "helsinki" }] });
    if (path === "/api/v1/projects/helsinki/pipelines/registry") return json(pipeline(stub.mode));
    if (path.endsWith("/pipelines")) return json(list([pipeline(stub.mode)]));
    return json(list([]));
  });
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_SHOTS;
  if (!dir) return;
  for (const width of [1440, 400]) {
    await page.setViewportSize({ width, height: 900 });
    const section = page.locator("#source");
    await section.scrollIntoViewIfNeeded();
    await section.screenshot({ path: `${dir}/source-report-${name}-${width}.png` });
  }
  await page.setViewportSize({ width: 1440, height: 900 });
}

async function open(page: Page, stub: Stub) {
  await stubApi(page, stub);
  await page.goto("/projects/helsinki/pipelines/registry?lang=en#source");
  return page.locator("#source");
}

test("a steward releases a held run and the panel goes from 3 orphans to 0 with the reason", async ({ page }) => {
  const section = await open(page, { mode: "snapshot", report: HELD, verbs: ["read", "approve"] });
  const panel = section.getByTestId("source-report");
  await expect(panel.getByText("Orphans", { exact: true }).locator("..")).toContainText("3");
  await expect(panel.getByText("Run 2026-10-21T03:00:00Z deleted nothing")).toBeVisible();
  await section.getByText("Orphan ids (3)").click();
  await shot(page, "held");
  await panel.getByRole("button", { name: "Delete 3 anyway" }).click();
  await expect(panel.getByText("Orphans", { exact: true }).locator("..")).toContainText("0");
  await expect(panel.getByText(/Removed at the source \(pipeline registry/)).toHaveCount(3);
  await shot(page, "released");
  // The table of removals scrolls in its own box at phone width; the page never does.
  await page.setViewportSize({ width: 400, height: 900 });
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
  expect(overflow).toBeLessThanOrEqual(0);
});

test("a reader without the space sees counts, no ids and no export", async ({ page }) => {
  const section = await open(page, { mode: "snapshot", report: HIDDEN, verbs: ["read"] });
  const panel = section.getByTestId("source-report");
  await expect(panel.getByRole("status").filter({ hasText: "but not the space open" })).toBeVisible();
  await expect(panel.getByRole("button", { name: "Export CSV" })).toHaveAttribute("aria-disabled", "true");
  await shot(page, "refusal");
});

test("a pipeline that keeps what its source drops says what to set", async ({ page }) => {
  const section = await open(page, { mode: "never", report: HELD, verbs: ["read"] });
  await expect(section.getByText("This pipeline keeps entities its source drops")).toBeVisible();
  await shot(page, "empty");
});

test("a report that cannot be read says why and what to do", async ({ page }) => {
  const section = await open(page, { mode: "snapshot", report: HELD, status: 503, verbs: ["read"] });
  await expect(section.getByRole("alert")).toContainText("could not be read; try again");
  await shot(page, "error");
});
