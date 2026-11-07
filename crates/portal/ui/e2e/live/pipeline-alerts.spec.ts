/**
 * A person chooses a pipeline's alerts on dev, mutes them and stops them (API/01 §37, PL-71,
 * T-3261). The steward opens the seeded `citybikes-gbfs-info` pipeline's Alerts from its row,
 * subscribes, mutes for a day and stops; e-mail is refused with its reason. Only the Portal's own
 * records are written, and a `finally` stops whatever the journey left.
 */
import { expect, test } from "@playwright/test";
import { STEWARD, csrf, signIn } from "./portal";

const PROJECT = "helsinki";
const PIPELINE = "citybikes-gbfs-info";

test("a person subscribes to a pipeline's alerts, mutes them for a day and stops them", async ({ browser }) => {
  const { context, page } = await signIn(browser, STEWARD, `/projects/${PROJECT}/pipelines?lang=en`);
  try {
    const headers = { "x-csrf-token": await csrf(context), "content-type": "application/json" };
    const mail = await page.request.put("/api/v1/alerts", {
      headers,
      data: { project: PROJECT, scope: "pipeline", target: PIPELINE, events: ["failure"], delivery: "email" },
    });
    expect(mail.status(), "e-mail is refused while the Portal has no mail relay").toBe(400);

    await page.getByRole("button", { name: `More actions for ${PIPELINE}` }).click();
    await page.getByRole("menuitem", { name: "Alerts…" }).click();
    const dialog = page.getByRole("dialog", { name: `Alerts for ${PIPELINE}` });
    await expect(dialog.getByRole("radio", { name: /By e-mail/ })).toBeDisabled();
    await dialog.getByRole("button", { name: "Alert me" }).click();
    await expect(dialog.getByRole("status")).toHaveText("Active.", { timeout: 30_000 });

    await dialog.getByLabel("Mute").selectOption("1d");
    await expect(dialog.getByRole("status")).toContainText("Muted until", { timeout: 30_000 });
    await dialog.getByRole("button", { name: "Stop these alerts" }).click();
    await expect(dialog.getByRole("button", { name: "Alert me" })).toBeVisible({ timeout: 30_000 });

    const left = (await (await page.request.get("/api/v1/alerts")).json()) as { items: { target: string }[] };
    expect(left.items.some((one) => one.target === PIPELINE)).toBe(false);
  } finally {
    const headers = { "x-csrf-token": await csrf(context) };
    const mine = (await (await page.request.get("/api/v1/alerts")).json()) as { items?: { id: number; target: string }[] };
    for (const one of mine.items ?? []) {
      if (one.target === PIPELINE) await page.request.delete(`/api/v1/alerts/${one.id}`, { headers });
    }
    await context.close();
  }
});
