/**
 * A steward dry-runs a device message on a dev MqttBroker (API/01 §44a, T-5534).
 *
 * The journey takes the first helsinki MqttBroker and a role of it that may publish, sends one
 * sample message through test-publish and reads a step list that published nothing, then reads the
 * watch. An account outside the broker's publishing roles is refused 422 naming `account`, and a
 * broker that does not exist answers 404. Nothing is published or written, so the journey leaves
 * dev as it found it.
 */
import { expect, test } from "@playwright/test";
import { STEWARD, csrf, signIn } from "./portal";

const PROJECT = "helsinki";
const BROKERS = `/api/v1/projects/${PROJECT}/mqttbrokers`;

interface Broker {
  metadata: { name: string };
  spec: { roles?: { serviceAccounts?: string[]; publish?: string[] }[] };
}

test("a steward dry-runs a device message and reads the watch", async ({ browser }) => {
  const { context, page } = await signIn(browser, STEWARD, `/projects/${PROJECT}/mqttbrokers?lang=en`);
  try {
    const listed = (await (await page.request.get(BROKERS)).json()) as { items: Broker[] };
    expect(listed.items.length, "dev holds an MqttBroker in helsinki").toBeGreaterThan(0);
    const broker = listed.items[0];
    const name = broker.metadata.name;
    const headers = { "x-csrf-token": await csrf(context), "content-type": "application/json" };
    const testPublish = `${BROKERS}/${name}/test-publish`;

    const stranger = await page.request.post(testPublish, {
      headers,
      data: { account: "no-such-account-t5534", topic: "t5534/probe", payload: "{}" },
    });
    expect(stranger.status(), await stranger.text()).toBe(422);
    expect(await stranger.text()).toContain("account");

    const account = (broker.spec.roles ?? []).find((role) => (role.publish ?? []).length > 0)?.serviceAccounts?.[0];
    if (account !== undefined) {
      const run = await page.request.post(testPublish, {
        headers,
        data: { account, topic: "t5534/probe", payload: '{"t": 21.5}' },
      });
      expect(run.status(), await run.text()).toBe(200);
      const dry = (await run.json()) as { published: boolean; steps: { step: string; outcome: string }[] };
      expect(dry.published, "a dry run publishes nothing").toBe(false);
      expect(dry.steps.map((step) => step.step)).toEqual(expect.arrayContaining(["acl", "size"]));

      const watched = await page.request.get(`${BROKERS}/${name}/watch?account=${encodeURIComponent(account)}`);
      expect(watched.status(), await watched.text()).toBe(200);
    }

    const unknown = await page.request.post(`${BROKERS}/no-such-broker-t5534/test-publish`, {
      headers,
      data: { account: "cam", topic: "t5534/probe", payload: "{}" },
    });
    expect(unknown.status()).toBe(404);
  } finally {
    await context.close();
  }
});
