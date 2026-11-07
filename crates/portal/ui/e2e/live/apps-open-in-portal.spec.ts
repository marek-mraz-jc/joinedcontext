/**
 * An App opens inside the Portal, under its header and sidebar (T-2689, AP-122, UI-44).
 *
 * The steward opens three published Apps of Helsinki from their catalog cards: each one runs in a
 * sandboxed frame of the Portal page and shows its data there, the frame's own login being the
 * two redirects of the realm session. "Open in new window" opens the same App as a page of its
 * own. The edge lets only the Portal's host frame an App, so the frame loading at all is the
 * header's check; a framed App that the edge refused would show the browser's refusal instead.
 *
 * Every wait is on the thing the step needs, never on the network going idle (T-2452).
 */
import { expect, test } from "@playwright/test";
import type { FrameLocator, Locator, Page } from "@playwright/test";
import { guideShot } from "./guide";
import { STEWARD, signIn } from "./portal";

const PROJECT = "helsinki";
const menu = (page: Page): Locator => page.getByRole("banner").getByRole("button", { name: "Menu" });

test.setTimeout(600_000);

/** Each App and what shows that it read its data inside the frame. */
const APPS: { name: string; data: (frame: FrameLocator) => Locator }[] = [
  {
    name: "helsinki-bikes",
    data: (frame) => frame.getByRole("region", { name: "Overview" }).getByText(/\d/).first(),
  },
  {
    name: "helsinki-alerts",
    data: (frame) =>
      frame
        .getByRole("region", { name: "Overview" })
        .getByText(/^[a-zA-Z][\w -]*: \d+$/)
        .first(),
  },
  {
    name: "air-quality",
    data: (frame) => frame.getByRole("heading", { level: 1, name: "Air quality" }),
  },
];

for (const app of APPS) {
  test(`${app.name} opens inside the Portal with its data, and in a window of its own`, async ({ browser }) => {
    // The App page folds the project navigation away (T-2908): its landmark is the header's Menu.
    const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/apps/${app.name}/open?lang=en`, menu);
    try {
      const page = steward.page;
      // The Portal stays around the App: its header with the menu that folds the project
      // navigation away, and the slim bar with the App's name (T-2908).
      await expect(page.getByRole("banner").getByRole("button", { name: "Menu" })).toBeVisible({ timeout: 60_000 });
      await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
      const frameElement = page.locator("iframe[sandbox]");
      // The frame keeps its own origin only on an Apps origin apart from the Portal's (AP-19,
      // T-2840); it never gets the Portal's window.
      const src = new URL((await frameElement.getAttribute("src")) ?? "", page.url());
      const ownOrigin = src.origin !== new URL(page.url()).origin;
      await expect(frameElement).toHaveAttribute(
        "sandbox",
        `allow-scripts allow-forms allow-popups allow-downloads${ownOrigin ? " allow-same-origin" : ""}`,
      );
      await expect(app.data(page.frameLocator("iframe[sandbox]"))).toBeVisible({ timeout: 120_000 });
      if (app === APPS[0]) await guideShot(page, "app-2-open-in-portal");
      expect(new URL(page.url()).pathname, "the Portal kept its window").toBe(
        `/projects/${PROJECT}/apps/${app.name}/open`,
      );

      const popup = page.waitForEvent("popup");
      await page.getByRole("link", { name: /Open in new window/ }).click();
      const own = await popup;
      // The realm session makes the App's own login two redirects before the App's address: the
      // frame's, on the Apps origin when it has one (T-2840).
      await own.waitForURL((url) => url.origin === src.origin && url.pathname === src.pathname, { timeout: 60_000 });
      await own.close();
    } finally {
      await steward.context.close();
    }
  });
}
