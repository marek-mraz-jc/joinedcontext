import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import type { Locator } from "@playwright/test";

// T-6134: on the Endpoint editor every dropdown of a calendar or map output reads its hint to a
// screen reader, as the walker asks of every form field (UI-16).

const API = "joinedcontext.com/v1alpha1";
const LINKML = "id: x\nname: x\nclasses:\n  Event:\n    slots: [name, startDate, location]\nslots:\n  name: { required: true }\n  startDate: { range: datetime, required: true }\n  location: { range: GeoProperty }\n";
const ENDPOINT = {
  apiVersion: API,
  kind: "Endpoint",
  metadata: { name: "events", namespace: "helsinki", labels: { "joinedcontext.com/space": "tapahtumat" } },
  spec: {
    contextSpaceRef: "tapahtumat",
    slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa",
    audience: "public",
    enabledRepresentations: ["ngsi-ld", "ical"],
    representations: {
      ical: [{ name: "kalendar", title: { en: "City events" }, sources: [{ type: "Event", preset: "sdm-event" }] }],
      widget: [{ name: "mapa", sources: [{ type: "Event" }] }],
    },
  },
  status: { phase: "Live" },
};
const MODEL = { apiVersion: API, kind: "DataModel", metadata: { name: "events", namespace: "helsinki" }, spec: { contextSpaceRef: "tapahtumat", linkml: LINKML, version: "1.0.0" } };

async function hintOf(control: Locator): Promise<string> {
  return control.evaluate((element) =>
    (element.getAttribute("aria-describedby") ?? "")
      .split(/\s+/)
      .map((id) => document.getElementById(id)?.textContent?.trim() ?? "")
      .join(" ")
      .trim(),
  );
}

for (const width of [1440, 400]) {
  test(`each calendar and map dropdown of the Endpoint editor has its hint at ${width} px (T-6134)`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    const list = (items: unknown[]) => ({ apiVersion: API, kind: "List", items });
    await page.route("**/api/v1/**", (route) => {
      const path = new URL(route.request().url()).pathname;
      const json = (body: unknown) => route.fulfill({ contentType: "application/json", body: JSON.stringify(body) });
      if (path.endsWith("/auth/me")) {
        return json({ subject: "s1", username: "steward", name: "Steward", email: "s@hel.fi", roles: ["portal-editor"] });
      }
      if (path.endsWith("/permissions/me")) return json({ project: "helsinki", grants: [{ role: "steward", rule: { verbs: ["read", "propose", "approve"], kinds: ["Endpoint", "DataModel", "ContextSpace"] }, binding: "stewards", scope: "project:helsinki" }] });
      if (path === "/api/v1/projects") return json(list([{ name: "helsinki" }]));
      if (path === "/api/v1/projects/helsinki/endpoints/events") return json(ENDPOINT);
      if (path === "/api/v1/projects/helsinki/endpoints") return json(list([ENDPOINT]));
      if (path === "/api/v1/projects/helsinki/datamodels") return json(list([MODEL]));
      return json(list([]));
    });
    await page.route("**/cs/**", (route) => route.fulfill({ contentType: "application/ld+json", body: "[]" }));
    await page.route("**/api/endpoint/**", (route) => route.fulfill({ status: 404, contentType: "application/json", body: "{}" }));

    await page.goto("/projects/helsinki/endpoints/events/edit?lang=en");
    const start = page.getByLabel("DTSTART", { exact: true });
    await expect(start).toBeVisible();
    expect(await hintOf(start)).toContain("for example Event");
    expect(await hintOf(page.getByLabel("SUMMARY", { exact: true }))).toContain("for example Event");
    const types = page.getByRole("combobox", { name: "Type it maps" });
    await expect(types).toHaveCount(2);
    for (const type of await types.all()) expect(await hintOf(type)).toContain("for example Event");
    expect(await hintOf(page.getByLabel("Position from", { exact: true }))).not.toBe("");

    const dir = join("test-results", "T-6134");
    mkdirSync(dir, { recursive: true });
    await start.scrollIntoViewIfNeeded();
    await page.screenshot({ path: join(dir, `endpoint-calendar-hints-${width}.png`) });
    await types.nth(1).scrollIntoViewIfNeeded();
    await page.screenshot({ path: join(dir, `endpoint-map-hints-${width}.png`) });
  });
}
