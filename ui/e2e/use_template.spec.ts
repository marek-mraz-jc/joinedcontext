/**
 * T-5617: an App from a template while the model key is out, in a real browser at 1440 and 400 px.
 * The gallery opens and says why Generate is unavailable; "Use as is" asks the App's name and an
 * Endpoint the person reads, refuses a taken name under its field, and shows the Change the
 * template route answers. The API is answered here.
 */
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

const SHOTS = process.env.T5617_SHOTS ?? "test-results/use-template";
const API = "joinedcontext.com/v1alpha1";
const LIST = (items: unknown[]) => ({ apiVersion: API, kind: "List", items });
const TEMPLATES = [
  {
    name: "citizen-map",
    title: "Citizen map app",
    purpose: "Help a resident find something near them on a map: a place, a station, an event.",
    audience: "citizens",
    access: "Read-only, one endpoint that may be public.",
    dataNeeds: [{ types: ["BikeHireDockingStation"], attrs: ["name"], operations: ["queryEntity"] }],
  },
  {
    name: "kpi-dashboard",
    title: "KPI dashboard",
    purpose: "Show managers how the organization's indicators stand against their targets.",
    audience: "managers",
    access: "Read-only; the indicators may be public.",
    dataNeeds: [{ types: ["KeyPerformanceIndicator"], attrs: ["kpiValue"], operations: ["queryEntity"] }],
  },
];
const ENDPOINTS = [
  { apiVersion: API, kind: "Endpoint", metadata: { name: "bikes", namespace: "sharing", title: { en: "City bikes" } }, spec: { slug: "bikes", contextSpaceRef: "mobility" } },
];
const CHANGE = {
  apiVersion: API,
  kind: "Change",
  metadata: { name: "change-41", namespace: "sharing" },
  spec: { title: "create App bike-finder", lane: "red", operation: "create", files: [] },
  status: { phase: "Open", lane: "red" },
};

async function stubApi(page: Page, posts: unknown[]): Promise<void> {
  await page.route(
    (url) => url.pathname.startsWith("/api/"),
    async (route) => {
      const request = route.request();
      const path = new URL(request.url()).pathname;
      const json = (body: unknown, status = 200) =>
        route.fulfill({ status, contentType: status >= 400 ? "application/problem+json" : "application/json", body: JSON.stringify(body) });
      if (path.endsWith("/auth/me")) return json({ subject: "s1", username: "demo.author", name: "Demo Author", email: "author@hel.fi", roles: [] });
      if (path.endsWith("/permissions/me")) return json({ project: "sharing", bootstrap: true, grants: [] });
      if (path.endsWith("/events")) return route.fulfill({ status: 200, contentType: "text/event-stream", body: "" });
      if (path === "/api/v1/preferences") return json({});
      if (path === "/api/v1/projects") return json({ apiVersion: API, kind: "ProjectList", items: [{ name: "sharing" }] });
      if (path === "/api/v1/app-templates") return json({ templates: TEMPLATES, modelAvailable: false });
      if (path.includes("/screenshot/")) return route.fulfill({ status: 404, body: "" });
      if (path === "/api/v1/projects/sharing/endpoints") return json(LIST(ENDPOINTS));
      if (path === "/api/v1/projects/sharing/app-templates/citizen-map/apps") {
        const body = request.postDataJSON() as { name: string };
        posts.push(body);
        if (body.name === "taken") {
          const refusal = "name: the project 'sharing' has an App named 'taken' already; choose another name";
          return json({ type: "https://joinedcontext.com/problems/unprocessable", title: "Unprocessable Content", status: 422, detail: refusal, errors: [refusal] }, 422);
        }
        return json(CHANGE, 202);
      }
      return json(LIST([]));
    },
  );
}

async function shoot(page: Page, name: string): Promise<void> {
  const width = page.viewportSize()?.width ?? 1440;
  await page.screenshot({ path: `${SHOTS}/${name}-${width}.png`, fullPage: false });
}

for (const width of [1440, 400]) {
  test(`an App from a template while the model key is out at ${width} px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    const posts: unknown[] = [];
    await stubApi(page, posts);
    await page.goto("/projects/sharing/apps");
    const gallery = page.getByTestId("app-templates");
    await expect(gallery.getByText(/Generate is unavailable/)).toBeVisible();
    await gallery.scrollIntoViewIfNeeded();
    await shoot(page, "1-gallery-key-out");

    await gallery.getByRole("button", { name: "Use as is" }).first().click();
    const dialog = page.getByRole("dialog", { name: "Use Citizen map app as is" });
    await expect(dialog.getByRole("option", { name: "City bikes" })).toBeAttached();
    await shoot(page, "2-dialog-empty");

    await dialog.getByLabel(/App name/).fill("taken");
    await dialog.getByLabel(/Endpoint/).selectOption("bikes");
    await shoot(page, "3-dialog-filled");
    await dialog.getByRole("button", { name: "Create the App" }).click();
    await expect(dialog.getByText(/has an App named 'taken' already/)).toBeVisible();
    await expect(dialog.getByLabel(/App name/)).toHaveAttribute("aria-invalid", "true");
    await shoot(page, "4-dialog-refused");

    await dialog.getByLabel(/App name/).fill("bike-finder");
    await dialog.getByRole("button", { name: "Create the App" }).click();
    await expect(dialog.getByText("change-41")).toBeVisible();
    await shoot(page, "5-change-proposed");
    expect(posts).toEqual([
      { name: "taken", endpointName: "bikes" },
      { name: "bike-finder", endpointName: "bikes" },
    ]);
    expect(await axeViolations(page)).toEqual([]);
  });
}
