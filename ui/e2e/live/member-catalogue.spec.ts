/**
 * The signed-in catalogue of an Organization member, on dev (ADR-N-067 §2.4, T-6411).
 *
 * A visitor, demo.orgmember (/helsinki/members) and demo.analyst (also /helsinki/traffic-analysts)
 * each read GET /api/v1/catalogue/readable: the visitor gets the public Endpoints alone, the member
 * those plus every Endpoint the gateway grants the Organization, the analyst also what the group
 * is granted. Visitor < member < analyst. A member opens "/" and lands on the catalogue in the
 * member shell, with no project page. Shots at 1440 and 400 go to test-results/T-6411.
 */
import { expect, test } from "@playwright/test";
import type { APIRequestContext, Page } from "@playwright/test";
import { ANALYST, ORGMEMBER, signIn } from "./portal";

test.setTimeout(240_000);

type Readable = { datasets: { slug: string; access: string; group?: string }[]; unanswered: number };

async function readable(request: APIRequestContext): Promise<Readable> {
  const answer = await request.get("/api/v1/catalogue/readable");
  expect(answer.status()).toBe(200);
  return (await answer.json()) as Readable;
}

const memberShell = (page: Page) => page.getByRole("navigation", { name: "Member menu" });

async function shots(page: Page, who: string): Promise<void> {
  for (const width of [1440, 400]) {
    await page.setViewportSize({ width, height: width === 400 ? 860 : 900 });
    await page.screenshot({ path: `test-results/T-6411/${who}-${width}.png`, fullPage: true });
  }
}

test("visitor < member < analyst, each row marked by what grants it", async ({ browser }) => {
  test.skip(!ORGMEMBER.password || !ANALYST.password, "ORGMEMBER_PASSWORD / ANALYST_PASSWORD are not in this run's environment");
  const visitor = await browser.newContext();
  const open = await readable(visitor.request);
  expect(open.datasets.every((row) => row.access === "public")).toBe(true);
  const page = await visitor.newPage();
  await page.goto("/catalogue?lang=en");
  await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
  await shots(page, "visitor");
  await visitor.close();

  const sets: Record<string, Readable> = {};
  for (const [who, person] of [
    ["orgmember", ORGMEMBER],
    ["analyst", ANALYST],
  ] as const) {
    const { context, page } = await signIn(browser, person, "/?lang=en", memberShell);
    try {
      // The literal "/" lands a member on the catalogue, in the member shell.
      await expect(page).toHaveURL(/\/catalogue/);
      await expect(page.getByRole("heading", { name: "Datasets you may read" })).toBeVisible();
      await expect(page.locator('a[href^="/projects/"]')).toHaveCount(0);
      sets[who] = await readable(context.request);
      await shots(page, who);
      const granted = sets[who].datasets.find((row) => row.access !== "public");
      if (granted) {
        // Read live through the Endpoint with the person's own session, never a copy.
        await page.goto(`/catalogue/endpoint/${granted.slug}?lang=en`);
        await expect(page.getByText("You have no access to this dataset")).toHaveCount(0);
        await shots(page, `${who}-dataset`);
      }
      // A project page by its address shows nothing of the project.
      const list = await context.request.get("/api/v1/projects/helsinki/endpoints");
      expect([403, 404]).toContain(list.status());
    } finally {
      await context.close();
    }
  }
  const slugs = (set?: Readable) => new Set(set?.datasets.map((row) => row.slug));
  const member = slugs(sets.orgmember);
  const analyst = slugs(sets.analyst);
  for (const slug of slugs(open)) expect(member.has(slug)).toBe(true);
  for (const slug of member) expect(analyst.has(slug)).toBe(true);
  expect(member.size).toBeGreaterThan(slugs(open).size);
  expect(analyst.size).toBeGreaterThan(member.size);
  expect(sets.analyst?.datasets.some((row) => row.access === "group" && row.group === "traffic-analysts")).toBe(true);
  test.info().annotations.push({ type: "catalogue", description: `visitor ${slugs(open).size}, member ${member.size}, analyst ${analyst.size}` });
});
