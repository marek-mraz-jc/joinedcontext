import { expect, test } from "@playwright/test";
import { LIVE_BLOCKS, WIDTHS, layoutProblems } from "@joinedcontext/sdk/responsive";

// UI-84, SDK-12 (T-2825): the stations as an anonymous reader sees them and with a steward's
// form, through the real binary, at a phone, a tablet, a laptop and a wall: no sideways scroll, no
// two blocks over each other, no control cut off, nothing axe finds at WCAG 2.1 AA.
const BASE = "/apps/air-quality/";

/** The headers the edge puts in front of the app for a signed-in person (AP-28, ADR-N-019). */
const edge = (who: string) => ({
  "x-access-token": `token-for-${who}`,
  "x-userinfo": Buffer.from(
    JSON.stringify({ sub: `f:1:demo.${who}`, preferred_username: `demo.${who}@hel.fi`, email: `demo.${who}@hel.fi` }),
  ).toString("base64"),
});

// The page follows the reader's colour scheme, so a steward's view is checked in both.
const VIEWS = [
  { who: "an anonymous reader", scheme: "light" },
  { who: "a steward", scheme: "light" },
  { who: "a steward", scheme: "dark" },
] as const;

for (const { who, scheme } of VIEWS) {
  for (const size of WIDTHS) {
    test(`the stations for ${who}, ${scheme}, at ${size.width} px: no sideways scroll, no overlap, axe clean`, async ({ page }, testInfo) => {
      const problems: string[] = [];
      page.on("pageerror", (error) => problems.push(error.message));
      await page.emulateMedia({ colorScheme: scheme });
      if (who === "a steward") await page.setExtraHTTPHeaders(edge("steward"));
      await page.setViewportSize(size);
      await page.goto(BASE);
      await expect(page.getByRole("heading", { name: "Kallio", exact: true })).toBeVisible();
      if (who === "a steward") {
        await expect(page.getByRole("form", { name: "New station" })).toBeVisible();
        await page.getByRole("button", { name: "Edit Kallio" }).click();
        await expect(page.getByRole("form", { name: "Edit Kallio" })).toBeVisible();
      }
      await testInfo.attach(`${who}-${scheme}-${size.width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });
      expect(await layoutProblems(page, LIVE_BLOCKS)).toEqual([]);
      expect(problems).toEqual([]);
    });
  }
}

// T-3374, SDK-40: a station opened in the SDK's entity panel, at a phone and a laptop, light and
// dark; a steward corrects the note there, which goes through the App's backend.
for (const scheme of ["light", "dark"] as const) {
  for (const width of [375, 1440]) {
    test(`a station in the entity panel, ${scheme}, at ${width} px: opened, edited by a steward, axe clean`, async ({ page }) => {
      const problems: string[] = [];
      page.on("pageerror", (error) => problems.push(error.message));
      await page.emulateMedia({ colorScheme: scheme });
      await page.setExtraHTTPHeaders(edge("steward"));
      await page.setViewportSize({ width, height: 900 });
      await page.goto(BASE);
      await page.getByRole("button", { name: "Details of Kallio" }).click();
      const panel = page.getByRole("dialog", { name: "Kallio" });
      await expect(panel.getByText("PM10 (µg/m³)")).toBeVisible();
      expect(await layoutProblems(page, LIVE_BLOCKS)).toEqual([]);

      await panel.getByRole("button", { name: "Edit" }).click();
      await panel.getByLabel("Steward note").fill(`Checked at ${width} px.`);
      await panel.getByRole("button", { name: "Review the change" }).click();
      await panel.getByRole("button", { name: "Save the change" }).click();
      await expect(page.getByText(`Note: Checked at ${width} px.`)).toBeVisible();
      await panel.getByRole("button", { name: "Close" }).click();
      await expect(page.getByRole("dialog")).toHaveCount(0);
      expect(problems).toEqual([]);
    });
  }
}
