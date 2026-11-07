import { expect, test } from "@playwright/test";
import { axeViolations } from "./axe";

// T-4973 (walker): the old /shared address lands on the Endpoints page, whose "Connect an AI
// assistant" panel holds two code lines wider than a phone. A scrolling block the keyboard
// cannot reach is axe's scrollable-region-focusable; jsdom has no layout, so only a browser sees it.

const ENDPOINT = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name: "parking", namespace: "bbsk", title: { sk: "Parkovanie", en: "Parking" } },
  spec: { contextSpaceRef: "otvorene-data", slug: "parking", audience: "public", enabledRepresentations: ["ngsi-ld"] },
  status: { phase: "Live" },
};

for (const width of [1440, 400]) {
  test(`the assistant panel's code lines scroll by keyboard at ${width} px`, async ({ page }, info) => {
    await page.setViewportSize({ width, height: 900 });
    await page.route("**/api/**", async (route) => {
      const path = new URL(route.request().url()).pathname;
      const json = (body: unknown) =>
        route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
      if (path.endsWith("/auth/me")) {
        return json({ subject: "s", username: "jana", name: "Jana", email: "jana@bbsk.sk", roles: ["portal-editor"] });
      }
      if (path.endsWith("/permissions/me")) return json({ project: "bbsk", bootstrap: true, grants: [] });
      if (path === "/api/v1/projects") {
        return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "bbsk" }] });
      }
      if (path === "/api/v1/projects/bbsk/endpoints") {
        return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [ENDPOINT] });
      }
      if (path.endsWith("/events")) return route.fulfill({ status: 200, contentType: "text/event-stream", body: "" });
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
    });

    await page.goto("/projects/bbsk/shared?lang=en");
    await expect(page).toHaveURL(/\/projects\/bbsk\/endpoints/);
    // Behind the header's button since T-6095, so the list starts right under the title.
    await page.getByRole("button", { name: "Connect AI" }).click();
    const panel = page.getByRole("dialog", { name: "Connect an AI assistant" });
    await expect(panel).toBeVisible();
    const command = panel.getByLabel("Claude Code command");
    await command.focus();
    await expect(command).toBeFocused();
    await command.scrollIntoViewIfNeeded();
    await page.screenshot({ path: info.outputPath(`endpoints-hub-${width}.png`), fullPage: true });
    expect(await axeViolations(page)).toEqual([]);
  });
}
