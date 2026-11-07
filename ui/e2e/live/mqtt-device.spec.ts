/**
 * A steward adds a device to a helsinki MQTT broker on dev with a key pair made in the browser
 * (API/01 §56, T-5532, PF-47).
 *
 * The journey opens the brokers page, generates the key pair with WebCrypto, proposes the device
 * and reads the pending Change and the sign-in card. The request that left the browser carries the
 * public key alone. A private key member is refused with a 422 naming it, and a viewer is refused.
 * The Change is rejected afterwards, so dev is left as it was; the token request and the publish
 * with the downloaded key are the approved run of the task's Done-when.
 */
import { expect, test } from "@playwright/test";
import { STEWARD, VIEWER, csrf, proposedChange, reject, signIn } from "./portal";

test.setTimeout(300_000);

const PROJECT = "helsinki";
const BROKERS = `/api/v1/projects/${PROJECT}/mqttbrokers`;
const DEVICE = "t5532-dev1";
const KEY = {
  kty: "EC",
  crv: "P-256",
  kid: "k1",
  x: "f83OJ3D2xF1Bg8vub9tLe1gHMzV76e8Tus9uPHvRVEU",
  y: "x_FEzRu9m36HLN_tue659LNpXW6pCyStikYjKIWI5a0",
};

test("a steward adds a device by a browser-made key pair; a private key and a viewer are refused", async ({ browser }) => {
  const { context, page } = await signIn(browser, STEWARD, `/projects/${PROJECT}/mqttbrokers?lang=en`);
  let change = "";
  let broker = "";
  try {
    const listed = (await (await page.request.get(BROKERS)).json()) as { items: { metadata: { name: string } }[] };
    expect(listed.items.length, "dev holds an MqttBroker in helsinki").toBeGreaterThan(0);
    broker = listed.items[0].metadata.name;
    const devices = `${BROKERS}/${broker}/devices`;
    const options = (await (await page.request.get(devices)).json()) as { mayAdd: boolean; roles: { name: string; publish: string[] }[] };
    expect(options.mayAdd, "the steward may propose ServiceAccounts and MqttBrokers").toBe(true);

    const headers = { "x-csrf-token": await csrf(context) };
    const role = options.roles[0];
    const leaked = await page.request.post(devices, {
      headers,
      data: { name: DEVICE, role: role.name, topic: role.publish[0] ?? null, jwks: { keys: [{ ...KEY, d: "jpsQnnGQmL-YBIffH1136cspYG6-0iY7X1fCE9-E9LI" }] } },
    });
    expect(leaked.status(), await leaked.text()).toBe(422);
    expect(await leaked.text()).toContain("jwks.keys[0].d");

    const card = page.locator(`section[aria-labelledby="broker-${broker}"]`);
    await card.getByRole("button", { name: "Add device" }).click();
    await card.getByRole("textbox", { name: /Device name/ }).fill(DEVICE);
    await card.getByRole("button", { name: "Generate key pair" }).click();
    await expect(card.getByText("Save the private key now")).toBeVisible();
    const sent = page.waitForRequest((request) => request.method() === "POST" && request.url().endsWith("/devices"));
    await card.getByRole("button", { name: "Propose device" }).click();
    const body = (await sent).postData() ?? "";
    expect(body).not.toContain("PRIVATE KEY");
    expect(JSON.parse(body).jwks.keys[0].d).toBeUndefined();
    await expect(card.getByText(`${DEVICE} is proposed`)).toBeVisible({ timeout: 60_000 });
    await expect(card.getByRole("heading", { name: "How this device signs in" })).toBeVisible();
    await expect(card.getByText("-----BEGIN PRIVATE KEY-----")).toHaveCount(0);
    change = await proposedChange(page);
  } finally {
    if (change) await reject(page, PROJECT, change, "Rejected by a live journey: the device was only proved, then withdrawn.");
    await context.close();
  }

  const viewer = await signIn(browser, VIEWER, `/projects/${PROJECT}/mqttbrokers?lang=en`);
  try {
    const devices = `${BROKERS}/${broker}/devices`;
    const headers = { "x-csrf-token": await csrf(viewer.context) };
    const refused = await viewer.page.request.post(devices, { headers, data: { name: DEVICE, role: "camera", jwks: { keys: [KEY] } } });
    expect([403, 404], await refused.text()).toContain(refused.status());
  } finally {
    await viewer.context.close();
  }
});
