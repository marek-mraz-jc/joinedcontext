import { test, expect } from "@playwright/test";

// Seeded names: prod's by default; another catalogue names its own (CATALOGUE_DATASET, CATALOGUE_ORG).
const DATASET = process.env.CATALOGUE_DATASET ?? "air-quality";
const ORG = process.env.CATALOGUE_ORG ?? "banskabystrica";

const PAGES = [
  { name: "home", path: "/" },
  { name: "dataset-list", path: "/dataset/" },
  { name: `dataset-${DATASET}`, path: `/dataset/${DATASET}` },
  { name: "organization-list", path: "/organization/" },
  { name: `organization-${ORG}`, path: `/organization/${ORG}` },
  { name: "assistants", path: "/assistants" },
  { name: "assistants-claude", path: "/assistants/claude" },
  { name: "about", path: "/about" },
  { name: "connector", path: "/connector" },
  { name: "privacy", path: "/privacy" },
  { name: "terms", path: "/terms" },
  { name: "support", path: "/support" },
  { name: "accessibility", path: "/accessibility" },
];

const LANGUAGES = ["en", "sk"] as const;
const VIEWPORTS = [
  { width: 1440, height: 900 },
  { width: 400, height: 844 },
] as const;

for (const lang of LANGUAGES) {
  for (const viewport of VIEWPORTS) {
    for (const entry of PAGES) {
      test(`${entry.name} (${lang} @ ${viewport.width}px)`, async ({ page }) => {
        await page.setViewportSize(viewport);
        const response = await page.goto(`/${lang}${entry.path}`);
        expect(response?.status()).toBe(200);
        await expect(page.locator("html")).toHaveAttribute("lang", lang);
        await expect(page.locator("h1").first()).toBeVisible();
        await page.screenshot({
          path: `test-results/dataroom-${entry.name}-${lang}-${viewport.width}px.png`,
          fullPage: true,
        });
      });
    }
  }
}

// The footer (jc-footer, 2026-11-11): the language switch sends itself with script on, so its Go
// button shows only without script; the assistant corner stops above the footer, so at 400 px no
// footer control lies under it at any scroll position.
const FOOTER_PAGES = ["/", "/dataset/", `/dataset/${DATASET}`, "/assistants/chatgpt"];
const LANG_GO = "form.lang-select button[type=submit]";

