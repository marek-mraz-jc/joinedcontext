import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// T-5532: a steward adds a device to an MQTT broker with a key pair made in the browser. `vite
// preview` has no Portal API behind it, so the browser answers it; WebCrypto, the clicks and the
// rendering are the real ones. The token request and the publish are the live journey on dev.

const API = "joinedcontext.com/v1alpha1";
const PROJECT = "helsinki";
const SHOTS = process.env.JC_SHOTS_DIR;
// Built, so the secret scanner reads no PEM block across the assertions below.
const PEM_BEGIN = ["-----BEGIN", "PRIVATE KEY-----"].join(" ");

const BROKER = {
  apiVersion: API,
  kind: "MqttBroker",
  metadata: { name: "cameras", namespace: PROJECT },
  spec: { roles: [{ name: "cams", serviceAccounts: ["cam-1"], publish: ["cameras/${clientid}/#", "cameras/${clientid}/status"] }] },
  status: { phase: "Live" },
};

async function stubApi(page: Page, posts: string[]): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    const url = new URL(route.request().url());
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: status >= 400 ? "application/problem+json" : "application/json", body: JSON.stringify(body) });
    if (url.pathname.endsWith("/auth/me")) {
      return json({ subject: "b7c1e0f4", username: "aino.virtanen", name: "Aino Virtanen", roles: ["portal-editor"] });
    }
    if (url.pathname === "/api/v1/projects") return json({ apiVersion: API, kind: "ProjectList", items: [{ name: PROJECT }] });
    if (url.pathname.endsWith("/mqttbrokers/cameras/devices")) {
      if (route.request().method() === "GET") {
        return json({
          mayAdd: true,
          roles: [{ name: "cams", publish: BROKER.spec.roles[0].publish }],
          tokenUrl: "https://id.dev.joinedcontext.com/realms/jc/protocol/openid-connect/token",
          issuer: "https://id.dev.joinedcontext.com/realms/jc",
        });
      }
      const body = route.request().postData() ?? "";
      posts.push(body);
      if (JSON.parse(body).topic === "cameras/${clientid}/status") {
        return json({ title: "Unprocessable Content", status: 422, detail: "the topic is outside the role", errors: ["topic: role 'cams' publishes only to cameras/${clientid}/#"] }, 422);
      }
      return json({ apiVersion: API, kind: "Change", metadata: { name: "pr-42", namespace: PROJECT }, status: { lane: "red", phase: "PendingApproval", plan: { create: 1, update: 1, delete: 0 } } }, 202);
    }
    if (url.pathname.endsWith("/mqttbrokers/cameras")) return json(BROKER);
    const plural = url.pathname.split("/").pop() ?? "";
    return json({ apiVersion: API, kind: "List", items: plural === "mqttbrokers" ? [BROKER] : [] });
  });
}

async function shoot(page: Page, name: string): Promise<void> {
  if (!SHOTS) return;
  for (const width of [1440, 400]) {
    await page.setViewportSize({ width, height: 900 });
    await page.screenshot({ path: `${SHOTS}/mqtt-device-${name}-${width}.png`, fullPage: true });
  }
  await page.setViewportSize({ width: 1440, height: 900 });
}

test("a steward adds a device by a browser-made key pair; the private key never leaves the page", async ({ page }) => {
  const posts: string[] = [];
  await stubApi(page, posts);
  await page.goto(`/projects/${PROJECT}/mqttbrokers/cameras?lang=en`);
  await page.getByRole("button", { name: "Add device" }).click();
  await expect(page.getByText("A DNS label, e.g. sensor-001.")).toBeVisible();
  await shoot(page, "form-empty");

  await page.getByRole("textbox", { name: /Device name/ }).fill("t5532-dev1");
  await page.getByRole("radio", { name: /Paste a public key/ }).check();
  await page.getByRole("textbox", { name: /Public key/ }).fill('{"keys": [{"kty": "EC", "crv": "P-256", "kid": "k1", "x": "f83OJ3D2xF1Bg8vub9tLe1gHMzV76e8Tus9uPHvRVEU", "y": "x_FEzRu9m36HLN_tue659LNpXW6pCyStikYjKIWI5a0", "d": "jpsQnnGQmL-YBIffH1136cspYG6-0iY7X1fCE9-E9LI"}]}');
  await page.getByRole("button", { name: "Propose device" }).click();
  await expect(page.getByText("This is a private key. Paste the public key only")).toBeVisible();
  expect(posts).toHaveLength(0);
  await shoot(page, "pasted-private-key");

  await page.getByRole("textbox", { name: /Public key/ }).fill('{"keys": [{"kty": "EC", "crv": "P-256", "kid": "k1", "x": "f83OJ3D2xF1Bg8vub9tLe1gHMzV76e8Tus9uPHvRVEU", "y": "x_FEzRu9m36HLN_tue659LNpXW6pCyStikYjKIWI5a0"}]}');
  await page.getByRole("combobox", { name: /Topic/ }).selectOption("cameras/${clientid}/status");
  await page.getByRole("button", { name: "Propose device" }).click();
  await expect(page.getByRole("alert").filter({ hasText: "The device was not proposed" })).toContainText("topic: role 'cams'");
  await shoot(page, "refused-topic");

  await page.getByRole("radio", { name: /Generate in this browser/ }).check();
  await page.getByRole("button", { name: "Generate key pair" }).click();
  await expect(page.getByText("Save the private key now")).toBeVisible();
  const pem = (await page.locator("[data-private-key] pre, [data-private-key] code").first().textContent()) ?? "";
  expect(pem).toContain(PEM_BEGIN);
  const download = page.waitForEvent("download");
  await page.getByRole("button", { name: "Download t5532-dev1-key.pem" }).click();
  expect((await download).suggestedFilename()).toBe("t5532-dev1-key.pem");
  await shoot(page, "generated-key-once");

  await page.getByRole("combobox", { name: /Topic/ }).selectOption("cameras/${clientid}/#");
  await page.getByRole("button", { name: "Propose device" }).click();
  await expect(page.getByText("t5532-dev1 is proposed")).toBeVisible();
  await expect(page.getByRole("heading", { name: "How this device signs in" })).toBeVisible();
  const sent = posts.at(-1) ?? "";
  expect(sent).not.toContain("PRIVATE KEY");
  expect(sent).not.toContain(pem.split("\n")[1]);
  expect(JSON.parse(sent).jwks.keys[0].d).toBeUndefined();
  await expect(page.getByText(PEM_BEGIN)).toHaveCount(0);
  expect(await axeViolations(page)).toEqual([]);
  await shoot(page, "pending-sign-in");
});
