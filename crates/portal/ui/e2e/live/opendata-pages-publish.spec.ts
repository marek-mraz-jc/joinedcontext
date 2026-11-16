/**
 * T-7041 (T-7031): an Organization's administrator writes the open data portal's connector page
 * in Slovak and English on Administration → Open data pages, publishes each as a Change an
 * approver merges, and the open data portal shows both texts. The texts the Organization had
 * before are put back at the end (or the built-in ones again), each a Change the approver merges.
 * Run by hand after a deploy (sweep.sh byhand): every run proposes the Organization manifest four
 * times, and a parallel spec editing that manifest would meet a 409.
 *
 *   pnpm exec playwright test -c playwright.live.config.ts e2e/live/opendata-pages-publish.spec.ts
 */
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { APPROVER, STEWARD, approve, csrf, proposedChange, signIn } from "./portal";

test.setTimeout(900_000);

interface PageText {
  page: string;
  lang: string;
  markdown?: string;
}

async function texts(page: Page): Promise<PageText[]> {
  const answer = await page.request.get("/api/v1/organization/opendata/pages");
  expect(answer.ok(), "the steward administers the Organization's open data pages").toBe(true);
  return ((await answer.json()) as { pages: PageText[] }).pages;
}

const own = async (page: Page, lang: string) =>
  (await texts(page)).find((t) => t.page === "connector" && t.lang === lang)?.markdown;

async function publishInScreen(page: Page, approver: Page, lang: string, markdown: string): Promise<void> {
  await page.goto("/organization/opendata-pages?lang=en");
  const connector = page
    .getByTestId("opendata-pages-list")
    .getByRole("listitem")
    .filter({ has: page.getByRole("heading", { name: "AI connector" }) });
  await connector.getByRole("button", { name: new RegExp(`\\(${lang}\\)$`) }).click();
  const box = page.getByRole("textbox", { name: /Page text in Markdown/ });
  await box.fill(markdown);
  // The preview is the catalogue's renderer: the bold marker comes back as bold.
  await expect(page.locator("strong", { hasText: "journey" }).first()).toBeVisible({ timeout: 30_000 });
  await page.getByRole("button", { name: "Publish" }).click();
  await expect(page.getByText(/once an administrator approves the change/)).toBeVisible({ timeout: 60_000 });
  await approve(approver, "org", await proposedChange(page));
  await expect.poll(() => own(page, lang), { timeout: 240_000, intervals: [5_000] }).toBe(markdown);
}

test("an administrator edits the connector page in sk and en, publishes, approves, and the open data portal shows it", async ({
  browser,
}) => {
  const steward = await signIn(browser, STEWARD, "/organization/opendata-pages?lang=en");
  const approver = await signIn(browser, APPROVER, "/organization/opendata-pages?lang=en");
  const { page } = steward;
  const hosts = await page.request.get("/api/v1/organization/portal-hosts");
  expect(hosts.ok(), "the steward reads the Organization's portal hosts").toBe(true);
  const opendata = ((await hosts.json()) as { items: { opendata?: string | null }[] }).items[0]?.opendata;
  expect(opendata, "the Organization has an open data portal").toBeTruthy();
  const before = { sk: await own(page, "sk"), en: await own(page, "en") };
  const marker = Date.now().toString(36);
  const wanted = {
    sk: `# Konektor ${marker}\n\nTento text píše **journey** ${marker}.`,
    en: `# Connector ${marker}\n\nThis text is written by the **journey** ${marker}.`,
  };
  try {
    for (const lang of ["sk", "en"] as const) {
      await publishInScreen(page, approver.page, lang, wanted[lang]);
      await expect
        .poll(async () => (await page.request.get(`${opendata}/connector?lang=${lang}`)).text(), {
          timeout: 300_000,
          intervals: [10_000],
        })
        .toContain(`${lang === "sk" ? "Konektor" : "Connector"} ${marker}`);
    }
    await page.goto("/organization/opendata-pages?lang=en");
    await expect(page.getByTestId("opendata-pages-history").getByRole("link").first()).toBeVisible({ timeout: 60_000 });
  } finally {
    // Put back what the Organization had: its own text, or the built-in one.
    const headers = { "x-csrf-token": await csrf(steward.context) };
    for (const lang of ["sk", "en"] as const) {
      if ((await own(page, lang)) === before[lang]) continue;
      const path = `/api/v1/organization/opendata/pages/connector/${lang}`;
      const sent =
        before[lang] === undefined
          ? await page.request.delete(path, { headers })
          : await page.request.put(path, { headers, data: { markdown: before[lang] } });
      if (sent.status() === 202) {
        await approve(approver.page, "org", ((await sent.json()) as { metadata: { name: string } }).metadata.name);
        await expect.poll(() => own(page, lang), { timeout: 240_000, intervals: [5_000] }).toBe(before[lang]);
      }
    }
    await approver.context.close();
    await steward.context.close();
  }
});
