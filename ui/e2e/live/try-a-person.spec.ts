/**
 * T-3311, EP-103: "Try a person" says what the gateway decides for someone, so it must say what
 * the gateway really does. The organization's administrator (demo.steward) asks about
 * demo.viewer on the seeded bike stations Endpoint; demo.viewer then makes the same calls for
 * real, and the two answers must agree. The write asked about is a delete of an entity that does
 * not exist, so whichever way the gateway decides, no data changes.
 */
import { expect, test } from "@playwright/test";
import type { BrowserContext, Page } from "@playwright/test";
import { STEWARD, VIEWER, csrf, signIn } from "./portal";

const PROJECT = "helsinki";
const ENDPOINT = "app-bike-stations";
const SLUG = "335ub3brbavwl7sfpc3dph4zi4";
const TYPE = "BikeHireDockingStation";
const NOBODY = `urn:ngsi-ld:${TYPE}:jc-e2e-try-a-person-none`;

/** What the panel answers for demo.viewer, `action` on `TYPE`: true for allowed. */
async function simulated(page: Page, action: string): Promise<boolean> {
  const panel = page
    .locator("div.rounded-lg")
    .filter({ has: page.getByRole("heading", { name: "Try a person" }) });
  await panel.getByLabel("Endpoint").selectOption(ENDPOINT);
  await panel.getByLabel("Who").selectOption("person");
  await panel.getByLabel("Find a person").fill("demo.viewer");
  await expect(panel.getByRole("option", { name: VIEWER.user })).toBeAttached();
  await panel
    .getByLabel("Person", { exact: true })
    .selectOption({ label: VIEWER.user });
  await panel.getByLabel("Action").selectOption(action);
  await panel.getByLabel("Entity type").fill(TYPE);
  const answer = page.waitForResponse((r) =>
    r.url().endsWith(`/endpoints/${ENDPOINT}/access/simulate`),
  );
  await panel.getByRole("button", { name: "Ask the gateway" }).click();
  expect((await answer).status(), "the Portal answered the simulation").toBe(
    200,
  );
  const verdict = page.getByTestId("try-verdict");
  await expect(verdict).toBeVisible();
  return (await verdict.textContent())?.includes("Allowed") ?? false;
}

/** The same question asked for real, as demo.viewer: refused is 401 or 403, anything else is admitted. */
async function real(
  viewer: { page: Page; context: BrowserContext },
  action: string,
): Promise<boolean> {
  const base = `/api/endpoint/${SLUG}/ngsi-ld/v1/entities`;
  const answer =
    action === "queryEntity"
      ? await viewer.page.request.get(`${base}?type=${TYPE}&limit=1`, {
          headers: { Accept: "application/json" },
        })
      : await viewer.page.request.delete(
          `${base}/${encodeURIComponent(NOBODY)}`,
          {
            headers: { "x-csrf-token": await csrf(viewer.context) },
          },
        );
  expect(answer.status(), `${action} reached the gateway`).toBeLessThan(500);
  return ![401, 403].includes(answer.status());
}

test("Try a person agrees with what the gateway does for demo.viewer", async ({
  browser,
}) => {
  const steward = await signIn(
    browser,
    STEWARD,
    `/projects/${PROJECT}/policies?lang=en`,
  );
  const viewer = await signIn(browser, VIEWER, `/projects/${PROJECT}?lang=en`);
  await expect(
    steward.page.getByRole("heading", { name: "Try a person" }),
  ).toBeVisible();

  for (const action of ["queryEntity", "deleteEntity"]) {
    const said = await simulated(steward.page, action);
    const did = await real(viewer, action);
    expect(
      said,
      `the simulator and demo.viewer's real ${action} disagree`,
    ).toBe(did);
  }

  await steward.context.close();
  await viewer.context.close();
});
