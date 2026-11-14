/**
 * T-6875 (ODP-15): an Organization's administrator makes a second open data Portal through the
 * Portals area, edits it, opens it and removes it, each write a Change an approver confirms.
 * Run by hand after a deploy (sweep.sh byhand): every Portal counts toward `portals.max`.
 * T-6876: the second test adds a dcat-ap source to such a portal, tests it, runs it, reads the
 * run log and hides a dataset. Its source is LIVE_DCAT_SOURCE, else the Organization's default
 * portal's own catalog.ttl (a host the harvest allow-list must hold).
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

test("an administrator adds a dcat-ap source, tests it, runs it, reads its run log and hides a dataset", async ({ browser }) => {
  const steward = await signIn(browser, STEWARD, "/portals?lang=en");
  const approver = await signIn(browser, APPROVER, "/portals?lang=en");
  const { page } = steward;
  const hosts = (await (await page.request.get("/api/v1/organization/portal-hosts")).json()) as { items: { organization: string; opendata?: string }[] };
  const { organization, opendata } = hosts.items[0];
  const source = process.env.LIVE_DCAT_SOURCE ?? `${opendata}/catalog.ttl`;
  const path = `sources-${Date.now().toString(36)}`;
  const name = `${organization}-${path}`;
  try {
    await page.goto("/portals/new?lang=en");
    await page.getByLabel(/^Organization/).selectOption(organization);
    await page.getByLabel(/Name in the address/).fill(path);
    await page.getByLabel(/Title in Slovak/).fill("Zdroje portálu");
    await page.getByRole("button", { name: "Propose the portal" }).click();
    await approve(approver.page, "org", await proposedChange(page));
    await expect
      .poll(async () => (await page.request.get(`/api/v1/projects/org/portals/${name}`)).status(), { timeout: 180_000, intervals: [5_000] })
      .toBe(200);

    await page.goto(`/portals/${name}?lang=en`);
    await page.getByRole("tab", { name: "Sources" }).click();
    await page.getByRole("button", { name: "Add a source" }).click();
    await page.getByLabel(/^Type/).selectOption("dcat-ap");
    await page.getByLabel(/^Name/).fill("federated");
    await page.getByLabel(/^Address/).fill(source);
    await page.getByRole("button", { name: "Propose the source" }).click();
    await approve(approver.page, "org", await proposedChange(page));
    await expect
      .poll(async () => JSON.stringify(await (await page.request.get(`/api/v1/projects/org/portals/${name}`)).json()), { timeout: 180_000, intervals: [5_000] })
      .toContain('"federated"');

    await page.goto(`/portals/${name}?lang=en`);
    await page.getByRole("tab", { name: "Sources" }).click();
    await page.getByRole("button", { name: "Test connection of federated" }).click();
    await expect(page.getByText("The source answered in the expected format.")).toBeVisible({ timeout: 30_000 });
    await page.getByRole("button", { name: "Run federated now" }).click();
    await expect(page.getByText(/The run started/)).toBeVisible();
    const log = page.getByRole("table", { name: "Last runs of federated" });
    await expect(log.getByText("Done")).toBeVisible({ timeout: 300_000 });

    await page.getByRole("tab", { name: "Datasets" }).click();
    const datasets = page.getByRole("table", { name: "Datasets of the portal" });
    await expect(datasets).toBeVisible({ timeout: 60_000 });
    await datasets.getByRole("button", { name: /^Hide / }).first().click();
    await approve(approver.page, "org", await proposedChange(page));
  } finally {
    await cleanUp(steward, "org", "portals", name, "portal sources journey");
    await approver.context.close();
    await steward.context.close();
  }
});
