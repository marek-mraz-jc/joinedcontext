import { mkdirSync } from "node:fs";
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";
import { greenVerdict, isCheck } from "./verdict";

// A control that cannot act says why and keeps its place in the tab order (UI-44, T-7048): an
// agent run's Publish while the run builds, while its tests hold it and once the run is over; a
// pipeline's Pause for a reader, and only the row being toggled waiting while it does.
// `vite preview` has no API, so it is answered here. JC_EVIDENCE_DIR takes the shots.

const PROJECT = "helsinki";

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  await page.screenshot({ path: `${dir}/${name}.png`, fullPage: true });
}

const RUN = {
  id: "01J8ZQ4T7K9M2N3P4Q5R6S7T8V",
  project: PROJECT,
  appName: "bike-stations",
  endpointName: "helsinki-bikes",
  appClass: "static",
  visibility: "project",
  prompt: "Map the shared bike stations, colour-coded by occupancy, and count the empty ones",
  steps: 3,
  tokensUsed: 34_785,
  createdBy: "demo.steward",
  createdAt: "2026-11-02T13:31:40Z",
};

const STATES = [
  { name: "not-ready", run: { ...RUN, status: "generating" }, reason: /No version is ready to publish yet/ },
  {
    name: "tests-hold",
    run: { ...RUN, status: "previewing", previewUrl: "/apps/bike-stations-preview/?v=2" },
    tests: { seq: 9, version: 2, outcome: "failed", failed: 1, failures: [{ file: "src/App.test.tsx", name: "lists the stations" }] },
    reason: /Version 2 is not published while 1 of its tests fail/,
  },
  {
    name: "over",
    run: { ...RUN, status: "failed", error: "The Portal's database did not answer in time. Start the run again in a minute." },
    reason: /This run has ended, so it has nothing to publish/,
  },
];

for (const width of [1440, 400]) {
  for (const state of STATES) {
    test(`Publish says why it waits: ${state.name} at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: width === 400 ? 860 : 900 });
      await page.route(/\/api\//, async (route) => {
        const path = new URL(route.request().url()).pathname;
        const json = (body: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
        if (path.endsWith("/auth/me")) return json({ subject: "s1", username: "demo.steward", name: "Demo Steward", roles: [] });
        if (path === "/api/v1/projects") return json({ kind: "ProjectList", items: [{ name: PROJECT }] });
        if (path.endsWith("/agent-runs")) return json({ items: [state.run] });
        if (path.endsWith(`/agent-runs/${RUN.id}`)) return json(state.run);
        if (path.endsWith("/events")) {
          if (!state.tests) return route.fulfill({ status: 204 });
          return route.fulfill({
            status: 200,
            contentType: "text/event-stream",
            headers: { "cache-control": "no-cache" },
            body: `event: tests\nid: 9\ndata: ${JSON.stringify(state.tests)}\n\n`,
          });
        }
        return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
      });
      await page.goto(`/projects/${PROJECT}/apps/${RUN.appName}?lang=en`);
      if (width === 400) await page.getByRole("button", { name: "Hide the assistant" }).click();
      const publish = page.getByRole("button", { name: "Publish this app" });
      await expect(publish).toHaveAttribute("aria-disabled", "true");
      await expect(publish).toHaveAccessibleDescription(state.reason);
      await publish.focus();
      await expect(publish).toBeFocused();
      expect(await axeViolations(page)).toEqual([]);
      await shot(page, `publish-${state.name}-${width}`);
    });
  }
}

const pipeline = (name: string, title: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Pipeline",
  metadata: { name, namespace: PROJECT, title: { en: title } },
  spec: { class: "resident" },
  status: { phase: "Live" },
});
const PIPELINES = { apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [pipeline("air-quality", "Air quality"), pipeline("bike-counts", "Bike counts")] };
const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-77aa11bb", namespace: PROJECT },
  status: { lane: "yellow", phase: "PendingApproval", plan: { update: 1 } },
};

/** The pipelines page as `verbs` allow; a write waits on `release` so a shot catches it pending. */
async function pipelines(page: Page, verbs: string[], release?: Promise<void>): Promise<string[]> {
  const writes: string[] = [];
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json({ subject: "s1", username: "demo.reader", name: "Demo Reader", roles: [] });
    if (path === "/api/v1/projects") return json({ kind: "ProjectList", items: [{ name: PROJECT }] });
    if (path.endsWith("/permissions/me")) {
      return json({ project: PROJECT, bootstrap: false, grants: [{ role: "r", binding: "b", rule: { kinds: ["Pipeline"], verbs } }] });
    }
    if (isCheck(request.method(), request.url())) return json(greenVerdict(request.postData()));
    if (request.method() !== "GET") {
      writes.push(`${request.method()} ${path}`);
      await release;
      return json(CHANGE, 202);
    }
    if (path.endsWith("/pipelines")) return json(PIPELINES);
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
  return writes;
}

for (const width of [1440, 400]) {
  test(`a reader's Pause says why and writes nothing at ${width} px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    const writes = await pipelines(page, ["read"]);
    await page.goto(`/projects/${PROJECT}/pipelines?lang=en`);
    const pause = page.getByRole("button", { name: "Pause" }).first();
    await expect(pause).toHaveAttribute("aria-disabled", "true");
    await expect(pause).toHaveAccessibleDescription(/.+/);
    await pause.click({ force: true });
    expect(writes).toEqual([]);
    expect(await axeViolations(page)).toEqual([]);
    await shot(page, `pipelines-reader-${width}`);
  });

  test(`only the pipeline being paused waits at ${width} px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    let open = (): void => undefined;
    const release = new Promise<void>((resolve) => {
      open = resolve;
    });
    const writes = await pipelines(page, ["read", "propose"], release);
    await page.goto(`/projects/${PROJECT}/pipelines?lang=en`);
    const buttons = page.getByRole("button", { name: "Pause" });
    await expect(buttons).toHaveCount(2);
    await buttons.first().click();
    await expect.poll(() => writes.length).toBe(1);
    await expect(buttons.first()).toHaveAttribute("aria-disabled", "true");
    await expect(buttons.first()).toHaveAccessibleDescription("Wait for the action in progress to finish.");
    await expect(buttons.nth(1)).not.toHaveAttribute("aria-disabled");
    await shot(page, `pipelines-toggling-${width}`);
    open();
    await expect(page.getByText("chg-77aa11bb")).toBeVisible();
  });
}