test.describe("footer", () => {
  for (const path of FOOTER_PAGES) {
    test(`the language Go button hides with script on (${path})`, async ({ page }) => {
      await page.goto(`/en${path}`);
      await expect(page.locator("html")).toHaveClass(/(^|\s)js(\s|$)/);
      await expect(page.locator("#field-lang-select")).toBeVisible();
      await expect(page.locator(LANG_GO)).toBeHidden();
    });

    test(`no footer control lies under the assistant corner at 400px (${path})`, async ({ page }) => {
      await page.setViewportSize({ width: 400, height: 844 });
      await page.goto(`/en${path}`);
      await expect(page.locator(".jc-corner")).toBeVisible();
      const overlaps = await page.evaluate(() => {
        const corner = document.querySelector(".jc-corner")!;
        const controls = Array.from(
          document.querySelectorAll<HTMLElement>(".site-footer a, .site-footer select, .site-footer button"),
        ).filter((el) => el.getClientRects().length > 0);
        const hits: string[] = [];
        const check = (where: string) => {
          const c = corner.getBoundingClientRect();
          for (const el of controls) {
            const r = el.getBoundingClientRect();
            if (r.left < c.right && c.left < r.right && r.top < c.bottom && c.top < r.bottom) {
              hits.push(`${where}: ${el.tagName} "${el.textContent!.trim()}"`);
            }
          }
        };
        // Each control in turn at the window's bottom edge (mid-scroll), then the page's end.
        for (const el of controls) {
          const r = el.getBoundingClientRect();
          window.scrollTo(0, window.scrollY + r.bottom - window.innerHeight);
          check(`at ${Math.round(window.scrollY)}`);
        }
        window.scrollTo(0, document.documentElement.scrollHeight);
        check("at the end");
        return hits;
      });
      expect(overlaps).toEqual([]);
    });
  }

  // T-6748: the owner's phone shot had the corner over the live table's last rows and pager. While
  // it would sit over a frame it steps aside (jc-viz.js), so a tap there reaches the frame.
  test("the assistant corner steps aside over the live data frame at 400px", async ({ page }) => {
    await page.setViewportSize({ width: 400, height: 844 });
    await page.goto(`/sk/dataset/${DATASET}`);
    const frame = page.locator("iframe[data-jc-viz]").first();
    await expect(frame).toBeVisible();
    await expect.poll(async () => (await frame.boundingBox())?.height ?? 0, { timeout: 20000 }).toBeGreaterThan(500);
    // The frame's end (the table's pager) at the window's bottom edge, where the corner sits.
    await frame.evaluate((el) => window.scrollTo(0, window.scrollY + el.getBoundingClientRect().bottom - window.innerHeight + 8));
    await expect(page.locator("body")).toHaveClass(/(^|\s)jc-corner-aside(\s|$)/);
    const summary = page.locator(".jc-assistant > summary");
    await expect(summary).toHaveCSS("opacity", "0");
    const box = (await summary.boundingBox())!;
    expect(await page.evaluate(([x, y]) => document.elementFromPoint(x, y)?.tagName, [box.x + box.width / 2, box.y + box.height / 2])).toBe("IFRAME");
    // Past the frame it is back.
    await page.evaluate(() => window.scrollTo(0, document.documentElement.scrollHeight));
    await expect(page.locator("body")).not.toHaveClass(/(^|\s)jc-corner-aside(\s|$)/);
    await expect(summary).not.toHaveCSS("opacity", "0");
  });

  test("the language Go button shows and works without script", async ({ browser }) => {
    const context = await browser.newContext({ javaScriptEnabled: false });
    const page = await context.newPage();
    await page.goto("/en/dataset/");
    await expect(page.locator(LANG_GO)).toBeVisible();
    await page.selectOption("#field-lang-select", { index: await otherLanguage(page) });
    await page.locator(LANG_GO).click();
    await expect(page).not.toHaveURL(/\/en\/dataset\/$/);
    await expect(page.locator("html")).not.toHaveAttribute("lang", "en");
    await context.close();
  });

  test("choosing a language with script on opens the page in it", async ({ page }) => {
    await page.goto("/en/dataset/");
    await expect(page.locator("html")).toHaveClass(/(^|\s)js(\s|$)/);
    const index = await otherLanguage(page);
    const code = await page.locator("#field-lang-select option").nth(index).getAttribute("lang");
    await page.selectOption("#field-lang-select", { index });
    await expect(page).toHaveURL(new RegExp(`/${code}/dataset/$`));
    await expect(page.locator("html")).toHaveAttribute("lang", code!);
  });
});

async function otherLanguage(page: import("@playwright/test").Page): Promise<number> {
  const langs = await page.locator("#field-lang-select option").evaluateAll((os) => os.map((o) => o.getAttribute("lang")));
  const index = langs.findIndex((l) => l !== "en");
  expect(index, "a second language").toBeGreaterThanOrEqual(0);
  return index;
}

test("every publisher card in a row ends its dataset count on one line at 1440px", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto("/en/organization/");
  const rows = await page.evaluate(() => {
    const byRow = new Map<number, number[]>();
    for (const card of Array.from(document.querySelectorAll<HTMLElement>(".media-grid > .media-item"))) {
      const top = Math.round(card.getBoundingClientRect().top);
      const count = card.querySelector(".count")!.getBoundingClientRect();
      byRow.set(top, [...(byRow.get(top) ?? []), Math.round(card.getBoundingClientRect().bottom - count.bottom)]);
    }
    return Array.from(byRow.values());
  });
  expect(rows.length).toBeGreaterThan(0);
  for (const offsets of rows) expect(new Set(offsets).size, `bottom offsets ${offsets}`).toBe(1);
});
