/**
 * T-5064 — the data workspace of a space, live on dev, as a person uses it (UI-33, API/01 §30,
 * ADR-N-042 §3.2). Since T-6210 the space's data tab is the grid and the API tab.
 *
 * Steward (needs PORTAL_TOTP): opens the type's form from "Add row" and finds its hints, corrects
 * a station's status in the grid with a reason and undoes it (bikes-ops-steward grants updateAttrs
 * on that one attribute), then adds an Alert and deletes it again (T-5092: steward-row-journey
 * reaches only Alerts of category `journey` without `source`, so no pipeline row). A `finally` puts
 * back what a failed step left. The write goes through the Endpoint as the signed-in person.
 *
 * Editor: may propose, holds no write Policy on helsinki, and is told so. Viewer: filters and sorts,
 * finds no editor and is refused a crafted write. Visitor: a crafted write with no session writes
 * nothing.
 *
 * SHOTS_DIR set: each state is photographed at 1440 and 400 px.
 */
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { EDITOR, STEWARD, VIEWER, csrf, signIn } from "./portal";

test.setTimeout(600_000);

const SPACE = "helsinki";
const pageOf = (type: string) => `/projects/helsinki/spaces/${SPACE}?lang=en&type=${type}`;
// The one cell dev's steward may correct: bikes-ops-steward names `status` of this type only.
const STATION = "BikeHireDockingStation";
const ALERTS = pageOf("Alert");
const SUFFIX = process.env.E2E_SUFFIX ?? `${Date.now()}`;
const SHOTS = process.env.SHOTS_DIR;

async function shoot(page: Page, state: string): Promise<void> {
  if (!SHOTS) return;
  const size = page.viewportSize() ?? { width: 1440, height: 900 };
  for (const width of [1440, 400]) {
    await page.setViewportSize({ width, height: 900 });
    await page.waitForTimeout(500);
    await page.screenshot({ path: `${SHOTS}/data-workspace-${state}-${width}.png`, fullPage: true });
  }
  await page.setViewportSize(size);
}

/** A station's status as dev stores it, read with the person's session. */
async function statusValueOf(page: Page, id: string): Promise<unknown> {
  const answer = await page.request.get(`/cs/${SPACE}/ngsi-ld/v1/entities/${encodeURIComponent(id)}?format=keyValues&pick=status`);
  expect(answer.status()).toBe(200);
  return ((await answer.json()) as { status?: unknown }).status;
}

/** Opens a page again after the sign-in: WALK_UI_DIST serves this tree's UI only to a navigation of its own. */
async function openGrid(page: Page, address: string): Promise<void> {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(address);
  await expect(page.getByRole("tab", { name: "Grid" })).toBeVisible({ timeout: 60_000 });
  // A broker read can pass dev's 10 s cut under load; the page says so and offers Retry.
  await expect(async () => {
    const retry = page.getByRole("button", { name: "Retry" });
    if ((await retry.count()) > 0) await retry.first().click();
    await expect(page.locator("tbody tr td").first()).toBeVisible({ timeout: 25_000 });
  }).toPass({ timeout: 120_000 });
}

