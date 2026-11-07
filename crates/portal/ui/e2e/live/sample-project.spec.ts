/**
 * T-3234, PF-109 — "Try it with sample data" on dev: a newcomer (`demo.viewer`) who keeps the
 * first-run guidance is offered the sample project from another project's home, one click opens
 * it read-only, and the Portal marks it Sample on its home and in the project switcher. Nothing is
 * created. In English at 1440 px and in Slovak at 375 px, a screenshot of each step attached.
 */
import { expect, test } from "@playwright/test";
import type { Page, TestInfo } from "@playwright/test";
import { VIEWER, csrf, signIn } from "./portal";

test.setTimeout(240_000);

const WORDS = {
  en: { home: "Home", try: "Try it with sample data", title: "A sample project of real open data", badge: "Sample" },
  sk: { home: "Domov", try: "Vyskúšajte to na ukážkových dátach", title: "Ukážkový projekt so skutočnými otvorenými dátami", badge: "Ukážka" },
} as const;

async function shot(page: Page, info: TestInfo, name: string): Promise<void> {
  await info.attach(name, { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
}

for (const [lang, width] of [["en", 1440], ["sk", 375]] as const) {
  test(`a newcomer tries the sample from another project's home (${lang}, ${width} px)`, async ({ browser }, info) => {
    const words = WORDS[lang];
    const { context, page } = await signIn(browser, VIEWER, `/?lang=${lang}`);
    await page.setViewportSize({ width, height: 900 });
    const listed = await page.request.get("/api/v1/projects");
    expect(listed.ok(), "the projects the viewer may read").toBe(true);
    const projects = ((await listed.json()) as { items: { name: string; sample: boolean }[] }).items;
    const sample = projects.find((project) => project.sample)?.name;
    expect(sample, "dev labels one project the viewer reads as the sample").toBeTruthy();
    const other = projects.find((project) => !project.sample)?.name;
    expect(other, "the viewer reads a project that is not the sample").toBeTruthy();

    // The guidance is the viewer's own preference: keep it shown for the walk, put it back after.
    const token = await csrf(context);
    const before = (await (await page.request.get("/api/v1/preferences")).json()) as Record<string, unknown>;
    const put = (body: Record<string, unknown>) =>
      page.request.put("/api/v1/preferences", { data: body, headers: { "x-csrf-token": token } });
    expect((await put({ ...before, firstRunDismissed: false })).ok()).toBe(true);
    try {
      await page.goto(`/projects/${other}/home?lang=${lang}`, { waitUntil: "load" });
      await expect(page.getByRole("heading", { level: 1, name: words.home })).toBeVisible({ timeout: 60_000 });
      const offer = page.getByRole("main").getByRole("link", { name: words.try });
      await expect(offer).toBeVisible({ timeout: 60_000 });
      await shot(page, info, `before-${lang}-${width}`);

      await offer.click();
      await expect(page).toHaveURL(new RegExp(`/projects/${sample}/home`));
      await expect(page.getByText(words.title)).toBeVisible({ timeout: 60_000 });
      await expect(page.getByRole("main").getByText(words.badge, { exact: true })).toBeVisible();
      // No page sideways at a phone's width (UI-27).
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true);
      await shot(page, info, `after-${lang}-${width}`);
    } finally {
      await put(before);
      await context.close();
    }
  });
}
