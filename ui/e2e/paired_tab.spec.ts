/**
 * Claude Code steers the person's open Portal tab (T-5967): on My connectors the steward presses
 * Connect this tab, the connector's client sends `jc_ui_open_form` (the pipeline editor with a
 * mapping) and a highlight, and the tab runs both, saying who sent them. Disconnect ends it. The
 * only writes are the person's own two clicks: nothing the connector sent saves anything. The API
 * is answered in the browser.
 */
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

const API = "joinedcontext.com/v1alpha1";
const PROJECT = "helsinki";
const ID = "abcdefghijklmnopqrstuvwxyz";
const TAB = "0123456789abcdef0123456789abcdef";
const MAPPING = "root.count = this.count.number()";
const IDENTITY = { subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", email: "jana.kovacova@banskabystrica.sk", roles: [], groups: [PROJECT] };
const CONNECTOR = {
  id: ID,
  name: "helsinki-in-claude-code",
  owner: IDENTITY.subject,
  segments: ["pipeline"],
  scope: [{ project: PROJECT, access: "read" }],
  writes: false,
  expiresAt: "2026-11-20T23:59:59Z",
  allowedClients: ["mcp-local"],
  clientIsBoundary: false,
  limits: { parallelCalls: 4, resultBytes: 1048576 },
  state: "active",
  createdAt: "2026-10-28T12:00:00Z",
  lastUsedAt: "2026-10-29T08:00:00Z",
  url: `https://portal.dev.example/c/${ID}`,
};
const PIPELINE = {
  apiVersion: API,
  kind: "Pipeline",
  metadata: { name: "hel-news", namespace: PROJECT, title: { en: "Helsinki news" } },
  spec: { class: "auto", enabled: true, targetEndpoint: "urn:ngsi-ld:Endpoint:hel.fi:news", output: { type: "NewsItem", mode: "upsert" }, compute: { kind: "bloblang", bloblang: "root = this" } },
};
const PLANNED = { ...PIPELINE, spec: { ...PIPELINE.spec, compute: { kind: "bloblang", bloblang: MAPPING } } };
const FROM = { connector: CONNECTOR.name, client: "mcp-local" };

function actions(): string {
  const frames = [
    { frame: "navigate", tool: "jc_ui_open_form", project: PROJECT, payload: { route: `/projects/${PROJECT}/pipelines?edit=hel-news`, prefill: PLANNED }, from: FROM, at: "2026-10-29T08:01:00Z" },
    { frame: "ui", tool: "jc_ui_highlight", project: PROJECT, payload: { action: "highlight", kind: "Pipeline", field: "spec.compute.bloblang", message: "Claude filled in this mapping; test it, then propose." }, from: FROM, at: "2026-10-29T08:01:01Z" },
  ];
  // A long retry: the stub's stream ends after the frames, and a reconnect would replay them.
  return `retry: 600000\n\n${frames.map((data) => `event: action\ndata: ${JSON.stringify(data)}\n\n`).join("")}`;
}

async function stubApi(page: Page): Promise<{ method: string; path: string }[]> {
  const writes: { method: string; path: string }[] = [];
  let paired = false;
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path.endsWith("/permissions/me")) return json({ project: PROJECT, bootstrap: true, grants: [] });
    if (path === "/api/v1/projects") return json({ apiVersion: API, kind: "ProjectList", items: [{ name: PROJECT }] });
    if (request.method() !== "GET") writes.push({ method: request.method(), path: path + new URL(request.url()).search });
    if (path === "/api/v1/connectors") return json({ items: [CONNECTOR] });
    if (path === `/api/v1/connectors/${ID}/tab` && request.method() === "POST") {
      paired = true;
      return json({ tab: TAB, connector: ID, name: CONNECTOR.name, until: "2026-10-29T18:00:00Z" }, 201);
    }
    if (path === `/api/v1/connectors/${ID}/tab` && request.method() === "DELETE") {
      paired = false;
      return route.fulfill({ status: 204 });
    }
    if (path === `/api/v1/connectors/${ID}/tab/${TAB}/events`) {
      if (!paired) return json({ title: "Not Found", status: 404, detail: "this tab is not connected to the connector; connect it again" }, 404);
      return route.fulfill({ status: 200, contentType: "text/event-stream", headers: { "cache-control": "no-cache" }, body: actions() });
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
  test(`a connector's client opens the pipeline editor prefilled in the paired tab, and disconnect ends it (${label})`, async ({ page }) => {
    await page.setViewportSize({ width, height });
    const writes = await stubApi(page);
    await page.goto("/ai-connectors?lang=en");
    await expect(page.getByText(/Connect this tab to let the AI client/)).toBeVisible();
    if (process.env.SHOTS_DIR) await page.screenshot({ path: `${process.env.SHOTS_DIR}/paired-tab-offer-${label}.png` });
    await page.getByRole("button", { name: `Connect this tab to ${CONNECTOR.name}` }).click();

    // The connector's two actions arrive: the editor opens with the mapping, the field is pointed at.
    await expect(page.getByRole("heading", { name: "Edit pipeline", level: 1 })).toBeVisible();
    const mapping = page.getByRole("textbox", { name: "Bloblang mapping" });
    await expect(mapping).toHaveValue(MAPPING);
    await expect(page.getByRole("status").filter({ hasText: "Claude filled in this mapping; test it, then propose." })).toBeVisible();
    const bar = page.getByRole("region", { name: "Tab connected to a connector" });
    await expect(bar.getByText(`Connected to ${CONNECTOR.name}`)).toBeVisible();
    await expect(bar.getByText("Last: Pointed at a field: spec.compute.bloblang")).toBeVisible();
    if (process.env.SHOTS_DIR) await page.screenshot({ path: `${process.env.SHOTS_DIR}/paired-tab-editor-${label}.png` });
    await bar.getByRole("button", { name: "Actions (2)" }).click();
    await expect(bar.getByText(/via helsinki-in-claude-code/).first()).toBeVisible();
    if (process.env.SHOTS_DIR) await page.screenshot({ path: `${process.env.SHOTS_DIR}/paired-tab-actions-${label}.png` });
    expect(await axeViolations(page)).toEqual([]);

    await bar.getByRole("button", { name: "Disconnect" }).click();
    await expect(bar).toHaveCount(0);
    // Nothing the connector sent saved or proposed anything: besides the person's two clicks the
    // only requests are the editor's own (the person's private draft of the open form, a dry-run
    // check), as when they type; no write to the pipeline, no change request.
    const tabWrites = writes.filter((w) => w.path.startsWith(`/api/v1/connectors/${ID}/tab`));
    expect(tabWrites).toEqual([
      { method: "POST", path: `/api/v1/connectors/${ID}/tab` },
      { method: "DELETE", path: `/api/v1/connectors/${ID}/tab` },
    ]);
    const proposals = writes.filter(
      (w) => !w.path.startsWith(`/api/v1/connectors/${ID}/tab`) && !w.path.startsWith(`/api/v1/projects/${PROJECT}/drafts/`) && !w.path.includes("dryRun=All"),
    );
    expect(proposals).toEqual([]);
  });
}
