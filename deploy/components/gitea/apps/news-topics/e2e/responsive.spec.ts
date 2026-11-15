import { expect, test } from "@playwright/test";
import { WIDTHS, layoutProblems } from "@joinedcontext/sdk/responsive";
import { BASE, STEWARD, serve } from "./serve";

// UI-84, SDK-12, AP-138 (T-3334): the news topics page at a phone, a tablet, a laptop and a wall,
// with its chart and topics list drawn: no sideways scroll, no two blocks over each other,
// nothing axe finds at WCAG 2.1 AA.
const VIEWS = [
  { name: "topics", search: null, lang: "en" },
  { name: "a search", search: "traffic", lang: "en" },
  { name: "topics in Finnish", search: null, lang: "fi" },
  { name: "topics in Swedish", search: null, lang: "sv" },
];

for (const view of VIEWS) {
  for (const size of WIDTHS) {
    test(`${view.name} at ${size.width} px: no sideways scroll, no overlap, axe clean`, async ({ page }, testInfo) => {
      const { outside, missing, problems } = await serve(page);
      await page.setViewportSize(size);
      await page.goto(`${BASE}?lang=${view.lang}#topics`);
      const topicsList = page.getByRole("list", { name: /Topics|Aiheet/i }).or(page.locator(".app-topics"));
      await expect(topicsList.getByRole("listitem").first()).toBeVisible();
      await expect(page.locator(".jc-chart-canvas canvas")).toHaveCount(1);
      if (view.search) {
        await page.getByRole("searchbox").fill(view.search);
        await expect(topicsList.getByRole("listitem").first()).toBeVisible();
      }
      await testInfo.attach(`${view.name}-${size.width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
      expect(await layoutProblems(page)).toEqual([]);
      expect({ outside, missing, problems }).toEqual({ outside: [], missing: [], problems: [] });
    });
  }
}

// SDK-40, T-3401: an article of the chosen topic opens in the shell's entity panel, read through the
// app's endpoint, at a phone and a laptop, light and dark; a public App writes nothing, so the panel
// links to the Portal.
for (const scheme of ["light", "dark"] as const) {
  for (const width of [375, 1440]) {
    test(`${scheme} at ${width} px: an article in the entity panel, linked to the Portal, axe clean`, async ({ page }) => {
      await page.emulateMedia({ colorScheme: scheme });
      const { outside, missing, problems } = await serve(page);
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`${BASE}#topics`);
      const articles = page.getByRole("list", { name: /articles of topic/i });
      await articles.getByRole("heading").first().getByRole("button").click();
      const panel = page.getByRole("dialog");
      await expect(panel.getByRole("link", { name: "Open in the Portal" })).toBeVisible();
      await expect(panel.getByRole("button", { name: "Edit" })).toHaveCount(0);
      expect(await layoutProblems(page)).toEqual([]);
      await page.keyboard.press("Escape");
      await expect(page.getByRole("dialog")).toHaveCount(0);
      expect({ outside, missing, problems }).toEqual({ outside: [], missing: [], problems: [] });
    });
  }
}

// T-3352: the weeks the App's server keeps, still there after a reload, and a week's articles
// downloaded from the store.
test("the weeks the server keeps, after a reload too, and a week's articles as a file", async ({ page }) => {
  const { outside, missing, problems } = await serve(page);
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`${BASE}?lang=en#topics`);
  const kept = page.getByRole("table", { name: "Topics week by week" });
  await expect(kept.getByRole("row")).toHaveCount(3);
  await expect(kept.getByRole("row").nth(1)).toContainText("raitiotie, liikenne (100%)");
  await page.reload();
  await expect(kept.getByRole("row").nth(2)).toContainText("kirjasto (50%); uimahalli (50%)");
  // The download opens in its own tab, which the page's routes do not reach: the context's do.
  await page.context().route("http://portal.test/store/**", (route) => route.fulfill({ status: 200, contentType: "application/json", body: "[]" }));
  const popup = page.waitForEvent("popup");
  await kept.getByRole("button", { name: "Download the articles of week 2026-W41" }).click();
  const tab = await popup;
  await tab.waitForLoadState();
  expect(tab.url()).toBe("http://portal.test/store/corpus/2026-W41.json");
  await tab.close();
  expect(await layoutProblems(page)).toEqual([]);
  expect({ outside, missing, problems }).toEqual({ outside: [], missing: [], problems: [] });
});

// T-3828: a viewer follows a topic and lets it go; a steward names a topic in Swedish, which the
// list shows, and takes the name back; both see the history, at a phone and a laptop, light and dark.
for (const scheme of ["light", "dark"] as const) {
  for (const width of [375, 1440]) {
    test(`${scheme} at ${width} px: a viewer follows a topic and lets it go; naming stays a steward's`, async ({ page }) => {
      await page.emulateMedia({ colorScheme: scheme });
      const { outside, missing, problems } = await serve(page);
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`${BASE}?lang=en#topics`);
      await expect(page.getByLabel("Role: Viewer")).toBeVisible();
      const follow = page.locator(".app-follow").first();
      await follow.click();
      await expect(follow).toHaveAttribute("aria-pressed", "true");
      const followed = page.getByRole("list", { name: "Followed topics" });
      await expect(followed.getByText("in this period's topics")).toBeVisible();
      const name = page.getByLabel("Name of the selected topic");
      await expect(name).toBeDisabled();
      await expect(name).toHaveAttribute("title", "Only a steward names and merges topics.");
      expect(await layoutProblems(page)).toEqual([]);
      await followed.getByRole("button", { name: /^Stop following / }).click();
      await expect(page.getByText("You follow no topic yet.")).toBeVisible();
      await page.getByRole("button", { name: "Changes" }).first().click();
      await expect(page.getByText(/^Added: followed /)).toBeVisible();
      expect(await layoutProblems(page)).toEqual([]);
      expect({ outside, missing, problems }).toEqual({ outside: [], missing: [], problems: [] });
    });

    test(`${scheme} at ${width} px: a steward names a topic in Swedish and takes the name back`, async ({ page }) => {
      await page.emulateMedia({ colorScheme: scheme });
      const { outside, missing, problems } = await serve(page, STEWARD);
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`${BASE}?lang=sv#topics`);
      await expect(page.getByLabel("Roll: Förvaltare")).toBeVisible();
      await page.getByLabel("Det valda ämnets namn").fill("Spårvägar");
      await page.getByRole("button", { name: "Namnge ämnet" }).click();
      await expect(page.getByText("Namngivet: Spårvägar")).toBeVisible();
      await expect(page.locator(".app-topics strong", { hasText: "Spårvägar" })).toBeVisible();
      expect(await layoutProblems(page)).toEqual([]);
      await page.getByRole("button", { name: /^Ångra namnet för / }).click();
      await expect(page.getByText("Inget ämne har ett namn ännu.")).toBeVisible();
      await page.getByRole("button", { name: "Ändringar" }).first().click();
      await expect(page.getByText("Allas ändringar, nyaste först.")).toBeVisible();
      expect(await layoutProblems(page)).toEqual([]);
      expect({ outside, missing, problems }).toEqual({ outside: [], missing: [], problems: [] });
    });
  }
}