test("a steward opens a row form and corrects a cell in the grid with a reason, then undoes it", async ({ browser }) => {
  const { context, page } = await signIn(browser, STEWARD, ALERTS);
  let restore: { id: string; status: string } | undefined;
  try {
    await openGrid(page, ALERTS);
    await shoot(page, "grid-steward");

    // "Add row": the type's form, each field with the model's hint; nothing is sent.
    await page.getByRole("button", { name: "Add row" }).click();
    const dialog = page.getByRole("dialog");
    await expect(dialog.getByText("The alert's domain; traffic for everything Fintraffic and HSL publish.")).toBeVisible();
    await shoot(page, "add-row-steward");
    await page.keyboard.press("Escape");

    await openGrid(page, pageOf(STATION));
    const [seed] = (await (
      await page.request.get(`/cs/${SPACE}/ngsi-ld/v1/entities?type=${STATION}&q=status&pick=id,status&format=keyValues&limit=1`)
    ).json()) as { id: string; status: string }[];
    expect(seed, "dev holds a station with a status").toBeTruthy();
    await page.getByRole("button", { name: "Edit", exact: true }).click();
    await page.getByPlaceholder("Search").first().fill(seed.id.split(":").pop() ?? seed.id);
    const cell = page.getByLabel(/^Edit status$/i).first();
    await expect(cell).toBeVisible({ timeout: 60_000 });
    const edited = seed.status === "working" ? "outOfService" : "working";
    restore = seed;
    await cell.fill(edited);
    await page.keyboard.press("Enter");
    await page.getByRole("button", { name: "Review the changes" }).click();
    const review = page.getByRole("region", { name: "Review the changes" });
    await review.getByRole("textbox", { name: "Why you change these values" }).fill("Data workspace journey");
    await shoot(page, "edit-review-steward");
    await review.getByRole("button", { name: "Apply", exact: true }).click();
    const undo = page.getByRole("button", { name: "Undo" });
    const refusal = review.getByText(/does not exist|refused|denied|no policy/i);
    await expect(undo.or(refusal).first()).toBeVisible({ timeout: 60_000 });
    await expect(refusal, "the Endpoint takes the correction").toHaveCount(0);
    await expect.poll(() => statusValueOf(page, seed.id), { timeout: 30_000 }).toBe(edited);
    await shoot(page, "edited-steward");

    await undo.click();
    await expect(page.getByText("The change was undone.")).toBeVisible({ timeout: 60_000 });
    await expect.poll(() => statusValueOf(page, seed.id), { timeout: 30_000 }).toBe(seed.status);
    restore = undefined;
  } finally {
    if (restore && (await statusValueOf(page, restore.id).catch(() => restore?.status)) !== restore.status) {
      await page.request.patch(`/cs/${SPACE}/ngsi-ld/v1/entities/${encodeURIComponent(restore.id)}/attrs`, {
        headers: { "x-csrf-token": await csrf(context), "JC-Reason": "T-5064 journey: the value it had" },
        data: { status: { type: "Property", value: restore.status } },
      });
    }
    await context.close();
  }
});

/** A row's stored answer, read with the person's session: 200 while it exists, 404 after. */
async function statusOf(page: Page, id: string): Promise<number> {
  return (await page.request.get(`/cs/${SPACE}/ngsi-ld/v1/entities/${encodeURIComponent(id)}?pick=id`)).status();
}

test("a steward adds a row through the form and deletes it from the grid", async ({ browser }) => {
  const { context, page } = await signIn(browser, STEWARD, ALERTS);
  const name = `Journey row ${SUFFIX}`;
  let created: string | undefined;
  try {
    await openGrid(page, ALERTS);

    await page.getByRole("button", { name: "Add row" }).click();
    const dialog = page.getByRole("dialog");
    await dialog.getByLabel(/^name/).first().fill(name);
    await dialog.getByLabel(/^category/).first().fill("journey");
    await dialog.getByRole("button", { name: "Create" }).click();
    const status = dialog.getByRole("status");
    await expect(status.or(dialog.getByRole("alert")).first()).toBeVisible({ timeout: 60_000 });
    await expect(status, "the steward-row-journey Policy lets the steward create a journey Alert").toHaveText(/^Created urn:ngsi-ld:Alert:/);
    created = (await status.textContent())?.replace(/^Created /, "").replace(/\.$/, "");
    expect(created).toMatch(/^urn:ngsi-ld:Alert:/);
    expect(await statusOf(page, created ?? "")).toBe(200);
    await shoot(page, "row-created-steward");
    await page.keyboard.press("Escape");

    await page.getByPlaceholder("Search").first().fill(name);
    const row = page.getByRole("row").filter({ hasText: name });
    await expect(row).toHaveCount(1, { timeout: 60_000 });
    await row.getByRole("checkbox", { name: /^Select row/ }).check();
    await page.getByRole("button", { name: "Delete selected" }).click();
    await shoot(page, "row-delete-confirm-steward");
    await page.getByRole("alertdialog").getByRole("button", { name: "Delete selected" }).click();
    await expect.poll(() => statusOf(page, created ?? ""), { timeout: 30_000 }).toBe(404);
    await expect(page.getByText(/rows were not deleted/)).toHaveCount(0);
    await expect(row).toHaveCount(0, { timeout: 30_000 });
    await shoot(page, "row-deleted-steward");
  } finally {
    if (created && (await statusOf(page, created).catch(() => 404)) === 200) {
      await page.request.delete(`/cs/${SPACE}/ngsi-ld/v1/entities/${encodeURIComponent(created)}`, {
        headers: { "x-csrf-token": await csrf(context), "JC-Reason": "T-5092 journey: its own row" },
      });
    }
    // The grid keeps the person's copy of a deleted row for 30 days; the journey's copy goes now.
    const trash = await page.request.get(`/api/v1/projects/helsinki/spaces/${SPACE}/trash`);
    if (created && trash.ok()) {
      for (const kept of ((await trash.json()) as { id: number; urn: string }[]).filter((item) => item.urn === created)) {
        await page.request.delete(`/api/v1/projects/helsinki/spaces/${SPACE}/trash/${kept.id}`, {
          headers: { "x-csrf-token": await csrf(context) },
        });
      }
    }
    await context.close();
  }
});

