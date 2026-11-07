import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import { axeViolations } from "./axe";

// T-4975: the live walker found axe scrollable-region-focusable on an Endpoint's page at 400 px.
// The "Connect an AI assistant" lines scroll sideways there, so each must take keyboard focus.
// jsdom has no layout and axe skips the rule there; only a browser at phone width measures it.

const API = "joinedcontext.com/v1alpha1";

const ENDPOINT = {
  apiVersion: API,
  kind: "Endpoint",
  metadata: { name: "bikes", namespace: "helsinki", labels: { "joinedcontext.com/space": "liikenne" } },
  spec: {
    contextSpaceRef: "liikenne",
    slug: "abcdefghijklmnopqrstuvwxyz",
    audience: "public",
    enabledRepresentations: ["ngsi-ld", "mcp"],
  },
  status: { phase: "Live" },
};

for (const width of [400, 1440]) {
  test(`an Endpoint's connect lines are reachable by keyboard at ${width} px (T-4975)`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1024 });
    await page.route("**/api/v1/**", (route) => {
      const path = new URL(route.request().url()).pathname;
      const json = (body: unknown) => route.fulfill({ contentType: "application/json", body: JSON.stringify(body) });
      if (path.endsWith("/auth/me")) {
        return json({ subject: "s1", username: "steward", name: "Steward", email: "s@hel.fi", roles: ["portal-editor"] });
      }
      if (path === "/api/v1/projects") return json({ apiVersion: API, kind: "List", items: [{ name: "helsinki" }] });
      if (path === "/api/v1/projects/helsinki/endpoints/bikes") return json(ENDPOINT);
      return json({ apiVersion: API, kind: "List", items: [] });
    });

    // The query card's public reads: none answers here, and the card says so.
    await page.route("**/api/endpoint/**", (route) => route.fulfill({ status: 404, contentType: "application/json", body: "{}" }));

    await page.goto("/projects/helsinki/endpoints/bikes?lang=en");
    const section = page.getByRole("region", { name: "Connect an AI assistant" });
    await expect(section).toBeVisible();
    for (const line of await section.locator("pre").all()) {
      await line.focus();
      await expect(line).toBeFocused();
    }
    expect(await axeViolations(page)).toEqual([]);

    const dir = join("test-results", "T-4975");
    mkdirSync(dir, { recursive: true });
    await section.screenshot({ path: join(dir, `connect-${width}.png`) });
  });
}
