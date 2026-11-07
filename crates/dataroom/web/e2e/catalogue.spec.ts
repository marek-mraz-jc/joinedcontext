import { test, expect } from "@playwright/test";

const PAGES = [
  { name: "home", path: "/" },
  { name: "dataset-list", path: "/dataset/" },
  { name: "dataset-air-quality", path: "/dataset/air-quality" },
  { name: "organization-list", path: "/organization/" },
  { name: "organization-banskabystrica", path: "/organization/banskabystrica" },
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
