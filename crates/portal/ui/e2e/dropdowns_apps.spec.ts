/**
 * T-4948: the apps and publishing fields that named something by typed text are pickers, in a real
 * browser at 1440 and 400 px: a catalogue's default organization, a connector's projects, spaces and
 * themes, the projects an MCP server admits and the assistant a conversation list is filtered by.
 * Each one is shot closed, open, searching and with nothing matching; the API is answered here.
 */
import { expect, test } from "@playwright/test";
import type { Locator, Page } from "@playwright/test";

const SHOTS = process.env.T4948_SHOTS ?? "test-results/dropdowns-apps";
const API = "joinedcontext.com/v1alpha1";
const LIST = (items: unknown[]) => ({ apiVersion: API, kind: "List", items });
const manifest = (kind: string, name: string, namespace: string, title: string, spec: unknown = {}) => ({
  apiVersion: API,
  kind,
  metadata: { name, namespace, title: { en: title } },
  spec,
});
const none: unknown[] = [];

async function stubApi(page: Page): Promise<void> {
  await page.route(
    (url) => url.pathname.startsWith("/api/"),
    async (route) => {
      const request = route.request();
      const path = new URL(request.url()).pathname;
      const json = (body: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
      if (path.endsWith("/auth/me")) return json({ subject: "s1", username: "eva", name: "Eva Steward", email: "eva@hel.fi", roles: [] });
      if (path.endsWith("/permissions/me")) return json({ project: "helsinki", bootstrap: true, grants: [] });
      if (path.endsWith("/events")) return route.fulfill({ status: 200, contentType: "text/event-stream", body: "" });
      if (path === "/api/v1/preferences") return json({});
      if (path === "/api/v1/projects") return json({ apiVersion: API, kind: "ProjectList", items: [{ name: "helsinki" }, { name: "espoo" }, { name: "vantaa" }] });
      if (path === "/api/v1/catalogue") {
        return json({
          total: 0, page: 1, pageSize: 20, datasets: [], unavailable: [],
          facets: {
            publisher: [
              { value: "helsinki", label: "City of Helsinki", count: 12 },
              { value: "hsl", label: "Helsinki Regional Transport", count: 4 },
            ],
            theme: none, format: none, licence: none, spatial: none, year: none,
            keyword: [{ value: "air", label: "air", count: 6 }],
          },
        });
      }
      if (path === "/api/v1/projects/helsinki/ckan/status") return json({ instances: [], publications: [] });
      if (path === "/api/v1/projects/helsinki/contextspaces") {
        return json(LIST([manifest("ContextSpace", "air", "helsinki", "Air quality"), manifest("ContextSpace", "parking", "helsinki", "Parking")]));
      }
      if (path === "/api/v1/projects/espoo/contextspaces") return json(LIST([manifest("ContextSpace", "bikes", "espoo", "City bikes")]));
      if (path === "/api/v1/projects/helsinki/endpoints") {
        return json(LIST([manifest("Endpoint", "air", "helsinki", "Air now", { slug: "air1", audience: "organization", contextSpaceRef: { name: "air" } })]));
      }
      if (path === "/api/v1/projects/helsinki/assistantdeployments") {
        return json(LIST([manifest("AssistantDeployment", "citizen", "helsinki", "Citizen desk"), manifest("AssistantDeployment", "staff", "helsinki", "Staff helper")]));
      }
      if (path.endsWith("/knowledge/conversations")) return json({ items: [] });
      return json(LIST([]));
    },
  );
}

/** Shoots `name` at the page's width, the open list included. */
async function shoot(page: Page, name: string): Promise<void> {
  const width = page.viewportSize()?.width ?? 1440;
  await page.screenshot({ path: `${SHOTS}/${name}-${width}.png`, fullPage: false });
}

/** Closed, open, searching and nothing matching, for one picker. */
async function states(page: Page, picker: Locator, name: string, search: string): Promise<void> {
  await picker.scrollIntoViewIfNeeded();
  await shoot(page, `${name}-1-closed`);
  await picker.click();
  await expect(page.getByRole("listbox").getByRole("option").first()).toBeVisible();
  await shoot(page, `${name}-2-open`);
  await picker.fill(search);
  await expect(page.getByRole("listbox").getByRole("option").first()).toBeVisible();
  await shoot(page, `${name}-3-searching`);
  await picker.fill("zzzz");
  await shoot(page, `${name}-4-empty`);
  await picker.fill("");
}

for (const width of [1440, 400]) {
  test.describe(`the apps and publishing pickers at ${width} px (T-4948)`, () => {
    test.use({ viewport: { width, height: 900 } });

    test("a catalogue's default organization is picked from the catalogue's, a new one typed", async ({ page }) => {
      await stubApi(page);
      await page.goto("/projects/helsinki/ckan?lang=en");
      const org = page.getByRole("combobox", { name: /Default organization/ });
      await states(page, org, "ckan-org", "trans");
      await org.fill("espoo-city");
      await page.getByRole("option", { name: /New: “espoo-city”/ }).click();
      await expect(org).toHaveValue("espoo-city");
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    });


    test("an MCP server admits projects picked from the person's own", async ({ page }) => {
      await stubApi(page);
      await page.goto("/projects/helsinki/mcp?lang=en");
      await page.getByRole("button", { name: "New MCP server" }).first().click();
      const dialog = page.getByRole("dialog", { name: "New MCP server" });
      await dialog.getByLabel(/^Who may connect/).selectOption("project-list");
      const admits = dialog.getByRole("combobox", { name: /^Projects it admits/ });
      await states(page, admits, "mcp-projects", "van");
      await admits.fill("esp");
      await page.getByRole("option", { name: /espoo/ }).click();
      await expect(dialog.getByRole("list", { name: /Projects it admits/ })).toContainText("espoo");
      await shoot(page, "mcp-projects-5-filled");
    });

    test("the conversations are filtered by an assistant of the project", async ({ page }) => {
      await stubApi(page);
      await page.goto("/projects/helsinki/knowledge/conversations?lang=en");
      const assistant = page.getByRole("combobox", { name: "Assistant" });
      await states(page, assistant, "conversations-assistant", "staff");
      await assistant.click();
      await page.getByRole("option", { name: /Citizen desk/ }).click();
      await expect(assistant).toHaveValue("Citizen desk");
    });
  });
}
