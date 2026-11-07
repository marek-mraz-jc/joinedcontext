/**
 * The assistant run as one connector segment (T-5958): the Capabilities panel's Tools select
 * picks a segment, its hint names what that segment reads, and the conversation's start carries
 * it, so the server answers with that segment's registry tools, each call checked as a
 * connector's. The API is answered in the browser; the stub records what the dock posts, so the
 * choice is judged by the request, not by a label.
 */
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

const API = "joinedcontext.com/v1alpha1";
const PROJECT = "banskabystrica";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-approver"],
  groups: [PROJECT],
};

async function stubApi(page: Page): Promise<{ path: string; body: unknown }[]> {
  const posts: { path: string; body: unknown }[] = [];
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });

    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json({ apiVersion: API, kind: "List", items: [{ name: PROJECT }] });
    if (request.method() !== "GET") posts.push({ path, body: request.postDataJSON() });
    if (path.endsWith("/assistant/conversations") && request.method() === "POST") {
      return json({ id: "run-conv-9", status: "queued" }, 202);
    }
    if (path === `/api/v1/projects/${PROJECT}/agent-runs`) return json({ items: [] });
    if (path.endsWith("/events")) return route.fulfill({ status: 204 });
    return json({ apiVersion: API, kind: "List", items: [] });
  });
  return posts;
}

for (const [label, width, height] of [
  ["1440", 1440, 900],
  ["400", 400, 860],
] as const) {
  test(`the Tools select runs the conversation as one segment (${label})`, async ({ page }) => {
    await page.setViewportSize({ width, height });
    const posts = await stubApi(page);
    await page.goto(`/projects/${PROJECT}/assistant?lang=en`);
    await page.getByRole("main").getByRole("button", { name: "Open the assistant" }).click();
    const dock = page.getByRole("complementary", { name: "Conversation" });
    await expect(dock).toBeVisible();

    await dock.getByRole("button", { name: /^Capabilities/ }).click();
    const tools = dock.getByRole("combobox", { name: "Tools" });
    await expect(tools).toHaveValue("");
    await expect(tools).toHaveAccessibleDescription(/e\.g\. Pipelines/);
    if (process.env.SHOTS_DIR) await page.screenshot({ path: `${process.env.SHOTS_DIR}/assistant-tools-none-${label}.png` });

    await tools.selectOption("pipeline");
    await expect(tools).toHaveAccessibleDescription("reads pipelines, sources and their metrics");
    expect(await axeViolations(page)).toEqual([]);
    if (process.env.SHOTS_DIR) await page.screenshot({ path: `${process.env.SHOTS_DIR}/assistant-tools-pipeline-${label}.png` });

    await dock.getByRole("button", { name: /^Capabilities/ }).click();
    await dock.getByLabel("Ask the assistant").fill("Which pipelines failed today?");
    await dock.getByLabel("Ask the assistant").press("Enter");
    await expect.poll(() => posts.find((post) => post.path.endsWith("/assistant/conversations"))).toBeTruthy();
    const started = posts.find((post) => post.path.endsWith("/assistant/conversations"));
    expect((started?.body as { access?: unknown }).access).toMatchObject({ preset: "build", segment: "pipeline" });
  });
}
