// Find in the service on dev (T-7076, G-08): a steward types the Helsinki WFS into a new http
// source, picks the WFS preset, and the Portal lists the feature types the service offers, read
// once on the project's runner; Use writes one into the preset. An agent reads the same listing
// through `jc_datasource_discover`. Nothing is proposed or written.
import { expect, test } from "@playwright/test";
import { STEWARD, csrf, goSignedIn } from "./portal";

const PROJECT = "helsinki";
const WFS = "https://kartta.hel.fi/ws/geoserver/avoindata/wfs";

test("a steward finds the feature types of a public WFS and writes one into the preset", async ({ page }) => {
  await goSignedIn(page, STEWARD, `/projects/${PROJECT}/datasources/new?lang=en`);
  const form = page.getByTestId("form-page");
  await form.getByRole("textbox", { name: "Name", exact: true }).fill("street-trees");
  await form.getByRole("textbox", { name: /^URL/ }).fill(WFS);
  await form.getByRole("combobox", { name: "API", exact: true }).selectOption({ label: "WFS 2.0" });
  await page.getByRole("button", { name: "Find", exact: true }).click();
  const choice = page.getByRole("combobox", { name: /Feature type · Found/ });
  await expect(choice).toBeVisible({ timeout: 60_000 });
  const value = (await choice.locator("option").evaluateAll((options) => options.map((o) => (o as HTMLOptionElement).value))).find(Boolean);
  expect(value, "the WFS lists a feature type").toBeTruthy();
  await choice.selectOption(value ?? "");
  await page.getByRole("button", { name: "Use", exact: true }).click();
  await expect(form.getByRole("textbox", { name: /^Feature type/ })).toHaveValue(value ?? "");
});

test("an agent lists the same feature types through jc_datasource_discover", async ({ page, context }) => {
  await goSignedIn(page, STEWARD, `/projects/${PROJECT}/datasources?lang=en`);
  const answer = await page.request.post(`/api/v1/projects/${PROJECT}/ops/jc_datasource_discover`, {
    headers: { "x-csrf-token": await csrf(context), "content-type": "application/json" },
    data: { kind: "wfs", url: WFS },
  });
  expect(answer.status(), await answer.text()).toBe(200);
  const found = (await answer.json()) as { field: string; choices: { value: string }[]; skipped?: string };
  expect(found.skipped).toBeUndefined();
  expect(found.field).toBe("typeNames");
  expect(found.choices.length).toBeGreaterThan(0);
});
