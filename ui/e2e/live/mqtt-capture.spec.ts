/**
 * A steward reads and clears what a capture MQTT broker kept on dev (API/01 §47, T-4880, PL-87).
 *
 * The journey takes the first helsinki MqttBroker, reads its capture (a `capture` broker answers its
 * newest messages, any other mode an empty list), clears it and reads it empty. A viewer, who holds
 * no `approve` on MqttBroker, is answered the 404 of a missing broker on both routes, as is the
 * steward for a broker that does not exist. Clearing a capture loses only design-time samples the
 * window would expire anyway.
 */
import { expect, test } from "@playwright/test";
import { STEWARD, VIEWER, csrf, signIn } from "./portal";

const PROJECT = "helsinki";
const BROKERS = `/api/v1/projects/${PROJECT}/mqttbrokers`;

interface Captured {
  broker: string;
  mode: string;
  messages: { topic: string; receivedAt: string; bytes: number }[];
  truncated: boolean;
}

test("a steward reads and clears a broker's capture; a viewer finds no broker", async ({ browser }) => {
  const { context, page } = await signIn(browser, STEWARD, `/projects/${PROJECT}/mqttbrokers?lang=en`);
  let name = "";
  try {
    const listed = (await (await page.request.get(BROKERS)).json()) as { items: { metadata: { name: string } }[] };
    expect(listed.items.length, "dev holds an MqttBroker in helsinki").toBeGreaterThan(0);
    name = listed.items[0].metadata.name;
    const captured = `${BROKERS}/${name}/captured`;

    const read = await page.request.get(`${captured}?limit=50`);
    expect(read.status(), await read.text()).toBe(200);
    const before = (await read.json()) as Captured;
    expect(before.broker).toBe(name);
    if (before.mode !== "capture") expect(before.messages, "only a capture broker keeps messages").toEqual([]);

    const wildcard = await page.request.get(`${captured}?topic=${encodeURIComponent("devices/#")}`);
    expect(wildcard.status(), "a topic is one exact topic, no wildcard").toBe(400);

    const headers = { "x-csrf-token": await csrf(context) };
    const cleared = await page.request.delete(captured, { headers });
    expect(cleared.status(), await cleared.text()).toBe(204);
    const after = await page.request.get(captured);
    expect(after.status()).toBe(200);
    // A device may publish again at once; a broker outside capture stays empty.
    if (before.mode !== "capture") expect(((await after.json()) as Captured).messages).toEqual([]);

    const unknown = await page.request.get(`${BROKERS}/no-such-broker-t4880/captured`);
    expect(unknown.status()).toBe(404);
  } finally {
    await context.close();
  }

  const viewer = await signIn(browser, VIEWER, `/projects/${PROJECT}/mqttbrokers?lang=en`);
  try {
    const captured = `${BROKERS}/${name}/captured`;
    expect((await viewer.page.request.get(captured)).status(), "a viewer cannot tell the broker exists").toBe(404);
    const headers = { "x-csrf-token": await csrf(viewer.context) };
    expect((await viewer.page.request.delete(captured, { headers })).status()).toBe(404);
  } finally {
    await viewer.context.close();
  }
});
