/**
 * T-2849 — the open-data page on dev (EP-62…EP-67, TS-26): what the page shows is what
 * `/ckan/status` answers the same person, and a viewer meets Propose catalogue refused with the
 * reason.
 *
 * dev's `helsinki` project publishes to the dev CKAN through its seeded catalogue (T-2407), so
 * the status holds at least one catalogue and one published endpoint; the journey reads both
 * from the API in the steward's own session and finds each on the page, rather than naming them.
 */
import { expect, test } from "@playwright/test";
import { guideShot } from "./guide";
import { STEWARD, VIEWER, signIn } from "./portal";

test.setTimeout(180_000);

const PROJECT = "helsinki";
const PAGE = `/projects/${PROJECT}/ckan?lang=en`;

interface Status {
  instances: { name: string; url: string; apiTokenRef: string }[];
  publications: { endpoint: string; instance: string; instanceMissing: boolean; dataset: string }[];
}

test("ckan/status: a steward reads each catalogue and each published endpoint the status names", async ({ browser }) => {
  const { context, page } = await signIn(browser, STEWARD, PAGE);
  try {
    await expect(page.getByRole("heading", { level: 1, name: "Open-data catalogue" })).toBeVisible({ timeout: 60_000 });
    const answer = await page.request.get(`/api/v1/projects/${PROJECT}/ckan/status`);
    expect(answer.ok(), `ckan/status: ${answer.status()} ${await answer.text()}`).toBe(true);
    const status = (await answer.json()) as Status;
    expect(status.instances.length, "dev's helsinki project has its seeded catalogue").toBeGreaterThan(0);
    expect(status.publications.length, "dev's helsinki project publishes at least one endpoint").toBeGreaterThan(0);

    const catalogues = page.getByRole("table", { name: "Catalogues" });
    for (const instance of status.instances) {
      const row = catalogues.getByRole("row", { name: new RegExp(instance.name) });
      await expect(row.getByRole("link", { name: instance.url })).toBeVisible();
      await expect(row.getByText(instance.apiTokenRef)).toBeVisible();
    }
    const published = page.getByRole("region", { name: "Published endpoints" });
    for (const publication of status.publications) {
      const card = published.getByRole("listitem").filter({ hasText: publication.endpoint }).first();
      await expect(card.getByText(publication.dataset).first()).toBeVisible();
      await expect(card).toContainText(
        publication.instanceMissing
          ? `catalogue ${publication.instance} is missing`
          : `published to ${publication.instance}`,
      );
    }
    await expect(page.getByRole("alert")).toHaveCount(0);
    await guideShot(page, "ckan-2-published-endpoints", { reloads: true });
  } finally {
    await context.close();
  }
});

test("a viewer meets Propose catalogue refused with the reason", async ({ browser }) => {
  const { context, page } = await signIn(browser, VIEWER, PAGE);
  try {
    const propose = page.getByRole("button", { name: "Propose catalogue" });
    await expect(propose).toBeVisible({ timeout: 60_000 });
    await expect(propose).toHaveAttribute("aria-disabled", "true");
    await expect(propose).toHaveAccessibleDescription(
      "Disabled: your role does not permit 'propose' on 'CkanInstance' in this project",
    );
  } finally {
    await context.close();
  }
});
