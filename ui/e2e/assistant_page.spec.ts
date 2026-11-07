/**
 * The assistant's page in a browser (T-2729; AG-87, AG-93, UI-15, UI-16).
 *
 * `tests/assistant_page.test.tsx` holds the list's logic under jsdom. This run opens the page at
 * its own address, the way a person arrives from the navigation: the conversations and the work
 * the assistant did are listed, Open and Continue hand the run to the dock, and a project with
 * no run yet offers the assistant itself. The API is answered in the browser; the stub records
 * what the page posts, so the continued conversation is judged by the request, not by a label.
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

const run = (fields: Record<string, unknown>) => ({
  project: PROJECT,
  appName: "",
  endpointName: "",
  endpointSlug: "",
  profile: "app-builder",
  appClass: "static",
  visibility: "private",
  kind: "conversation",
  unattended: false,
  continues: null,
  steps: 2,
  tokensUsed: 500,
  createdBy: "jana.kovacova",
  createdAt: "2026-09-14T09:00:00Z",
  ...fields,
});

const ENDED = run({ id: "run-conv-1", prompt: "Find all air quality sensors in town", status: "cancelled" });
const BUILT = run({
  id: "run-work-1",
  kind: "application",
  appName: "city-bikes-app",
  endpointName: "bikes-endpoint",
  endpointSlug: "slug123",
  visibility: "project",
  unattended: true,
  prompt: "Create city bikes overview",
  status: "awaiting_approval",
});

async function stubApi(page: Page, runs: unknown[]): Promise<{ method: string; path: string; body: unknown }[]> {
  const posts: { method: string; path: string; body: unknown }[] = [];
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });

    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json({ apiVersion: API, kind: "List", items: [{ name: PROJECT }] });
    if (request.method() !== "GET") posts.push({ method: request.method(), path, body: request.postDataJSON() });
    if (path.endsWith("/assistant/conversations") && request.method() === "POST") {
      return json(run({ id: "run-conv-2", continues: ENDED.id, prompt: "Continue this conversation.", status: "interviewing" }), 202);
    }
    if (path === `/api/v1/projects/${PROJECT}/agent-runs`) return json({ items: runs });
    // The dock follows its run with an event stream; this page's test is the list, not the stream.
    if (path.endsWith("/events")) return route.fulfill({ status: 204 });
    return json({ apiVersion: API, kind: "List", items: [] });
  });
  return posts;
}

test.describe("the assistant's page", () => {
  test("lists the conversations and the work, and Continue starts a conversation that continues the ended one", async ({
    page,
  }) => {
    const posts = await stubApi(page, [ENDED, BUILT]);
    await page.goto(`/projects/${PROJECT}/assistant?lang=en`);

    await expect(page.getByRole("heading", { level: 1, name: "Assistant" })).toBeVisible();
    const table = page.getByRole("table", { name: "Assistant" });
    const ended = table.getByRole("row", { name: /Find all air quality sensors in town/ });
    await expect(ended).toBeVisible();
    await expect(table.getByRole("row", { name: /City bikes app/ })).toContainText("Application");
    expect(await axeViolations(page)).toEqual([]);

    await ended.getByRole("button", { name: "Continue" }).click();
    await expect(page.getByRole("complementary", { name: "Conversation" })).toBeVisible();
    const continued = posts.find((post) => post.path.endsWith("/assistant/conversations"));
    expect(continued?.body).toEqual({ message: "Continue this conversation.", continues: ENDED.id });
    expect(posts.filter((post) => post.path.endsWith("/agent-runs")), "the page starts no work of its own").toEqual([]);
  });

  test("a project with no run yet offers the assistant itself", async ({ page }) => {
    const posts = await stubApi(page, []);
    await page.goto(`/projects/${PROJECT}/assistant?lang=en`);

    await expect(page.getByText(/^Nothing yet\. Open the assistant/)).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);
    await page.getByRole("main").getByRole("button", { name: "Open the assistant" }).click();
    await expect(page.getByRole("complementary", { name: "Conversation" })).toBeVisible();
    expect(posts).toEqual([]);
  });
});
