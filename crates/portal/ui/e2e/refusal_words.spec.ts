import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import { manifestReadsAsRest } from "./manifestReads";

test.beforeEach(({ page }) => manifestReadsAsRest(page));

// T-7106: a viewer on a project page reads what the role cannot do in words ("cannot add or change
// sync sources"), never the verb and the kind's code name ("'propose' on 'SyncSource'").

const API = "joinedcontext.com/v1alpha1";
const PAGES = [
  { plural: "syncsources", button: "Add source", sentence: "Your role in helsinki cannot add or change sync sources. Whoever manages the project can grant it." },
  { plural: "pipelines", button: "New pipeline", sentence: "Your role in helsinki cannot add or change pipelines. Whoever manages the project can grant it." },
];

for (const { plural, button, sentence } of PAGES) {
  for (const width of [1440, 400]) {
    test(`a viewer is told in words why ${plural} cannot be added, at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      const list = (items: unknown[]) => ({ apiVersion: API, kind: "List", items });
      await page.route("**/api/v1/**", (route) => {
        const path = new URL(route.request().url()).pathname;
        const json = (body: unknown) => route.fulfill({ contentType: "application/json", body: JSON.stringify(body) });
        if (path.endsWith("/auth/me")) return json({ subject: "v1", username: "viewer", name: "Vera Viewer", email: "v@hel.fi", roles: [] });
        if (path.endsWith("/permissions/me")) {
          return json({ project: "helsinki", grants: [{ role: "viewer", binding: "viewers", scope: "project", rule: { kinds: ["*"], verbs: ["read"] } }] });
        }
        if (path === "/api/v1/preferences") return json({ firstRunDismissed: true });
        if (path === "/api/v1/projects") return json(list([{ name: "helsinki" }]));
        return json(list([]));
      });

      await page.goto(`/projects/helsinki/${plural}?lang=en`);
      const add = page.getByRole("button", { name: button }).first();
      await expect(add).toHaveAttribute("aria-disabled", "true");
      await expect(add).toHaveAccessibleDescription(sentence);
      // Said once on the page too: a phone has no tooltip to show the description.
      await expect(page.getByText(sentence, { exact: true }).last()).toBeVisible();
      await expect(page.locator("body")).not.toContainText(/'propose'|'SyncSource'|'Pipeline'/);

      const dir = join("test-results", "T-7106");
      mkdirSync(dir, { recursive: true });
      await page.screenshot({ path: join(dir, `${plural}-refused-${width}.png`), fullPage: true });
    });
  }
}
