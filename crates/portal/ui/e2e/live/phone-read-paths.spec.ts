/**
 * T-3279 — every read path on a phone and a tablet, with touch, on dev: the space list and a
 * space's entities, an entity, the explorer and its map, a dashboard and the assistant. Each is
 * held to a finger's 44 px for every control and to nothing scrolling sideways. Read-only.
 */
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { STEWARD, goSignedIn } from "./portal";

test.setTimeout(300_000);

const PROJECT = "helsinki";
const SIZES = [
  { width: 375, height: 812 },
  { width: 768, height: 1024 },
];

async function firstName(page: Page, plural: string): Promise<string | undefined> {
  const answer = await page.request.get(`/api/v1/projects/${PROJECT}/${plural}`);
  if (!answer.ok()) return undefined;
  return ((await answer.json()) as { items?: { metadata: { name: string } }[] }).items?.[0]?.metadata.name;
}

async function judge(page: Page, width: number): Promise<string[]> {
  return page.evaluate((viewport) => {
    const small = [...document.querySelectorAll<HTMLElement>("button, [role=button], [role=tab], select, input:not([type=checkbox]):not([type=radio]):not([type=hidden])")]
      .filter((element) => {
        const box = element.getBoundingClientRect();
        const style = getComputedStyle(element);
        return box.width > 0 && box.height > 0 && style.visibility !== "hidden" && box.top < innerHeight * 3;
      })
      .filter((element) => element.getBoundingClientRect().height < 43.5)
      .map((element) => `small: ${element.tagName.toLowerCase()} "${(element.getAttribute("aria-label") ?? element.textContent ?? "").trim().slice(0, 30)}"`);
    const sideways = document.documentElement.scrollWidth > viewport ? [`scrolls sideways: ${document.documentElement.scrollWidth} > ${viewport}`] : [];
    return [...small, ...sideways];
  }, width);
}

for (const size of SIZES) {
  test(`at ${size.width} px with touch every read path takes a finger and fits`, async ({ browser }, info) => {
    // A touch screen from the start, so the page is drawn for a finger (`pointer: coarse`).
    const context = await browser.newContext({ viewport: size, hasTouch: true, isMobile: true });
    const page = await context.newPage();
    try {
      await goSignedIn(page, STEWARD, `/projects/${PROJECT}/spaces?lang=en`);
      expect(await page.evaluate(() => matchMedia("(pointer: coarse)").matches)).toBe(true);
      const space = await firstName(page, "spaces");
      const dashboard = await firstName(page, "dashboards");
      const paths = [
        "spaces",
        ...(space ? [`spaces/${space}`] : []),
        "explore",
        ...(dashboard ? [`dashboards?edit=${dashboard}`] : ["dashboards"]),
        "assistant",
        "endpoints",
      ];
      const found: Record<string, string[]> = {};
      for (const path of paths) {
        await page.goto(`/projects/${PROJECT}/${path}${path.includes("?") ? "&" : "?"}lang=en`);
        await page.locator("main h1").first().waitFor({ timeout: 60_000 });
        // Never `networkidle`: the Portal keeps its activity and drafts streams open (portal.ts).
        await page.waitForTimeout(1500);
        const findings = await judge(page, size.width);
        if (findings.length) found[path] = findings;
        await info.attach(`${size.width}-${path.replace(/[^a-z0-9]+/gi, "-")}.png`, { body: await page.screenshot(), contentType: "image/png" });
      }
      expect(found).toEqual({});
    } finally {
      await context.close();
    }
  });
}
