/**
 * T-5066: a model page read live in every combination a person meets it in: steward and viewer,
 * 1440 and 400 px, light and dark, English and Slovak. Each cell opens the first model of the
 * project that belongs to a space, checks the relationship diagram on Overview and the Fields
 * tab's table, and that only the steward is offered Edit and Remove. With `JC_EVIDENCE_DIR`
 * each cell leaves two full-page shots, `model-<who>-<width>-<scheme>-<lang>-{overview,fields}.png`.
 */
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { STEWARD, VIEWER, signIn } from "./portal";

const PROJECT = "helsinki";
const TEXT = {
  en: { edit: "Edit model", fields: "Fields", diagram: "The model's classes and what joins them", remove: /^Remove / },
  sk: { edit: "Upraviť model", fields: "Polia", diagram: "Triedy modelu a čo ich spája", remove: /^Odstrániť / },
} as const;

async function shot(page: Page, name: string): Promise<void> {
  const dir = process.env.JC_EVIDENCE_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  await page.screenshot({ path: join(dir, `${name}.png`), fullPage: true });
}

for (const [who, person] of [
  ["steward", STEWARD],
  ["viewer", VIEWER],
] as const) {
  test(`the model page in every width, scheme and language for a ${who}`, async ({ browser }) => {
    test.setTimeout(600_000);
    const { page, context } = await signIn(browser, person, `/projects/${PROJECT}/models?lang=en`);
    const listed = await page.request.get(`/api/v1/projects/${PROJECT}/datamodels`);
    expect(listed.ok()).toBe(true);
    const items = ((await listed.json()) as { items: { metadata: { name: string }; spec: { contextSpaceRef?: string } }[] }).items;
    const model = items.find((item) => item.spec.contextSpaceRef)?.metadata.name;
    expect(model, "the project has no model that belongs to a space").toBeDefined();

    for (const width of [1440, 400]) {
      for (const scheme of ["light", "dark"] as const) {
        for (const lang of ["en", "sk"] as const) {
          await page.setViewportSize({ width, height: 900 });
          await page.emulateMedia({ colorScheme: scheme });
          await page.goto(`/projects/${PROJECT}/models/${model}?lang=${lang}`);
          await expect(page.getByRole("group", { name: TEXT[lang].diagram })).toBeVisible({ timeout: 60_000 });
          const edit = page.getByRole(who === "steward" ? "link" : "button", { name: TEXT[lang].edit });
          await expect(edit).toBeVisible();
          const remove = page.getByRole("button", { name: TEXT[lang].remove });
          if (who === "viewer") {
            await expect(edit).toHaveAttribute("aria-disabled", "true");
            await expect(remove).toHaveAttribute("aria-disabled", "true");
          }
          const cell = `model-${who}-${width}-${scheme}-${lang}`;
          await shot(page, `${cell}-overview`);
          await page.getByRole("tab", { name: TEXT[lang].fields }).click();
          await expect(page.getByRole("table").first()).toBeVisible({ timeout: 30_000 });
          await shot(page, `${cell}-fields`);
        }
      }
    }
    await context.close();
  });
}

test("every space with a model has a daily validation report less than a day old", async ({ browser }) => {
  test.setTimeout(600_000);
  const { page, context } = await signIn(browser, STEWARD, `/projects/${PROJECT}/models?lang=en`);
  const listed = await page.request.get(`/api/v1/projects/${PROJECT}/datamodels`);
  expect(listed.ok()).toBe(true);
  const items = ((await listed.json()) as { items: { spec: { contextSpaceRef?: string } }[] }).items;
  // A model whose space was removed (a journey's leftover) has no space to validate.
  const existing = await page.request.get(`/api/v1/projects/${PROJECT}/spaces`);
  expect(existing.ok()).toBe(true);
  const live = new Set(((await existing.json()) as { items: { metadata: { name: string } }[] }).items.map((item) => item.metadata.name));
  const spaces = [...new Set(items.map((item) => item.spec.contextSpaceRef).filter((one): one is string => Boolean(one) && live.has(one ?? "")))].sort();
  const stale: string[] = [];
  for (const space of spaces) {
    let answer = await page.request.get(`/api/v1/projects/${PROJECT}/spaces/${space}/quality`);
    // The dev edge answers 429 when the swarm's address is over its minute: wait the window out.
    for (let retry = 0; answer.status() === 429 && retry < 2; retry += 1) {
      await page.waitForTimeout(60_000);
      answer = await page.request.get(`/api/v1/projects/${PROJECT}/spaces/${space}/quality`);
    }
    const report = answer.ok() ? ((await answer.json()) as { observedAt?: string; checked?: number; invalid?: number } | null) : null;
    const age = report?.observedAt ? Date.now() - Date.parse(report.observedAt) : Infinity;
    // One line per space, the counts the task records.
    console.log(`quality ${space}: http ${answer.status()} observedAt ${report?.observedAt ?? "-"} checked ${report?.checked ?? "-"} invalid ${report?.invalid ?? "-"}`);
    if (!(age < 86_400_000)) stale.push(`${space} (http ${answer.status()}, observedAt ${report?.observedAt ?? "-"})`);
  }
  expect(stale, "spaces without a report from the last 24 h").toEqual([]);
  await context.close();
});
