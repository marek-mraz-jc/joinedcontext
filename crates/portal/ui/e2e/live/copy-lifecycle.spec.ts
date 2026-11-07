/**
 * A copy's own life on dev, after `copy-employee.spec.ts` brought one back (T-2729; CC-76, CC-79,
 * CC-80, TS-26): its owner updates it from the project and throws it away through the list, and a
 * viewer is refused a copy of their own with the reason, in the dialog and at the API.
 *
 * Nothing reaches the project: the copy is started empty, updated, and discarded, and the
 * `finally` removes it through the API if the journey stops before its Discard.
 */
import { expect, test } from "@playwright/test";
import { STEWARD, VIEWER, csrf, signIn } from "./portal";

const PROJECT = "helsinki";
const SUFFIX = process.env.E2E_SUFFIX ?? new Date().toISOString().slice(11, 16).replace(":", "");
const COPY = `life-${SUFFIX}`;

test.setTimeout(300_000);

test("the owner updates a copy from the project and discards it from the list", async ({ browser }) => {
  const { page } = await signIn(browser, STEWARD, `/projects/${PROJECT}/workspaces?lang=en`);
  try {
    await page.getByRole("button", { name: "Work on a copy" }).first().click();
    const start = page.getByRole("dialog");
    await start.getByLabel(/^Name/).fill(COPY);
    await start.getByRole("button", { name: "Start the copy" }).click();
    await expect(start).toBeHidden({ timeout: 60_000 });

    // T-3219: a second copy of the same name is answered at the name, with what to do.
    await page.goto(`/projects/${PROJECT}/workspaces?lang=en`, { waitUntil: "load" });
    await page.getByRole("button", { name: "Work on a copy" }).first().click();
    const again = page.getByRole("dialog");
    await expect(again.getByLabel("Keep it for")).toHaveAccessibleDescription(/the copy and whatever it holds that was not brought back are deleted/);
    await again.getByLabel(/^Name/).fill(COPY);
    await again.getByRole("button", { name: "Start the copy" }).click();
    await expect(again.getByLabel(/^Name/)).toHaveAccessibleDescription(new RegExp(`A copy named ${COPY} exists already`), { timeout: 30_000 });
    await again.getByRole("button", { name: "Cancel" }).click();

    // Update from the project: the copy takes in what the project changed since it started (CC-80),
    // and says what it took — nothing, when nobody changed the project in the meantime.
    await page.goto(`/projects/${PROJECT}/workspaces/${COPY}/bring-back?lang=en`, { waitUntil: "load" });
    await page.getByRole("button", { name: "Update from the project" }).click();
    await expect(page.getByRole("status").filter({ hasText: /Updated from the project|already has everything/ })).toBeVisible({
      timeout: 60_000,
    });
    await expect(page.getByRole("alert")).toHaveCount(0);

    // Discard, from the list, and only after the person confirms it.
    await page.goto(`/projects/${PROJECT}/workspaces?lang=en`, { waitUntil: "load" });
    const mine = page.getByRole("heading", { level: 2, name: "My copies" }).locator("..");
    const row = mine.getByRole("row").filter({ hasText: COPY });
    await expect(row).toHaveCount(1, { timeout: 30_000 });
    await row.getByRole("button", { name: "Discard" }).click();
    const confirm = page.getByRole("alertdialog", { name: "Discard" });
    await expect(confirm).toContainText(`Discard the copy ${COPY}?`);
    await confirm.getByRole("button", { name: "Discard" }).click();
    await expect(confirm).toBeHidden({ timeout: 30_000 });
    await expect(page.getByRole("row").filter({ hasText: COPY })).toHaveCount(0, { timeout: 30_000 });
    const gone = await page.request.get(`/api/v1/projects/${PROJECT}/workspaces/${COPY}`);
    expect(gone.status(), "a discarded copy still answers").toBe(404);
  } finally {
    const token = await csrf(page.context());
    await page.request.delete(`/api/v1/projects/${PROJECT}/workspaces/${COPY}`, {
      headers: { "x-csrf-token": token },
    });
  }
});

test("a viewer is refused a copy with the reason, and the API refuses the same", async ({ browser }) => {
  const { page } = await signIn(browser, VIEWER, `/projects/${PROJECT}/workspaces?lang=en`);
  // Refused where it stands, with the reason in reach (UI-44), not removed; a click opens nothing.
  const open = page.getByRole("button", { name: "Work on a copy" }).first();
  await expect(open).toHaveAttribute("aria-disabled", "true", { timeout: 30_000 });
  await expect(open).toHaveAccessibleDescription(/Your role may not propose anything in this project/);
  await open.click({ force: true });
  await expect(page.getByRole("dialog")).toHaveCount(0);

  const token = await csrf(page.context());
  const answer = await page.request.post(`/api/v1/projects/${PROJECT}/workspaces`, {
    headers: { "x-csrf-token": token },
    data: { name: `viewer-${SUFFIX}`, ttlDays: 1, scope: { kind: "project" } },
  });
  expect(answer.status(), await answer.text()).toBe(403);
});
