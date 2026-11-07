/**
 * T-3245 — a person starts an App's build, goes on to another page, and is told when it is done,
 * with the way back to the App. The build itself is answered in this browser (`page.route`): a
 * real dispatch would take a build pod of dev's one node for every run of this journey, and what
 * is under test is the Portal noticing, not the forge building. The App is one dev already has.
 */
import { expect, test } from "@playwright/test";
import { STEWARD, signIn } from "./portal";

test.setTimeout(240_000);

const PROJECT = "helsinki";

for (const width of [375, 1440]) {
  test(`at ${width}px a build started on the App page is announced on another page when it finishes`, async ({ browser }, info) => {
    const { context, page } = await signIn(browser, STEWARD, `/projects/${PROJECT}/apps?lang=en`);
    try {
      await page.setViewportSize({ width, height: 900 });
      const listed = await page.request.get(`/api/v1/projects/${PROJECT}/apps`);
      expect(listed.ok(), await listed.text()).toBe(true);
      const apps = ((await listed.json()) as { items?: { metadata: { name: string } }[] }).items ?? [];
      test.skip(apps.length === 0, "dev holds no App in helsinki to open");
      const name = apps[0].metadata.name;

      // Run 7 is there before; Rebuild starts run 8, which is running for a while, then succeeds.
      let phase: "before" | "running" | "done" = "before";
      const build = (run: Record<string, unknown>) => ({
        repositoryUrl: null,
        configurationUrl: null,
        run: { commit: "3f1c0e2d7a6b5c4f1b9c0e2d7a6b5c4f1b9c0e2d", url: "https://forge.example", ...run },
        typicalSeconds: 180,
        packageUrl: null,
        rebuild: { allowed: true },
      });
      await page.route(`**/api/v1/projects/${PROJECT}/apps/${name}/build`, (route) =>
        route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify(
            phase === "before"
              ? build({ number: 7, status: "completed", conclusion: "success" })
              : phase === "running"
                ? build({ number: 8, status: "in_progress", startedAt: new Date().toISOString() })
                : build({ number: 8, status: "completed", conclusion: "success" }),
          ),
        }),
      );
      await page.route(`**/api/v1/projects/${PROJECT}/apps/${name}/rebuild`, (route) => {
        phase = "running";
        return route.fulfill({ status: 202, body: "" });
      });

      await page.goto(`/projects/${PROJECT}/apps/${name}?lang=en`);
      await page.getByRole("button", { name: "Rebuild" }).click({ timeout: 60_000 });
      // The person goes on with something else.
      await page.goto(`/projects/${PROJECT}/spaces?lang=en`);
      const jobs = page.getByRole("button", { name: /^What you started: 1 running/ });
      await expect(jobs).toBeVisible({ timeout: 30_000 });
      await jobs.click();
      await expect(page.getByText(/^Running for .*left$/)).toBeVisible({ timeout: 30_000 });
      await info.attach(`running-${width}.png`, { body: await page.screenshot(), contentType: "image/png" });
      await page.keyboard.press("Escape");

      phase = "done";
      await expect(page.getByRole("status").filter({ hasText: `The build of ${name} finished.` })).toBeAttached({ timeout: 30_000 });
      const finished = page.getByRole("button", { name: "What you started: nothing running, 1 finished" });
      await finished.click();
      await info.attach(`finished-${width}.png`, { body: await page.screenshot(), contentType: "image/png" });
      await page.getByRole("menuitem", { name: new RegExp(`Build of ${name}`) }).click();
      await expect(page).toHaveURL(new RegExp(`/projects/${PROJECT}/apps/${name}`));
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    } finally {
      await context.close();
    }
  });
}
