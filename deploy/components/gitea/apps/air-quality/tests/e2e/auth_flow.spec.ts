/**
 * The record form through the real binary (T-2617, AP-40, AP-62, AP-109, GW10): an anonymous
 * reader and a viewer get no form, a steward adds, corrects and removes a station, and a direct
 * write without a token or with a viewer's token is refused.
 */
import { expect, test } from "@playwright/test";

const BASE = "/apps/air-quality/";
const ID = "urn:ngsi-ld:AirQualityObserved:hel.fi:helsinki:kallio";

/** The headers the edge puts in front of the app for a signed-in person (AP-28, ADR-N-019). */
const edge = (who: string) => ({
  "x-access-token": `token-for-${who}`,
  "x-userinfo": Buffer.from(
    JSON.stringify({ sub: `f:1:demo.${who}`, preferred_username: `demo.${who}@hel.fi`, email: `demo.${who}@hel.fi` }),
  ).toString("base64"),
});

test.describe("who may edit a station record", () => {
  test("an anonymous reader sees the measurements and no control", async ({ page }) => {
    await page.goto(BASE);

    await expect(page.getByRole("heading", { name: "Kallio", exact: true })).toBeVisible();
    await expect(page.getByText("34.2 µg/m³")).toBeVisible();
    await expect(page.getByText("You are viewing anonymously.")).toBeVisible();
    // Nothing that writes: no form and no Add, Edit, Save, Remove or Confirm. The station picker
    // beside the map (T-2925) is reading, and an anonymous reader has it like everyone else.
    await expect(page.getByRole("form")).toHaveCount(0);
    await expect(page.getByRole("button", { name: /^(Add|Edit|Save|Remove|Confirm)\b/ })).toHaveCount(0);
    await expect(page.getByRole("button", { name: /^Kallio/ })).toHaveAttribute("aria-pressed", "true");
  });

  test("a signed-in viewer is named on the page and sees Edit disabled with the reason", async ({ page }) => {
    await page.setExtraHTTPHeaders(edge("viewer"));
    await page.goto(BASE);

    await expect(page.getByText("demo.viewer@hel.fi", { exact: true })).toBeVisible();
    const edit = page.getByRole("button", { name: "Edit Kallio" });
    await expect(edit).toBeDisabled();
    await expect(edit).toHaveAccessibleDescription("Only a steward adds, corrects or removes station records.");
    await expect(page.getByRole("form")).toHaveCount(0);
  });

  test("a steward adds a station, corrects it and removes it", async ({ page }) => {
    await page.setExtraHTTPHeaders(edge("steward"));
    await page.goto(BASE);

    const add = page.getByRole("form", { name: "New station" });
    await add.getByLabel("Station id").fill("vallila");
    await add.getByLabel("Name in Finnish").fill("Vallila");
    await add.getByLabel("Longitude").fill("24.95");
    await add.getByLabel("Latitude").fill("60.19");
    await add.getByRole("button", { name: "Add station" }).click();
    await expect(page.getByRole("heading", { name: "Vallila", exact: true })).toBeVisible();

    await page.getByRole("button", { name: "Edit Vallila" }).click();
    const edit = page.getByRole("form", { name: "Edit Vallila" });
    await edit.getByLabel("Steward note").fill("Sensor cleaned.");
    await edit.getByRole("button", { name: "Save changes" }).click();
    await expect(page.getByText("Note: Sensor cleaned.")).toBeVisible();

    // The pipeline's station stays: its Remove is disabled and says why.
    await expect(page.getByRole("button", { name: "Remove Kallio" })).toBeDisabled();
    await page.getByRole("button", { name: "Remove Vallila" }).click();
    await page.getByRole("button", { name: "Confirm removal of Vallila" }).click();
    await expect(page.getByRole("heading", { name: "Vallila", exact: true })).toHaveCount(0);
  });

  // The buttons are a convenience; the refusal is the control. These call the app the way a
  // script would, with no page in the way, so what is asserted is the server's own answer.
  test("the app refuses a write with no access token and does not retry anonymously", async ({ request }) => {
    const response = await request.patch(`${BASE}api/stations/${encodeURIComponent(ID)}`, {
      data: { stewardNote: "Anyone at all." },
    });

    expect(response.status()).toBe(401);
    expect(await response.text()).toContain("signed-in");
  });

  test("a viewer who calls the write directly is refused in the gateway's own words", async ({ request }) => {
    const response = await request.patch(`${BASE}api/stations/${encodeURIComponent(ID)}`, {
      headers: edge("viewer"),
      data: { stewardNote: "Trying anyway." },
    });

    expect(response.status()).toBe(403);
    expect(await response.text()).toContain("needs the steward role");
  });
});
