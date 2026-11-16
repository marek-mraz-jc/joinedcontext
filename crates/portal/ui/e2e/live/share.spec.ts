/**
 * Share in 60 seconds, through the real UI on dev (T-0631, EP-14, EP-15, EP-16, CC-34, UI-31,
 * AG-58): a steward publishes a new public endpoint over the Helsinki space with one attribute
 * hidden; an approver approves; the endpoint goes Live and its NGSI-LD link answers without the
 * hidden attribute; the viewer may read it, may not read the organization endpoint, and sees
 * Propose disabled with the reason. The clock measures the person's part: first click to the
 * approval, excluding the wait for the platform to merge and reconcile.
 */
import { expect, test, type Page } from "@playwright/test";
import { guideShot } from "./guide";
import { journeyClock } from "./journeys";
import { proposeFrom } from "./kindJourney";
import { APPROVER, STEWARD, approve, listedNames, proposeDelete, proposedChange, signIn } from "./portal";

const PROJECT = "helsinki";
const SUFFIX = process.env.E2E_SUFFIX ?? new Date().toISOString().slice(11, 16).replace(":", "");
const ENDPOINT = `bikes-regional-${SUFFIX}`;
const HIDDEN = "source";
const VIEWER = { user: "demo.viewer@hel.fi", password: process.env.VIEWER_PASSWORD ?? "" };
// The seeded organization endpoint (helsinki-bikes-ops): a signed-in person without a Policy
// grant on the space is refused there (PF-50, EP-14).
const ORGANIZATION_SLUG = "ydp6nddchg4sewmylsiclg4377mlldrj";

test.setTimeout(720_000);

async function read(page: Page, url: string): Promise<{ status: number; entity: Record<string, unknown> | null }> {
  return page.evaluate(async (target) => {
    const res = await fetch(`${target}?type=BikeHireDockingStation&limit=1`, { headers: { Accept: "application/json" } });
    const text = await res.text();
    let entity: Record<string, unknown> | null = null;
    try {
      const parsed = JSON.parse(text);
      entity = Array.isArray(parsed) ? (parsed[0] ?? null) : null;
    } catch {
      entity = null;
    }
    return { status: res.status, entity };
  }, url);
}

test("an endpoint proposed and approved through the UI: Live, the hidden attribute absent, the viewer refused on the organization one", async ({ browser }) => {
  const { page } = await signIn(browser, STEWARD, `/projects/${PROJECT}/endpoints?lang=en`);
  let personSeconds = 0;

  const clock = journeyClock("Share");
  let start = Date.now();
  await page.getByRole("button", { name: "New endpoint" }).click();
  const dialog = page.getByTestId("form-page");
  await dialog.locator("#root_name").fill(ENDPOINT);
  await dialog.locator("#root_title").fill("City bikes for the regional transport team");
  await dialog.locator("#root_contextSpaceRef").selectOption(PROJECT);
  await dialog.locator("#root_audience").selectOption({ label: "Public: anyone with the address" });
  await expect(dialog.getByText(/public audience/)).toBeVisible();
  await dialog.getByRole("checkbox", { name: "ngsi-ld" }).check();
  // The model projection: the bike stations, with one slot left out.
  const projection = dialog.getByRole("group", { name: "Model projection" });
  await projection.getByRole("checkbox", { name: "BikeHireDockingStation", exact: true }).check();
  await dialog.getByRole("textbox", { name: "Projection name" }).fill(`${ENDPOINT}-view`);
  await projection.getByRole("checkbox", { name: `BikeHireDockingStation.${HIDDEN}`, exact: true }).uncheck();
  // A guide shot is not the person's time.
  const shooting = Date.now();
  await guideShot(page, "endpoint-1-form");
  start += Date.now() - shooting;
  await proposeFrom(dialog);
  const change = await proposedChange(page);
  personSeconds += (Date.now() - start) / 1000;
  await guideShot(page, "endpoint-2-proposed");

  const approver = await signIn(browser, APPROVER, `/projects/${PROJECT}/approvals?lang=en`);
  start = Date.now();
  await approve(approver.page, PROJECT, change, ENDPOINT);
  personSeconds += (Date.now() - start) / 1000;
  clock.person(personSeconds * 1000);
  console.log(`person-seconds: ${personSeconds.toFixed(1)}`);
  expect(personSeconds).toBeLessThan(60);

  // The endpoint is Live once the reconciler has read the merged manifest (one sync interval).
  const row = page.getByRole("row").filter({ hasText: ENDPOINT });
  await expect(async () => {
    await page.goto(`/projects/${PROJECT}/endpoints?lang=en`, { waitUntil: "load" });
    await expect(row.getByText("Live")).toBeVisible({ timeout: 5_000 });
  }).toPass({ timeout: 180_000, intervals: [10_000] });
  clock.live();
  await guideShot(page, "endpoint-3-live", { reloads: true });
  // The surface links are on the endpoint's own page; the row opens it (T-4989).
  await row.locator("a[data-row-link]").click();
  const href = await page.getByRole("link", { name: "ngsi-ld" }).first().getAttribute("href");
  expect(href).toContain("/api/endpoint/");
  const link = href ?? "";

  const steward = await read(page, link);
  expect(steward.status).toBe(200);
  expect(steward.entity).not.toBeNull();
  expect(steward.entity).toHaveProperty("availableBikeNumber");
  expect(steward.entity).not.toHaveProperty(HIDDEN);

  const viewer = await signIn(browser, VIEWER, `/projects/${PROJECT}/endpoints?lang=en`);
  const newButton = viewer.page.getByRole("button", { name: "New endpoint" });
  await expect(newButton).toBeDisabled();
  await expect(viewer.page.getByText(/your role does not permit/)).toBeAttached();
  const viewerPublic = await read(viewer.page, link);
  expect(viewerPublic.status).toBe(200);
  expect(viewerPublic.entity).not.toHaveProperty(HIDDEN);
  const viewerOrganization = await read(viewer.page, `${new URL(link).origin}/api/endpoint/${ORGANIZATION_SLUG}/ngsi-ld/v1/entities`);
  expect(viewerOrganization.status).toBe(403);

  // The take leaves the project as it found it (T-0667): the endpoint is deleted through the
  // same review, and the list no longer names it once the reconciler has read the merge.
  await approve(approver.page, PROJECT, await proposeDelete(page, PROJECT, "endpoints", ENDPOINT), ENDPOINT);
  await expect
    .poll(async () => (await listedNames(page, PROJECT, "endpoints")).includes(ENDPOINT), { timeout: 180_000, intervals: [10_000] })
    .toBe(false);
});
