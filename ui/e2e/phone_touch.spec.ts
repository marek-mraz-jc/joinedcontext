// T-3279: on a phone every control is big enough for a finger and nothing scrolls sideways. The
// pages are read with an empty project, which is what a newcomer's phone shows first.
import { expect, test } from "@playwright/test";
import { smallTouchTargets } from "./touch";

const PROJECT = "helsinki";
const IDENTITY = { subject: "s", username: "jana", name: "Jana", email: "jana@hel.fi", roles: ["portal-editor"] };

test.use({ viewport: { width: 375, height: 812 }, hasTouch: true, isMobile: true });

const READ_PATHS = ["spaces", "endpoints", "dashboards", "explore", "approvals", "assistant", "activity"];

/** A build this browser started and saw finish: the header then carries the jobs button too. */
const FINISHED_BUILD = [
  { id: "appBuild:helsinki:kartta:8", kind: "appBuild", project: PROJECT, name: "kartta", startedAt: "2026-10-08T03:00:00Z", finishedAt: "2026-10-08T03:03:00Z", outcome: "success" },
];

for (const [path, jobs] of [...READ_PATHS.map((path) => [path, []] as const), ["spaces", FINISHED_BUILD] as const]) {
  test(`/${path} at 375 px with touch${jobs.length > 0 ? " and a finished build" : ""}: 44 px targets and nothing sideways`, async ({ page }) => {
    // The fullest header (T-3319): an administrator (bootstrap), with a job to show.
    await page.addInitScript((stored) => localStorage.setItem("jc.jobs", stored), JSON.stringify(jobs));
    await page.route("**/api/v1/**", async (route) => {
      const p = new URL(route.request().url()).pathname;
      const json = (b: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(b) });
      if (p.endsWith("/auth/me")) return json(IDENTITY);
      if (p.endsWith("/permissions/me")) return json({ project: PROJECT, bootstrap: true, grants: [] });
      if (p === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [{ name: PROJECT }] });
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
    });
    await page.goto(`/projects/${PROJECT}/${path}?lang=en`);
    await page.locator("main h1").first().waitFor({ timeout: 30_000 });
    expect(await page.evaluate(() => matchMedia("(pointer: coarse)").matches), "the phone is a touch screen").toBe(true);

    const small = await page.evaluate(() =>
      [...document.querySelectorAll<HTMLElement>("button, [role=button], [role=tab], select, input:not([type=checkbox]):not([type=radio]):not([type=hidden])")]
        .filter((element) => {
          const box = element.getBoundingClientRect();
          const style = getComputedStyle(element);
          return box.width > 0 && box.height > 0 && style.visibility !== "hidden" && style.display !== "none";
        })
        .map((element) => ({ element, box: element.getBoundingClientRect() }))
        .filter(({ box }) => box.height < 44 - 0.5)
        .map(({ element, box }) => `${element.tagName.toLowerCase()} "${(element.getAttribute("aria-label") ?? element.textContent ?? "").trim().slice(0, 40)}" ${Math.round(box.width)}×${Math.round(box.height)}`),
    );
    expect(small, "every control is at least 44 px tall").toEqual([]);
    expect(await page.evaluate(() => document.documentElement.scrollWidth), "nothing scrolls sideways").toBeLessThanOrEqual(375);
  });
}

// T-5596: the links that are controls (brand, crumbs, a table's name links, the footer's address)
// are a finger's 44 px too, at the 400 and 360 px phones the UI critic measured; a link inside a
// sentence keeps its line.
const POLICIES = ["app-air-quality-1", "app-air-weather-explorer-1-history", "app-bike-weather-demand-2-history"].map((name) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Policy",
  metadata: { name, namespace: PROJECT },
  spec: { subjects: [{ kind: "Group", name: "readers" }], rules: [{ operations: ["read"] }] },
}));

for (const width of [400, 360]) {
  for (const path of ["spaces", "policies"]) {
    test(`/${path} at ${width} px with touch: links and boxes are 44 px targets`, async ({ page }) => {
      await page.setViewportSize({ width, height: 800 });
      await page.route("**/api/v1/**", async (route) => {
        const p = new URL(route.request().url()).pathname;
        const json = (b: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(b) });
        if (p.endsWith("/auth/me")) return json(IDENTITY);
        if (p.endsWith("/permissions/me")) return json({ project: PROJECT, bootstrap: true, grants: [] });
        if (p === "/api/v1/branding") return json({ organisation: "joinedcontext demo", contactEmail: "hello@joinedcontext.com" });
        if (p === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [{ name: PROJECT }] });
        if (p.endsWith(`/projects/${PROJECT}/policies`)) return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: POLICIES });
        return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
      });
      await page.goto(`/projects/${PROJECT}/${path}?lang=en`);
      await page.locator("main h1").first().waitFor({ timeout: 30_000 });
      if (path === "policies") await page.getByRole("link", { name: "app-air-quality-1", exact: true }).waitFor();
      await expect(page.getByRole("contentinfo").getByRole("link", { name: "hello@joinedcontext.com" })).toBeVisible();

      expect(await smallTouchTargets(page), "every control is at least 44 x 44 px").toEqual([]);
      expect(await page.evaluate(() => document.documentElement.scrollWidth), "nothing scrolls sideways").toBeLessThanOrEqual(width);
      if (process.env.SHOTS_DIR) await page.screenshot({ path: `${process.env.SHOTS_DIR}/phone-${path}-${width}.png` });
    });
  }
}
