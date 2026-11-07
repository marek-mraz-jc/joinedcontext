/**
 * T-3976 (owed by T-3995): a pipeline of the project saved as a template keeps its shape and
 * none of the project's credentials or references, the gallery offers it on a new pipeline, and
 * using it fills the form; the journey's template is deleted again.
 */
import { expect, test } from "@playwright/test";
import { STEWARD, csrf, signIn } from "./portal";

test.setTimeout(300_000);

const PROJECT = "helsinki";
const TITLE = `journey-${Date.now().toString(36)}`;

test("a pipeline saved as a template starts the next one and keeps no credential", async ({ browser }) => {
  const { context, page } = await signIn(browser, STEWARD, `/projects/${PROJECT}/pipelines?lang=en`);
  let id = "";
  try {
    const list = (await (await page.request.get(`/api/v1/projects/${PROJECT}/pipelines`)).json()) as {
      items: { apiVersion: string; kind: string; metadata: { name: string; namespace?: string }; spec: Record<string, unknown> }[];
    };
    expect(list.items.length).toBeGreaterThan(0);
    const pipeline = list.items[0];
    const saved = await page.request.post(`/api/v1/projects/${PROJECT}/pipelinetemplates`, {
      headers: { "x-csrf-token": await csrf(context), "content-type": "application/json" },
      data: { title: TITLE, manifest: pipeline },
    });
    expect(saved.status(), await saved.text()).toBe(201);
    const template = (await saved.json()) as { id: string; spec: Record<string, unknown> };
    id = template.id;
    const kept = JSON.stringify(template.spec);
    expect(kept).not.toContain("secretRefs");
    expect(kept).not.toContain("urn:ngsi-ld:Endpoint:");
    expect(kept).not.toContain("dataSourceRef");

    await page.goto(`/projects/${PROJECT}/pipelines?lang=en`);
    await page.getByRole("button", { name: "New pipeline" }).first().click();
    const offered = page.getByTestId("pipeline-template").filter({ hasText: TITLE });
    await expect(offered).toBeVisible({ timeout: 60_000 });
    await offered.getByRole("button", { name: "Use" }).click();
    await expect(page.getByTestId("form-page")).toBeVisible();
  } finally {
    if (id) {
      const gone = await page.request.delete(`/api/v1/projects/${PROJECT}/pipelinetemplates/${id}`, {
        headers: { "x-csrf-token": await csrf(context) },
      });
      expect(gone.status()).toBe(204);
    }
    await context.close();
  }
});
