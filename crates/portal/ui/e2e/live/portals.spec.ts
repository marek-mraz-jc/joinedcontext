/**
 * T-6875 (ODP-15): an Organization's administrator makes a second open data Portal through the
 * Portals area, edits it, opens it and removes it, each write a Change an approver confirms.
 * Run by hand after a deploy (sweep.sh byhand): every Portal counts toward `portals.max`.
 *
 *   pnpm exec playwright test -c playwright.live.config.ts e2e/live/portals.spec.ts
 */
import { expect, test } from "@playwright/test";
import { APPROVER, STEWARD, approve, cleanUp, proposedChange, signIn, waitGone } from "./portal";

test.setTimeout(600_000);

test("an administrator creates, edits, opens and removes a second portal", async ({ browser }) => {
  const steward = await signIn(browser, STEWARD, "/portals?lang=en");
  const approver = await signIn(browser, APPROVER, "/portals?lang=en");
  const { page } = steward;
  const hosts = await page.request.get("/api/v1/organization/portal-hosts");
  expect(hosts.ok(), "the steward makes portals in an Organization").toBe(true);
  const organization = ((await hosts.json()) as { items: { organization: string }[] }).items[0].organization;
  const path = `journey-${Date.now().toString(36)}`;
  const name = `${organization}-${path}`;
  try {
    await expect(page.getByRole("heading", { level: 1, name: "Open data portals" })).toBeVisible();
    await page.goto("/portals/new?lang=en");
    await page.getByLabel(/^Organization/).selectOption(organization);
    await page.getByLabel(/Name in the address/).fill(path);
    await page.getByLabel(/Title in Slovak/).fill("Cesta portálu");
    await page.getByLabel(/Title in English/).fill("Portal journey");
    await page.getByRole("button", { name: "Propose the portal" }).click();
    await approve(approver.page, "org", await proposedChange(page));
    await expect
      .poll(async () => (await page.request.get(`/api/v1/projects/org/portals/${name}`)).status(), { timeout: 180_000, intervals: [5_000] })
      .toBe(200);

    await page.goto("/portals?lang=en");
    await expect(page.getByRole("link", { name: "Portal journey" })).toBeVisible({ timeout: 60_000 });
    await page.goto(`/portals/${name}?lang=en`);
    await page.getByLabel(/Title in English/).fill("Portal journey, edited");
    await page.getByRole("button", { name: "Propose the changes" }).click();
    await approve(approver.page, "org", await proposedChange(page));

    await page.goto(`/portals/${name}?lang=en`);
    const open = page.getByRole("link", { name: /Open the portal/ });
    const href = await open.getAttribute("href");
    expect(href, "the editor links the portal's address").toMatch(new RegExp(`/-/${path}/$`));
    await expect
      .poll(async () => (await page.request.get(href ?? "", { maxRedirects: 0 })).status(), { timeout: 180_000, intervals: [10_000] })
      .toBeLessThan(400);

    await page.getByRole("button", { name: /Remove portal/ }).click();
    const dialog = page.getByRole("dialog");
    await dialog.getByLabel(`Type ${name} to confirm`).fill(name);
    await dialog.getByRole("button", { name: "Propose removal" }).click();
    const change = (await dialog.getByText(/^chg-[0-9a-f]{8}$/).textContent()) ?? "";
    await approve(approver.page, "org", change, name);
    await waitGone(page, "org", "portals", name);
  } finally {
    await cleanUp(steward, "org", "portals", name, "portals journey");
    await approver.context.close();
    await steward.context.close();
  }
});
