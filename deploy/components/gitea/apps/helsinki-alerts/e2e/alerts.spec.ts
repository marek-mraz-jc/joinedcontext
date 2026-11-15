import { expect, test } from "@playwright/test";
import { STEWARD, VIEWER } from "../src/fixtures/access";
import { SLUG, serve } from "./serve";

// AP-09, AP-96, SDK-40: the viewer reads an alert in the entity panel, linked to the Portal, and is
// offered no way to change one.
test("a viewer reads the alerts and gets no form, no edit and no delete", async ({ page }) => {
  const { alerts, writes, outside, problems } = await serve(page, "viewer", VIEWER);

  await alerts.getByRole("table").getByText("Kauppatori, Helsinki").click();
  const panel = page.getByRole("dialog");
  await expect(panel.getByRole("link", { name: "Open in the Portal" })).toBeVisible();
  await expect(panel.getByRole("button", { name: "Edit" })).toHaveCount(0);
  for (const name of ["New alert", "Correct names and place", "Delete"]) await expect(alerts.getByRole("button", { name })).toHaveCount(0);
  await page.keyboard.press("Escape");
  await expect(panel).toHaveCount(0);
  expect(writes).toEqual([]);
  expect(outside).toEqual([]);
  expect(problems).toEqual([]);
});

// AP-62, SDK-40: the steward corrects an alert in the entity panel: the change shown first, then one
// PATCH of the attribute that changed, and nothing else.
test("a steward corrects an alert with one PATCH of the changed attribute", async ({ page }) => {
  const { alerts, writes, outside, problems } = await serve(page, "steward", STEWARD);

  await alerts.getByRole("table").getByText("Mannerheimintie resurfacing").click();
  const panel = page.getByRole("dialog");
  await panel.getByRole("button", { name: "Edit" }).click();
  await panel.getByLabel(/address/i).fill("Mannerheimintie 14, Helsinki");
  await panel.getByRole("button", { name: "Review the change" }).click();
  expect(writes).toEqual([]);
  await panel.getByRole("button", { name: "Save the change" }).click();
  await expect(panel.getByText("Saved.")).toBeVisible();

  expect(writes).toEqual([
    {
      method: "PATCH",
      path: `/api/endpoint/${SLUG}/ngsi-ld/v1/entities/${encodeURIComponent("urn:ngsi-ld:Alert:hel.fi:helsinki:GUID50001")}/attrs`,
      body: { address: { type: "Property", value: "Mannerheimintie 14, Helsinki" } },
    },
  ]);
  await expect(alerts.getByRole("table").getByText("Mannerheimintie 14, Helsinki")).toBeVisible();
  expect(outside).toEqual([]);
  expect(problems).toEqual([]);
});

// AP-09: a steward adds an alert, and removes only an alert a steward added.
test("a steward adds an alert and deletes it, and cannot delete Fintraffic's", async ({ page }) => {
  const { alerts, writes, problems } = await serve(page, "steward", STEWARD);

  await alerts.getByRole("button", { name: "New alert" }).click();
  const form = alerts.getByRole("form", { name: "New alert" });
  await form.getByLabel("Local id").fill("steward-closure");
  await form.getByLabel("address", { exact: true }).fill("Senaatintori, Helsinki");
  await form.getByLabel("category", { exact: true }).fill("event");
  await form.getByRole("button", { name: "Save" }).click();
  await expect(form).toHaveCount(0);

  await alerts.getByRole("table").getByText("Mannerheimintie resurfacing").click();
  await expect(alerts.getByRole("button", { name: "Delete" })).toHaveCount(0);

  await alerts.getByRole("table").getByText("Senaatintori, Helsinki").click();
  await alerts.getByRole("button", { name: "Delete" }).click();
  await expect(alerts.getByRole("table").getByText("Senaatintori, Helsinki")).toHaveCount(0);

  expect(writes.map((write) => write.method)).toEqual(["POST", "DELETE"]);
  expect(writes[0].body).not.toHaveProperty("source");
  expect(writes[1].path).toContain(encodeURIComponent("urn:ngsi-ld:Alert:hel.fi:helsinki:steward-closure"));
  expect(problems).toEqual([]);
});
