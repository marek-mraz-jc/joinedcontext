/**
 * The assistant steers the open page (T-5959, S-41): asked to open pipeline hel-news with a
 * mapping, the run's stream carries `jc_ui_open_form`'s navigate (the editor's route and the
 * manifest as its values) and a highlight of the mapping. The editor opens prefilled and nothing
 * is written: the only request the page sends besides reads is the conversation itself, until the
 * person proposes. The API is answered in the browser.
 */
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

const API = "joinedcontext.com/v1alpha1";
const PROJECT = "helsinki";
const RUN = "run-ui-actions-1";
const MAPPING = "root.count = this.count.number()";
const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-approver"],
  groups: [PROJECT],
};
const PIPELINE = {
  apiVersion: API,
  kind: "Pipeline",
  metadata: { name: "hel-news", namespace: PROJECT, title: { en: "Helsinki news" } },
  spec: { class: "auto", enabled: true, targetEndpoint: "urn:ngsi-ld:Endpoint:hel.fi:news", output: { type: "NewsItem", mode: "upsert" }, compute: { kind: "bloblang", bloblang: "root = this" } },
};
const PLANNED = { ...PIPELINE, spec: { ...PIPELINE.spec, compute: { kind: "bloblang", bloblang: MAPPING } } };

function stream(started: boolean): string {
  if (!started) return "";
  const frames = [
    ["message", { seq: 1, text: "Open the pipeline editor for hel-news with your mapping filled in." }],
    ["tool", { seq: 2, tool: "jc_ui_open_form", status: "ok", input: { kind: "Pipeline", name: "hel-news" }, output: { route: `/projects/${PROJECT}/pipelines?edit=hel-news` } }],
    ["navigate", { seq: 3, route: `/projects/${PROJECT}/pipelines?edit=hel-news`, prefill: PLANNED }],
    ["ui", { seq: 4, action: "highlight", kind: "Pipeline", field: "spec.compute.bloblang", message: "This is the mapping I filled in; test it, then propose." }],
    ["thought", { seq: 5, text: "The editor is open with the mapping. Nothing is saved until you propose it." }],
  ] as const;
  return frames.map(([kind, data]) => `id: ${data.seq}\nevent: ${kind}\ndata: ${JSON.stringify(data)}\n\n`).join("");
}

async function stubApi(page: Page): Promise<{ method: string; path: string }[]> {
  const writes: { method: string; path: string }[] = [];
  let started = false;
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path.endsWith("/permissions/me")) return json({ project: PROJECT, bootstrap: true, grants: [] });
    if (path === "/api/v1/projects") return json({ apiVersion: API, kind: "ProjectList", items: [{ name: PROJECT }] });
    if (request.method() !== "GET") writes.push({ method: request.method(), path });
    if (path.endsWith("/assistant/conversations") && request.method() === "POST") {
      started = true;
      return json({ id: RUN, status: "queued" }, 202);
    }
    if (path.endsWith(`/agent-runs/${RUN}/events`)) {
      return route.fulfill({ status: 200, contentType: "text/event-stream", headers: { "cache-control": "no-cache" }, body: stream(started) });
    }
    if (path.endsWith(`/agent-runs/${RUN}`)) {
      return json({ id: RUN, project: PROJECT, kind: "conversation", appName: "", status: "waiting", createdBy: "jana.kovacova", createdAt: "2026-10-29T08:00:00Z" });
    }
    if (path === `/api/v1/projects/${PROJECT}/pipelines`) return json({ apiVersion: API, kind: "List", items: [PIPELINE] });
    return json({ apiVersion: API, kind: "List", items: [] });
  });
  return writes;
}

for (const [label, width, height] of [
  ["1440", 1440, 900],
  ["400", 400, 860],
] as const) {
  test(`asked for a mapping, the editor opens prefilled and nothing is saved (${label})`, async ({ page }) => {
    await page.setViewportSize({ width, height });
    const writes = await stubApi(page);
    await page.goto(`/projects/${PROJECT}/pipelines?lang=en`);
    await page.getByRole("button", { name: "Open the assistant" }).first().click();
    const dock = page.getByRole("complementary", { name: "Conversation" });
    await expect(dock).toBeVisible();
    await dock.getByLabel("Ask the assistant").fill("Open the pipeline editor for hel-news with your mapping filled in.");
    if (process.env.SHOTS_DIR) await page.screenshot({ path: `${process.env.SHOTS_DIR}/ui-actions-ask-${label}.png` });
    await dock.getByLabel("Ask the assistant").press("Enter");

    await expect(page.getByRole("heading", { name: "Edit pipeline", level: 1 })).toBeVisible();
    const mapping = page.getByRole("textbox", { name: "Bloblang mapping" });
    await expect(mapping).toHaveValue(MAPPING);
    if (process.env.SHOTS_DIR) await page.screenshot({ path: `${process.env.SHOTS_DIR}/ui-actions-open-form-${label}.png` });
    await expect(page.getByRole("status").filter({ hasText: "This is the mapping I filled in; test it, then propose." })).toBeVisible();
    await expect(mapping).toBeFocused();
    if (process.env.SHOTS_DIR) await page.screenshot({ path: `${process.env.SHOTS_DIR}/ui-actions-highlight-${label}.png` });
    // On a phone the dock covers the page: closed, the form shows the field it pointed at.
    if (width < 600) {
      await dock.getByRole("button", { name: "Close the assistant" }).click();
      await mapping.scrollIntoViewIfNeeded();
      if (process.env.SHOTS_DIR) await page.screenshot({ path: `${process.env.SHOTS_DIR}/ui-actions-highlight-form-${label}.png` });
    }

    // Nothing was proposed: no write to the pipeline, no change request.
    expect(writes.filter((write) => !write.path.endsWith("/assistant/conversations"))).toEqual([]);
    expect(await axeViolations(page)).toEqual([]);
  });
}
