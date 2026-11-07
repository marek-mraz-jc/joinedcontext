/**
 * Who may talk to the assistant (T-6464, AG-67): propose on any kind opens a conversation, an App
 * grant is not needed, and a read-only role is told so before they type. The API is answered in
 * the browser; the stub records what the dock posts.
 */
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

const API = "joinedcontext.com/v1alpha1";
const PROJECT = "bbsk";
const ASKED = "Which air quality data do we have?";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "demo.editor",
  name: "Demo Editor",
  email: "demo.editor@bbsk.sk",
  roles: [],
  groups: [PROJECT],
};

const READ = ["ContextSpace", "Endpoint", "DataSource", "Pipeline", "DataModel", "App", "Dashboard", "Layer"];

function grantsOf(role: "editor" | "viewer") {
  const reads = { role: "viewer", binding: "readers", scope: `project:${PROJECT}`, rule: { kinds: READ, verbs: ["read"] } };
  const proposes = {
    role: "editor",
    binding: "editors",
    scope: `project:${PROJECT}`,
    rule: { kinds: ["ContextSpace", "Endpoint", "DataSource", "Pipeline", "DataModel"], verbs: ["read", "propose"] },
  };
  return role === "editor" ? [reads, proposes] : [reads];
}

async function stubApi(page: Page, role: "editor" | "viewer", started = 202): Promise<unknown[]> {
  const asked: unknown[] = [];
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });

    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json({ apiVersion: API, kind: "List", items: [{ name: PROJECT }] });
    if (path.endsWith("/permissions/me")) return json({ project: PROJECT, bootstrap: false, grants: grantsOf(role) });
    if (path.endsWith("/assistant/conversations") && request.method() === "POST") {
      asked.push(request.postDataJSON());
      if (started !== 202) {
        return json({ type: "about:blank", title: "Forbidden", status: started, detail: "the runs of this project for today are used up" }, started);
      }
      return json({ id: "run-conv-9", status: "queued" }, 202);
    }
    if (path === `/api/v1/projects/${PROJECT}/agent-runs`) return json({ items: [] });
    if (path.endsWith("/events")) return route.fulfill({ status: 204 });
    return json({ apiVersion: API, kind: "List", items: [] });
  });
  return asked;
}

async function openDock(page: Page) {
  await page.goto(`/projects/${PROJECT}/assistant?lang=en`);
  await page.getByRole("main").getByRole("button", { name: "Open the assistant" }).click();
  const dock = page.getByRole("complementary", { name: "Conversation" });
  await expect(dock).toBeVisible();
  return dock;
}

for (const [label, width, height] of [
  ["1440", 1440, 900],
  ["400", 400, 860],
] as const) {
  const shot = async (page: Page, name: string) => {
    if (process.env.SHOTS_DIR) await page.screenshot({ path: `${process.env.SHOTS_DIR}/${name}-${label}.png` });
  };

  test(`an editor without an App grant asks about their data (${label})`, async ({ page }) => {
    await page.setViewportSize({ width, height });
    const asked = await stubApi(page, "editor");
    const dock = await openDock(page);
    await expect(dock.getByRole("button", { name: /^Build an app/ })).toHaveAttribute("aria-disabled", "true");
    await expect(dock.getByTestId("assistant-read-only")).toHaveCount(0);
    await expect(dock.getByRole("button", { name: /^Find data/ })).not.toHaveAttribute("aria-disabled", "true");
    await dock.getByLabel("Ask the assistant").fill(ASKED);
    expect(await axeViolations(page)).toEqual([]);
    await shot(page, "editor-empty");
    await dock.getByLabel("Ask the assistant").press("Enter");
    await expect.poll(() => asked.length).toBe(1);
    expect(asked[0]).toMatchObject({ message: ASKED });
    await expect(dock.getByTestId("assistant-empty")).toHaveCount(0);
    await shot(page, "editor-asked");
  });

  test(`a refused start keeps the question and says why (${label})`, async ({ page }) => {
    await page.setViewportSize({ width, height });
    await stubApi(page, "editor", 429);
    const dock = await openDock(page);
    await dock.getByLabel("Ask the assistant").fill(ASKED);
    await dock.getByLabel("Ask the assistant").press("Enter");
    await expect(dock.getByText("the runs of this project for today are used up")).toBeVisible();
    await expect(dock.getByLabel("Ask the assistant")).toHaveValue(ASKED);
    await shot(page, "editor-refused");
  });

  test(`a read-only role is told before they type (${label})`, async ({ page }) => {
    await page.setViewportSize({ width, height });
    const asked = await stubApi(page, "viewer");
    const dock = await openDock(page);
    await expect(dock.getByTestId("assistant-read-only")).toContainText("can read but not change anything");
    await expect(dock.getByLabel("Ask the assistant")).toBeDisabled();
    await expect(dock.getByRole("button", { name: /^Find data/ })).toHaveAttribute("aria-disabled", "true");
    await dock.getByRole("button", { name: /^Find data/ }).click({ force: true });
    expect(asked).toEqual([]);
    expect(await axeViolations(page)).toEqual([]);
    await shot(page, "viewer-empty");
  });
}
