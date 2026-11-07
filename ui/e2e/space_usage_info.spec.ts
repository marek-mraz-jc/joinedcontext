import { expect, test } from "@playwright/test";
import type { Page, Route } from "@playwright/test";
import { axeViolations } from "./axe";

// T-6208: the Spaces list keeps each space's count and size; the (i) beside it says when the
// broker measured it and offers Recalculate. Route stubs only: the shots show the popover open,
// a recalculation in flight and the broker's refusal, at 1440 and 400.

const V = "joinedcontext.com/v1alpha1";
const list = (items: unknown[]) => ({ apiVersion: V, kind: "List", items });
const spaces = ["air", "mobility"].map((name) => ({ apiVersion: V, kind: "ContextSpace", metadata: { name, namespace: "helsinki" }, spec: {}, status: { phase: "Live" } }));
const kept = (name: string, measuredAt: string) => ({
  entities: name.length * 12_345,
  attrInstances: name.length * 98_765,
  broker: "main",
  bytes: { entities: 4_194_304, attrInstances: 52_428_800, byType: {} },
  observedAt: measuredAt,
  measuredAt,
  estimated: false,
});

async function stub(page: Page, recalculate: "ok" | "slow" | "refuse") {
  await page.route("**/api/**", async (route: Route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: status >= 400 ? "application/problem+json" : "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json({ subject: "s", username: "jana", name: "Jana Kováčová", roles: ["portal-viewer"] });
    if (path.endsWith("/permissions/me")) return json({ project: "helsinki", bootstrap: false, grants: [{ scope: "project:helsinki", rule: { kinds: ["ContextSpace"], verbs: ["read"] } }] });
    if (path === "/api/v1/projects") return json({ apiVersion: V, kind: "ProjectList", items: [{ name: "helsinki" }] });
    const again = /\/spaces\/(\w+)\/usage\/recalculate$/.exec(path)?.[1];
    if (again && request.method() === "POST") {
      if (recalculate === "refuse") return json({ type: "about:blank", title: "Service Unavailable", status: 503, detail: "the broker took longer than 20 s to count the space; try again in a minute" }, 503);
      if (recalculate === "slow") await new Promise((resolve) => setTimeout(resolve, 60_000));
      return json(kept(again, "2026-10-31T18:42:00Z"));
    }
    const space = /\/spaces\/(\w+)\/usage$/.exec(path)?.[1];
    if (space) return json(kept(space, "2026-10-31T17:05:00Z"));
    if (path === "/api/v1/projects/helsinki/spaces") return json(list(spaces));
    if (path.endsWith("/events")) return route.fulfill({ status: 200, contentType: "text/event-stream", body: "" });
    return json(list([]));
  });
}

async function openInfo(page: Page) {
  await page.goto("/projects/helsinki/spaces?lang=en");
  const info = page.getByTitle("When the size of air was measured");
  await info.click();
  await expect(page.getByText("Measured Oct 31, 2026").first()).toBeVisible();
}

for (const width of [1440, 400]) {
  test.describe(`at ${width} px`, () => {
    test.beforeEach(async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
    });

    test("the (i) says when the size was measured, and Recalculate shows the new time", async ({ page }, info) => {
      await stub(page, "ok");
      await openInfo(page);
      await page.screenshot({ path: info.outputPath(`spaces-measured-${width}.png`) });
      expect(await axeViolations(page)).toEqual([]);
      await page.getByRole("button", { name: "Recalculate" }).click();
      await expect(page.getByTestId("space-measured").first()).toContainText(/Oct 31, 2026, \d+:42/);
      await page.screenshot({ path: info.outputPath(`spaces-recalculated-${width}.png`) });
    });

    test("a recalculation in flight says so", async ({ page }, info) => {
      await stub(page, "slow");
      await openInfo(page);
      await page.getByRole("button", { name: "Recalculate" }).click();
      await expect(page.getByRole("button", { name: "Measuring…" })).toHaveAttribute("aria-disabled", "true");
      await page.screenshot({ path: info.outputPath(`spaces-measuring-${width}.png`) });
    });

    test("the broker's refusal is shown in the popover", async ({ page }, info) => {
      await stub(page, "refuse");
      await openInfo(page);
      await page.getByRole("button", { name: "Recalculate" }).click();
      await expect(page.getByRole("alert").filter({ hasText: "could not be measured" })).toBeVisible();
      await page.screenshot({ path: info.outputPath(`spaces-refused-${width}.png`) });
      expect(await axeViolations(page)).toEqual([]);
    });
  });
}