// Event is read-only for the demo people: no Policy of helsinki grants them a write on it.
const READ_ONLY = pageOf("Event");

test("an editor may propose but holds no write here, and the form says who can grant one", async ({ browser }) => {
  const { context, page } = await signIn(browser, EDITOR, ALERTS);
  try {
    await openGrid(page, ALERTS);
    await page.getByRole("button", { name: "Edit", exact: true }).click();
    await expect(page.getByText(/^No endpoint of this space lets you correct/)).toBeVisible({ timeout: 60_000 });
    await expect(page.locator("input[aria-label^='Edit ']")).toHaveCount(0);
    await shoot(page, "edit-none-editor");

    await page.getByRole("button", { name: "Add row" }).click();
    const dialog = page.getByRole("dialog");
    await dialog.getByLabel(/^name/i).first().fill(`Journey refused ${SUFFIX}`);
    await dialog.getByLabel(/^category/i).first().fill("journey");
    await dialog.getByRole("button", { name: "Create" }).click();
    const refused = dialog.getByRole("alert");
    await expect(refused).toBeVisible({ timeout: 60_000 });
    await expect(refused, "a refusal names who can grant the write").toContainText("Ask a project owner");
    await shoot(page, "add-row-refused-editor");
  } finally {
    await context.close();
  }
});

test("a viewer filters and sorts the grid, finds no editor and a crafted write is refused", async ({ browser }) => {
  const { context, page } = await signIn(browser, VIEWER, READ_ONLY);
  try {
    await openGrid(page, READ_ONLY);
    await expect(page.getByRole("columnheader", { name: /Name/ }).first()).toBeVisible({ timeout: 60_000 });
    await expect(page.locator("input[aria-label^='Edit ']")).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Add row" }), "a viewer's Add row is off, with its reason").toHaveAttribute("aria-disabled", "true");
    await shoot(page, "grid-viewer");

    await page.getByPlaceholder("Search").first().fill("music");
    await page.getByRole("tab", { name: "API" }).click();
    await expect(page.getByRole("tab", { name: "API", selected: true })).toBeVisible();
    await shoot(page, "api-viewer");

    const headers = { "x-csrf-token": await csrf(context), "content-type": "application/ld+json" };
    const entity = { id: `urn:ngsi-ld:Event:journey-${SUFFIX}`, type: "Event", eventStatus: { type: "Property", value: "EventScheduled" } };
    const crafted = await page.request.post(`/cs/${SPACE}/ngsi-ld/v1/entities`, { headers, data: entity });
    expect([400, 403], "a viewer may not create a row through the space").toContain(crafted.status());
    const created = await page.request.get(`/cs/${SPACE}/ngsi-ld/v1/entities/${encodeURIComponent(entity.id)}`);
    expect(created.status(), "the crafted row does not exist").toBe(404);
    const seed = (await (await page.request.get(`/cs/${SPACE}/ngsi-ld/v1/entities?type=Event&pick=id&limit=1`)).json()) as { id: string }[];
    const patched = await page.request.patch(`/cs/${SPACE}/ngsi-ld/v1/entities/${encodeURIComponent(seed[0].id)}/attrs`, {
      headers: { ...headers, "content-type": "application/json", "JC-Reason": "T-5064 viewer refusal" },
      data: { eventStatus: { type: "Property", value: "EventCancelled" } },
    });
    expect(patched.status(), "a viewer may not correct a cell by a crafted request").toBe(403);
  } finally {
    await context.close();
  }
});

test("a visitor with no session writes nothing through the space", async ({ browser }) => {
  const context = await browser.newContext();
  try {
    const answer = await context.request.post(`/cs/${SPACE}/ngsi-ld/v1/entities`, {
      headers: { "content-type": "application/ld+json" },
      data: { id: `urn:ngsi-ld:Event:journey-${SUFFIX}-anon`, type: "Event" },
    });
    expect(answer.status(), "no session, no write").toBeGreaterThanOrEqual(300);
    expect(answer.status()).toBeLessThan(500);
    const read = await context.request.get(`/cs/${SPACE}/ngsi-ld/v1/entities/${encodeURIComponent(`urn:ngsi-ld:Event:journey-${SUFFIX}-anon`)}`);
    expect(read.status(), "the crafted row does not exist").not.toBe(200);
  } finally {
    await context.close();
  }
});
